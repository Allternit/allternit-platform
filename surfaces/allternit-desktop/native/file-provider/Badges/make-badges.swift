// Draws the Finder badge images (design 6A, colour badges) as .iconset
// folders; build.sh turns each into an .icns with iconutil.
//
//   swift make-badges.swift <output dir>
//
// The colours are the design's status colours: conflict = warning amber,
// agent = info blue, pinned = graphite, protected = secondary grey. A white
// ring keeps each badge readable on any file icon, light or dark.
import AppKit

struct Badge {
    let name: String
    let symbol: String
    let fill: NSColor
    /// The conflict badge is a filled triangle, not a disc (design 6A).
    let triangle: Bool
}

let badges = [
    Badge(name: "conflict", symbol: "exclamationmark", fill: NSColor(srgbRed: 0.85, green: 0.56, blue: 0.13, alpha: 1), triangle: true),
    Badge(name: "protected", symbol: "lock.fill", fill: NSColor(srgbRed: 0.42, green: 0.42, blue: 0.41, alpha: 1), triangle: false),
    Badge(name: "agent", symbol: "sparkle", fill: NSColor(srgbRed: 0.18, green: 0.47, blue: 0.93, alpha: 1), triangle: false),
    Badge(name: "pinned", symbol: "pin.fill", fill: NSColor(srgbRed: 0.12, green: 0.12, blue: 0.11, alpha: 1), triangle: false),
]

let sizes: [(points: Int, scale: Int)] = [(16, 1), (16, 2), (32, 1), (32, 2), (128, 1), (128, 2), (256, 1), (256, 2), (512, 1), (512, 2)]

func render(_ badge: Badge, pixels: Int) -> Data? {
    guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels, bitsPerSample: 8,
                                     samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                     bytesPerRow: 0, bitsPerPixel: 0) else { return nil }
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let size = CGFloat(pixels)
    let rect = NSRect(x: 0, y: 0, width: size, height: size).insetBy(dx: size * 0.04, dy: size * 0.04)
    let ring = size * 0.07

    let shape: NSBezierPath
    if badge.triangle {
        shape = NSBezierPath()
        shape.move(to: NSPoint(x: rect.midX, y: rect.maxY))
        shape.line(to: NSPoint(x: rect.maxX, y: rect.minY + size * 0.06))
        shape.line(to: NSPoint(x: rect.minX, y: rect.minY + size * 0.06))
        shape.close()
        shape.lineJoinStyle = .round
    } else {
        shape = NSBezierPath(ovalIn: rect.insetBy(dx: ring / 2, dy: ring / 2))
    }
    NSColor.white.setStroke()
    shape.lineWidth = ring
    shape.stroke()
    badge.fill.setFill()
    shape.fill()

    let pointSize = size * (badge.triangle ? 0.42 : 0.5)
    let config = NSImage.SymbolConfiguration(pointSize: pointSize, weight: .bold)
        .applying(NSImage.SymbolConfiguration(paletteColors: [badge.triangle ? NSColor(srgbRed: 0.12, green: 0.12, blue: 0.11, alpha: 1) : .white]))
    if let symbol = NSImage(systemSymbolName: badge.symbol, accessibilityDescription: nil)?.withSymbolConfiguration(config) {
        let glyph = symbol.size
        let origin = NSPoint(x: rect.midX - glyph.width / 2,
                             y: (badge.triangle ? rect.midY - size * 0.12 : rect.midY) - glyph.height / 2)
        symbol.draw(in: NSRect(origin: origin, size: glyph))
    }
    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:])
}

let output = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : ".")
for badge in badges {
    let set = output.appendingPathComponent("badge-\(badge.name).iconset", isDirectory: true)
    try FileManager.default.createDirectory(at: set, withIntermediateDirectories: true)
    for (points, scale) in sizes {
        guard let png = render(badge, pixels: points * scale) else {
            FileHandle.standardError.write(Data("could not draw \(badge.name) \(points)@\(scale)x\n".utf8))
            exit(1)
        }
        let file = scale == 1 ? "icon_\(points)x\(points).png" : "icon_\(points)x\(points)@2x.png"
        try png.write(to: set.appendingPathComponent(file))
    }
}
