import SwiftUI

// MARK: - gizzi code wordmark

/// The gizzi code lockup (components/brand/GizziCodeWordmark.tsx): the Gizzi
/// mark plus "GIZZI CODE" on the lockup's 10px pixel grid. Ink follows the
/// text colour; the mark's coral accents stay brand coral, like the
/// published lockup.
struct GizziCodeWordmark: View {
    var height: CGFloat = 15
    var withMark = true

    static let coral = Color(hex: "#D97757")

    private static let rows = [
        ".###..###.#####.#####.###....####..###..####..#####",
        "#......#.....#.....#...#....#.....#...#.#...#.#....",
        "#.###..#....#.....#....#....#.....#...#.#...#.####.",
        "#...#..#...#.....#.....#....#.....#...#.#...#.#....",
        ".###..###.#####.#####.###....####..###..####..#####",
    ]

    private var viewWidth: CGFloat { (withMark ? 70 : 0) + CGFloat(Self.rows[0].count * 10) }
    private var viewHeight: CGFloat { withMark ? 54 : 50 }

    var body: some View {
        Canvas { context, size in
            let scale = size.height / viewHeight
            context.scaleBy(x: scale, y: scale)
            let ink = GraphicsContext.Shading.color(Color("TextPrimary"))
            if withMark { drawMark(in: context, ink: ink) }
            var word = context
            word.translateBy(x: withMark ? 70 : 0, y: withMark ? 2 : 0)
            for (y, row) in Self.rows.enumerated() {
                for (x, cell) in row.enumerated() where cell == "#" {
                    let rect = CGRect(x: CGFloat(x) * 10 + 0.75, y: CGFloat(y) * 10 + 0.75, width: 8.5, height: 8.5)
                    word.fill(Path(roundedRect: rect, cornerRadius: 1.5), with: ink)
                }
            }
        }
        .frame(width: height * viewWidth / viewHeight, height: height)
        .accessibilityElement()
        .accessibilityLabel("gizzi code")
    }

    /// LockupMark: the 84×84 mark (viewBox 6 4 84 84) drawn into 54×54.
    private func drawMark(in context: GraphicsContext, ink: GraphicsContext.Shading) {
        var mark = context
        let s: CGFloat = 54.0 / 84.0
        mark.scaleBy(x: s, y: s)
        mark.translateBy(x: -6, y: -4)
        let coral = GraphicsContext.Shading.color(Self.coral)
        func rr(_ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat, _ r: CGFloat) -> Path {
            Path(roundedRect: CGRect(x: x, y: y, width: w, height: h), cornerRadius: r)
        }
        mark.fill(rr(44, 8, 8, 5, 2), with: coral)
        mark.fill(rr(24, 15, 17, 7, 3), with: ink)
        mark.fill(rr(55, 15, 17, 7, 3), with: ink)
        // Head + ears, with the face window cut out (the SVG mask).
        mark.drawLayer { layer in
            var head = Path()
            head.addLines([
                CGPoint(x: 25, y: 23), CGPoint(x: 71, y: 23), CGPoint(x: 78, y: 30), CGPoint(x: 78, y: 56),
                CGPoint(x: 72, y: 63), CGPoint(x: 72, y: 69), CGPoint(x: 64, y: 76), CGPoint(x: 32, y: 76),
                CGPoint(x: 24, y: 69), CGPoint(x: 24, y: 63), CGPoint(x: 18, y: 56), CGPoint(x: 18, y: 30),
            ])
            head.closeSubpath()
            var left = Path()
            left.addLines([
                CGPoint(x: 18, y: 40), CGPoint(x: 14, y: 40), CGPoint(x: 14, y: 44), CGPoint(x: 10, y: 44),
                CGPoint(x: 10, y: 48), CGPoint(x: 14, y: 48), CGPoint(x: 14, y: 52), CGPoint(x: 18, y: 52),
                CGPoint(x: 18, y: 56), CGPoint(x: 22, y: 56), CGPoint(x: 22, y: 40),
            ])
            left.closeSubpath()
            var right = Path()
            right.addLines([
                CGPoint(x: 78, y: 40), CGPoint(x: 82, y: 40), CGPoint(x: 82, y: 44), CGPoint(x: 86, y: 44),
                CGPoint(x: 86, y: 48), CGPoint(x: 82, y: 48), CGPoint(x: 82, y: 52), CGPoint(x: 78, y: 52),
                CGPoint(x: 78, y: 56), CGPoint(x: 74, y: 56), CGPoint(x: 74, y: 40),
            ])
            right.closeSubpath()
            layer.fill(head, with: ink)
            layer.fill(left, with: ink)
            layer.fill(right, with: ink)
            layer.blendMode = .destinationOut
            layer.fill(rr(24, 29, 48, 31, 9), with: .color(.black))
        }
        mark.fill(rr(33, 36, 8, 10, 2), with: ink)
        mark.fill(rr(59, 36, 8, 10, 2), with: ink)
        var a = Path()
        a.addLines([CGPoint(x: 44.5, y: 52), CGPoint(x: 48, y: 42), CGPoint(x: 51.5, y: 52), CGPoint(x: 49.6, y: 52),
                    CGPoint(x: 48.8, y: 49.5), CGPoint(x: 47.2, y: 49.5), CGPoint(x: 46.4, y: 52)])
        a.closeSubpath()
        mark.fill(a, with: coral)
        mark.fill(rr(46, 47.2, 4, 1.4, 0.7), with: coral)
        for x in [24, 36, 52, 64] as [CGFloat] {
            mark.fill(rr(x, 74, 8, 12, 1), with: ink)
        }
    }
}

