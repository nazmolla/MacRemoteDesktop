// Deterministic desktop workloads for the performance harness.
// Build: tools/workload/build.sh   Run: target/workload typing 60 macrdp
import AppKit

enum Mode: String { case idle, typing, scroll, motion }

final class WorkloadView: NSView {
    let mode: Mode
    var tick = 0
    var typed = ""
    let source = Array("fn main() { let total: u64 = (1..=100).sum(); println!(\"{total}\"); } // ")
    // Built once: re-creating the font every frame intermittently crashed CoreText
    // (nil font-attribute insert) on macOS 27 in scroll mode.
    let attrs: [NSAttributedString.Key: Any] = [
        .font: NSFont.monospacedSystemFont(ofSize: 14, weight: .regular),
        .foregroundColor: NSColor(srgbRed: 0.85, green: 0.85, blue: 0.85, alpha: 1),
    ]

    init(mode: Mode, frame: NSRect) {
        self.mode = mode
        super.init(frame: frame)
    }
    required init?(coder: NSCoder) { fatalError("unused") }
    override var isFlipped: Bool { true }

    func step() {
        tick += 1
        switch mode {
        case .idle:
            return
        case .typing:
            guard tick % 6 == 0 else { return } // 10 chars/s at 60 Hz
            typed.append(source[(tick / 6) % source.count])
            if typed.count > 4000 { typed = "" }
            needsDisplay = true
        case .scroll, .motion:
            needsDisplay = true
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor(srgbRed: 0.12, green: 0.12, blue: 0.14, alpha: 1).setFill()
        bounds.fill()
        switch mode {
        case .idle:
            ("idle workload: static content" as NSString).draw(at: NSPoint(x: 40, y: 40), withAttributes: attrs)
        case .typing:
            (typed as NSString).draw(in: bounds.insetBy(dx: 40, dy: 40), withAttributes: attrs)
        case .scroll:
            let lineHeight: CGFloat = 20
            let offset = CGFloat(tick) // 60 px/s
            let first = Int(offset / lineHeight)
            for i in 0..<(Int(bounds.height / lineHeight) + 2) {
                let line = first + i
                let text = "\(line): let value_\(line) = compute(\(line * 7 % 101)); // scrolling source line"
                (text as NSString).draw(at: NSPoint(x: 40, y: CGFloat(line) * lineHeight - offset), withAttributes: attrs)
            }
        case .motion:
            let t = CGFloat(tick) / 60
            let gradient = NSGradient(colors: [
                NSColor(srgbRed: (sin(t) + 1) / 2, green: 0.3, blue: 0.6, alpha: 1),
                NSColor(srgbRed: 0.1, green: (cos(t * 1.3) + 1) / 2, blue: 0.4, alpha: 1),
            ])!
            gradient.draw(in: bounds, angle: CGFloat(tick % 360))
        }
    }
}

let args = CommandLine.arguments
guard args.count >= 3, let mode = Mode(rawValue: args[1]), let seconds = Double(args[2]) else {
    FileHandle.standardError.write("usage: workload idle|typing|scroll|motion <seconds> [screen-name-substring]\n".data(using: .utf8)!)
    exit(2)
}
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let screen: NSScreen
if args.count >= 4 {
    guard let match = NSScreen.screens.first(where: { $0.localizedName.contains(args[3]) }) else {
        let names = NSScreen.screens.map(\.localizedName).joined(separator: ", ")
        FileHandle.standardError.write("no screen matching '\(args[3])'; screens: \(names)\n".data(using: .utf8)!)
        exit(3)
    }
    screen = match
} else {
    screen = NSScreen.main!
}
let window = NSWindow(contentRect: screen.frame, styleMask: [.borderless], backing: .buffered, defer: false, screen: screen)
let view = WorkloadView(mode: mode, frame: NSRect(origin: .zero, size: screen.frame.size))
window.contentView = view
window.setFrame(screen.frame, display: true)
window.makeKeyAndOrderFront(nil)
app.activate(ignoringOtherApps: true)
Timer.scheduledTimer(withTimeInterval: 1.0 / 60.0, repeats: true) { _ in view.step() }
DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { exit(0) }
app.run()
