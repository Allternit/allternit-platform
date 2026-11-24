import SwiftUI

// Docking block title (contract D6, D11, D13). The page title is drawn in the
// block type; as the page scrolls the letters hop one by one into the nav bar.
// Every block is drawn in one Canvas, driven by the scroll offset (no per-pixel
// view state). The per-letter springs of the web version are replaced by the
// direct scroll-driven mapping.

/// Pure docking math, ported from CONTRACT.md `dockFrame`.
enum DockMath {
    nonisolated static func ease(_ v: Double) -> Double {
        v < 0.5 ? 4 * v * v * v : 1 - pow(-2 * v + 2, 3) / 2
    }
    nonisolated static func clamp(_ v: Double) -> Double { min(1, max(0, v)) }

    struct Frame: Equatable {
        var t: Double
        var landed: Int
        var docked: Bool
        var reducedDone: Bool
        var bandFactor: Double
        var descOpacity: Double
        var descBlur: Double
        var descDrift: Double
    }

    nonisolated static func frame(y: CGFloat, n: Int, reduced: Bool) -> Frame {
        let dock = Double(PolishTokens.dock)
        let y = Double(max(0, y))
        let t = clamp(y / dock)
        if reduced {
            let done = y >= dock / 2
            return Frame(t: t, landed: done ? n : 0, docked: done, reducedDone: done,
                         bandFactor: done ? 0 : 1, descOpacity: done ? 0 : 1, descBlur: 0, descDrift: 0)
        }
        let landed = min(n, Int(floor(t * (Double(n) + 0.6) + 0.0001)))
        let f = min(1, y / (dock * Double(PolishTokens.descriptionFadeFraction)))
        return Frame(t: t, landed: landed, docked: t >= 1, reducedDone: false,
                     bandFactor: 1 - ease(clamp((t - 0.82) / 0.18)),
                     descOpacity: 1 - f, descBlur: f * Double(PolishTokens.descriptionBlur),
                     descDrift: y * Double(PolishTokens.descriptionDrift))
    }

    /// Letter progress for letter `l`.
    nonisolated static func progress(t: Double, n: Int, letter l: Int) -> Double {
        ease(clamp(t * (Double(n) + 0.6) - Double(l)))
    }
}

/// Big (wrapped) and docked (one row) cells of a title, paired by order.
struct DockTitleLayout {
    struct Pair {
        let bigCol: Int, bigRow: Int, line: Int, letter: Int
        let smallCol: Int, smallRow: Int
    }
    let pairs: [Pair]
    let lines: Int
    let letters: Int
    let smallCols: Int

    init(text: String) {
        let wrapped = BlockText.lines(for: text, height: PolishTokens.bigHeight, maxWidth: PolishTokens.titleMaxWidth)
        var big: [(col: Int, row: Int, line: Int, letter: Int)] = []
        var offset = 0
        for (li, line) in wrapped.enumerated() {
            let r = BlockText.cells(for: line)
            var count = 0
            for c in r.cells {
                big.append((c.col, c.row, li, c.letter + offset))
                count = max(count, c.letter + 1)
            }
            offset += count
        }
        let small = BlockText.cells(for: text)
        self.pairs = zip(big, small.cells).map {
            Pair(bigCol: $0.col, bigRow: $0.row, line: $0.line, letter: $0.letter,
                 smallCol: $1.col, smallRow: $1.row)
        }
        self.lines = wrapped.count
        self.letters = offset
        self.smallCols = small.cols
    }

    var titleHeight: CGFloat {
        CGFloat(lines) * PolishTokens.bigHeight + CGFloat(max(0, lines - 1)) * (PolishTokens.lineGap - PolishTokens.bigHeight)
    }
}

private struct DockOffsetKey: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}

/// A scroll page with a big block title that docks into the nav bar's
/// principal area. Put it inside a NavigationStack. The nav bar is forced
/// inline; the docked title is drawn by this view over the bar.
struct DockingTitleScrollView<Content: View>: View {
    let title: String
    var description: String?
    /// Freeweights uses a taller header: top 112, bar centre 75 in the mockup.
    var taller = false
    @ViewBuilder var content: () -> Content

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var offset: CGFloat = 0
    @State private var flashStart: Date?
    @State private var layout: DockTitleLayout

