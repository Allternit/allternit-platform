import SwiftUI
import UIKit

/// The project folder, drawn the same as `mock/folder2.jsx` `Folder` and the
/// web's `FolderCover` (viewBox 250x304 scaled to `width`): tab, back panel,
/// 0-3 papers, front cover with pattern, frame, title / symbol / text / image
/// art and the mode chip. `flat` drops the art and chip for small icons.
struct FolderCoverView: View {
    let title: String
    let mode: String
    var appearance: Appearance?
    var width: CGFloat = 170
    var open: Bool = false
    var flat: Bool = false

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private static let viewW: CGFloat = 250
    private static let viewH: CGFloat = 304

    private var look: FolderLook {
        FolderLook(title: title, mode: mode, appearance: appearance, flat: flat)
    }

    var body: some View {
        let look = look
        let height = width * Self.viewH / Self.viewW
        let ease: Animation? = reduceMotion ? nil : .timingCurve(0.2, 0.8, 0.2, 1, duration: 0.26)
        ZStack {
            Canvas { ctx, size in
                var c = ctx
                let s = size.width / Self.viewW
                c.scaleBy(x: s, y: s)
                look.drawBack(&c)
            }
            Canvas { ctx, size in
                var c = ctx
                let s = size.width / Self.viewW
                c.scaleBy(x: s, y: s)
                look.drawPapers(&c)
            }
            .offset(y: open ? -14 * width / Self.viewW : 0)
            Canvas { ctx, size in
                var c = ctx
                let s = size.width / Self.viewW
                c.scaleBy(x: s, y: s)
                look.drawCover(&c)
            }
            .rotationEffect(.degrees(open ? -2.5 : 0),
                            anchor: UnitPoint(x: 2 / Self.viewW, y: 300 / Self.viewH))
        }
        .frame(width: width, height: height)
        .animation(ease, value: open)
        .shadow(color: look.cover.edge == .shadow ? .black.opacity(0.18) : .clear,
                radius: 7 * width / Self.viewW, y: 3 * width / Self.viewW)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(flat ? title : "\(title), \(mode) project folder")
    }
}

// MARK: - Drawing

/// Everything the three canvas layers need, resolved once per render.
private struct FolderLook {
    let title: String
    let mode: String
    let cover: Cover
    let color: String
    let paper: Color
    let symbol: String
    let artText: String
    let imageDataURL: String?
    let flat: Bool

    init(title: String, mode: String, appearance: Appearance?, flat: Bool) {
        self.title = title
        self.mode = mode
        self.cover = Appearance.resolvedCover(appearance)
        let hex = appearance?.color ?? Appearance.defaultColor
        self.color = FolderHex.rgb(hex) != nil ? hex : Appearance.defaultColor
        self.paper = FolderHex.color(appearance?.paperColor ?? "#ffffff") ?? .white
        self.symbol = appearance?.symbol ?? "sparkle"
        self.artText = appearance?.text ?? ""
        self.imageDataURL = appearance?.image
        self.flat = flat
    }

    var ink: Color { FolderHex.isLight(color) ? Color(red: 0.12, green: 0.118, blue: 0.114) : .white }
    var edgeColor: Color? {
        switch cover.edge {
        case .ink: return .black.opacity(0.62)
        case .soft: return .black.opacity(0.2)
        case .shadow: return nil
        }
    }
    var edgeWidth: CGFloat { cover.edge == .ink ? 1.7 : 1 }

    private func stroke(_ ctx: inout GraphicsContext, _ path: Path, fill: Color, extra: CGFloat = 0) {
        ctx.fill(path, with: .color(fill))
        if let edge = edgeColor { ctx.stroke(path, with: .color(edge), lineWidth: edgeWidth + extra) }
    }

    private func rrect(_ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat, _ r: CGFloat) -> Path {
        Path(roundedRect: CGRect(x: x, y: y, width: w, height: h), cornerRadius: r)
    }

