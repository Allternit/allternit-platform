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

    /// Clear room to add below a page's content so its title can finish docking.
    /// `maxScroll` is how far the page scrolls without that room. A page that
    /// overflows by less than the dock distance could not scroll far enough, so
    /// its last letters froze mid-flight; the room makes up the difference and
    /// the title motion stays the same on every page. No overflow (up to 1 pt of
    /// rounding) gets no room, so a short page stays put and never docks.
    nonisolated static func bottomRoom(maxScroll: CGFloat) -> CGFloat {
        let dock = PolishTokens.dock
        guard maxScroll > 1, maxScroll < dock else { return 0 }
        return ceil(dock - maxScroll)
    }
}

/// Smallest height of the bottom-room row of a `DockingTitleList`. A List row
/// is never shorter than its minimum row height (44 pt), so the row is drawn at
/// least this tall and its height is always known exactly. A little extra
/// scroll past the docked point is harmless; a guessed height could make the
/// room flip on and off.
private let dockListRoomMinimum: CGFloat = 64

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
        let bh = PolishTokens.bigHeight
        return CGFloat(lines) * bh + CGFloat(max(0, lines - 1)) * (bh * 1.36 - bh)
    }
}

/// Where the docked title sits (T11).
enum DockChrome: Equatable {
    /// An inline navigation bar directly above the page; the title docks into
    /// its centre. The bar's own background is hidden and the page draws the
    /// bar's fill under the docked title, because UIKit draws the bar above
    /// SwiftUI content and its material would cover the title.
    case navigationBar
    /// No navigation bar. The docked title is drawn over a custom header
    /// (ACIViewHeader, LibrarySectionHeader, a sheet header) that the page is
    /// drawn above. `barCenter` is the y of the bar's centre, measured from the
    /// top of the page's safe-area-extended frame (the page's own top when it
    /// does not reach a safe-area edge); negative when the bar sits above it.
    case overlay(barCenter: CGFloat)
}

private struct DockOffsetKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}

/// Height of a ScrollView page's content without its bottom room (iOS 17).
private struct DockContentHeightKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}

/// A scroll page with a big block title that docks into the nav bar's
/// principal area. Put it inside a NavigationStack. With the default chrome the
/// nav bar is forced inline; the docked title is drawn by this view in the bar.
///
/// - `chrome`: `.navigationBar` (default) or `.overlay(barCenter:)`.
///
/// A page that overflows by less than the dock distance gets clear room below
/// its content so the title still finishes docking; a page that does not
/// overflow gets none and its title stays big.
struct DockingTitleScrollView<Content: View>: View {
    let title: String
    var description: String?
    /// Freeweights uses a taller header: top 112, bar centre 75 in the mockup.
    var taller = false
    var chrome: DockChrome = .navigationBar
    /// True when `content` is List rows (see `DockingTitleList`).
    fileprivate var isList = false
    @ViewBuilder var content: () -> Content

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var offset: CGFloat = 0
    @State private var baseline: CGFloat?
    @State private var flashStart: Date?
    @State private var layout: DockTitleLayout
    /// Clear room below the content (`DockMath.bottomRoom`).
    @State private var room: CGFloat = 0
    /// iOS 17 ScrollView only: content height without the room.
    @State private var contentH: CGFloat = 0

    init(title: String, description: String? = nil, taller: Bool = false,
         chrome: DockChrome = .navigationBar,
         @ViewBuilder content: @escaping () -> Content) {
        self.title = title
        self.description = description
        self.taller = taller
        self.chrome = chrome
        self.content = content
        _layout = State(initialValue: DockTitleLayout(text: title))
    }

    fileprivate init(listTitle title: String, description: String?, chrome: DockChrome,
                     @ViewBuilder content: @escaping () -> Content) {
        self.init(title: title, description: description, chrome: chrome, content: content)
        self.isList = true
    }

    private var frame: DockMath.Frame {
        DockMath.frame(y: offset, n: layout.letters, reduced: reduceMotion)
    }

    private var usesNavBar: Bool { chrome == .navigationBar }

    /// Height the room actually adds to the scroll range: the ScrollView pads
    /// its content by `room`; the List adds a row at least `dockListRoomMinimum` tall.
    private var roomHeight: CGFloat {
        guard room > 0 else { return 0 }
        return isList ? max(room, dockListRoomMinimum) : room
    }