// MARK: - What's new

/// The "What's new" pill beside the lockup (CodeWhatsNew.tsx): a coral dot
/// while the latest gizzi-code release is unseen; tapping opens the sheet.
struct CodeWhatsNewButton: View {
    @AppStorage(GizziChangelog.seenKey) private var seenVersion: String = ""
    @State private var latest: String? = GizziChangelog.cached().flatMap(GizziChangelog.latestVersion)
    @State private var isPresented = false

    private var hasUnseen: Bool {
        guard let latest else { return false }
        return latest != seenVersion
    }

    var body: some View {
        Button {
            isPresented = true
            if let latest { seenVersion = latest }
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "sparkle")
                    .font(.system(size: 11, weight: .semibold))
                Text("What's new")
                    .font(.system(size: 12, weight: .semibold))
            }
            .foregroundColor(Color("TextSecondary"))
            .padding(.horizontal, 10)
            .frame(height: 28)
            .background(Color("FillNeutral"))
            .clipShape(Capsule())
            .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
            .overlay(alignment: .topTrailing) {
                if hasUnseen {
                    Circle()
                        .fill(GizziCodeWordmark.coral)
                        .frame(width: 8, height: 8)
                        .overlay(Circle().stroke(Color("BgPrimary"), lineWidth: 1.5))
                        .offset(x: 2, y: -2)
                }
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(hasUnseen ? "What's new (unread)" : "What's new")
        .task {
            if let releases = try? await GizziChangelog.load() {
                latest = GizziChangelog.latestVersion(releases)
            }
        }
        .sheet(isPresented: $isPresented) {
            CodeWhatsNewSheet()
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
        }
    }
}

