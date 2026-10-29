import SwiftUI

/// The picture on a "Make something new" tile (allternit-ai
/// src/views/library/ArtifactMotionTile.tsx, at rest): the finished thing on
/// the Allternit cream plate, with the kind's stamp (ArtifactKindIcon) in
/// the corner. Scenes are drawn on the web's 220×154 stage and scaled to the
/// tile (phone row scale 0.582).
struct ArtifactCreateTileArt: View {
    let kind: ArtifactStudioKind

    private static let stage = CGSize(width: 220, height: 154)
    private static let ink = Color(hex: "#141413")
    private static let coral = Color(hex: "#D97757")
    private static let cream = Color(hex: "#FDF8F3")

    var body: some View {
        Canvas { context, size in
            let scale = 0.582
            context.scaleBy(x: scale, y: scale)
            let k = kind.tileColor
            context.fill(Path(CGRect(origin: .zero, size: Self.stage)), with: .color(Self.cream))
            // Dot grid (radial-gradient 1.1px dots every 12px).
            var dots = Path()
            var y = 6.0
            while y < Self.stage.height {
                var x = 6.0
                while x < Self.stage.width {
                    dots.addEllipse(in: CGRect(x: x - 1.1, y: y - 1.1, width: 2.2, height: 2.2))
                    x += 12
                }
                y += 12
            }
            context.fill(dots, with: .color(Self.ink.opacity(0.07)))
            Self.drawScene(kind, k: k, in: &context)
            Self.drawStamp(kind, in: &context)
        }
        .accessibilityHidden(true)
    }

    // MARK: - Primitives

    private enum Fill { case page, ink, soft, k, coral, hl }

    private static func box(_ ctx: inout GraphicsContext, _ x: Double, _ y: Double, _ w: Double, _ h: Double,
                            _ fill: Fill, k: Color, radius: Double? = nil, opacity: Double = 1) {
        let rect = CGRect(x: x, y: y, width: w, height: h)
        switch fill {
        case .page:
            let path = Path(roundedRect: rect, cornerRadius: radius ?? 8)
            var shadow = ctx
            shadow.addFilter(.shadow(color: .black.opacity(0.06), radius: 7, y: 4))
            shadow.fill(path, with: .color(.white))
        case .ink:
            ctx.fill(Path(roundedRect: rect, cornerRadius: radius ?? 3), with: .color(ink.opacity(opacity)))
        case .soft:
            ctx.fill(Path(roundedRect: rect, cornerRadius: radius ?? 3), with: .color(ink.opacity(0.2 * opacity)))
        case .k:
            ctx.fill(Path(roundedRect: rect, cornerRadius: radius ?? 3), with: .color(k.opacity(opacity)))
        case .coral:
            ctx.fill(Path(roundedRect: rect, cornerRadius: radius ?? 2), with: .color(coral.opacity(opacity)))
        case .hl:
            ctx.fill(Path(roundedRect: rect, cornerRadius: radius ?? 3), with: .color(coral.opacity(0.28)))
        }
    }

    /// The page's coloured top edge.
    private static func band(_ ctx: inout GraphicsContext, _ x: Double, _ y: Double, _ w: Double, k: Color) {
        let path = Path(roundedRect: CGRect(x: x, y: y, width: w, height: 5),
                        cornerRadii: RectangleCornerRadii(topLeading: 8, topTrailing: 8))
        ctx.fill(path, with: .color(k))
    }

    // MARK: - Scenes (rest state of each web scene)

