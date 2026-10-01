// Builds the Portico app icon from a square render of the squircle artwork.
//
//   swift branding/make-icon.swift <render.png> <out-dir>
//
// Finds the squircle's edges in the render (the area outside it is near-white),
// masks it with a continuous-corner rounded square, places it on Apple's macOS
// icon grid (body 824/1024 of the canvas, centred, soft drop shadow) and writes
// 16-bit-per-channel Display P3 PNGs for every size in an .iconset, plus a
// 1024 px master. Run `iconutil -c icns <out-dir>/Portico.iconset` afterwards.
import AppKit
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers

let args = CommandLine.arguments
guard args.count == 3 else { fatalError("usage: make-icon.swift <render.png> <out-dir>") }
let outDir = URL(fileURLWithPath: args[2])
guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: args[1]) as CFURL, nil),
      let art = CGImageSourceCreateImageAtIndex(src, 0, nil) else { fatalError("cannot read \(args[1])") }

// Sample the render to find where the squircle starts on each side.
let w = art.width, h = art.height
let rgb = CGColorSpace(name: CGColorSpace.sRGB)!
var px = [UInt8](repeating: 0, count: w * h * 4)
let probe = CGContext(data: &px, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                      space: rgb, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
probe.draw(art, in: CGRect(x: 0, y: 0, width: w, height: h))
func isBackground(_ x: Int, _ y: Int) -> Bool {
    let i = (y * w + x) * 4
    return Int(px[i]) + Int(px[i + 1]) + Int(px[i + 2]) > 3 * 225
}
func edge(_ coords: [(Int, Int)]) -> Int {
    for (n, (x, y)) in coords.enumerated() where !isBackground(x, y) { return n }
    return 0
}
let midX = w / 2, midY = h / 2
let left = edge((0..<w).map { ($0, midY) })
let right = edge((0..<w).reversed().map { ($0, midY) })
let top = edge((0..<h).map { (midX, $0) })
let bottom = edge((0..<h).reversed().map { (midX, $0) })
// Trim a little inside the detected edge so no anti-aliased white fringe survives.
let trim = Double(w) * 0.012
let body = CGRect(x: Double(left) + trim, y: Double(bottom) + trim,
                  width: Double(w - left - right) - 2 * trim,
                  height: Double(h - top - bottom) - 2 * trim)
print("squircle in render: left \(left) right \(right) top \(top) bottom \(bottom)")
guard let cropped = art.cropping(to: CGRect(x: body.minX, y: Double(h) - body.maxY,
                                             width: body.width, height: body.height)) else {
    fatalError("crop failed")
}

// macOS continuous-corner ("squircle") path, Apple's corner ratio ~0.2237.
func squircle(_ r: CGRect) -> CGPath {
    let path = NSBezierPath(roundedRect: r, xRadius: r.width * 0.2237, yRadius: r.height * 0.2237)
    return path.cgPath
}

let p3 = CGColorSpace(name: CGColorSpace.displayP3)!
func render(size: Int) -> CGImage {
    let ctx = CGContext(data: nil, width: size, height: size, bitsPerComponent: 16, bytesPerRow: 0,
                        space: p3, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
                            | CGBitmapInfo.byteOrder16Little.rawValue)!
    ctx.interpolationQuality = .high
    let s = Double(size)
    let rect = CGRect(x: s * 100 / 1024, y: s * 100 / 1024, width: s * 824 / 1024, height: s * 824 / 1024)
    // Soft shadow under the body (skipped at tiny sizes, where it only blurs).
    if size >= 64 {
        ctx.saveGState()
        ctx.setShadow(offset: CGSize(width: 0, height: -s * 10 / 1024), blur: s * 28 / 1024,
                      color: CGColor(gray: 0, alpha: 0.35))
        ctx.addPath(squircle(rect))
        ctx.setFillColor(CGColor(gray: 0.1, alpha: 1))
        ctx.fillPath()
        ctx.restoreGState()
    }
    ctx.addPath(squircle(rect))
    ctx.clip()
    ctx.draw(cropped, in: rect)
    return ctx.makeImage()!
}

func writePNG(_ img: CGImage, _ url: URL) {
    let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(dest, img, nil)
    guard CGImageDestinationFinalize(dest) else { fatalError("cannot write \(url.path)") }
}

let iconset = outDir.appendingPathComponent("Portico.iconset")
try? FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)
writePNG(render(size: 1024), outDir.appendingPathComponent("portico-icon-1024.png"))
for base in [16, 32, 128, 256, 512] {
    writePNG(render(size: base), iconset.appendingPathComponent("icon_\(base)x\(base).png"))
    writePNG(render(size: base * 2), iconset.appendingPathComponent("icon_\(base)x\(base)@2x.png"))
}
print("wrote \(iconset.path)")