/// What's new: "gizzi code" (live gizzi-code changelog) and "Allternit"
/// (curated Code mode notes) tabs, close, and a footer with the source and
/// a Full changelog link.
private struct CodeWhatsNewSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    @State private var tab: Tab = .gizzi
    @State private var releases: [GizziRelease] = GizziChangelog.cached() ?? []
    @State private var isLive = false
    @State private var loadError: String? = nil

    enum Tab { case gizzi, app }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Text("What's new")
                    .font(.system(size: 13, weight: .bold))
                    .foregroundColor(Color("TextPrimary"))
                HStack(spacing: 2) {
                    tabButton(.gizzi) { GizziCodeWordmark(height: 9, withMark: false) }
                    tabButton(.app) { Text("Allternit").font(.system(size: 11.5, weight: .semibold)) }
                }
                .padding(.leading, 4)
                Spacer()
                Button(action: { dismiss() }) {
                    Image(systemName: "xmark")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(Color("TextTertiary"))
                        .frame(width: 32, height: 32)
                }
                .accessibilityLabel("Close what's new")
            }
            .padding(.horizontal, 14)
            .padding(.top, 14)
            .padding(.bottom, 10)
            .overlay(alignment: .bottom) { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }

            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    if tab == .gizzi {
                        if releases.isEmpty, let loadError {
                            Text(loadError)
                                .font(.system(size: 12))
                                .foregroundColor(Color("TextSecondary"))
                        } else if releases.isEmpty {
                            ProgressView().frame(maxWidth: .infinity).padding(.top, 24)
                        }
                        ForEach(releases) { release in
                            releaseEntry(release)
                        }
                    } else {
                        ForEach(CodeChangelogEntry.all) { entry in
                            appEntry(entry)
                        }
                    }
                }
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
            }

            HStack {
                Text(tab == .gizzi ? (isLive ? "Live from GitHub" : "Offline snapshot") : "Code mode")
                Spacer()
                if tab == .gizzi {
                    Button(action: { openURL(GizziChangelog.pageURL) }) {
                        HStack(spacing: 3) {
                            Text("Full changelog")
                            Image(systemName: "arrow.up.right.square")
                        }
                    }
                    .buttonStyle(.plain)
                }
            }
            .font(.system(size: 10.5))
            .foregroundColor(Color("TextTertiary"))
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .overlay(alignment: .top) { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
        }
        .background(Color("BgPanel"))
        .task {
            do {
                releases = try await GizziChangelog.load()
                isLive = true
            } catch {
                loadError = "Couldn't load the gizzi code changelog. \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }

    private func tabButton<Label: View>(_ id: Tab, @ViewBuilder label: () -> Label) -> some View {
        Button(action: { tab = id }) {
            label()
                .foregroundColor(tab == id ? Color("TextPrimary") : Color("TextTertiary"))
                .padding(.horizontal, 8)
                .frame(height: 24)
                .background(tab == id ? Color("TextPrimary").opacity(0.09) : .clear)
                .clipShape(RoundedRectangle(cornerRadius: 6))
        }
        .buttonStyle(.plain)
    }

    private func releaseEntry(_ release: GizziRelease) -> some View {
        let unreleased = release.version == "Unreleased"
        return VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .firstTextBaseline) {
                Text(unreleased ? "Unreleased · on main" : "v\(release.version)")
                    .font(.system(size: 12, weight: .bold, design: .monospaced))
                    .foregroundColor(unreleased ? Color("TextSecondary") : Color("TextPrimary"))
                Spacer()
                if let date = release.date {
                    Text(Self.formatDate(date))
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundColor(Color("TextTertiary"))
                }
            }
            if let summary = release.summary {
                Text(Self.inline(summary))
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextPrimary"))
            }
            ForEach(Array(release.sections.enumerated()), id: \.offset) { _, section in
                let color = Self.kindColor(section.kind)
                VStack(alignment: .leading, spacing: 4) {
                    if !section.kind.isEmpty {
                        Text(section.kind.uppercased())
                            .font(.system(size: 10, weight: .bold))
                            .tracking(0.8)
                            .foregroundColor(color)
                    }
                    ForEach(Array(section.items.enumerated()), id: \.offset) { _, item in
                        bullet(item, color: color)
                    }
                }
                .padding(.top, 2)
            }
        }
    }

    private func appEntry(_ entry: CodeChangelogEntry) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline) {
                Text(entry.title)
                    .font(.system(size: 12, weight: .bold))
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Text(Self.formatDate(entry.date))
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundColor(Color("TextTertiary"))
            }
            ForEach(entry.items, id: \.self) { item in
                bullet(item, color: Color("AccentPrimary"))
            }
        }
    }

    private func bullet(_ text: String, color: Color) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Circle().fill(color).frame(width: 4, height: 4).padding(.top, 7)
            Text(Self.inline(text))
                .font(.system(size: 12))
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    /// `code` spans become mono chips (InlineMarkdown).
    private static func inline(_ text: String) -> AttributedString {
        var result = AttributedString()
        for (index, part) in text.components(separatedBy: "`").enumerated() {
            var piece = AttributedString(part)
            if index % 2 == 1 {
                piece.font = .system(size: 11, design: .monospaced)
                piece.foregroundColor = Color("TextPrimary")
                piece.backgroundColor = Color("TextPrimary").opacity(0.07)
            }
            result += piece
        }
        return result
    }

    private static func kindColor(_ kind: String) -> Color {
        switch kind.lowercased() {
        case "added": return Color(hex: "#6B9A7B")
        case "changed": return Color(hex: "#D4B08C")
        case "fixed": return GizziCodeWordmark.coral
        case "removed": return Color(hex: "#A1A1AA")
        case "security": return Color(hex: "#C06A6A")
        default: return Color("AccentPrimary")
        }
    }

    private static func formatDate(_ iso: String) -> String {
        guard let date = try? Date("\(iso)T12:00:00Z", strategy: .iso8601) else { return iso }
        return date.formatted(.dateTime.month(.abbreviated).day().year())
    }
}