    private static func drawScene(_ kind: ArtifactStudioKind, k: Color, in ctx: inout GraphicsContext) {
        switch kind {
        case .docs:
            box(&ctx, 36, 16, 120, 150, .page, k: k); band(&ctx, 36, 16, 120, k: k)
            for (i, (y, w)) in zip([38.0, 51, 64, 77, 90], [70.0, 86, 78, 82, 52]).enumerated() {
                box(&ctx, 48, y, w, 5, i == 0 ? .ink : .soft, k: k)
            }
        case .sheets:
            box(&ctx, 30, 18, 130, 118, .page, k: k); band(&ctx, 30, 18, 130, k: k)
            let filled: Set<[Int]> = [[1, 0], [3, 1], [0, 2], [4, 3], [2, 4], [1, 5]]
            for r in 0..<6 {
                for q in 0..<5 {
                    box(&ctx, 40 + Double(q) * 24, 32 + Double(r) * 16, 18, 10, filled.contains([q, r]) ? .k : .soft, k: k)
                }
            }
        case .slides:
            for (x, y) in [(48.0, 34.0), (58, 44), (68, 54)] {
                box(&ctx, x, y, 104, 64, .page, k: k); band(&ctx, x, y, 104, k: k)
                box(&ctx, x + 10, y + 14, 46, 5, .ink, k: k)
                box(&ctx, x + 10, y + 26, 84, 26, .k, k: k, opacity: 0.25)
            }
        case .dashboard:
            box(&ctx, 22, 16, 150, 124, .page, k: k); band(&ctx, 22, 16, 150, k: k)
            box(&ctx, 34, 30, 40, 5, .ink, k: k); box(&ctx, 84, 30, 36, 5, .soft, k: k)
            for (i, h) in [30.0, 50, 38, 64, 48].enumerated() {
                box(&ctx, 36 + Double(i) * 26, 128 - h, 18, h, i == 3 ? .k : .ink, k: k, opacity: 0.85)
            }
        case .ops:
            box(&ctx, 22, 16, 150, 124, .page, k: k); band(&ctx, 22, 16, 150, k: k)
            box(&ctx, 34, 30, 52, 6, .ink, k: k); box(&ctx, 92, 30, 26, 6, .k, k: k, radius: 6, opacity: 0.5)
            for i in 0..<5 {
                box(&ctx, 34 + Double(i) * 26, 46, 22, 4, i < 4 ? .k : .ink, k: k)
            }
            for (i, y) in [62.0, 80, 98, 116].enumerated() {
                box(&ctx, 34, y, 8, 8, .k, k: k, radius: 4)
                box(&ctx, 48, y + 2, [70.0, 58, 84, 46][i], 4, .soft, k: k)
            }
        case .motion:
            box(&ctx, 16, 22, 188, 100, .page, k: k); band(&ctx, 16, 22, 188, k: k)
            for (i, x) in [28.0, 72, 116, 160].enumerated() {
                box(&ctx, x, 38, 34, 22, i == 0 ? .k : .ink, k: k, radius: 4, opacity: i == 0 ? 1 : 0.85)
            }
            box(&ctx, 28, 84, 158, 3, .soft, k: k)
            box(&ctx, 186, 76, 3, 18, .coral, k: k)
        case .design:
            box(&ctx, 18, 14, 184, 128, .page, k: k); band(&ctx, 18, 14, 184, k: k)
            box(&ctx, 42, 30, 38, 38, .k, k: k, radius: 19)
            box(&ctx, 98, 60, 62, 40, .ink, k: k, radius: 6, opacity: 0.85)
            box(&ctx, 42, 110, 90, 6, .soft, k: k); box(&ctx, 42, 122, 60, 6, .soft, k: k)
        case .page:
            box(&ctx, 24, 14, 172, 128, .page, k: k)
            box(&ctx, 32, 22, 4, 4, .soft, k: k, radius: 2); box(&ctx, 40, 22, 4, 4, .soft, k: k, radius: 2)
            box(&ctx, 34, 34, 150, 10, .ink, k: k)
            box(&ctx, 34, 52, 92, 40, .k, k: k, opacity: 0.3)
            box(&ctx, 132, 52, 52, 40, .soft, k: k)
            box(&ctx, 34, 100, 50, 12, .coral, k: k)
        case .build:
            for r in 0..<2 {
                for c in 0..<3 {
                    let dark = (r + c) % 2 == 0
                    box(&ctx, 52 + Double(c) * 40, 34 + Double(r) * 42, 34, 34, dark ? .ink : .soft, k: k, radius: 8, opacity: dark ? 0.85 : 1)
                }
            }
        case .research:
            box(&ctx, 30, 14, 140, 132, .page, k: k); band(&ctx, 30, 14, 140, k: k)
            for (i, y) in [32.0, 46, 60, 74].enumerated() {
                box(&ctx, 42, y, 110, 5, .soft, k: k)
                if i != 2 { box(&ctx, 42, y - 2, 70 + Double(i) * 10, 9, .hl, k: k) }
            }
            box(&ctx, 42, 98, 96, 6, .ink, k: k); box(&ctx, 42, 110, 70, 6, .ink, k: k)
        case .image:
            box(&ctx, 24, 14, 172, 122, .page, k: k)
            let art = CGRect(x: 24, y: 14, width: 172, height: 122)
            ctx.drawLayer { layer in
                layer.clip(to: Path(roundedRect: art, cornerRadius: 8))
                layer.fill(Path(art), with: .linearGradient(
                    Gradient(colors: [Color(hex: "#FDE9D2"), Color(hex: "#FFF7ED")]),
                    startPoint: CGPoint(x: art.midX, y: art.minY), endPoint: CGPoint(x: art.midX, y: art.maxY)))
                layer.fill(Path(ellipseIn: CGRect(x: 142, y: 32, width: 26, height: 26)), with: .color(coral))
                let top = art.maxY - 70
                let pts: [(Double, Double)] = [(0, 1), (0, 0.55), (0.22, 0.2), (0.40, 0.6), (0.62, 0.1), (0.85, 0.5), (1, 0.3), (1, 1)]
                var mountain = Path()
                for (i, p) in pts.enumerated() {
                    let point = CGPoint(x: art.minX + p.0 * art.width, y: top + p.1 * 70)
                    if i == 0 { mountain.move(to: point) } else { mountain.addLine(to: point) }
                }
                mountain.closeSubpath()
                layer.fill(mountain, with: .color(ink.opacity(0.85)))
            }
        case .video:
            box(&ctx, 30, 20, 64, 40, .ink, k: k, radius: 5)
            let clap = CGRect(x: 30, y: 12, width: 64, height: 9)
            ctx.drawLayer { layer in
                layer.clip(to: Path(roundedRect: clap, cornerRadius: 3))
                layer.fill(Path(clap), with: .color(cream))
                var x = clap.minX - 12
                while x < clap.maxX + 12 {
                    var stripe = Path()
                    stripe.move(to: CGPoint(x: x, y: clap.minY))
                    stripe.addLine(to: CGPoint(x: x + 6 * 1.414, y: clap.minY))
                    stripe.addLine(to: CGPoint(x: x + 6 * 1.414 + 9, y: clap.maxY))
                    stripe.addLine(to: CGPoint(x: x + 9, y: clap.maxY))
                    stripe.closeSubpath()
                    layer.fill(stripe, with: .color(ink))
                    x += 12 * 1.414
                }
            }
            box(&ctx, 104, 28, 96, 72, .page, k: k)
            var play = Path()
            play.move(to: CGPoint(x: 142, y: 50))
            play.addLine(to: CGPoint(x: 160, y: 61))
            play.addLine(to: CGPoint(x: 142, y: 72))
            play.closeSubpath()
            ctx.fill(play, with: .color(k))
            box(&ctx, 110, 108, 84, 4, .k, k: k)
        }
    }

