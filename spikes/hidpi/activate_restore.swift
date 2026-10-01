// Throwaway: one virtual display per process; report activation time or FAIL.
import AppKit
let opts = [kCGDisplayShowDuplicateLowResolutionModes: kCFBooleanTrue] as CFDictionary
let d = CGVirtualDisplayDescriptor(); d.queue = .global(); d.name = "act"
d.maxPixelsWide = 8192; d.maxPixelsHigh = 8192; d.sizeInMillimeters = CGSize(width: 600, height: 338)
d.productID = 0x6D616372; d.vendorID = 0x6D616372; d.serialNum = UInt32(getpid())
guard let vd = CGVirtualDisplay(descriptor: d) else { print("FAIL init"); exit(1) }
let s = CGVirtualDisplaySettings(); s.hiDPI = 1; s.modes = [CGVirtualDisplayMode(width: 1920, height: 1080, refreshRate: 60)]
CGRestorePermanentDisplayConfiguration(); let ok = vd.apply(s)
let t0 = Date()
while Date().timeIntervalSince(t0) < 8 {
    let modes = (CGDisplayCopyAllDisplayModes(vd.displayID, opts) as? [CGDisplayMode]) ?? []
    if modes.contains(where: { $0.width == 1920 && $0.height == 1080 }) {
        print(String(format: "OK %.2fs id=%u", Date().timeIntervalSince(t0), vd.displayID)); exit(0)
    }
    usleep(50_000)
}
var n: UInt32 = 0; CGGetOnlineDisplayList(0, nil, &n)
print("FAIL apply=\(ok) id=\(vd.displayID) online=\(n)"); exit(1)