/// Curated Code mode notes for the "Allternit" tab (code-whats-new.ts `CODE_CHANGELOG`).
struct CodeChangelogEntry: Identifiable {
    let id: String
    let date: String
    let title: String
    let items: [String]

    static let all: [CodeChangelogEntry] = [
        .init(id: "code-mode-chrome", date: "2026-09-26", title: "Code mode top deck and sessions", items: [
            "Top deck: environment picker, working-folder chip, and folder opening wired to a real workspace",
            "Worktree and Canvas pills appear only when they apply",
            "Permission modes (Manual / Auto-accept / Plan / Read only) in a Claude-style overlay, enforced by the runtime",
            "Sessions name themselves from the first turn — header and rail together",
            "Usage overview card with activity heatmap",
        ]),
        .init(id: "skills-connectors-directory", date: "2026-09-26", title: "Skills and connectors directory", items: [
            "One-click install for directory skills, with progress and errors",
            "Skill and plugin detail pages: file tree, bundled skills and connectors",
            "Actions: check for updates, edit, duplicate, remove",
            "Connectors: shared directory of 1,100+ providers with real sign-in",
        ]),
        .init(id: "memory-v2", date: "2026-09-26", title: "Memory that reaches your chats", items: [
            "Model-based memory extraction with add, update, and forget",
            "Settings → Memory: search, fact list, delete, and import",
            "Fixed a backend defect where no memory reached any chat",
        ]),
        .init(id: "projects-parity", date: "2026-09-26", title: "Projects, Claude-style", items: [
            "Project pages: Memory, Context, Folder, and Scheduled in one column",
            "Every new project ships the Allternit prompting guide as context",
            "Native folder picker and a create flow that matches Claude’s",
        ]),
        .init(id: "shell-parity", date: "2026-09-25", title: "Shell and rail", items: [
            "Account menu with plan, Usage, and platform console link",
            "Denser recents and a working pinned section",
            "Quick task stack with its own approval modes",
        ]),
    ]
}

// MARK: - Usage

/// The Code usage card (CodeUsageDashboard.tsx): Overview | Models tabs,
/// All / 30d / 7d, close; six stat tiles and a 12-week activity heatmap,
/// computed from the account's code sessions.
struct CodeUsageDashboard: View {
    let sessions: [AgentSession]
    let onClose: () -> Void

    @State private var range: Range = .all
    @State private var tab: Tab = .overview

    enum Range: String, CaseIterable { case all = "All", d30 = "30d", d7 = "7d" }
    enum Tab { case overview, models }

    private var filtered: [AgentSession] {
        let days: Int? = range == .d7 ? 6 : range == .d30 ? 29 : nil
        guard let days else { return sessions }
        let start = Calendar.current.date(byAdding: .day, value: -days, to: Calendar.current.startOfDay(for: Date())) ?? .distantPast
        return sessions.filter { CodeSessionDates.timestamp($0) >= start }
    }

    private struct Metrics {
        var sessions = 0
        var messages = 0
        var tokens = 0
        var activeDays = 0
        var favoriteModel: String? = nil
        var peakHour: Int? = nil
        var dateCount: [String: Int] = [:]
        var modelRows: [(String, Int)] = []
    }

    private var metrics: Metrics {
        var m = Metrics()
        var models: [String: Int] = [:]
        var hours: [Int: Int] = [:]
        let calendar = Calendar.current
        for session in filtered {
            let date = CodeSessionDates.timestamp(session)
            let weight = max(session.messageCount, 1)
            if date > .distantPast {
                m.dateCount[Self.dayKey(date), default: 0] += weight
                hours[calendar.component(.hour, from: date), default: 0] += weight
            }
            if let model = session.agentName, !model.isEmpty {
                models[model, default: 0] += weight
            }
            m.messages += session.messageCount
            // No message bodies on the list: the web's estimate for that case.
            m.tokens += Int((Double((session.name ?? "").count) / 3.7).rounded(.up)) * weight
        }
        m.sessions = filtered.count
        m.activeDays = m.dateCount.count
        m.modelRows = models.sorted { $0.value > $1.value }.prefix(6).map { ($0.key, $0.value) }
        m.favoriteModel = m.modelRows.first?.0
        m.peakHour = filtered.isEmpty ? nil : hours.max { $0.value < $1.value }?.key
        return m
    }