    // MARK: - Stamp (ArtifactKindIcon plate, 24px at (12,12))

    private static let letters: [String: [[Int]]] = [
        "D": [[0,0],[0,1],[0,2],[0,3],[0,4],[1,0],[2,0],[3,0],[4,1],[4,2],[4,3],[1,4],[2,4],[3,4]],
        "S": [[0,0],[1,0],[2,0],[3,0],[4,0],[0,1],[0,2],[1,2],[2,2],[3,2],[4,2],[4,3],[0,4],[1,4],[2,4],[3,4],[4,4]],
        "P": [[0,0],[1,0],[2,0],[3,0],[0,1],[4,1],[0,2],[1,2],[2,2],[3,2],[0,3],[0,4]],
        "M": [[0,0],[4,0],[0,1],[1,1],[3,1],[4,1],[0,2],[2,2],[4,2],[0,3],[4,3],[0,4],[4,4]],
        "W": [[0,0],[4,0],[0,1],[4,1],[0,2],[2,2],[4,2],[0,3],[1,3],[3,3],[4,3],[0,4],[4,4]],
        "B": [[0,0],[0,1],[0,2],[0,3],[0,4],[1,0],[2,0],[3,0],[4,1],[1,2],[2,2],[3,2],[4,3],[1,4],[2,4],[3,4]],
        "R": [[0,0],[1,0],[2,0],[3,0],[0,1],[4,1],[0,2],[1,2],[2,2],[3,2],[0,3],[2,3],[0,4],[3,4],[4,4]],
        "I": [[1,0],[2,0],[3,0],[2,1],[2,2],[2,3],[1,4],[2,4],[3,4]],
        "O": [[1,0],[2,0],[3,0],[0,1],[4,1],[0,2],[4,2],[0,3],[4,3],[1,4],[2,4],[3,4]],
        "V": [[0,0],[4,0],[0,1],[4,1],[0,2],[4,2],[1,3],[3,3],[2,4]],
    ]

