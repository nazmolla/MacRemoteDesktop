// Throwaway: helper-style lifecycle in one process, app-scoped selects, no restore.
import AppKit
let opts = [kCGDisplayShowDuplicateLowResolutionModes: kCFBooleanTrue] as CFDictionary
func modes(_ id: CGDirectDisplayID) -> [CGDisplayMode] { (CGDisplayCopyAllDisplayModes(id, opts) as? [CGDisplayMode]) ?? [] }
func select(_ vd: CGVirtualDisplay, _ w: Int, _ h: Int, _ scale: Int) -> Bool {
    let t0 = Date()
    while Date().timeIntervalSince(t0) < 4 {
        if let m = modes(vd.displayID).first(where: { $0.width == w && $0.height == h && $0.pixelWidth == scale * w }) {
            var c: CGDisplayConfigRef?; CGBeginDisplayConfiguration(&c); CGConfigureDisplayWithDisplayMode(c, vd.displayID, m, nil)
            CGCompleteDisplayConfiguration(c, .forAppOnly)
            for _ in 0..<60 { if let cur = CGDisplayCopyDisplayMode(vd.displayID), cur.width == w, cur.pixelWidth == scale * w { return true }; usleep(50_000) }
            return false
        }
        usleep(50_000)
    }
    return false
}
func apply(_ vd: CGVirtualDisplay, _ w: UInt32, _ h: UInt32) -> Bool {
    let s = CGVirtualDisplaySettings(); s.hiDPI = 1; s.modes = [CGVirtualDisplayMode(width: w, height: h, refreshRate: 60)]; return vd.apply(s)
}
let d = CGVirtualDisplayDescriptor(); d.queue = .global(); d.name = "seq"
d.maxPixelsWide = 8192; d.maxPixelsHigh = 8192; d.sizeInMillimeters = CGSize(width: 600, height: 338)
d.productID = 0x6D616372; d.vendorID = 0x6D616372; d.serialNum = UInt32(getpid())
d.redPrimary = CGPoint(x: 0.64, y: 0.33); d.greenPrimary = CGPoint(x: 0.30, y: 0.60); d.bluePrimary = CGPoint(x: 0.15, y: 0.06); d.whitePoint = CGPoint(x: 0.3127, y: 0.3290)
let vd = CGVirtualDisplay(descriptor: d)!
var r = [String]()
r.append(apply(vd, 1920, 1080) && select(vd, 1920, 1080, 1) ? "1x" : "FAIL-1x")
r.append(apply(vd, 2000, 1333) && select(vd, 2000, 1333, 2) ? "retina" : "FAIL-retina")
r.append(apply(vd, 1714, 1288) && select(vd, 1714, 1288, 1) ? "1x-again" : "FAIL-1x-again")
print(r.joined(separator: " "))