    init(title: String, description: String? = nil, taller: Bool = false,
         @ViewBuilder content: @escaping () -> Content) {
        self.title = title
        self.description = description
        self.taller = taller
        self.content = content
        _layout = State(initialValue: DockTitleLayout(text: title))
    }

    private var frame: DockMath.Frame {
        DockMath.frame(y: offset, n: layout.letters, reduced: reduceMotion)
    }

    var body: some View {
        GeometryReader { geo in
            let safeTop = geo.safeAreaInsets.top
            let f = frame
            ZStack(alignment: .topLeading) {
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        Color.clear.frame(height: 0).background(
                            GeometryReader { m in
                                Color.clear.preference(
                                    key: DockOffsetKey.self,
                                    value: safeTop - m.frame(in: .global).minY)
                            })
                        // Room for the title the canvas draws.
                        Color.clear.frame(height: layout.titleHeight + 8 + 12)
                        if let description {
                            Text(description)
                                .font(.system(size: 12.5))
                                .foregroundStyle(Color("TextSecondary"))
                                .lineSpacing(3)
                                .padding(.bottom, 14)
                                .dockDescription(frame: f)
                        }
                        content()
                    }
                    .padding(.horizontal, 14)
                }
                .modifier(DockScrollReader(offset: $offset))
                .onPreferenceChange(DockOffsetKey.self) { v in
                    if #unavailable(iOS 18.0) { offset = v }
                }

                titleLayer(frame: f, width: geo.size.width, safeTop: safeTop)
            }
            .background(Color("BgPrimary").ignoresSafeArea())
        }
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .principal) {
                Color.clear.frame(width: 1, height: 22).accessibilityHidden(true)
            }
        }
        .onChange(of: frame.landed) { old, new in
            if new == layout.letters, new > 0 { Haptics.soft() }
            else if new > old { Haptics.selection() }
        }
        .onChange(of: frame.docked) { _, docked in
            guard docked, !reduceMotion else { flashStart = nil; return }
            let start = Date()
            flashStart = start
            let total = PolishTokens.flashDuration + Double(layout.smallCols) * PolishTokens.flashDelayPerColumn + 0.1
            Task { @MainActor in
                try? await Task.sleep(for: .seconds(total))
                if flashStart == start { flashStart = nil }
            }
        }
        .onChange(of: title) { _, new in layout = DockTitleLayout(text: new) }
    }

    @ViewBuilder
    private func titleLayer(frame f: DockMath.Frame, width: CGFloat, safeTop: CGFloat) -> some View {
        let top = safeTop + 44 + 8
        let hc = safeTop + 22
        let hairline = f.docked
        ZStack(alignment: .topLeading) {
            // Band holds the waiting letters and collapses over t .82 to 1.
            let bandH = (layout.titleHeight + PolishTokens.bandPad) * f.bandFactor
            LinearGradient(stops: [.init(color: Color("BgPrimary"), location: 0.8),
                                   .init(color: Color("BgPrimary").opacity(0), location: 1)],
                           startPoint: .top, endPoint: .bottom)
                .frame(width: width, height: max(0, bandH))
                .offset(y: top - 8)
            if reduceMotion {
                letters(fixed: 0, f: f, width: width, top: top, hc: hc, flash: nil)
                    .opacity(f.reducedDone ? 0 : 1)
                letters(fixed: 1, f: f, width: width, top: top, hc: hc, flash: nil)
                    .opacity(f.reducedDone ? 1 : 0)
            } else {
                TimelineView(.animation(paused: flashStart == nil)) { tl in
                    letters(fixed: nil, f: f, width: width, top: top, hc: hc,
                            flash: flashStart.map { tl.date.timeIntervalSince($0) })
                }
            }
            Rectangle().fill(PolishTokens.glassBorder).frame(width: width, height: 0.5)
                .offset(y: safeTop + 44).opacity(hairline ? 1 : 0)
        }
        .frame(width: width, alignment: .topLeading)
        .animation(.easeInOut(duration: 0.2), value: f.reducedDone)
        .animation(.easeInOut(duration: 0.2), value: hairline)
        .ignoresSafeArea()
        .allowsHitTesting(false)
        .accessibilityElement()
        .accessibilityLabel(title)
        .accessibilityAddTraits(f.docked ? .isHeader : [])
    }

    private func letters(fixed: Double?, f: DockMath.Frame, width: CGFloat,
                         top: CGFloat, hc: CGFloat, flash: TimeInterval?) -> some View {
        let n = layout.letters
        let pairs = layout.pairs
        let scrollY = offset
        return Canvas { ctx, _ in
            let bigB = PolishTokens.bigHeight / 5
            let smallS = PolishTokens.smallHeight / 5
            let sx0 = width / 2 - CGFloat(layout.smallCols) * smallS / 2
            let sy0 = hc - PolishTokens.smallHeight / 2
            let ratio = CGFloat(BlockGlyphs.block / BlockGlyphs.cell)
            let radius = CGFloat(BlockGlyphs.radius / BlockGlyphs.cell)
            let scroll = max(0, scrollY - PolishTokens.dock)
            let ink = PolishTokens.ink
            let coral = PolishTokens.coral
            for p in pairs {
                let pk = fixed ?? DockMath.progress(t: f.t, n: n, letter: p.letter)
                let x0 = 14 + CGFloat(p.bigCol) * bigB
                let y0 = top - scroll + CGFloat(p.line) * PolishTokens.lineGap + CGFloat(p.bigRow) * bigB
                let x1 = sx0 + CGFloat(p.smallCol) * smallS
                let y1 = sy0 + CGFloat(p.smallRow) * smallS
                let k = CGFloat(pk)
                let hop = fixed == nil ? -CGFloat(sin(Double.pi * pk)) * CGFloat(8 + (p.letter % 3) * 3) : 0
                let tilt = fixed == nil ? sin(Double.pi * pk) * (p.letter % 2 == 1 ? 18 : -18) : 0
                let sz = bigB + (smallS - bigB) * k
                let cx = x0 + (x1 - x0) * k + sz / 2
                let cy = y0 + (y1 - y0) * k + hop + sz / 2
                var color = ink
                if fixed == nil {
                    if pk > 0 && pk < 1 { color = coral }
                    else if let flash, f.docked {
                        let u = flash - Double(p.smallCol) * PolishTokens.flashDelayPerColumn
                        let w = DockMath.clamp(u / PolishTokens.flashDuration)
                        if w > 0 && w < 1 { color = coral.opacity(sin(Double.pi * w)) }
                    }
                }
                let side = sz * ratio
                var c = ctx
                c.translateBy(x: cx, y: cy)
                c.rotate(by: .degrees(tilt))
                let rect = CGRect(x: -side / 2, y: -side / 2, width: side, height: side)
                let path = Path(roundedRect: rect, cornerSize: CGSize(width: sz * radius, height: sz * radius))
                c.fill(path, with: .color(ink))
                if color != ink { c.fill(path, with: .color(color)) }
            }
        }
    }
}

/// Reads the scroll offset: `onScrollGeometryChange` on iOS 18+; on iOS 17 the
/// page reports it through a preference key instead.
private struct DockScrollReader: ViewModifier {
    @Binding var offset: CGFloat
    func body(content: Content) -> some View {
        if #available(iOS 18.0, *) {
            content.onScrollGeometryChange(for: CGFloat.self) { g in
                g.contentOffset.y + g.contentInsets.top
            } action: { _, new in
                offset = max(0, new)
            }
        } else {
            content
        }
    }
}

private struct DockDescriptionModifier: ViewModifier {
    let frame: DockMath.Frame
    func body(content: Content) -> some View {
        content
            .opacity(frame.descOpacity)
            .blur(radius: frame.descBlur)
            .offset(y: frame.descDrift)
            .accessibilityHidden(frame.descOpacity < 0.05)
    }
}

extension View {
    /// Fade, blur and drift for the grey line under a docking title.
    fileprivate func dockDescription(frame: DockMath.Frame) -> some View {
        modifier(DockDescriptionModifier(frame: frame))
    }
}