    private static func drawStamp(_ kind: ArtifactStudioKind, in ctx: inout GraphicsContext) {
        let spec = kind.stamp
        var c = ctx
        c.translateBy(x: 12, y: 12)
        c.scaleBy(x: 0.5, y: 0.5) // 48-unit icon grid → 24px
        var shadowed = c
        shadowed.addFilter(.shadow(color: .black.opacity(0.14), radius: 2, y: 1))
        let plate = Path(roundedRect: CGRect(x: 0.6, y: 0.6, width: 46.8, height: 46.8), cornerRadius: 11)
        shadowed.fill(plate, with: .color(cream))
        c.stroke(plate, with: .color(Color(hex: "#E7DDD2")), lineWidth: 1.2)
        c.fill(Path(roundedRect: CGRect(x: 20, y: 8, width: 22, height: 30), cornerRadius: 3),
               with: .linearGradient(Gradient(colors: [spec.from, spec.to]), startPoint: CGPoint(x: 20, y: 8), endPoint: CGPoint(x: 42, y: 38)))
        c.fill(Path(roundedRect: CGRect(x: 22, y: 10, width: 18, height: 26), cornerRadius: 2), with: .color(.white.opacity(0.95)))
        for (i, w) in [10.0, 10, 7].enumerated() {
            c.fill(Path(roundedRect: CGRect(x: 25.7, y: 17 + Double(i) * 5 - 0.8, width: w, height: 1.6), cornerRadius: 0.8), with: .color(spec.to))
        }
        for cell in letters[spec.letter] ?? [] {
            c.fill(Path(roundedRect: CGRect(x: 4.6 + Double(cell[0]) * 2.5, y: 25.2 + Double(cell[1]) * 2.5, width: 2.125, height: 2.125), cornerRadius: 0.375), with: .color(ink))
        }
        c.fill(Path(roundedRect: CGRect(x: 5.2, y: 7.2, width: 4.42, height: 4.42), cornerRadius: 0.78), with: .color(coral))
    }
}

extension ArtifactStudioKind {
    /// The scene's kind colour (`col` in ArtifactMotionTile's scenes).
    var tileColor: Color {
        switch self {
        case .docs: return Color(hex: "#2F6FD0")
        case .sheets: return Color(hex: "#16884F")
        case .slides: return Color(hex: "#D2542F")
        case .dashboard: return Color(hex: "#4F46E5")
        case .ops: return Color(hex: "#047857")
        case .motion: return Color(hex: "#7C3AED")
        case .design: return Color(hex: "#B0268F")
        case .page: return Color(hex: "#0369A1")
        case .build: return Color(hex: "#334155")
        case .research: return Color(hex: "#0F8A8A")
        case .image: return Color(hex: "#C2410C")
        case .video: return Color(hex: "#BE123C")
        }
    }

    /// ArtifactKindIcon SPECS: page-card gradient and block letter.
    fileprivate var stamp: (from: Color, to: Color, letter: String) {
        switch self {
        case .docs: return (Color(hex: "#3B8AE0"), Color(hex: "#185ABD"), "D")
        case .sheets: return (Color(hex: "#2BB673"), Color(hex: "#107C41"), "S")
        case .slides: return (Color(hex: "#E06543"), Color(hex: "#C43E1C"), "P")
        case .design: return (Color(hex: "#EC4899"), Color(hex: "#A21CAF"), "D")
        case .page: return (Color(hex: "#38BDF8"), Color(hex: "#0369A1"), "W")
        case .build: return (Color(hex: "#64748B"), Color(hex: "#1E293B"), "B")
        case .dashboard: return (Color(hex: "#818CF8"), Color(hex: "#4338CA"), "D")
        case .ops: return (Color(hex: "#34D399"), Color(hex: "#047857"), "O")
        case .motion: return (Color(hex: "#C084FC"), Color(hex: "#7E22CE"), "M")
        case .research: return (Color(hex: "#14B8A6"), Color(hex: "#0E7490"), "R")
        case .image: return (Color(hex: "#FBBF24"), Color(hex: "#C2410C"), "I")
        case .video: return (Color(hex: "#FB7185"), Color(hex: "#9F1239"), "V")
        }
    }
}
