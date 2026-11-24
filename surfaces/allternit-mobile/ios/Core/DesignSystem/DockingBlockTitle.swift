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
/// With a mark (T5) the A:// mark is letter 0 on both rows, a 1-column gap
/// follows it, and the text starts at column 16. A mark title never wraps (T6).
struct DockTitleLayout {
    struct Pair {
        let bigCol: Int, bigRow: Int, line: Int, letter: Int
        let smallCol: Int, smallRow: Int
        var isMark = false
        var isCore = false
    }
    let pairs: [Pair]
    let lines: Int
    let letters: Int
    let smallCols: Int
    /// Column count of the (single) big row of a mark title.
    let bigCols: Int
    let hasMark: Bool

    /// First text column after the mark (15 mark columns + 1 gap).
    nonisolated static let markTextColumn = 16

    init(text: String, mark: Bool = false) {
        hasMark = mark
        if mark {
            // One line, big and small share the same cells; only the scale differs.
            var out: [Pair] = []
            for c in PixelWordmark.markCells {
                out.append(Pair(bigCol: c.0, bigRow: c.1, line: 0, letter: 0,
                                smallCol: c.0, smallRow: c.1, isMark: true,
                                isCore: c.0 == PixelWordmark.coreCell.0 && c.1 == PixelWordmark.coreCell.1))
            }
            let r = BlockText.cells(for: text)
            for c in r.cells {
                let col = c.col + Self.markTextColumn
                out.append(Pair(bigCol: col, bigRow: c.row, line: 0, letter: c.letter + 1,
                                smallCol: col, smallRow: c.row))
            }
            pairs = out
            lines = 1
            letters = r.cells.isEmpty ? 1 : (r.cells.map(\.letter).max() ?? 0) + 2
            smallCols = r.cols > 0 ? Self.markTextColumn + r.cols : 15
            bigCols = smallCols
            return
        }
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
        self.bigCols = small.cols
    }

    /// Big letter height for a view `width` wide (T6): a mark title that is
    /// wider than width - 28 shrinks instead of wrapping. Other titles keep 22.
    func bigHeight(width: CGFloat) -> CGFloat {
        guard hasMark, bigCols > 0 else { return PolishTokens.bigHeight }
        return min(PolishTokens.bigHeight, max(8, (width - 28) * 5 / CGFloat(bigCols)))
    }

    func titleHeight(width: CGFloat) -> CGFloat {
        let bh = bigHeight(width: width)
        return CGFloat(lines) * bh + CGFloat(max(0, lines - 1)) * (bh * 1.36 - bh)
    }

    var titleHeight: CGFloat { titleHeight(width: .infinity) }
}

/// Where the docked title sits (T11).
enum DockChrome: Equatable {
    /// Today's behaviour: an inline navigation bar, the title docks into it.
    case navigationBar
    /// No navigation bar. The docked title is drawn over a custom header
    /// (PhoneHeader, ACIViewHeader). `barCenter` is the y of the bar's centre,
    /// measured from the top of the screen.
    case overlay(barCenter: CGFloat)
}

private struct DockOffsetKey: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}

/// A scroll page with a big block title that docks into the nav bar's
/// principal area. Put it inside a NavigationStack. With the default chrome the
/// nav bar is forced inline; the docked title is drawn by this view over the bar.
///
/// - `markLabel`: when set, the title starts with the A:// mark as one letter
///   (core always coral, skipped by the docked flash) and this is its
///   accessibility label. Home is the only user.
/// - `chrome`: `.navigationBar` (default) or `.overlay(barCenter:)`.
struct DockingTitleScrollView<Content: View>: View {
    let title: String
    var description: String?
    /// Freeweights uses a taller header: top 112, bar centre 75 in the mockup.
    var taller = false
    var markLabel: String?
    var chrome: DockChrome = .navigationBar
    /// True when `content` is List rows (see `DockingTitleList`).
    fileprivate var isList = false
    @ViewBuilder var content: () -> Content

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var offset: CGFloat = 0
    @State private var baseline: CGFloat?
    @State private var flashStart: Date?
    @State private var layout: DockTitleLayout

