import AppKit
import ScreenCaptureKit

func esc(_ s: String) -> String { s.replacingOccurrences(of: "\"", with: "'") }

var result: [String: String] = ["user": NSUserName()]
let session = CGSessionCopyCurrentDictionary() as? [String: Any] ?? [:]
result["console"] = String(describing: session["kCGSSessionOnConsoleKey"] ?? "unknown")

// 1. Virtual display
let desc = CGVirtualDisplayDescriptor()
desc.queue = DispatchQueue.main
desc.name = "spike"
desc.maxPixelsWide = 1920; desc.maxPixelsHigh = 1080
desc.sizeInMillimeters = CGSize(width: 600, height: 340)
desc.productID = 0x5350; desc.vendorID = 0x5350; desc.serialNum = 1
let vd = CGVirtualDisplay(descriptor: desc)!
let settings = CGVirtualDisplaySettings()
settings.modes = [CGVirtualDisplayMode(width: 1920, height: 1080, refreshRate: 60)]
settings.hiDPI = 0
result["virtual_display"] = (vd.apply(settings) && vd.displayID != 0) ? "ok id=\(vd.displayID)" : "error applySettings"

// 2. Input injection (a harmless mouse move). Prompts for Accessibility if not yet trusted.
let trusted = AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary)
if let ev = CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: CGPoint(x: 10, y: 10), mouseButton: .left) {
    ev.post(tap: .cghidEventTap)
    result["input"] = trusted ? "ok (posted, AX trusted)" : "error (posted but process not AX-trusted)"
} else {
    result["input"] = "error CGEvent nil"
}

// 3. One ScreenCaptureKit frame of the virtual display (prompts for Screen Recording if needed)
let done = DispatchSemaphore(value: 0)
Task {
    do {
        var content = try await SCShareableContent.current
        for _ in 0..<50 where !content.displays.contains(where: { $0.displayID == vd.displayID }) {
            try await Task.sleep(nanoseconds: 100_000_000)
            content = try await SCShareableContent.current
        }
        result["sck_displays"] = content.displays.map { "\($0.displayID):\($0.width)x\($0.height)" }.joined(separator: " ")
        result["own_display_listed"] = String(content.displays.contains(where: { $0.displayID == vd.displayID }))
        guard let d = content.displays.first(where: { $0.displayID == vd.displayID }) ?? content.displays.first else {
            result["capture"] = "error no displays"; done.signal(); return
        }
        let cfg = SCStreamConfiguration(); cfg.width = 640; cfg.height = 360
        let img = try await SCScreenshotManager.captureImage(contentFilter: SCContentFilter(display: d, excludingWindows: []), configuration: cfg)
        result["capture"] = "ok \(img.width)x\(img.height) display=\(d.displayID)"
        let url = URL(fileURLWithPath: "/Users/Shared/viga-probe-\(NSUserName()).png")
        if let dest = CGImageDestinationCreateWithURL(url as CFURL, "public.png" as CFString, 1, nil) {
            CGImageDestinationAddImage(dest, img, nil); CGImageDestinationFinalize(dest)
        }
    } catch {
        result["capture"] = "error \(esc(error.localizedDescription))"
    }
    done.signal()
}
while done.wait(timeout: .now() + 0.05) == .timedOut { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
print("{" + result.sorted { $0.key < $1.key }.map { "\"\($0.key)\":\"\(esc($0.value))\"" }.joined(separator: ",") + "}")
