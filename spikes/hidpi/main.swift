// Throwaway: can a CGVirtualDisplay expose a HiDPI (2x backing) mode?
import AppKit

func dump(_ id: CGDirectDisplayID, _ tag: String) {
    let opts = [kCGDisplayShowDuplicateLowResolutionModes: kCFBooleanTrue] as CFDictionary
    let modes = (CGDisplayCopyAllDisplayModes(id, opts) as? [CGDisplayMode]) ?? []
    let cur = CGDisplayCopyDisplayMode(id)
    print("[\(tag)] bounds=\(CGDisplayBounds(id).size) pixels=\(CGDisplayPixelsWide(id))x\(CGDisplayPixelsHigh(id)) current=\(cur.map { "\($0.width)x\($0.height)px\($0.pixelWidth)x\($0.pixelHeight)" } ?? "nil") modes=\(modes.count)")
    for m in modes { print("   mode \(m.width)x\(m.height) pixels \(m.pixelWidth)x\(m.pixelHeight) usable=\(m.isUsableForDesktopGUI())") }
}

let variant = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "a"
let desc = CGVirtualDisplayDescriptor()
desc.queue = DispatchQueue.main
desc.name = "hidpi-spike"
desc.maxPixelsWide = 8192; desc.maxPixelsHigh = 8192
// ~ 27" 16:9 at 2560x1440 points (5K backing) => 597x336 mm
desc.sizeInMillimeters = CGSize(width: 597, height: 336)
desc.productID = 0x1234; desc.vendorID = 0x3456; desc.serialNum = UInt32(getpid())
let vd = CGVirtualDisplay(descriptor: desc)!
let s = CGVirtualDisplaySettings()
switch variant {
case "a": // hiDPI=1, modes in POINTS (Chromium style)
    s.hiDPI = 1
    s.modes = [CGVirtualDisplayMode(width: 1280, height: 720, refreshRate: 60)]
case "b": // hiDPI=1, both the point mode and the 2x pixel mode
    s.hiDPI = 1
    s.modes = [CGVirtualDisplayMode(width: 2560, height: 1440, refreshRate: 60), CGVirtualDisplayMode(width: 1280, height: 720, refreshRate: 60)]
default: // hiDPI=2
    s.hiDPI = 2
    s.modes = [CGVirtualDisplayMode(width: 1280, height: 720, refreshRate: 60)]
}
print("variant \(variant) apply=\(vd.apply(s)) id=\(vd.displayID)")
RunLoop.main.run(until: Date().addingTimeInterval(1.5))
dump(vd.displayID, "after apply")
// Try to switch to a mode whose pixel size is 2x its point size.
let opts = [kCGDisplayShowDuplicateLowResolutionModes: kCFBooleanTrue] as CFDictionary
if let hi = ((CGDisplayCopyAllDisplayModes(vd.displayID, opts) as? [CGDisplayMode]) ?? []).first(where: { $0.pixelWidth == 2 * $0.width }) {
    var cfg: CGDisplayConfigRef?
    CGBeginDisplayConfiguration(&cfg)
    CGConfigureDisplayWithDisplayMode(cfg, vd.displayID, hi, nil)
    let err = CGCompleteDisplayConfiguration(cfg, .forSession)
    print("switch to \(hi.width)x\(hi.height)@\(hi.pixelWidth)x\(hi.pixelHeight) err=\(err.rawValue)")
    RunLoop.main.run(until: Date().addingTimeInterval(1.5))
    dump(vd.displayID, "after switch")
} else { print("no 2x mode offered") }
if let scr = NSScreen.screens.first(where: { $0.localizedName.contains("hidpi-spike") }) { print("NSScreen backingScaleFactor=\(scr.backingScaleFactor) frame=\(scr.frame.size)") }