    init(title: String, description: String? = nil, taller: Bool = false,
         markLabel: String? = nil, chrome: DockChrome = .navigationBar,
         @ViewBuilder content: @escaping () -> Content) {
        self.title = title
        self.description = description
        self.taller = taller
        self.markLabel = markLabel
        self.chrome = chrome
        self.content = content
        _layout = State(initialValue: DockTitleLayout(text: title, mark: markLabel != nil))
    }

    fileprivate init(listTitle title: String, description: String?, markLabel: String?, chrome: DockChrome,
                     @ViewBuilder content: @escaping () -> Content) {
        self.init(title: title, description: description, markLabel: markLabel, chrome: chrome, content: content)
        self.isList = true
    }

    private var frame: DockMath.Frame {
        DockMath.frame(y: offset, n: layout.letters, reduced: reduceMotion)
    }

    private var usesNavBar: Bool { chrome == .navigationBar }

    var body: some View {
        GeometryReader { geo in
            let safeTop = geo.safeAreaInsets.top
            let f = frame
            let titleH = layout.titleHeight(width: geo.size.width)
            ZStack(alignment: .topLeading) {
                scroller(safeTop: safeTop, titleH: titleH, f: f)
                    .scrollBounceBehavior(.basedOnSize)
                    .modifier(DockScrollReader(offset: $offset))
                    .onPreferenceChange(DockOffsetKey.self) { v in
                        if #unavailable(iOS 18.0) {
                            if usesNavBar { offset = v } else {
                                if baseline == nil { baseline = v }
                                offset = v - (baseline ?? 0)
                            }
                        }
                    }

                titleLayer(frame: f, width: geo.size.width, safeTop: safeTop)
            }
            .background(Color("BgPrimary").ignoresSafeArea())
        }
        .modifier(DockNavChrome(enabled: usesNavBar))
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
        .onChange(of: title) { _, new in layout = DockTitleLayout(text: new, mark: markLabel != nil) }
    }

    /// Reads the offset on iOS 17: the first row's top relative to the screen.
    private func offsetReader(safeTop: CGFloat) -> some View {
        Color.clear.frame(height: 0).background(
            GeometryReader { m in
                Color.clear.preference(
                    key: DockOffsetKey.self,
                    value: (usesNavBar ? safeTop : 0) - m.frame(in: .global).minY)
            })
    }

    @ViewBuilder
    private func scroller(safeTop: CGFloat, titleH: CGFloat, f: DockMath.Frame) -> some View {
        let descView = Group {
            if let description {
                Text(description)
                    .font(.system(size: 12.5))
                    .foregroundStyle(Color("TextSecondary"))
                    .lineSpacing(3)
                    .padding(.bottom, 14)
                    .dockDescription(frame: f)
            }
        }
        if isList {
            // List rows: the header row carries the title room and the iOS 17
            // reader; the content keeps its own row insets and swipe actions.
            List {
                VStack(alignment: .leading, spacing: 0) {
                    offsetReader(safeTop: safeTop)
                    Color.clear.frame(height: titleH + 8 + 12)
                    descView
                }
                .padding(.horizontal, 14)
                .listRowInsets(EdgeInsets())
                .listRowSeparator(.hidden)
                .listRowBackground(Color.clear)
                .onDisappear { if offset > 0 { offset = max(offset, PolishTokens.dock) } }
                content()
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
        } else {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    offsetReader(safeTop: safeTop)
                    // Room for the title the canvas draws.
                    Color.clear.frame(height: titleH + 8 + 12)
                    descView
                    content()
                }
                .padding(.horizontal, 14)
            }
        }
    }

    @ViewBuilder
    private func titleLayer(frame f: DockMath.Frame, width: CGFloat, safeTop: CGFloat) -> some View {
        let barCenter: CGFloat = {
            if case .overlay(let c) = chrome { return c }
            return safeTop + 22
        }()
        let top = barCenter + 22 + 8
        let hc = barCenter
        let hairline = f.docked
        let titleH = layout.titleHeight(width: width)
        let bigH = layout.bigHeight(width: width)
        ZStack(alignment: .topLeading) {
            // Band holds the waiting letters and collapses over t .82 to 1.
            let bandH = (titleH + PolishTokens.bandPad) * f.bandFactor
            LinearGradient(stops: [.init(color: Color("BgPrimary"), location: 0.8),
                                   .init(color: Color("BgPrimary").opacity(0), location: 1)],
                           startPoint: .top, endPoint: .bottom)
                .frame(width: width, height: max(0, bandH))
                .offset(y: top - 8)
            if reduceMotion {
                letters(fixed: 0, f: f, width: width, top: top, hc: hc, bigH: bigH, flash: nil)
                    .opacity(f.reducedDone ? 0 : 1)
                letters(fixed: 1, f: f, width: width, top: top, hc: hc, bigH: bigH, flash: nil)
                    .opacity(f.reducedDone ? 1 : 0)
            } else {
                TimelineView(.animation(paused: flashStart == nil)) { tl in
                    letters(fixed: nil, f: f, width: width, top: top, hc: hc, bigH: bigH,
                            flash: flashStart.map { tl.date.timeIntervalSince($0) })
                }
            }
            Rectangle().fill(PolishTokens.glassBorder).frame(width: width, height: 0.5)
                .offset(y: barCenter + 22).opacity(hairline ? 1 : 0)
        }
        .frame(width: width, alignment: .topLeading)
        .animation(.easeInOut(duration: 0.2), value: f.reducedDone)
        .animation(.easeInOut(duration: 0.2), value: hairline)
        .ignoresSafeArea()
        .allowsHitTesting(false)
        .accessibilityElement()
        .accessibilityLabel(markLabel ?? title)
        .accessibilityAddTraits(f.docked ? .isHeader : [])
    }

    private func letters(fixed: Double?, f: DockMath.Frame, width: CGFloat,
                         top: CGFloat, hc: CGFloat, bigH: CGFloat, flash: TimeInterval?) -> some View {
        let n = layout.letters
        let pairs = layout.pairs
        let scrollY = offset
        let lineGap = bigH * 1.36
        return Canvas { ctx, _ in
            let bigB = bigH / 5
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
                let y0 = top - scroll + CGFloat(p.line) * lineGap + CGFloat(p.bigRow) * bigB
                let x1 = sx0 + CGFloat(p.smallCol) * smallS
                let y1 = sy0 + CGFloat(p.smallRow) * smallS
                let k = CGFloat(pk)
                let hop = fixed == nil ? -CGFloat(sin(Double.pi * pk)) * CGFloat(8 + (p.letter % 3) * 3) : 0
                let tilt = fixed == nil ? sin(Double.pi * pk) * (p.letter % 2 == 1 ? 18 : -18) : 0
                let sz = bigB + (smallS - bigB) * k
                let cx = x0 + (x1 - x0) * k + sz / 2
                let cy = y0 + (y1 - y0) * k + hop + sz / 2
                var color = ink
                if p.isCore {
                    color = coral
                } else if fixed == nil, !p.isMark {
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

/// A List page that docks its title like `DockingTitleScrollView` (T10).
/// `content` is List rows (ForEach, Section, ...), so swipe actions, edit mode
/// and separators keep working. The rows keep their own insets.
struct DockingTitleList<Content: View>: View {
    let title: String
    var description: String?
    var markLabel: String?
    var chrome: DockChrome = .navigationBar
    @ViewBuilder var content: () -> Content

    var body: some View {
        DockingTitleScrollView(listTitle: title, description: description, markLabel: markLabel,
                               chrome: chrome, content: content)
    }
}

/// Inline nav bar plus an empty principal item, only for `.navigationBar`.
private struct DockNavChrome: ViewModifier {
    let enabled: Bool
    func body(content: Content) -> some View {
        if enabled {
            content
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .principal) {
                        Color.clear.frame(width: 1, height: 22).accessibilityHidden(true)
                    }
                }
        } else {
            content
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