    /// `range` is the measured scroll range, room included.
    private func applyScrollRange(_ range: CGFloat) {
        setRoom(forMaxScroll: range - roomHeight)
    }

    private func setRoom(forMaxScroll maxScroll: CGFloat) {
        let r = DockMath.bottomRoom(maxScroll: maxScroll)
        if r != room { room = r }
    }

    var body: some View {
        GeometryReader { geo in
            let safeTop = geo.safeAreaInsets.top
            let f = frame
            let titleH = layout.titleHeight
            ZStack(alignment: .topLeading) {
                scroller(safeTop: safeTop, titleH: titleH, f: f)
                    .scrollBounceBehavior(.basedOnSize)
                    .modifier(DockScrollReader(offset: $offset, onRange: { applyScrollRange($0) }))
                    .onPreferenceChange(DockOffsetKey.self) { v in
                        if #unavailable(iOS 18.0) {
                            if usesNavBar { offset = v } else {
                                if baseline == nil { baseline = v }
                                offset = v - (baseline ?? 0)
                            }
                        }
                    }
                    .onPreferenceChange(DockContentHeightKey.self) { h in
                        if #unavailable(iOS 18.0) { contentH = h }
                    }
                    // iOS 17 ScrollView: the visible height is the safe-area
                    // frame (the scroll view insets itself by the safe area), so
                    // the page scrolls contentH - geo height without the room.
                    .onChange(of: contentH - geo.size.height, initial: true) { _, maxScroll in
                        if #unavailable(iOS 18.0), !isList, contentH > 0 {
                            setRoom(forMaxScroll: maxScroll)
                        }
                    }

                titleLayer(frame: f, width: geo.size.width, safeTop: safeTop)
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .preference(key: ScrollOffsetKey.self, value: max(0, offset))
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
        .onChange(of: title) { _, new in layout = DockTitleLayout(text: new) }
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
                        .background {
                            // iOS 17 has no scroll geometry and List rows are
                            // lazy, so the range comes from the List's UIScrollView.
                            if #available(iOS 18.0, *) { EmptyView() } else {
                                DockScrollRangeProbe { applyScrollRange($0) }
                            }
                        }
                    Color.clear.frame(height: titleH + 8 + 12)
                    descView
                }
                .padding(.horizontal, 14)
                .listRowInsets(EdgeInsets())
                .listRowSeparator(.hidden)
                .listRowBackground(Color.clear)
                .onDisappear { if offset > 0 { offset = max(offset, PolishTokens.dock) } }
                content()
                if room > 0 {
                    // Bottom room so the title can finish docking (DockMath.bottomRoom).
                    Color.clear.frame(height: roomHeight)
                        .listRowInsets(EdgeInsets())
                        .listRowSeparator(.hidden)
                        .listRowBackground(Color.clear)
                        .accessibilityHidden(true)
                }
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
                .background(GeometryReader { g in
                    Color.clear.preference(key: DockContentHeightKey.self, value: g.size.height)
                })
                .padding(.horizontal, 14)
                // Bottom room so the title can finish docking (DockMath.bottomRoom).
                .padding(.bottom, room)
            }
        }
    }

    @ViewBuilder
    private func titleLayer(frame f: DockMath.Frame, width: CGFloat, safeTop: CGFloat) -> some View {
        // The layer starts at the top of the safe-area-extended frame. The
        // inline nav bar ends where the safe area does (safeTop), so its centre
        // is 22 above that.
        let barCenter: CGFloat = {
            if case .overlay(let c) = chrome { return c }
            return safeTop - 22
        }()
        // A bar above the page's frame (ACI and Library headers, a sheet bar)
        // needs room above the layer for the docked letters and their hop, or
        // the Canvas would clip them.
        let lift = max(0, 38 - barCenter)
        let hc = barCenter + lift
        let top = hc + 22 + 8
        let hairline = f.docked
        let titleH = layout.titleHeight
        let bigH = PolishTokens.bigHeight
        ZStack(alignment: .topLeading) {
            if usesNavBar {
                // The bar's fill: content scrolling under the (transparent) bar
                // stays hidden behind the docked title.
                Color("BgPrimary").frame(width: width, height: hc + 22)
            }
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
                .offset(y: hc + 22).opacity(hairline ? 1 : 0)
        }
        .frame(width: width, alignment: .topLeading)
        .animation(.easeInOut(duration: 0.2), value: f.reducedDone)
        .animation(.easeInOut(duration: 0.2), value: hairline)
        .ignoresSafeArea()
        .offset(y: -lift)
        .allowsHitTesting(false)
        .accessibilityElement()
        .accessibilityLabel(title)
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

/// A List page that docks its title like `DockingTitleScrollView` (T10).
/// `content` is List rows (ForEach, Section, ...), so swipe actions, edit mode
/// and separators keep working. The rows keep their own insets.
struct DockingTitleList<Content: View>: View {
    let title: String
    var description: String?
    var chrome: DockChrome = .navigationBar
    @ViewBuilder var content: () -> Content

    var body: some View {
        DockingTitleScrollView(listTitle: title, description: description,
                               chrome: chrome, content: content)
    }
}

/// Inline nav bar plus an empty principal item, only for `.navigationBar`.
/// The bar's background is hidden: UIKit draws it above this page, so its
/// material would cover the docked title. The page draws the bar's fill instead.
private struct DockNavChrome: ViewModifier {
    let enabled: Bool
    func body(content: Content) -> some View {
        if enabled {
            content
                .navigationBarTitleDisplayMode(.inline)
                .toolbarBackground(.hidden, for: .navigationBar)
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

private struct DockScrollMetrics: Equatable {
    var offset: CGFloat
    var range: CGFloat
}

/// Reads the scroll offset and scroll range: `onScrollGeometryChange` on iOS
/// 18+; on iOS 17 the page reports them through preference keys (ScrollView)
/// or `DockScrollRangeProbe` (List) instead.
private struct DockScrollReader: ViewModifier {
    @Binding var offset: CGFloat
    /// How far the page scrolls (content plus insets past the visible height).
    var onRange: (CGFloat) -> Void
    func body(content: Content) -> some View {
        if #available(iOS 18.0, *) {
            content.onScrollGeometryChange(for: DockScrollMetrics.self) { g in
                // The visible rect spans the whole scroll view, inset areas
                // included. containerSize does not always: on iOS 26 it leaves
                // out the bar insets (724 pt container, 874 pt visible on ACI
                // Computers), which made a 150 pt page read as 300 pt.
                DockScrollMetrics(
                    offset: g.contentOffset.y + g.contentInsets.top,
                    range: g.contentSize.height + g.contentInsets.top + g.contentInsets.bottom
                        - g.visibleRect.height)
            } action: { _, new in
                offset = max(0, new.offset)
                onRange(new.range)
            }
        } else {
            content
        }
    }
}

/// iOS 17, List pages: reports the scroll range of the UIScrollView the List
/// draws in. Before iOS 18 SwiftUI has no scroll geometry, and List rows below
/// the screen are not laid out, so a geometry preference cannot see where the
/// content ends. Sits in the List's header row; zero size, no touches.
private struct DockScrollRangeProbe: UIViewRepresentable {
    var onRange: (CGFloat) -> Void

    func makeUIView(context: Context) -> ProbeView {
        let v = ProbeView()
        v.isUserInteractionEnabled = false
        v.isAccessibilityElement = false
        v.onRange = onRange
        return v
    }

    func updateUIView(_ uiView: ProbeView, context: Context) {
        uiView.onRange = onRange
    }

    final class ProbeView: UIView {
        var onRange: ((CGFloat) -> Void)?
        private weak var scrollView: UIScrollView?
        private var observations: [NSKeyValueObservation] = []
        private var last: CGFloat = .nan

        override func didMoveToWindow() {
            super.didMoveToWindow()
            guard window != nil else { return }
            var v = superview
            while let s = v, !(s is UIScrollView) { v = s.superview }
            guard let sv = v as? UIScrollView, sv !== scrollView else { return }
            scrollView = sv
            last = .nan
            observations = [
                sv.observe(\.contentSize, options: [.initial, .new]) { [weak self] _, _ in
                    MainActor.assumeIsolated { self?.report() }
                },
                sv.observe(\.bounds, options: [.new]) { [weak self] _, _ in
                    MainActor.assumeIsolated { self?.report() }
                },
            ]
        }

        private func report() {
            guard let sv = scrollView else { return }
            let inset = sv.adjustedContentInset
            let range = sv.contentSize.height + inset.top + inset.bottom - sv.bounds.height
            guard range != last else { return }
            last = range
            // KVO fires inside UIKit layout, which can be inside a SwiftUI
            // update; hand the value to SwiftUI on the next turn.
            Task { @MainActor [weak self] in self?.onRange?(range) }
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