    // Back layer: tab and back panel.
    func drawBack(_ c: inout GraphicsContext) {
        let tabHex = cover.tabColor.flatMap { FolderHex.rgb($0) != nil ? $0 : nil }
        let tab = tabHex.flatMap(FolderHex.color) ?? FolderHex.shade(color, 0.07)
        stroke(&c, rrect(196, 16, 50, 112, 11), fill: tab)
        if !cover.tabLabel.isEmpty {
            var t = c
            t.translateBy(x: 236, y: 72)
            t.rotate(by: .degrees(90))
            let label = Text(String(cover.tabLabel.uppercased().prefix(Cover.tabLabelMax)))
                .font(.system(size: 11, weight: .bold, design: .monospaced))
                .tracking(1.5)
                .foregroundColor(FolderHex.isLight(tabHex ?? color) ? Color(red: 0.12, green: 0.118, blue: 0.114) : .white)
            t.draw(t.resolve(label), at: .zero, anchor: .bottom)
        }
        stroke(&c, rrect(12, 4, 214, 294, 12), fill: FolderHex.shade(color, 0.03))
    }

    // Papers layer.
    func drawPapers(_ c: inout GraphicsContext) {
        guard cover.sheets > 0 else { return }
        for k in 0..<cover.sheets {
            let f = CGFloat(k)
            let p = rrect(18 + f * 4, 12 + f * 4, 200 - f * 4, 282 - f * 6, 6)
            c.fill(p, with: .color(paper))
            c.stroke(p, with: .color(.black.opacity(0.2)), lineWidth: 1)
        }
    }

    // Front cover layer.
    func drawCover(_ c: inout GraphicsContext) {
        let ink = self.ink
        let body = rrect(2, 10, 214, 290, 12)
        stroke(&c, body, fill: FolderHex.color(color) ?? .blue, extra: 0.1)
        var k = c
        k.clip(to: body)
        drawPattern(&k, ink: ink)
        switch cover.frame {
        case .dotted:
            k.stroke(rrect(14, 22, 190, 266, 5), with: .color(ink.opacity(0.32)),
                     style: StrokeStyle(lineWidth: 2, lineCap: .round, dash: [1, 4]))
        case .double:
            k.stroke(rrect(12, 20, 194, 270, 6), with: .color(ink.opacity(0.35)), lineWidth: 1)
            k.stroke(rrect(17, 25, 184, 260, 4), with: .color(ink.opacity(0.35)), lineWidth: 1)
        case .none: break
        }
        guard !flat else { return }
        switch cover.art {
        case .title: drawTitle(&k, ink: ink)
        case .symbol: drawSymbol(&k, ink: ink)
        case .text: drawText(&k, ink: ink)
        case .image: drawImage(&k)
        case .none: break
        }
        if cover.showMode {
            k.fill(rrect(82, 252, 54, 17, 3), with: .color(ink.opacity(0.16)))
            let chip = Text(mode.uppercased())
                .font(.system(size: 10, design: .monospaced)).tracking(1.5).foregroundColor(ink)
            k.draw(k.resolve(chip), at: CGPoint(x: 109, y: 264.5), anchor: .bottom)
        }
    }

    // MARK: Patterns

