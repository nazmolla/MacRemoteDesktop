// Minimal macOS VM host on Apple's Virtualization.framework (after Apple's sample
// "Running macOS in a virtual machine on Apple silicon"). Used for Phase 3 tests that
// can crash WindowServer: a crash inside the VM can't touch the host's sessions.
//
//   vmhost install <bundle-dir>     download the latest supported macOS (IPSW) and install
//   vmhost run <bundle-dir> [--gui] boot it (2 CPUs, 4 GB); --gui shows a window
import AppKit
import Foundation
import Virtualization

let cpus = 2
let memory: UInt64 = 4 << 30
let diskBytes: UInt64 = 64 << 30  // sparse: only used blocks take space

func die(_ msg: String) -> Never { FileHandle.standardError.write((msg + "\n").data(using: .utf8)!); exit(1) }

struct Bundle {
    let dir: URL
    var disk: URL { dir.appendingPathComponent("disk.img") }
    var aux: URL { dir.appendingPathComponent("aux.img") }
    var hw: URL { dir.appendingPathComponent("hardware-model") }
    var mid: URL { dir.appendingPathComponent("machine-id") }
    var ipsw: URL { dir.appendingPathComponent("restore.ipsw") }
}

func config(_ b: Bundle, platform: VZMacPlatformConfiguration, gui: Bool) -> VZVirtualMachineConfiguration {
    let c = VZVirtualMachineConfiguration()
    c.platform = platform
    c.bootLoader = VZMacOSBootLoader()
    c.cpuCount = cpus
    c.memorySize = memory
    let g = VZMacGraphicsDeviceConfiguration()
    g.displays = [VZMacGraphicsDisplayConfiguration(widthInPixels: 1920, heightInPixels: 1080, pixelsPerInch: 110)]
    c.graphicsDevices = [g]
    guard let att = try? VZDiskImageStorageDeviceAttachment(url: b.disk, readOnly: false) else { die("cannot open disk image") }
    c.storageDevices = [VZVirtioBlockDeviceConfiguration(attachment: att)]
    let net = VZVirtioNetworkDeviceConfiguration()
    net.attachment = VZNATNetworkDeviceAttachment()
    c.networkDevices = [net]
    c.keyboards = [VZUSBKeyboardConfiguration()]
    c.pointingDevices = [VZUSBScreenCoordinatePointingDeviceConfiguration()]
    do { try c.validate() } catch { die("invalid VM configuration: \(error)") }
    return c
}

