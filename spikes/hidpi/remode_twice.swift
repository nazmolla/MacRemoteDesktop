// Throwaway: does a live hiDPI=1 display refresh its mode list after re-applying settings?
// And can a second display be created after the first is released?
import AppKit
let opts = [kCGDisplayShowDuplicateLowResolutionModes: kCFBooleanTrue] as CFDictionary
func has(_ id: CGDirectDisplayID, _ w: Int, _ h: Int, _ s: Int) -> Bool {
    ((CGDisplayCopyAllDisplayModes(id, opts) as? [CGDisplayMode]) ?? []).contains { $0.width == w && $0.height == h && $0.pixelWidth == s*w }
}
func make(_ serial: UInt32, _ w: UInt32, _ h: UInt32) -> CGVirtualDisplay {
    let d = CGVirtualDisplayDescriptor(); d.queue = .global(); d.name = "remode"
    d.maxPixelsWide = 8192; d.maxPixelsHigh = 8192; d.sizeInMillimeters = CGSize(width: 600, height: 338)
    d.productID = 0x6D616372; d.vendorID = 0x6D616372; d.serialNum = serial; d.redPrimary = CGPoint(x: 0.64, y: 0.33); d.greenPrimary = CGPoint(x: 0.30, y: 0.60); d.bluePrimary = CGPoint(x: 0.15, y: 0.06); d.whitePoint = CGPoint(x: 0.3127, y: 0.3290)
    let vd = CGVirtualDisplay(descriptor: d)!
    let s = CGVirtualDisplaySettings(); s.hiDPI = 1; s.modes = [CGVirtualDisplayMode(width: w, height: h, refreshRate: 60)]
    print("apply \(w)x\(h):", vd.apply(s)); return vd
}
func wait(_ what: String, _ f: () -> Bool) {
    let t0 = Date(); while Date().timeIntervalSince(t0) < 6 { if f() { print(what, "after", String(format: "%.2fs", Date().timeIntervalSince(t0))); return }; usleep(50_000) }
    print(what, "NEVER (6s)")
}
var vd: CGVirtualDisplay? = make(1, 1920, 1080)
wait("A: 1920x1080@1x listed") { has(vd!.displayID, 1920, 1080, 1) }
let scope = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "session"
if let m = ((CGDisplayCopyAllDisplayModes(vd!.displayID, opts) as? [CGDisplayMode]) ?? []).first(where: { $0.width == 1920 && $0.pixelWidth == 1920 }) {
    var c: CGDisplayConfigRef?; CGBeginDisplayConfiguration(&c); CGConfigureDisplayWithDisplayMode(c, vd!.displayID, m, nil)
    print("select 1920x1080 via CGConfigure (\(scope)):", CGCompleteDisplayConfiguration(c, scope == "app" ? .forAppOnly : .forSession).rawValue)
}
let s1 = CGVirtualDisplaySettings(); s1.hiDPI = 1; s1.modes = [CGVirtualDisplayMode(width: 1920, height: 1080, refreshRate: 60)]
print("re-apply SAME 1920x1080:", vd!.apply(s1))
let s2 = CGVirtualDisplaySettings(); s2.hiDPI = 1; s2.modes = [CGVirtualDisplayMode(width: 2000, height: 1333, refreshRate: 60)]
let sem = DispatchSemaphore(value: 0)
Thread { print("re-apply 2000x1333 (bg thread):", vd!.apply(s2)); sem.signal() }.start(); sem.wait()
wait("B: 2000x1333@2x listed after re-apply") { has(vd!.displayID, 2000, 1333, 2) }