    private func drawPattern(_ c: inout GraphicsContext, ink: Color) {
        let area = CGRect(x: 2, y: 10, width: 214, height: 290)
        func lines(step: CGFloat, offset: CGFloat = 0, opacity: Double, width: CGFloat = 1, _ c: inout GraphicsContext) {
            var p = Path()
            var x = (area.minX / step).rounded(.down) * step + offset
            while x <= area.maxX { p.move(to: CGPoint(x: x, y: area.minY)); p.addLine(to: CGPoint(x: x, y: area.maxY)); x += step }
            var y = (area.minY / step).rounded(.down) * step + offset
            while y <= area.maxY { p.move(to: CGPoint(x: area.minX, y: y)); p.addLine(to: CGPoint(x: area.maxX, y: y)); y += step }
            c.stroke(p, with: .color(ink.opacity(opacity)), lineWidth: width)
        }
        switch cover.pattern {
        case .plain: break
        case .grain:
            c.fill(Path(area), with: .tiledImage(FolderGrain.image, origin: .zero, sourceRect: CGRect(x: 0, y: 0, width: 1, height: 1), scale: 1))
        case .linen:
            lines(step: 6, offset: 3, opacity: 0.2, width: 0.8, &c)
        case .grid:
            lines(step: 16, opacity: 0.2, &c)
        case .blueprint:
            lines(step: 10, opacity: 0.1, &c)
            lines(step: 40, opacity: 0.3, &c)
        case .dots:
            var p = Path()
            var y: CGFloat = 6
            while y <= area.maxY + 6 {
                var x: CGFloat = 6
                while x <= area.maxX + 6 { p.addEllipse(in: CGRect(x: x - 1.4, y: y - 1.4, width: 2.8, height: 2.8)); x += 12 }
                y += 12
            }
            c.fill(p, with: .color(ink.opacity(0.22)))
        case .stripes:
            var r = c
            r.rotate(by: .degrees(45))
            var p = Path()
            var x: CGFloat = -600
            while x <= 600 { p.move(to: CGPoint(x: x, y: -600)); p.addLine(to: CGPoint(x: x, y: 600)); x += 14 }
            r.stroke(p, with: .color(ink.opacity(0.1)), lineWidth: 5)
        case .circuit:
            var p = Path()
            var dots = Path()
            var ty: CGFloat = 0
            while ty <= area.maxY {
                var tx: CGFloat = 0
                while tx <= area.maxX {
                    func pt(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: tx + x, y: ty + y) }
                    p.move(to: pt(0, 12)); p.addLine(to: pt(18, 12)); p.addLine(to: pt(26, 20)); p.addLine(to: pt(26, 48))
                    p.move(to: pt(48, 30)); p.addLine(to: pt(34, 30)); p.addLine(to: pt(30, 26)); p.addLine(to: pt(30, 0))
                    p.move(to: pt(8, 48)); p.addLine(to: pt(8, 36)); p.addLine(to: pt(16, 36))
                    dots.addEllipse(in: CGRect(x: tx + 26 - 2.2, y: ty + 20 - 2.2, width: 4.4, height: 4.4))
                    dots.addEllipse(in: CGRect(x: tx + 16 - 2.2, y: ty + 36 - 2.2, width: 4.4, height: 4.4))
                    tx += 48
                }
                ty += 48
            }
            c.stroke(p, with: .color(ink.opacity(0.24)), lineWidth: 1)
            c.stroke(dots, with: .color(ink.opacity(0.3)), lineWidth: 1)
        }
    }

    // MARK: Art

    private static func coverLines(_ title: String, max: Int = 9) -> [String] {
        var out: [String] = []
        var line = ""
        for w in title.split(separator: " ").map(String.init) {
            let joined = line.isEmpty ? w : line + " " + w
            if joined.count <= max { line = joined } else { if !line.isEmpty { out.append(line) }; line = w }
        }
        if !line.isEmpty { out.append(line) }
        return Array(out.prefix(4))
    }

    private func font(_ size: CGFloat, weight: Font.Weight) -> Font {
        switch cover.font {
        case .mono: return .system(size: size, weight: weight, design: .monospaced)
        case .serif: return .system(size: size, weight: .regular, design: .serif).italic()
        case .sans: return .system(size: size, weight: weight, design: .default)
        case .typewriter: return .custom("Courier New", size: size).weight(weight)
        }
    }

    private func drawTitle(_ c: inout GraphicsContext, ink: Color) {
        let caps = cover.font == .mono || cover.font == .typewriter
        let serif = cover.font == .serif
        let lines = Self.coverLines(caps ? title.uppercased() : title, max: serif ? 11 : 9)
        guard !lines.isEmpty else { return }
        let size: CGFloat = serif ? (lines.contains { $0.count > 8 } ? 34 : 40) : (lines.contains { $0.count > 7 } ? 30 : 36)
        let block = CGFloat(lines.count) * size * 1.04
        let top: CGFloat
        switch cover.place {
        case .top: top = 52 + size * 0.8
        case .bottom: top = 236 - block + size * 0.8
        case .center: top = 150 - block / 2 + size * 0.8
        }
        let centered = cover.place == .center
        for (i, l) in lines.enumerated() {
            let t = Text(l).font(font(size, weight: .bold)).tracking(caps ? 1 : -0.5).foregroundColor(ink)
            let baseline = top + CGFloat(i) * size * 1.04
            c.draw(c.resolve(t), at: CGPoint(x: centered ? 109 : 26, y: baseline - size * 0.85),
                   anchor: centered ? .top : .topLeading)
        }
    }

    private func drawSymbol(_ c: inout GraphicsContext, ink: Color) {
        let y: CGFloat = cover.place == .top ? 50 : cover.place == .bottom ? 150 : 100
        var img = c.resolve(Image(systemName: FolderSymbols.systemName(for: symbol)))
        img.shading = .color(ink)
        let box = CGRect(x: 64, y: y, width: 90, height: 90)
        let aspect = img.size.width / max(img.size.height, 1)
        let fit = aspect >= 1
            ? CGSize(width: 90, height: 90 / aspect) : CGSize(width: 90 * aspect, height: 90)
        c.draw(img, in: CGRect(x: box.midX - fit.width / 2, y: box.midY - fit.height / 2, width: fit.width, height: fit.height))
    }

    private func drawText(_ c: inout GraphicsContext, ink: Color) {
        let t = Text(artText).font(font(artText.count > 4 ? 34 : 64, weight: .bold)).foregroundColor(ink)
        let size: CGFloat = artText.count > 4 ? 34 : 64
        c.draw(c.resolve(t), at: CGPoint(x: 109, y: 170 - size * 0.85), anchor: .top)
    }

    private func drawImage(_ c: inout GraphicsContext) {
        guard let ui = FolderImageCache.image(for: imageDataURL), ui.size.width > 0, ui.size.height > 0 else {
            return
        }
        let box: CGFloat = 120
        let ratio = min(box / ui.size.width, box / ui.size.height)
        let w = ui.size.width * ratio, h = ui.size.height * ratio
        let cy: CGFloat = cover.place == .top ? 100 : cover.place == .bottom ? 200 : 150
        c.draw(Image(uiImage: ui), in: CGRect(x: 109 - w / 2, y: cy - h / 2, width: w, height: h))
    }
}