    var body: some View {
        let metrics = self.metrics
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 12) {
                HStack(spacing: 4) {
                    tabButton(.overview, "Overview")
                    tabButton(.models, "Models")
                }
                Spacer()
                HStack(spacing: 0) {
                    ForEach(Range.allCases, id: \.self) { item in
                        Button(action: { range = item }) {
                            Text(item.rawValue)
                                .font(.system(size: 11, weight: .medium))
                                .foregroundColor(range == item ? Color("TextPrimary") : Color("TextTertiary"))
                                .padding(.horizontal, 8)
                                .frame(height: 22)
                                .background(range == item ? Color("BgPrimary") : .clear)
                                .clipShape(RoundedRectangle(cornerRadius: 6))
                                .shadow(color: range == item ? .black.opacity(0.06) : .clear, radius: 1, y: 1)
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(2)
                .background(Color("TextPrimary").opacity(0.04))
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderSubtle"), lineWidth: 1))
                Button(action: onClose) {
                    Image(systemName: "xmark")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundColor(Color("TextTertiary"))
                        .frame(width: 28, height: 28)
                        .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderSubtle"), lineWidth: 1))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Close usage")
            }
            .padding(.horizontal, 16)
            .padding(.top, 12)

            if tab == .overview {
                VStack(alignment: .leading, spacing: 12) {
                    LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 8), count: 3), spacing: 8) {
                        statTile("Sessions", "number", metrics.sessions.formatted())
                        statTile("Messages", "brain", metrics.messages.formatted())
                        statTile("Est. tokens", "brain", Self.compact(metrics.tokens))
                        statTile("Active days", "calendar", "\(metrics.activeDays)")
                        statTile("Peak hour", "sun.max", Self.peakHour(metrics.peakHour))
                        statTile("Favorite model", "brain", metrics.favoriteModel.map(Self.modelName) ?? "—")
                    }
                    heatmap(metrics.dateCount)
                }
                .padding(.horizontal, 16)
                .padding(.top, 12)
                .padding(.bottom, 16)
            } else {
                VStack(spacing: 8) {
                    if metrics.modelRows.isEmpty {
                        Text("No model usage in this period.")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextTertiary"))
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 12)
                    } else {
                        let maxCount = max(1, metrics.modelRows.map(\.1).max() ?? 1)
                        ForEach(metrics.modelRows, id: \.0) { row in
                            let (model, count) = row
                            HStack(spacing: 10) {
                                Image(systemName: "brain")
                                    .font(.system(size: 12))
                                    .foregroundColor(Color("TextTertiary"))
                                Text(Self.modelName(model))
                                    .font(.system(size: 12, weight: .medium))
                                    .foregroundColor(Color("TextPrimary"))
                                    .lineLimit(1)
                                    .frame(width: 120, alignment: .leading)
                                GeometryReader { geo in
                                    ZStack(alignment: .leading) {
                                        Capsule().fill(Color("TextPrimary").opacity(0.06))
                                        Capsule().fill(Color("AccentPrimary"))
                                            .frame(width: geo.size.width * max(0.04, CGFloat(count) / CGFloat(maxCount)))
                                    }
                                }
                                .frame(height: 6)
                                Text("\(Self.compact(count)) msgs")
                                    .font(.system(size: 11))
                                    .foregroundColor(Color("TextTertiary"))
                                    .frame(width: 56, alignment: .trailing)
                            }
                        }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 12)
                .padding(.bottom, 16)
            }
        }
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: 16))
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(Color("BorderSubtle"), lineWidth: 1))
        .shadow(color: .black.opacity(0.08), radius: 16, y: 6)
    }

    private func tabButton(_ id: Tab, _ label: String) -> some View {
        Button(action: { tab = id }) {
            Text(label)
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(tab == id ? Color("TextPrimary") : Color("TextTertiary"))
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(tab == id ? Color("TextPrimary").opacity(0.07) : .clear)
                .clipShape(RoundedRectangle(cornerRadius: 8))
        }
        .buttonStyle(.plain)
    }

    private func statTile(_ label: String, _ icon: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 5) {
                Image(systemName: icon).font(.system(size: 9))
                Text(label.uppercased())
                    .font(.system(size: 10))
                    .tracking(0.6)
                    .lineLimit(1)
            }
            .foregroundColor(Color("TextTertiary"))
            Text(value)
                .font(.system(size: 15, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(Color("TextPrimary").opacity(0.04))
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }

    /// Last 12 weeks as columns (Monday first) × 7 days, ending today.
    private func heatmap(_ dateCount: [String: Int]) -> some View {
        let calendar = Calendar.current
        let today = calendar.startOfDay(for: Date())
        let cells: [(String, Int)] = (0..<84).reversed().map { offset in
            let date = calendar.date(byAdding: .day, value: -offset, to: today) ?? today
            let key = Self.dayKey(date)
            return (key, dateCount[key] ?? 0)
        }
        let firstWeekday = calendar.component(.weekday, from: calendar.date(byAdding: .day, value: -83, to: today) ?? today)
        let pad = (firstWeekday + 5) % 7
        let padded: [(String, Int)?] = Array(repeating: nil, count: pad) + cells.map { Optional($0) }
        let weeks: [[(String, Int)?]] = stride(from: 0, to: padded.count, by: 7).map { Array(padded[$0..<min($0 + 7, padded.count)]) }
        let maxHeat = max(1, cells.map(\.1).max() ?? 1)
        return VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("ACTIVITY")
                    .font(.system(size: 10, weight: .semibold))
                    .tracking(0.8)
                Spacer()
                Text("Last 12 weeks").font(.system(size: 10))
            }
            .foregroundColor(Color("TextTertiary"))
            HStack(alignment: .top, spacing: 3) {
                ForEach(weeks.indices, id: \.self) { index in
                    VStack(spacing: 3) {
                        ForEach(Array(weeks[index].enumerated()), id: \.offset) { _, cell in
                            RoundedRectangle(cornerRadius: 3)
                                .fill(cell.map { Self.intensity($0.1, maxHeat) } ?? .clear)
                                .aspectRatio(1, contentMode: .fit)
                        }
                    }
                    .frame(maxWidth: .infinity)
                }
            }
            .accessibilityElement()
            .accessibilityLabel("Session activity for the last 12 weeks")
        }
    }

    private static func intensity(_ count: Int, _ max: Int) -> Color {
        guard count > 0 else { return Color("TextPrimary").opacity(0.05) }
        let ratio = Double(count) / Double(max)
        if ratio > 0.66 { return Color("AccentPrimary").opacity(0.9) }
        if ratio > 0.33 { return Color("AccentPrimary").opacity(0.55) }
        return Color("AccentPrimary").opacity(0.25)
    }

    private static func dayKey(_ date: Date) -> String {
        let c = Calendar.current.dateComponents([.year, .month, .day], from: date)
        return String(format: "%04d-%02d-%02d", c.year ?? 0, c.month ?? 0, c.day ?? 0)
    }

    static func compact(_ value: Int) -> String {
        if value >= 1_000_000 { return String(format: "%.1fM", Double(value) / 1_000_000) }
        if value >= 1_000 { return String(format: "%.1fK", Double(value) / 1_000) }
        return "\(value)"
    }

    private static func peakHour(_ hour: Int?) -> String {
        guard let hour else { return "—" }
        return "\(hour % 12 == 0 ? 12 : hour % 12) \(hour >= 12 ? "PM" : "AM")"
    }

    private static func modelName(_ model: String) -> String {
        let segment = model.split(separator: "/").last.map(String.init) ?? model
        var name = segment
        for prefix in ["claude-", "kimi-"] where name.hasPrefix(prefix) { name = String(name.dropFirst(prefix.count)) }
        return name.isEmpty ? model : name
    }
}

/// Session timestamps: `updated_at`, else `created_at` (RFC-3339, with or
/// without fractional seconds).
enum CodeSessionDates {
    static func timestamp(_ session: AgentSession) -> Date {
        parse(session.updatedAt) ?? parse(session.createdAt) ?? .distantPast
    }

    static func parse(_ value: String) -> Date? {
        if let date = try? Date(value, strategy: Date.ISO8601FormatStyle(includingFractionalSeconds: true)) {
            return date
        }
        return try? Date(value, strategy: Date.ISO8601FormatStyle())
    }

    /// The overview card's age ("now", "5m", "2h", "3d").
    static func age(_ session: AgentSession) -> String {
        let date = timestamp(session)
        guard date > .distantPast else { return "" }
        let seconds = max(0, Date().timeIntervalSince(date))
        if seconds < 60 { return "now" }
        if seconds < 3600 { return "\(Int(seconds / 60))m" }
        if seconds < 86400 { return "\(Int(seconds / 3600))h" }
        return "\(Int(seconds / 86400))d"
    }
}
