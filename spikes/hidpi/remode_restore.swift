// Throwaway: create (no restore) → app-scoped select → restore → re-apply new mode → does it publish?
import AppKit
let opts = [kCGDisplayShowDuplicateLowResolutionModes: kCFBooleanTrue] as CFDictionary
func modes(_ id: CGDirectDisplayID) -> [CGDisplayMode] { (CGDisplayCopyAllDisplayModes(id, opts) as? [CGDisplayMode]) ?? [] }
func waitFor(_ id: CGDirectDisplayID, _ f: (CGDisplayMode) -> Bool) -> Double? {
    let t0 = Date(); while Date().timeIntervalSince(t0) < 6 { if modes(id).contains(where: f) { return Date().timeIntervalSince(t0) }; usleep(50_000) }; return nil
}
let d = CGVirtualDisplayDescriptor(); d.queue = .global(); d.name = "rr"
d.maxPixelsWide = 8192; d.maxPixelsHigh = 8192; d.sizeInMillimeters = CGSize(width: 600, height: 338)
d.productID = 0x6D616372; d.vendorID = 0x6D616372; d.serialNum = UInt32(getpid())
let vd = CGVirtualDisplay(descriptor: d)!
let s = CGVirtualDisplaySettings(); s.hiDPI = 1; s.modes = [CGVirtualDisplayMode(width: 1920, height: 1080, refreshRate: 60)]
_ = vd.apply(s)
guard waitFor(vd.displayID, { $0.width == 1920 && $0.pixelWidth == 1920 }) != nil else { print("FAIL create"); exit(1) }
if let m = modes(vd.displayID).first(where: { $0.width == 1920 && $0.pixelWidth == 1920 }) {
    var c: CGDisplayConfigRef?; CGBeginDisplayConfiguration(&c); CGConfigureDisplayWithDisplayMode(c, vd.displayID, m, nil); CGCompleteDisplayConfiguration(c, .forAppOnly)
}
CGRestorePermanentDisplayConfiguration()
let s2 = CGVirtualDisplaySettings(); s2.hiDPI = 1; s2.modes = [CGVirtualDisplayMode(width: 2000, height: 1333, refreshRate: 60)]
_ = vd.apply(s2)
if let t = waitFor(vd.displayID, { $0.width == 2000 && $0.pixelWidth == 4000 }) { print(String(format: "OK remode %.2fs", t)) } else { print("FAIL remode") }