// MARK: - Helpers

private enum FolderHex {
    static func rgb(_ hex: String) -> (Double, Double, Double)? {
        var s = hex
        if s.hasPrefix("#") { s.removeFirst() }
        guard s.count == 6, let n = UInt32(s, radix: 16) else { return nil }
        return (Double((n >> 16) & 255), Double((n >> 8) & 255), Double(n & 255))
    }
    static func color(_ hex: String) -> Color? {
        guard let (r, g, b) = rgb(hex) else { return nil }
        return Color(red: r / 255, green: g / 255, blue: b / 255)
    }
    /// `shade()` in the mock: add `amt` of full scale to each channel.
    static func shade(_ hex: String, _ amt: Double) -> Color {
        let (r, g, b) = rgb(hex) ?? (57, 122, 213)
        func f(_ v: Double) -> Double { min(1, max(0, (v + amt * 255).rounded() / 255)) }
        return Color(red: f(r), green: f(g), blue: f(b))
    }
    static func isLight(_ hex: String) -> Bool {
        guard let (r, g, b) = rgb(hex) else { return false }
        return 0.299 * r + 0.587 * g + 0.114 * b > 165
    }
}

/// Tiled noise for the grain pattern, generated once in code.
private enum FolderGrain {
    static let image: Image = {
        let n = 64
        var px = [UInt8](repeating: 0, count: n * n * 4)
        var seed: UInt64 = 0x9E37_79B9_7F4A_7C15
        func next() -> Double {
            seed = seed &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
            return Double((seed >> 33) & 0xFF) / 255
        }
        for i in 0..<(n * n) {
            let gray = next(), alpha = next() * 0.28
            let v = UInt8((gray * alpha * 255).rounded())  // premultiplied
            px[i * 4] = v; px[i * 4 + 1] = v; px[i * 4 + 2] = v
            px[i * 4 + 3] = UInt8((alpha * 255).rounded())
        }
        let provider = CGDataProvider(data: Data(px) as CFData)
        let cg = provider.flatMap {
            CGImage(width: n, height: n, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: n * 4,
                    space: CGColorSpaceCreateDeviceRGB(),
                    bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
                    provider: $0, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
        }
        return cg.map { Image(decorative: $0, scale: 1) } ?? Image(systemName: "square")
    }()
}

/// Decodes the 256px PNG data URL once per distinct image.
private enum FolderImageCache {
    private static let cache = NSCache<NSString, UIImage>()
    static func image(for dataURL: String?) -> UIImage? {
        guard let dataURL, dataURL.hasPrefix("data:image/png;base64,") else { return nil }
        let key = "\(dataURL.count):\(dataURL.hashValue)" as NSString
        if let hit = cache.object(forKey: key) { return hit }
        let b64 = String(dataURL.dropFirst("data:image/png;base64,".count))
        guard let data = Data(base64Encoded: b64), let img = UIImage(data: data) else { return nil }
        cache.setObject(img, forKey: key)
        return img
    }
}

/// Web symbol names (appearance `SYMBOL_NAMES`, phosphor on web) to SF Symbols.
/// Brand marks have no SF Symbol and use the folder.
enum FolderSymbols {
    static let map: [String: String] = [
        "sparkle": "sparkles", "code": "chevron.left.forwardslash.chevron.right", "briefcase": "briefcase.fill",
        "robot": "cpu", "star": "star.fill", "heart": "heart.fill", "lightning": "bolt.fill", "leaf": "leaf.fill",
        "globe": "globe", "music": "music.note", "camera": "camera.fill", "book": "book.fill",
        "game": "gamecontroller.fill", "palette": "paintpalette.fill", "terminal": "terminal.fill",
        "diamond": "diamond.fill", "folder": "folder.fill", "rocket": "paperplane.fill", "shield": "shield.fill",
        "target": "target", "chart": "chart.bar.fill", "database": "cylinder.split.1x2.fill",
        "cloud": "cloud.fill", "puzzle": "puzzlepiece.fill", "gear": "gearshape.fill", "wrench": "wrench.fill",
        "bug": "ant.fill", "flask": "testtube.2", "graduation": "graduationcap.fill",
        "megaphone": "megaphone.fill", "users": "person.2.fill", "chat": "bubble.left.fill",
        "mail": "envelope.fill", "calendar": "calendar", "clock": "clock.fill", "check": "checkmark.circle.fill",
        "map": "map.fill", "compass": "safari.fill", "plant": "leaf.fill", "coffee": "cup.and.saucer.fill",
        "sun": "sun.max.fill", "moon": "moon.fill", "mountains": "mountain.2.fill",
        "lightbulb": "lightbulb.fill", "cube": "cube.fill", "stack": "square.stack.3d.up.fill",
        "shopping": "bag.fill", "gift": "gift.fill", "apple": "apple.logo",
    ]
    static func systemName(for symbol: String) -> String { map[symbol] ?? "folder" }
}
