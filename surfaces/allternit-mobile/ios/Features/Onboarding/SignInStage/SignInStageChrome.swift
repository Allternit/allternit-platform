import SwiftUI
import UIKit

/// Shared pieces of the sign-in stage scenes: block type, the app's line
/// icons, the app window and its rail, halos, rings, bursts and the typing
/// field. Ported from allternit-ai src/components/auth/sign-in-stage
/// (scenes.tsx, block-type.tsx). Every size is in the scene's native
/// 694 × 470 canvas points; the stage scales the whole canvas to fit.

// MARK: - Palette and type

/// Fixed sizes of the app window inside the scene canvas.
enum StageLayout {
    static let windowWidth: CGFloat = 670
    static let windowHeight: CGFloat = 446
    /// The main area's size (window minus border and the 148 pt rail).
    static let mainWidth: CGFloat = 520
    static let mainHeight: CGFloat = 444
}

/// Scene colours (the light app, in either theme).
enum StageInk {
    static let text = Color(hex: "#2A1F16")
    static let brown = Color(hex: "#5E4A3A")
    static let brown2 = Color(hex: "#4A3A2C")
    static let tan = Color(hex: "#9A7658")
    static let taupe = Color(hex: "#8C7B6B")
    static let muted = Color(hex: "#9A8F86")
    static let faint = Color(hex: "#A8988A")
    static let amber = Color(hex: "#D9B48F")
    static let coral = Color(hex: "#D97757")
    static let border = Color(hex: "#E9E3DC")
    static let hairline = Color(hex: "#F0EBE5")
    static let chip = Color(hex: "#F5F2EF")
    static let green = Color(hex: "#22A06B")
    static let greenText = Color(hex: "#17663F")
    static let greenSoft = Color(hex: "#E9F7EE")
    static let shadow = Color(red: 42 / 255, green: 31 / 255, blue: 22 / 255)
}

enum StageFont {
    static func sans(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight)
    }

    static func mono(_ size: CGFloat, _ weight: Font.Weight = .medium) -> Font {
        .system(size: size, weight: weight, design: .monospaced)
    }

    /// The app's display serif (web: Georgia, "New York").
    static func serif(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        .custom("Georgia", size: size).weight(weight)
    }
}

/// Real avatar art the bot picker uses (web public/avatar-packs).
enum StageAvatar {
    static let gizzi = "GizziMascot"
    static let drop = "StageAvatarDrop"
    static let pixel = "StageAvatarPixel"
    /// "ChatGPT dot"
    static let nimbus = "StageAvatarNimbus"
    static let grok = "StageAvatarGrok"
    static let muse = "StageAvatarMuse"
}

struct StageImage: View {
    let name: String
    let size: CGFloat
    var shadow = false

    var body: some View {
        Image(name)
            .resizable()
            .interpolation(.high)
            .aspectRatio(contentMode: .fit)
            .frame(width: size, height: size)
            .shadow(color: shadow ? StageInk.shadow.opacity(0.2) : .clear, radius: 3, y: 5)
    }
}

// MARK: - Block type

/// A-block mark and A://TERNIT block type: 10-unit cells, 8.5 blocks, rx 1.5.
enum BlockType {
    static let cell: CGFloat = 10
    static let block: CGFloat = 8.5
    static let coral = Color(hex: "#D97757")

    /// The 5 × 5 A of the mark, core excluded (AProtocolWordmark MARK_CELLS).
    static let aBlocks: [(Int, Int)] = [
        (2, 0), (1, 1), (3, 1), (0, 2), (1, 2), (3, 2), (4, 2), (0, 3), (4, 3), (0, 4), (4, 4),
    ]
    static let core = (2, 2)