func install(_ b: Bundle) {
    try? FileManager.default.createDirectory(at: b.dir, withIntermediateDirectories: true)
    let sem = DispatchSemaphore(value: 0)
    var image: VZMacOSRestoreImage?
    if FileManager.default.fileExists(atPath: b.ipsw.path) {
        VZMacOSRestoreImage.load(from: b.ipsw) { image = try? $0.get(); sem.signal() }
        sem.wait()
    } else {
        VZMacOSRestoreImage.fetchLatestSupported { r in
            guard let latest = try? r.get() else { die("could not fetch the restore image catalog") }
            print("downloading macOS \(latest.operatingSystemVersion.majorVersion).\(latest.operatingSystemVersion.minorVersion) build \(latest.buildVersion) from \(latest.url)")
            let task = URLSession.shared.downloadTask(with: latest.url) { tmp, _, err in
                guard let tmp else { die("download failed: \(err?.localizedDescription ?? "?")") }
                try? FileManager.default.moveItem(at: tmp, to: b.ipsw)
                VZMacOSRestoreImage.load(from: b.ipsw) { image = try? $0.get(); sem.signal() }
            }
            let obs = task.progress.observe(\.fractionCompleted) { p, _ in
                let pct = Int(p.fractionCompleted * 100)
                if pct % 5 == 0 { print("download \(pct)%") }
            }
            task.resume()
            _ = obs
            withExtendedLifetime(obs) { sem.wait() }
            sem.signal()
        }
        sem.wait()
    }
    guard let image, let req = image.mostFeaturefulSupportedConfiguration, req.hardwareModel.isSupported else {
        die("restore image not supported on this Mac")
    }
    let platform = VZMacPlatformConfiguration()
    platform.hardwareModel = req.hardwareModel
    platform.machineIdentifier = VZMacMachineIdentifier()
    guard let auxStore = try? VZMacAuxiliaryStorage(creatingStorageAt: b.aux, hardwareModel: req.hardwareModel, options: [.allowOverwrite]) else { die("cannot create auxiliary storage") }
    platform.auxiliaryStorage = auxStore
    try? req.hardwareModel.dataRepresentation.write(to: b.hw)
    try? platform.machineIdentifier.dataRepresentation.write(to: b.mid)
    FileManager.default.createFile(atPath: b.disk.path, contents: nil)
    guard let fh = try? FileHandle(forWritingTo: b.disk) else { die("cannot create disk image") }
    try? fh.truncate(atOffset: diskBytes); try? fh.close()

    let vm = VZVirtualMachine(configuration: config(b, platform: platform, gui: false))
    let installer = VZMacOSInstaller(virtualMachine: vm, restoringFromImageAt: b.ipsw)
    let obs = installer.progress.observe(\.fractionCompleted) { p, _ in print("install \(Int(p.fractionCompleted * 100))%") }
    installer.install { r in
        switch r {
        case .success: print("installed. Now run: vmhost run \(b.dir.path) --gui"); exit(0)
        case .failure(let e): die("install failed: \(e)")
        }
    }
    withExtendedLifetime(obs) { RunLoop.main.run() }
}

final class Delegate: NSObject, VZVirtualMachineDelegate {
    func guestDidStop(_ vm: VZVirtualMachine) { print("VM stopped"); exit(0) }
    func virtualMachine(_ vm: VZVirtualMachine, didStopWithError e: Error) { die("VM stopped with error: \(e)") }
}

func run(_ b: Bundle, gui: Bool) {
    guard let hwData = try? Data(contentsOf: b.hw), let hwModel = VZMacHardwareModel(dataRepresentation: hwData),
          let midData = try? Data(contentsOf: b.mid), let mid = VZMacMachineIdentifier(dataRepresentation: midData)
    else { die("not an installed VM bundle: \(b.dir.path)") }
    let platform = VZMacPlatformConfiguration()
    platform.hardwareModel = hwModel
    platform.machineIdentifier = mid
    platform.auxiliaryStorage = VZMacAuxiliaryStorage(contentsOf: b.aux)
    let vm = VZVirtualMachine(configuration: config(b, platform: platform, gui: gui))
    let delegate = Delegate()
    vm.delegate = delegate
    let app = NSApplication.shared
    var window: NSWindow?
    if gui {
        app.setActivationPolicy(.regular)
        let view = VZVirtualMachineView()
        view.virtualMachine = vm
        view.capturesSystemKeys = true
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1280, height: 720),
                          styleMask: [.titled, .closable, .resizable, .miniaturizable], backing: .buffered, defer: false)
        window?.title = "Viga test VM"
        window?.contentView = view
        window?.center()
        window?.makeKeyAndOrderFront(nil)
        app.activate(ignoringOtherApps: true)
    } else {
        app.setActivationPolicy(.prohibited)
    }
    vm.start { r in
        if case .failure(let e) = r { die("start failed: \(e)") }
        print("VM running (\(cpus) CPUs, \(memory >> 30) GB). Stop it from inside, or kill this process.")
    }
    withExtendedLifetime((delegate, window)) { app.run() }
}

let args = CommandLine.arguments
guard args.count >= 3 else { die("usage: vmhost install|run <bundle-dir> [--gui]") }
let bundle = Bundle(dir: URL(fileURLWithPath: args[2]))
switch args[1] {
case "install": install(bundle)
case "run": run(bundle, gui: args.contains("--gui"))
default: die("usage: vmhost install|run <bundle-dir> [--gui]")
}