    /// glyphs.json cells, (x, y) per letter (only the letters the stage uses).
    static let glyphs: [Character: [(Int, Int)]] = [
        "A": [(0, 2), (0, 3), (0, 4), (1, 1), (1, 2), (2, 0), (2, 2), (3, 1), (3, 2), (4, 2), (4, 3), (4, 4)],
        "B": [(0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (1, 0), (1, 2), (1, 4), (2, 0), (2, 2), (2, 4), (3, 0), (3, 2), (3, 4), (4, 1), (4, 3)],
        "C": [(0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (1, 0), (1, 4), (2, 0), (2, 4), (3, 0), (3, 4), (4, 0), (4, 4)],
        "D": [(0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (1, 0), (1, 4), (2, 0), (2, 4), (3, 0), (3, 4), (4, 1), (4, 2), (4, 3)],
        "E": [(0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (1, 0), (1, 2), (1, 4), (2, 0), (2, 2), (2, 4), (3, 0), (3, 2), (3, 4), (4, 0), (4, 4)],
        "G": [(0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (1, 0), (1, 2), (1, 4), (2, 0), (2, 2), (2, 4), (3, 0), (3, 2), (3, 4), (4, 0), (4, 2), (4, 3), (4, 4)],
        "I": [(1, 0), (1, 4), (2, 0), (2, 1), (2, 2), (2, 3), (2, 4), (3, 0), (3, 4)],
        "O": [(0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (1, 0), (1, 4), (2, 0), (2, 4), (3, 0), (3, 4), (4, 0), (4, 1), (4, 2), (4, 3), (4, 4)],
        "S": [(0, 0), (0, 1), (0, 2), (0, 4), (1, 0), (1, 2), (1, 4), (2, 0), (2, 2), (2, 4), (3, 0), (3, 2), (3, 4), (4, 0), (4, 2), (4, 3), (4, 4)],
        "T": [(0, 0), (1, 0), (2, 0), (2, 1), (2, 2), (2, 3), (2, 4), (3, 0), (4, 0)],
        "Z": [(0, 0), (0, 4), (1, 0), (1, 3), (1, 4), (2, 0), (2, 2), (2, 4), (3, 0), (3, 1), (3, 4), (4, 0), (4, 4)],
    ]

    /// Columns a word takes: letters at pitch 6, a space is 3.
    static func columns(_ text: String) -> Int {
        var col = 0
        for ch in text.uppercased() { col += ch == " " ? 3 : 6 }
        return max(1, col - 1)
    }

    static func blockRect(_ x: Int, _ y: Int, unit: CGFloat) -> CGRect {
        let off = (cell - block) / 2
        return CGRect(
            x: (CGFloat(x) * cell + off) * unit,
            y: (CGFloat(y) * cell + off) * unit,
            width: block * unit,
            height: block * unit
        )
    }

    /// The mark as a UIImage, for inline use inside a `Text`.
    @MainActor static func markImage(size: CGFloat, fill: UIColor) -> UIImage {
        let renderer = UIGraphicsImageRenderer(size: CGSize(width: size, height: size))
        return renderer.image { _ in
            let unit = size / 50
            fill.setFill()
            for (x, y) in aBlocks {
                UIBezierPath(roundedRect: blockRect(x, y, unit: unit), cornerRadius: 1.5 * unit).fill()
            }
            UIColor(red: 217 / 255, green: 119 / 255, blue: 87 / 255, alpha: 1).setFill()
            UIBezierPath(roundedRect: blockRect(core.0, core.1, unit: unit), cornerRadius: 1.5 * unit).fill()
        }
    }
}

/// The A-block mark: eleven blocks around the coral core.
struct BlockMark: View {
    var fill: Color = StageInk.text
    var size: CGFloat = 20

    var body: some View {
        Canvas { context, canvas in
            let unit = canvas.width / 50
            for (x, y) in BlockType.aBlocks {
                let r = BlockType.blockRect(x, y, unit: unit)
                context.fill(Path(roundedRect: r, cornerRadius: 1.5 * unit), with: .color(fill))
            }
            let core = BlockType.blockRect(BlockType.core.0, BlockType.core.1, unit: unit)
            context.fill(Path(roundedRect: core, cornerRadius: 1.5 * unit), with: .color(BlockType.coral))
        }
        .frame(width: size, height: size)
    }
}

/// A word in A://TERNIT block type, `height` tall.
struct PixelWord: View {
    let text: String
    var fill: Color = StageInk.text
    var height: CGFloat = 13

    var body: some View {
        let cols = BlockType.columns(text)
        Canvas { context, canvas in
            let unit = canvas.height / 50
            var col = 0
            for ch in text.uppercased() {
                if ch == " " {
                    col += 3
                    continue
                }
                for (x, y) in BlockType.glyphs[ch] ?? [] {
                    let r = BlockType.blockRect(col + x, y, unit: unit)
                    context.fill(Path(roundedRect: r, cornerRadius: 1.5 * unit), with: .color(fill))
                }
                col += 6
            }
        }
        .frame(width: CGFloat(cols) * BlockType.cell * height / 50, height: height)
    }
}

// MARK: - Icons (the app's 24 px line icons, drawn small)

enum StageIconName {
    case plus, folder, doc, clock, sliders, chev, inbox, wave, up, check, home, crown
    case spark, chart, monitor, hand, terminal, panel, puzzle, target, search
}

private struct StageIconShape: Shape {
    let name: StageIconName

    func path(in rect: CGRect) -> Path {
        var p = Path()
        func poly(_ pts: [(CGFloat, CGFloat)], close: Bool = false) {
            guard let first = pts.first else { return }
            p.move(to: CGPoint(x: first.0, y: first.1))
            for pt in pts.dropFirst() { p.addLine(to: CGPoint(x: pt.0, y: pt.1)) }
            if close { p.closeSubpath() }
        }
        func line(_ a: (CGFloat, CGFloat), _ b: (CGFloat, CGFloat)) { poly([a, b]) }
        func circle(_ cx: CGFloat, _ cy: CGFloat, _ r: CGFloat) {
            p.addEllipse(in: CGRect(x: cx - r, y: cy - r, width: r * 2, height: r * 2))
        }
        func rrect(_ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat, _ r: CGFloat) {
            p.addRoundedRect(in: CGRect(x: x, y: y, width: w, height: h), cornerSize: CGSize(width: r, height: r))
        }

        switch name {
        case .plus: line((12, 5), (12, 19)); line((5, 12), (19, 12))
        case .folder: poly([(3, 7), (9, 7), (11, 9), (21, 9), (21, 19), (3, 19)], close: true)
        case .doc: poly([(6, 3), (15, 3), (19, 7), (19, 21), (6, 21)], close: true); poly([(14, 3), (14, 8), (19, 8)])
        case .clock: circle(12, 12, 8); poly([(12, 8), (12, 12), (15, 14)])
        case .sliders:
            line((4, 8), (14, 8)); line((18, 8), (20, 8)); line((4, 16), (8, 16)); line((12, 16), (20, 16))
            circle(16, 8, 2); circle(10, 16, 2)
        case .chev: poly([(7, 10), (12, 15), (17, 10)])
        case .inbox: poly([(4, 13), (7, 5), (17, 5), (20, 13), (20, 19), (4, 19)], close: true); poly([(4, 13), (9, 13), (10, 15), (14, 15), (15, 13), (20, 13)])
        case .wave: line((6, 9), (6, 15)); line((9, 6), (9, 18)); line((12, 9), (12, 15)); line((15, 7), (15, 17)); line((18, 10), (18, 14))
        case .up: line((12, 19), (12, 5)); poly([(6, 11), (12, 5), (18, 11)])
        case .check: poly([(5, 12), (10, 17), (19, 7)])
        case .home: poly([(4, 11), (12, 5), (20, 11), (20, 20), (4, 20)], close: true)
        case .crown: poly([(4, 17), (6, 8), (10, 12), (12, 6), (14, 12), (18, 8), (20, 17)], close: true)
        case .spark:
            line((12, 4), (12, 8)); line((12, 16), (12, 20)); line((4, 12), (8, 12)); line((16, 12), (20, 12))
            line((7, 7), (9, 9)); line((15, 15), (17, 17)); line((7, 17), (9, 15)); line((15, 9), (17, 7))
        case .chart: line((6, 18), (6, 10)); line((12, 18), (12, 6)); line((18, 18), (18, 13))
        case .monitor: rrect(3, 5, 18, 11, 2); line((9, 20), (15, 20)); line((12, 16), (12, 20))
        case .hand:
            poly([(8, 12), (8, 6)]); p.addQuadCurve(to: CGPoint(x: 11, y: 6), control: CGPoint(x: 9.5, y: 4)); p.addLine(to: CGPoint(x: 11, y: 11))
            poly([(11, 11), (11, 5)]); p.addQuadCurve(to: CGPoint(x: 14, y: 5), control: CGPoint(x: 12.5, y: 3)); p.addLine(to: CGPoint(x: 14, y: 11))
            poly([(14, 11), (14, 7)]); p.addQuadCurve(to: CGPoint(x: 17, y: 7), control: CGPoint(x: 15.5, y: 5))
            p.addLine(to: CGPoint(x: 17, y: 14))
            p.addQuadCurve(to: CGPoint(x: 11, y: 20), control: CGPoint(x: 17, y: 20))
            p.addLine(to: CGPoint(x: 10, y: 20))
            p.addQuadCurve(to: CGPoint(x: 6, y: 18), control: CGPoint(x: 7.5, y: 20))
            p.addLine(to: CGPoint(x: 3, y: 14))
            p.addQuadCurve(to: CGPoint(x: 5.3, y: 12.1), control: CGPoint(x: 3.5, y: 11.5))
            p.addLine(to: CGPoint(x: 8, y: 14))
        case .terminal: poly([(5, 8), (9, 12), (5, 16)]); line((11, 16), (19, 16))
        case .panel: rrect(4, 5, 16, 14, 2); line((9, 5), (9, 19))
        case .puzzle: poly([(9, 4), (15, 4), (15, 8), (19, 8), (19, 14), (15, 14), (15, 20), (9, 20), (9, 14), (5, 14), (5, 8), (9, 8)], close: true)
        case .target: circle(12, 12, 8); circle(12, 12, 3)
        case .search: circle(11, 11, 6); line((20, 20), (15.5, 15.5))
        }
        let s = min(rect.width, rect.height) / 24
        return p.applying(CGAffineTransform(scaleX: s, y: s).translatedBy(x: rect.minX / s, y: rect.minY / s))
    }
}

struct StageIcon: View {
    let name: StageIconName
    var size: CGFloat = 11
    var color: Color? = nil
    var weight: CGFloat = 1.7

    var body: some View {
        StageIconShape(name: name)
            .stroke(style: StrokeStyle(lineWidth: weight * size / 24, lineCap: .round, lineJoin: .round))
            .foregroundStyle(color.map { AnyShapeStyle($0) } ?? AnyShapeStyle(ForegroundStyle()))
            .frame(width: size, height: size)
    }
}

// MARK: - App chrome

/// One row of a mode's rail.
struct StageRailItem: View {
    let icon: StageIconName
    let label: String
    var isNew = false
    var more = false

    var body: some View {
        HStack(spacing: 7) {
            StageIcon(name: icon, color: isNew ? StageInk.tan : StageInk.brown)
            Text(label).lineLimit(1)
            if more {
                Spacer(minLength: 0)
                Text("···").foregroundStyle(StageInk.faint)
            }
        }
        .font(StageFont.sans(10.5))
        .foregroundStyle(isNew ? StageInk.tan : StageInk.text)
        .padding(.horizontal, 6)
        .frame(maxWidth: .infinity, minHeight: 21, maxHeight: 21, alignment: .leading)
        .overlay(alignment: .leading) {
            if isNew { Rectangle().fill(Color(hex: "#E7D8C9")).frame(width: 1.5) }
        }
        .padding(.bottom, isNew ? 2 : 0)
    }
}

/// An uppercase rail heading with its disclosure chevron ("PROJECTS").
struct StageRailHeading: View {
    let title: String

    var body: some View {
        HStack(spacing: 5) {
            StageIcon(name: .chev, size: 8)
            Text(title.uppercased()).tracking(1)
        }
        .font(StageFont.sans(8.5, .bold))
        .foregroundStyle(StageInk.tan)
        .padding(.horizontal, 4)
        .frame(height: 18)
        .padding(.top, 9)
    }
}

/// The small "Recents" label.
struct StageRailLabel: View {
    let title: String

    var body: some View {
        Text(title)
            .font(StageFont.sans(9.5, .semibold))
            .foregroundStyle(StageInk.tan)
            .padding(.horizontal, 4)
            .frame(height: 18)
            .padding(.top, 9)
    }
}

/// Coral dot with its soft ring.
struct StageDot: View {
    var body: some View {
        Circle()
            .fill(StageInk.coral)
            .frame(width: 5, height: 5)
            .background(Circle().fill(StageInk.coral.opacity(0.15)).frame(width: 11, height: 11))
    }
}

/// A rail placeholder ("No projects yet.") that hands over to the new item.
struct StageRailSwap: View {
    let empty: String
    let item: String
    let at: Double

    var body: some View {
        ZStack(alignment: .topLeading) {
            Text(empty)
                .foregroundStyle(StageInk.tan)
                .lineLimit(1)
                .fixedSize()
                .beat(.fadeOut, at: at)
            HStack(spacing: 5) {
                StageDot()
                Text(item).fontWeight(.semibold).foregroundStyle(StageInk.text)
            }
            .fixedSize()
            .beat(.slideIn, at: at + 0.05)
        }
        .font(StageFont.sans(9))
        .padding(.horizontal, 6)
        .padding(.top, 3)
        .frame(maxWidth: .infinity, minHeight: 16, maxHeight: 16, alignment: .topLeading)
    }
}

/// The app window: nav row, the mode's rail, the account row, and the main area.
struct StageAppWindow<Rail: View, Main: View>: View {
    @ViewBuilder let rail: Rail
    @ViewBuilder let main: Main

    var body: some View {
        HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 1) {
                navRow
                rail
                Spacer(minLength: 0)
                account
            }
            .padding(.top, 8)
            .padding(.horizontal, 7)
            .frame(width: 147, height: StageLayout.mainHeight, alignment: .topLeading)
            .background(Color.white)
            Rectangle().fill(StageInk.hairline).frame(width: 1)
            ZStack(alignment: .topLeading) {
                main
            }
            .frame(width: StageLayout.mainWidth, height: StageLayout.mainHeight, alignment: .topLeading)
            .background(Color.white)
            .clipped()
        }
        .frame(width: StageLayout.windowWidth - 2, height: StageLayout.mainHeight)
        .background(Color.white)
        .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
        .padding(1)
        .background(
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .fill(Color.white)
                .shadow(color: StageInk.shadow.opacity(0.16), radius: 30, y: 26)
                .shadow(color: StageInk.shadow.opacity(0.06), radius: 5, y: 3)
        )
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(StageInk.border, lineWidth: 1))
        .foregroundStyle(StageInk.text)
    }

    private var navRow: some View {
        HStack(spacing: 7) {
            StageIcon(name: .chev, size: 10, weight: 2.4)
                .rotationEffect(.degrees(90))
            StageIcon(name: .chev, size: 10, weight: 2.4)
                .rotationEffect(.degrees(-90))
            RoundedRectangle(cornerRadius: 2).stroke(lineWidth: 1.4).frame(width: 10, height: 8)
            VStack(spacing: 1) {
                ForEach(0..<3, id: \.self) { _ in
                    RoundedRectangle(cornerRadius: 1).fill(Color(hex: "#C2AE9A")).frame(width: 5, height: 2)
                }
            }
            .frame(width: 15, height: 15)
            .background(RoundedRectangle(cornerRadius: 5).fill(Color(hex: "#F3EFEA")))
            .padding(.leading, 6)
        }
        .foregroundStyle(StageInk.faint.opacity(0.7))
        .frame(height: 20)
        .padding(.leading, 2)
        .padding(.bottom, 6)
    }

    private var account: some View {
        HStack(spacing: 6) {
            Text("A")
                .font(StageFont.sans(8))
                .frame(width: 16, height: 16)
                .background(Circle().fill(Color(hex: "#F1EEEA")))
            Text("Account")
            Spacer(minLength: 0)
            StageIcon(name: .inbox, color: StageInk.taupe)
            BlockMark(fill: Color(hex: "#B9A794"), size: 11)
        }
        .font(StageFont.sans(9.5))
        .padding(.horizontal, 9)
        .frame(height: 30)
        .overlay(alignment: .top) { Rectangle().fill(StageInk.hairline).frame(height: 1) }
        .padding(.horizontal, -7)
    }
}

// MARK: - Effects

/// A soft radial glow behind the hero; blooms in, then drifts.
struct StageHalo: View {
    let x: CGFloat
    let y: CGFloat
    let w: CGFloat
    var h: CGFloat? = nil
    let rgb: (Double, Double, Double)
    let alpha: Double

    @Environment(\.stageTime) private var t
    @Environment(\.stageStill) private var still

    var body: some View {
        let color = Color(red: rgb.0 / 255, green: rgb.1 / 255, blue: rgb.2 / 255)
        let height = h ?? w
        // bloom .9 s ease-out at .1 s, then drift 7 s ease-in-out, alternating.
        let bloom = StageCurve.easeOut(StageMotion.progress(t, delay: 0.1, duration: 0.9))
        var dx = 0.0, dy = 0.0, scale = 0.7 + 0.3 * bloom
        if !still, t > 1 {
            let phase = (t - 1) / 7
            var frac = phase.truncatingRemainder(dividingBy: 1)
            if Int(phase) % 2 == 1 { frac = 1 - frac }
            let e = StageCurve.easeInOut(frac)
            dx = 14 * e
            dy = -10 * e
            scale = 1 + 0.06 * e
        }
        return Circle()
            .fill(RadialGradient(
                stops: [.init(color: color.opacity(alpha), location: 0), .init(color: color.opacity(0), location: 0.66)],
                center: .center,
                startRadius: 0,
                endRadius: w / 2 * 1.414
            ))
            .frame(width: w, height: w)
            .scaleEffect(x: 1, y: height / w)
            .frame(width: w, height: height)
            .scaleEffect(scale)
            .offset(x: dx, y: dy)
            .opacity(bloom)
            .place(x, y)
            .allowsHitTesting(false)
    }
}

/// Expanding ring at an action point.
struct StageRing: View {
    let x: CGFloat
    let y: CGFloat
    let size: CGFloat
    let at: Double

    @Environment(\.stageTime) private var t

    var body: some View {
        let p = StageMotion.progress(t, delay: at, duration: 1.6)
        let opacity = p < 0.12 ? StageCurve.easeOut(p / 0.12) : 1 - StageCurve.easeOut((p - 0.12) / 0.88)
        Circle()
            .strokeBorder(Color(red: 217 / 255, green: 119 / 255, blue: 87 / 255, opacity: 0.55), lineWidth: 1.5)
            .frame(width: size, height: size)
            .scaleEffect(0.4 + 2.2 * StageCurve.easeOut(p))
            .opacity(p <= 0 ? 0 : opacity)
            .place(x - size / 2, y - size / 2)
            .allowsHitTesting(false)
    }
}

/// One spark of a burst: offset, colour, spin.
struct StageSpark {
    let dx: CGFloat
    let dy: CGFloat
    let color: Color
    let spin: Double

    init(_ dx: CGFloat, _ dy: CGFloat, _ hex: String, _ spin: Double) {
        self.dx = dx
        self.dy = dy
        self.color = Color(hex: hex)
        self.spin = spin
    }
}

/// A burst of block squares from an action point.
struct StageBurst: View {
    let x: CGFloat
    let y: CGFloat
    let at: Double
    let sparks: [StageSpark]

    @Environment(\.stageTime) private var t

    var body: some View {
        let p = StageMotion.progress(t, delay: at, duration: 0.95)
        let e = StageCurve.burst(p)
        let opacity = p <= 0 ? 0 : (p < 0.12 ? StageCurve.burst(p / 0.12) : 1 - StageCurve.burst((p - 0.12) / 0.88))
        ZStack(alignment: .topLeading) {
            ForEach(sparks.indices, id: \.self) { i in
                let s = sparks[i]
                RoundedRectangle(cornerRadius: 1.6)
                    .fill(s.color)
                    .frame(width: 7, height: 7)
                    .scaleEffect(0.5 + 0.5 * e)
                    .rotationEffect(.degrees(s.spin * e))
                    .offset(x: s.dx * e - 3.5, y: s.dy * e - 3.5)
            }
        }
        .opacity(opacity)
        .place(x, y)
        .allowsHitTesting(false)
    }
}

/// A placeholder that clears while the real text types itself into the field
/// (steps(n) over 1.15 s, caret blinking three times).
struct StageTyping: View {
    let placeholder: String
    let text: String
    let at: Double
    var font: Font = StageFont.sans(11)

    @Environment(\.stageTime) private var t

    var body: some View {
        let local = t - at
        let count = Int(StageMotion.progress(t, delay: at, duration: 1.15) * Double(text.count))
        let caret = local >= 0 && local < 2.1 && local.truncatingRemainder(dividingBy: 0.7) < 0.35
        ZStack(alignment: .topLeading) {
            Text(placeholder)
                .foregroundStyle(Color(hex: "#A79C93"))
                .beat(.fadeOut, at: at)
            HStack(spacing: 0) {
                Text(String(text.prefix(count)))
                    .foregroundStyle(StageInk.text)
                Rectangle()
                    .fill(caret ? StageInk.text : .clear)
                    .frame(width: 1.5, height: 12)
            }
        }
        .font(font)
        .lineLimit(1)
        .fixedSize()
    }
}

/// "Allternit Cloud · Kimi K3 ˅"
struct StageModelPicker: View {
    let label: String
    var mono = false

    var body: some View {
        HStack(spacing: 4) {
            BlockMark(fill: StageInk.amber, size: 9)
            Text(label).font(mono ? StageFont.mono(9.5) : StageFont.sans(9.5, .medium))
            StageIcon(name: .chev, size: 8)
        }
        .foregroundStyle(StageInk.text)
        .lineLimit(1)
        .fixedSize()
    }
}

/// The round send button.
struct StageSend: View {
    let at: Double

    var body: some View {
        StageIcon(name: .up, color: .white)
            .frame(width: 22, height: 22)
            .background(Circle().fill(StageInk.text))
            .beat(.press, at: at)
    }
}

/// A small inline thinking orb (web AllternitOrb).
struct StageOrb: View {
    var state: ThinkingOrbState = .working
    let size: CGFloat

    var body: some View {
        ThinkingOrb(state: state, size: .inline)
            .scaleEffect(size / ThinkingOrbSize.inline.rawValue)
            .frame(width: size, height: size)
    }
}

/// The square "+" button in composers.
struct StagePlusButton: View {
    var body: some View {
        StageIcon(name: .plus, color: StageInk.taupe)
            .frame(width: 20, height: 20)
            .background(RoundedRectangle(cornerRadius: 6).fill(StageInk.chip))
    }
}

/// Green check in a rounded square.
struct StageCheckBadge: View {
    let size: CGFloat
    let radius: CGFloat
    let icon: CGFloat

    var body: some View {
        StageIcon(name: .check, size: icon, color: .white)
            .frame(width: size, height: size)
            .background(RoundedRectangle(cornerRadius: radius).fill(StageInk.green))
    }
}

/// Vertical separator in composer rows.
struct StageVSep: View {
    var body: some View {
        Rectangle().fill(StageInk.border).frame(width: 1, height: 14)
    }
}
