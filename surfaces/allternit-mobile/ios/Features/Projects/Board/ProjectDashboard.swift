import SwiftUI

/// The project dashboards (components/projects/board/ProjectBoard.tsx): ten ways
/// to present one project, plus the one a person makes from words. All of them
/// draw the same real data and sit on the project's background. Pulse in Quiet
/// orange is what a new project opens with.
enum ProjectLayoutKind: String, CaseIterable, Identifiable, Codable {
    case pulse, bento, mission, score, cover, runway, board, crew, orbit, gallery, custom
    var id: String { rawValue }
    var name: String {
        switch self {
        case .pulse: return "Pulse"
        case .bento: return "Bento"
        case .mission: return "Mission control"
        case .score: return "Scoreboard"
        case .cover: return "Cover story"
        case .runway: return "Runway"
        case .board: return "Board"
        case .crew: return "Crew"
        case .orbit: return "Orbit"
        case .gallery: return "Gallery"
        case .custom: return "Yours"
        }
    }
    static var builtIn: [ProjectLayoutKind] { allCases.filter { $0 != .custom } }
    var tiles: [ProjectTile] {
        switch self {
        case .pulse: return [.decision, .chart, .schedule, .activity]
        case .bento, .custom: return ProjectTile.allCases
        case .mission: return [.decision, .team, .schedule, .activity]
        case .score: return [.countdown, .progress, .decision, .team]
        case .cover: return [.decision, .schedule, .files]
        case .runway: return [.decision, .schedule]
        case .crew: return [.decision, .activity]
        case .board, .orbit, .gallery: return [.decision]
        }
    }
}

enum ProjectTile: String, CaseIterable, Identifiable, Codable {
    case decision, countdown, progress, team, chart, schedule, files, activity
    var id: String { rawValue }
    var name: String {
        switch self {
        case .decision: return "Your decision"
        case .countdown: return "Countdown"
        case .progress: return "Progress"
        case .team: return "Team"
        case .chart: return "Activity"
        case .schedule: return "Schedule"
        case .files: return "Files"
        case .activity: return "Latest changes"
        }
    }
}

struct ProjectColorSet: Identifiable {
    let id: String
    let name: String
    let accent: Color
    let accentInk: Color
    let bright: [Color]?

    static let all: [ProjectColorSet] = [
        .init(id: "quiet", name: "Quiet orange", accent: Color(hex: 0xC8451F), accentInk: .white, bright: nil),
        .init(id: "blue", name: "Quiet blue", accent: Color(hex: 0x2F54EB), accentInk: .white, bright: nil),
        .init(id: "green", name: "Quiet green", accent: Color(hex: 0x0F7F57), accentInk: .white, bright: nil),
        .init(id: "mono", name: "Black and white", accent: Color(hex: 0x3A3D45), accentInk: .white, bright: nil),
        .init(id: "bright", name: "Bright", accent: Color(hex: 0xFF5A36), accentInk: Color(hex: 0x15171C),
              bright: [Color(hex: 0x2F54EB), Color(hex: 0xFF5A36), Color(hex: 0x0FA876), Color(hex: 0xFFC31F), Color(hex: 0xD6338F)]),
        .init(id: "sunset", name: "Sunset", accent: Color(hex: 0x7B3FF2), accentInk: .white,
              bright: [Color(hex: 0xD93A2A), Color(hex: 0x7B3FF2), Color(hex: 0xFFA51F), Color(hex: 0xE62E86), Color(hex: 0x0F8585)]),
    ]
    static func named(_ id: String) -> ProjectColorSet { all.first { $0.id == id } ?? all[0] }
}

private extension Color {
    init(hex: UInt32) {
        self.init(red: Double((hex >> 16) & 0xFF) / 255, green: Double((hex >> 8) & 0xFF) / 255, blue: Double(hex & 0xFF) / 255)
    }
}

/// One tile color with its readable text color, for light and dark.
struct ProjectSlot {
    let fill: Color
    let ink: Color
}

struct ProjectPalette {
    let set: ProjectColorSet
    let dark: Bool
    private static let inkDark = Color(hex: 0x15171C)
    private static let inkLight = Color(hex: 0xF2F3F5)

    /// a to e, as on the web's .pl-a to .pl-e.
    func slot(_ index: Int) -> ProjectSlot {
        let i = index % 5
        if let bright = set.bright {
            let inks: [Color] = set.id == "bright"
                ? [.white, Self.inkDark, Self.inkDark, Self.inkDark, .white]
                : [.white, .white, Self.inkDark, .white, .white]
            return ProjectSlot(fill: bright[i], ink: inks[i])
        }
        if i == 1 { return ProjectSlot(fill: set.accent, ink: set.accentInk) }
        let light: [UInt32] = [0x23252B, 0, 0xD3D6DB, 0xE1E3E6, 0xEAEBED]
        let night: [UInt32] = [0xEEF0F3, 0, 0x3B3E45, 0x303238, 0x292B30]
        let fill = Color(hex: (dark ? night : light)[i])
        if i == 0 { return ProjectSlot(fill: fill, ink: dark ? Self.inkDark : .white) }
        return ProjectSlot(fill: fill, ink: dark ? Self.inkLight : Self.inkDark)
    }
    var deep: Color {
        switch set.id {
        case "bright": return Color(hex: 0x0E1330)
        case "sunset": return Color(hex: 0x1F0F2E)
        default: return dark ? Color(hex: 0x101114) : Color(hex: 0x191A1F)
        }
    }
    var plain: ProjectSlot { ProjectSlot(fill: Color("BgPanel"), ink: Color("TextPrimary")) }
}

struct ProjectLook: Codable, Equatable {
    var layout: ProjectLayoutKind = .pulse
    var colors: String = "quiet"
    var hidden: [ProjectTile] = []
    var customTiles: [ProjectTile]? = nil
    var customPrompt: String? = nil

    func isOn(_ tile: ProjectTile) -> Bool {
        layout == .custom ? (customTiles ?? []).contains(tile) : !hidden.contains(tile)
    }
}

/// The tiles "Make one" shows for a description (project-look.ts tilesForPrompt).
func projectTiles(forPrompt prompt: String) -> [ProjectTile] {
    let s = prompt.lowercased()
    let rules: [(ProjectTile, [String])] = [
        (.decision, ["decision", "decide", "need", "approv", "blocker", "waiting"]),
        (.countdown, ["countdown", "deadline", "due", "launch", "days left"]),
        (.schedule, ["date", "schedule", "calendar", "deadline", "upcoming", "coming up", "routine"]),
        (.progress, ["progress", "step", "percent", "how far", "done"]),
        (.team, ["team", "bot", "who", "people", "crew"]),
        (.chart, ["chart", "graph", "activity", "number", "trend"]),
        (.files, ["file", "made", "artifact", "document", "doc", "asset"]),
        (.activity, ["change", "log", "latest", "feed", "stream", "history"]),
    ]
    let tiles = rules.filter { $0.1.contains(where: s.contains) }.map(\.0)
    return tiles.isEmpty ? [.decision, .countdown, .progress, .schedule] : tiles
}

/// Saved per project on this device, next to its background.
@MainActor
final class ProjectLookStore: ObservableObject {
    @Published var look: ProjectLook { didSet { save() } }
    private let key: String

    init(scope: String) {
        key = "project.look.for.\(scope)"
        if let data = UserDefaults.standard.data(forKey: key),
           let saved = try? JSONDecoder().decode(ProjectLook.self, from: data) {
            look = saved
        } else {
            look = ProjectLook()
        }
    }
    private func save() {
        if let data = try? JSONEncoder().encode(look) { UserDefaults.standard.set(data, forKey: key) }
    }
    func toggle(_ tile: ProjectTile) {
        if look.layout == .custom {
            var tiles = look.customTiles ?? []
            if let i = tiles.firstIndex(of: tile) { tiles.remove(at: i) } else { tiles.append(tile) }
            look.customTiles = tiles
        } else if let i = look.hidden.firstIndex(of: tile) {
            look.hidden.remove(at: i)
        } else {
            look.hidden.append(tile)
        }
    }
}

// MARK: - Data

struct ProjectBoardWork: Identifiable {
    enum Column: String, CaseIterable { case queued = "Queued", working = "Working", needsYou = "Needs you", done = "Done" }
    let id: String
    let title: String
    let column: Column
    let owner: String?
    let at: Date?
    let open: (() -> Void)?
}
struct ProjectBoardPerson: Identifiable {
    let id: String
    let name: String
    let doing: String?
    let state: String
    let bot: AgentRecord?
    let open: (() -> Void)?
}
struct ProjectBoardMade: Identifiable {
    let id: String
    let name: String
    let kind: String
    let open: (() -> Void)?
}
struct ProjectBoardDeadline: Identifiable {
    let id = UUID()
    let label: String
    let date: Date?
}
struct ProjectBoardStreamEntry: Identifiable {
    let id = UUID()
    let at: Date
    let by: String
    let text: String
    let open: (() -> Void)?
}

/// What a dashboard shows, worked out from real data (project-board-data.ts).
/// Nothing is invented: a project with no deadlines shows no countdown.
struct ProjectBoardData {
    static let activityDays = 14
    var state: String?
    var closed = false
    var nextAction = ""
    var summary = ""
    var goal = ""
    var decisions = ""
    var blockers = ""
    var deadlines: [ProjectBoardDeadline] = []
    var work: [ProjectBoardWork] = []
    var people: [ProjectBoardPerson] = []
    var made: [ProjectBoardMade] = []
    var stream: [ProjectBoardStreamEntry] = []
    var activity = [Int](repeating: 0, count: Self.activityDays)

    var nextDeadline: ProjectBoardDeadline? {
        let today = Calendar.current.startOfDay(for: Date())
        return deadlines.first { ($0.date ?? .distantPast) >= today }
    }
    var daysLeft: Int? {
        guard let d = nextDeadline?.date else { return nil }
        return Calendar.current.dateComponents([.day], from: Calendar.current.startOfDay(for: Date()), to: Calendar.current.startOfDay(for: d)).day
    }
    var done: Int { work.filter { $0.column == .done }.count }
    var total: Int { work.count }
    var needsYou: [ProjectBoardWork] { work.filter { $0.column == .needsYou } }
    var stateLabel: String? {
        switch state {
        case "on_track": return "On track"
        case "at_risk": return "At risk"
        case "blocked": return "Blocked"
        case "needs_decision": return "Needs decision"
        case "done": return "Done"
        default: return nil
        }
    }
    var statusNeedsYou: Bool { !closed && (state == "needs_decision" || state == "blocked") }

    init() {}

    init(status: ProjectStatusRecord?, work: [ProjectBoardWork], people: [ProjectBoardPerson], made: [ProjectBoardMade]) {
        self.work = work
        self.people = people
        self.made = made
        if let status {
            state = status.state
            closed = status.lifecycle == "closed"
            nextAction = status.nextAction.trimmingCharacters(in: .whitespacesAndNewlines)
            summary = status.sections["status"]?.text ?? ""
            goal = status.sections["goal"]?.text ?? ""
            decisions = status.sections["decisions"]?.text ?? ""
            blockers = status.sections["blockers"]?.text ?? ""
            deadlines = (status.sections["deadlines"]?.text ?? "")
                .split(separator: "\n")
                .compactMap { Self.parseDeadline(String($0)) }
                .sorted { ($0.date ?? .distantFuture) < ($1.date ?? .distantFuture) }
        }
        let cal = Calendar.current
        let today = cal.startOfDay(for: Date())
        func count(_ date: Date?) {
            guard let date else { return }
            let back = cal.dateComponents([.day], from: cal.startOfDay(for: date), to: today).day ?? -1
            if back >= 0 && back < Self.activityDays { activity[Self.activityDays - 1 - back] += 1 }
        }
        var entries: [ProjectBoardStreamEntry] = []
        for entry in status?.log ?? [] {
            let date = Self.date(entry.at)
            count(date)
            if let date { entries.append(.init(at: date, by: entry.by, text: entry.note, open: nil)) }
        }
        for w in work {
            count(w.at)
            if let at = w.at { entries.append(.init(at: at, by: w.owner ?? "", text: w.title, open: w.open)) }
        }
        stream = entries.sorted { $0.at > $1.at }
    }

    static func date(_ iso: String) -> Date? {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = f.date(from: iso) { return d }
        f.formatOptions = [.withInternetDateTime]
        return f.date(from: iso)
    }

    /// A date written in a deadlines line ("Nov 14: launch", "2026-11-14 launch").
    static func parseDeadline(_ line: String, now: Date = Date()) -> ProjectBoardDeadline? {
        var text = line.trimmingCharacters(in: .whitespaces)
        if let r = text.range(of: #"^(?:[-*•]|\d+[.)])\s+"#, options: .regularExpression) { text.removeSubrange(r) }
        guard !text.isEmpty else { return nil }
        let cal = Calendar.current
        var date: Date?
        var matched: Range<String.Index>?
        if let r = text.range(of: #"\b\d{4}-\d{1,2}-\d{1,2}\b"#, options: .regularExpression) {
            let p = text[r].split(separator: "-").compactMap { Int($0) }
            date = cal.date(from: DateComponents(year: p[0], month: p[1], day: p[2]))
            matched = r
        } else if let r = text.range(of: #"(?i)\b(jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?\s+\d{1,2}(st|nd|rd|th)?(,?\s+\d{4})?"#, options: .regularExpression) {
            let piece = text[r].lowercased()
            let months = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"]
            let month = (months.firstIndex { piece.hasPrefix($0) } ?? 0) + 1
            let nums = piece.split { !$0.isNumber }.compactMap { Int($0) }
            let day = nums.first ?? 1
            let year = nums.count > 1 ? nums[1] : cal.component(.year, from: now)
            date = cal.date(from: DateComponents(year: year, month: month, day: day))
            if nums.count < 2, let d = date, d < cal.date(byAdding: .day, value: -30, to: now) ?? now {
                date = cal.date(byAdding: .year, value: 1, to: d)
            }
            matched = r
        }
        var label = text
        if let matched { label.removeSubrange(matched) }
        label = label.trimmingCharacters(in: CharacterSet(charactersIn: " :,-–—"))
        return ProjectBoardDeadline(label: label.isEmpty ? text : label, date: date)
    }
}

private func shortDate(_ date: Date?) -> String {
    guard let date else { return "—" }
    return date.formatted(.dateTime.month(.abbreviated).day())
}

// MARK: - Parts

private struct Tile<Content: View>: View {
    let slot: ProjectSlot
    var minHeight: CGFloat = 0
    @ViewBuilder var content: () -> Content
    var body: some View {
        VStack(alignment: .leading, spacing: 8) { content() }
            .frame(maxWidth: .infinity, minHeight: minHeight, alignment: .topLeading)
            .padding(16)
            .background(RoundedRectangle(cornerRadius: 20, style: .continuous).fill(slot.fill))
            .foregroundColor(slot.ink)
    }
}

private struct TileLabel: View {
    let text: String
    var systemImage: String? = nil
    var body: some View {
        HStack(spacing: 5) {
            if let systemImage { Image(systemName: systemImage).font(.system(size: 12)) }
            Text(text)
        }
        .font(.system(size: 12.5, weight: .semibold))
    }
}

private struct EmptyNote: View {
    let text: String
    init(_ text: String) { self.text = text }
    var body: some View {
        Text(text).font(.system(size: 13)).opacity(0.8).fixedSize(horizontal: false, vertical: true)
    }
}

private struct BigNumber: View {
    let text: String
    var size: CGFloat = 64
    var body: some View {
        Text(text).font(.system(size: size, weight: .heavy)).monospacedDigit().lineLimit(1).minimumScaleFactor(0.5)
    }
}

private struct PillButton: View {
    let title: String
    let slot: ProjectSlot
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(slot.fill)
                .padding(.horizontal, 14)
                .frame(minHeight: 44)
                .background(Capsule().fill(slot.ink))
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
    }
}

private struct ProgressRing: View {
    let done: Int
    let total: Int
    let accent: Color
    var size: CGFloat = 120
    var body: some View {
        ZStack {
            Circle().stroke(Color.primary.opacity(0.12), lineWidth: size / 9)
            Circle()
                .trim(from: 0, to: total > 0 ? CGFloat(done) / CGFloat(total) : 0)
                .stroke(accent, style: StrokeStyle(lineWidth: size / 9, lineCap: .round))
                .rotationEffect(.degrees(-90))
            VStack(spacing: 1) {
                Text("\(done)/\(total)").font(.system(size: size / 4.2, weight: .heavy)).monospacedDigit()
                Text("done").font(.system(size: 12)).opacity(0.75)
            }
        }
        .frame(width: size, height: size)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(done) of \(total) done")
    }
}

private struct DatesList: View {
    let data: ProjectBoardData
    var body: some View {
        if data.deadlines.isEmpty {
            EmptyNote("No deadlines on the status page yet.")
        } else {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(data.deadlines.prefix(5)) { d in
                    HStack(alignment: .firstTextBaseline, spacing: 12) {
                        Text(shortDate(d.date)).font(.system(size: 13, weight: .bold)).frame(width: 52, alignment: .leading)
                        Text(d.label).font(.system(size: 14)).fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
        }
    }
}

private struct StreamList: View {
    let data: ProjectBoardData
    let limit: Int
    var body: some View {
        if data.stream.isEmpty {
            EmptyNote("Changes show here as work happens.")
        } else {
            VStack(alignment: .leading, spacing: 8) {
                ForEach(data.stream.prefix(limit)) { e in
                    Button { e.open?() } label: {
                        VStack(alignment: .leading, spacing: 1) {
                            (Text(e.by.isEmpty ? "" : "\(e.by) ").bold() + Text(e.text))
                                .font(.system(size: 13.5))
                                .multilineTextAlignment(.leading)
                                .fixedSize(horizontal: false, vertical: true)
                            Text(e.at.formatted(.dateTime.month(.abbreviated).day().hour().minute()))
                                .font(.system(size: 12)).opacity(0.7)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(e.open == nil)
                }
            }
        }
    }
}

private struct TeamList: View {
    let data: ProjectBoardData
    var body: some View {
        if data.people.isEmpty {
            EmptyNote("No bots on this project yet.")
        } else {
            VStack(alignment: .leading, spacing: 8) {
                ForEach(data.people.prefix(5)) { p in
                    Button { p.open?() } label: {
                        HStack(spacing: 10) {
                            if let bot = p.bot { BotAvatarView(bot: bot, size: 28) }
                            VStack(alignment: .leading, spacing: 0) {
                                Text(p.name).font(.system(size: 13.5, weight: .bold))
                                Text(p.doing ?? "Idle").font(.system(size: 13)).lineLimit(1)
                            }
                            Spacer(minLength: 0)
                        }
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(p.open == nil)
                }
            }
        }
    }
}

private struct MadeList: View {
    let data: ProjectBoardData
    var body: some View {
        if data.made.isEmpty {
            EmptyNote("Files and things made in this project show here.")
        } else {
            LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 8) {
                ForEach(data.made.prefix(4)) { m in
                    Button { m.open?() } label: {
                        VStack(alignment: .leading, spacing: 1) {
                            Text(m.name).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                            Text(m.kind).font(.system(size: 12)).foregroundColor(Color("TextSecondary")).lineLimit(1)
                        }
                        .foregroundColor(Color("TextPrimary"))
                        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 4)
                        .background(RoundedRectangle(cornerRadius: 12).fill(Color("FillNeutral")))
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }
}

private struct ActivityChart: View {
    let data: ProjectBoardData
    let accent: Color
    @State private var selected: Int? = nil

    private func dayLabel(_ index: Int) -> String {
        let back = ProjectBoardData.activityDays - 1 - index
        return shortDate(Calendar.current.date(byAdding: .day, value: -back, to: Date()))
    }
    var body: some View {
        let series = data.activity
        let index = selected ?? (series.count - 1)
        let top = CGFloat(max(4, series.max() ?? 0))
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                BigNumber(text: "\(series[index])", size: 38).foregroundColor(accent)
                Text("\(series[index] == 1 ? "change" : "changes") on \(dayLabel(index))").font(.system(size: 14)).opacity(0.8)
            }
            GeometryReader { geo in
                let w = geo.size.width
                let h = geo.size.height
                let point: (Int) -> CGPoint = { i in
                    CGPoint(x: CGFloat(i) / CGFloat(series.count - 1) * w, y: h - CGFloat(series[i]) / top * h * 0.92)
                }
                ZStack(alignment: .topLeading) {
                    Path { p in
                        p.move(to: CGPoint(x: 0, y: h))
                        series.indices.forEach { p.addLine(to: point($0)) }
                        p.addLine(to: CGPoint(x: w, y: h))
                        p.closeSubpath()
                    }
                    .fill(accent.opacity(0.18))
                    Path { p in
                        p.move(to: point(0))
                        series.indices.dropFirst().forEach { p.addLine(to: point($0)) }
                    }
                    .stroke(accent, lineWidth: 2.5)
                    Path { p in
                        p.move(to: CGPoint(x: point(index).x, y: 0))
                        p.addLine(to: CGPoint(x: point(index).x, y: h))
                    }
                    .stroke(Color.primary.opacity(0.4), style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
                }
                .contentShape(Rectangle())
                .gesture(DragGesture(minimumDistance: 0).onChanged { value in
                    selected = min(series.count - 1, max(0, Int((value.location.x / max(w, 1) * CGFloat(series.count - 1)).rounded())))
                })
            }
            .frame(height: 130)
            HStack {
                Text(dayLabel(0))
                Spacer()
                Text("Today")
            }
            .font(.system(size: 12))
            .opacity(0.75)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(series.reduce(0, +)) changes in the last \(ProjectBoardData.activityDays) days, \(series.last ?? 0) today")
    }
}

private struct MonthGrid: View {
    let data: ProjectBoardData
    let accent: ProjectSlot
    var body: some View {
        let cal = Calendar.current
        let anchor = data.nextDeadline?.date ?? Date()
        let comps = cal.dateComponents([.year, .month], from: anchor)
        let first = cal.date(from: comps) ?? anchor
        let lead = cal.component(.weekday, from: first) - 1
        let count = cal.range(of: .day, in: .month, for: first)?.count ?? 30
        let events: [Int: String] = Dictionary(data.deadlines.compactMap { d -> (Int, String)? in
            guard let date = d.date, cal.isDate(date, equalTo: anchor, toGranularity: .month) else { return nil }
            return (cal.component(.day, from: date), d.label)
        }, uniquingKeysWith: { a, _ in a })
        VStack(alignment: .leading, spacing: 8) {
            TileLabel(text: anchor.formatted(.dateTime.month(.wide).year()))
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 4), count: 7), spacing: 4) {
                ForEach(0..<7, id: \.self) { i in
                    Text(["S", "M", "T", "W", "T", "F", "S"][i]).font(.system(size: 11.5)).opacity(0.7)
                }
                ForEach(0..<lead, id: \.self) { i in Color.clear.frame(height: 1).id("lead-\(i)") }
                ForEach(1...count, id: \.self) { day in
                    let label = events[day]
                    VStack(alignment: .leading, spacing: 0) {
                        Text("\(day)").font(.system(size: 12.5, weight: label == nil ? .regular : .semibold))
                        if let label { Text(label).font(.system(size: 9.5, weight: .semibold)).lineLimit(1) }
                        Spacer(minLength: 0)
                    }
                    .padding(4)
                    .frame(maxWidth: .infinity, minHeight: 44, alignment: .topLeading)
                    .foregroundColor(label == nil ? Color("TextPrimary") : accent.ink)
                    .background(RoundedRectangle(cornerRadius: 9).fill(label == nil ? Color("FillNeutral") : accent.fill))
                    .accessibilityLabel(label.map { "\(day): \($0)" } ?? "\(day)")
                }
            }
        }
    }
}

// MARK: - The dashboard

struct ProjectDashboardView: View {
    let layout: ProjectLayoutKind
    let look: ProjectLook
    let data: ProjectBoardData
    let title: String
    /// Opens the status page.
    var openStatus: (() -> Void)? = nil
    @Environment(\.colorScheme) private var scheme

    private var palette: ProjectPalette { ProjectPalette(set: .named(look.colors), dark: scheme == .dark) }
    private func on(_ tile: ProjectTile) -> Bool {
        layout == look.layout ? look.isOn(tile) : (layout == .custom ? (look.customTiles ?? []).contains(tile) : true)
    }
    private var decision: (label: String, text: String, needs: Bool, open: (() -> Void)?)? {
        if let first = data.needsYou.first { return ("Needs you", first.title, true, first.open) }
        if data.statusNeedsYou {
            let text = data.nextAction.isEmpty ? (data.blockers.isEmpty ? "Check the status page" : data.blockers) : data.nextAction
            return (data.stateLabel ?? "Needs you", text, true, nil)
        }
        if !data.nextAction.isEmpty { return ("Next", data.nextAction, false, nil) }
        return nil
    }

    var body: some View {
        switch layout {
        case .pulse: pulse
        case .bento, .custom: bento
        case .mission: mission
        case .score: score
        case .cover: cover
        case .runway: runway
        case .board: board
        case .crew: crew
        case .orbit: orbit
        case .gallery: gallery
        }
    }

    @ViewBuilder private var banner: some View {
        if on(.decision), let d = decision, d.needs {
            let slot = palette.slot(1)
            Tile(slot: slot) {
                TileLabel(text: d.label, systemImage: "exclamationmark.triangle")
                Text(d.text).font(.system(size: 17, weight: .bold)).fixedSize(horizontal: false, vertical: true)
                PillButton(title: d.open == nil ? "Status page" : "Open", slot: slot) { (d.open ?? openStatus)?() }
            }
        }
    }

    @ViewBuilder private func decisionTile(_ slot: ProjectSlot, size: CGFloat) -> some View {
        Tile(slot: slot, minHeight: 170) {
            if let d = decision {
                TileLabel(text: d.label, systemImage: d.needs ? "exclamationmark.triangle" : "checkmark")
                Text(d.text).font(.system(size: size, weight: .bold)).lineLimit(3).fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
                PillButton(title: d.open == nil ? "Status page" : "Open", slot: slot) { (d.open ?? openStatus)?() }
            } else {
                TileLabel(text: "Nothing needs you", systemImage: "checkmark")
                Text("You're clear.").font(.system(size: size, weight: .bold))
            }
        }
    }

    @ViewBuilder private func countdownTile(_ slot: ProjectSlot) -> some View {
        Tile(slot: slot, minHeight: 170) {
            if let days = data.daysLeft {
                TileLabel(text: data.nextDeadline?.label ?? "Next deadline")
                Spacer(minLength: 0)
                BigNumber(text: "\(days)", size: 64)
                Text("\(days == 1 ? "day" : "days") to \(shortDate(data.nextDeadline?.date))").font(.system(size: 13, weight: .semibold))
            } else {
                TileLabel(text: "Countdown")
                EmptyNote("No dated deadline yet. Add one on the status page.")
            }
        }
    }

    // 1 Pulse
    private var pulse: some View {
        VStack(spacing: 12) {
            banner
            if on(.chart) { Tile(slot: palette.plain) { ActivityChart(data: data, accent: palette.set.accent) } }
            if on(.schedule) { Tile(slot: palette.plain) { MonthGrid(data: data, accent: palette.slot(1)) } }
            if on(.activity) {
                Tile(slot: palette.plain) {
                    TileLabel(text: "Stream")
                    StreamList(data: data, limit: 7)
                }
            }
        }
    }

    // 2 Bento
    private var bento: some View {
        VStack(spacing: 12) {
            if on(.decision) { decisionTile(palette.slot(0), size: 26) }
            if on(.countdown) || on(.progress) {
                HStack(alignment: .top, spacing: 12) {
                    if on(.countdown) { countdownTile(palette.slot(3)) }
                    if on(.progress) {
                        Tile(slot: palette.plain, minHeight: 170) {
                            TileLabel(text: "Progress")
                            if data.total == 0 {
                                EmptyNote("Progress shows here once this project has work.")
                            } else {
                                ProgressRing(done: data.done, total: data.total, accent: palette.set.accent, size: 112)
                                    .frame(maxWidth: .infinity)
                            }
                        }
                    }
                }
            }
            if on(.team) { Tile(slot: palette.slot(2)) { TileLabel(text: "Team"); TeamList(data: data) } }
            if on(.chart) { Tile(slot: palette.plain) { ActivityChart(data: data, accent: palette.set.accent) } }
            if on(.schedule) { Tile(slot: palette.slot(4)) { TileLabel(text: "Coming up"); DatesList(data: data) } }
            if on(.files) { Tile(slot: palette.plain) { TileLabel(text: "Made so far"); MadeList(data: data) } }
            if on(.activity) { Tile(slot: palette.plain) { TileLabel(text: "Latest changes"); StreamList(data: data, limit: 3) } }
        }
    }

    // 3 Mission control
    private var mission: some View {
        let ink = Color(red: 0.93, green: 0.94, blue: 0.96)
        let panel = ProjectSlot(fill: Color.white.opacity(0.07), ink: ink)
        let working = data.work.filter { $0.column == .working }.count
        let open = data.work.filter { $0.column != .done }.count
        return VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text((data.stateLabel ?? "No status yet").uppercased())
                Spacer()
                Text(data.daysLeft.map { "T-\($0) DAYS" } ?? "NO DEADLINE")
            }
            .font(.system(size: 12.5, design: .monospaced))
            .opacity(0.85)
            Tile(slot: panel) {
                HStack(spacing: 20) {
                    ProgressRing(done: data.done, total: data.total, accent: palette.set.accent, size: 130)
                        .environment(\.colorScheme, .dark)
                    VStack(alignment: .leading, spacing: 10) {
                        missionStat(data.total > 0 ? "\(data.done * 100 / data.total)%" : "—", "DONE")
                        missionStat("\(working)", "RUNNING")
                        missionStat("\(open)", "OPEN")
                    }
                }
                .frame(maxWidth: .infinity)
            }
            if on(.team) { Tile(slot: panel) { Text("CREW").font(.system(size: 11.5, design: .monospaced)).opacity(0.75); TeamList(data: data) } }
            if on(.decision) { decisionTile(palette.slot(1), size: 22) }
            if on(.schedule) { Tile(slot: panel) { Text("NEXT EVENTS").font(.system(size: 11.5, design: .monospaced)).opacity(0.75); DatesList(data: data) } }
            if on(.activity) { Tile(slot: panel) { Text("LIVE LOG").font(.system(size: 11.5, design: .monospaced)).opacity(0.75); StreamList(data: data, limit: 5) } }
        }
        .padding(14)
        .background(RoundedRectangle(cornerRadius: 22, style: .continuous).fill(palette.deep))
        .foregroundColor(ink)
    }
    private func missionStat(_ value: String, _ label: String) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(value).font(.system(size: 24, weight: .bold, design: .monospaced))
            Text(label).font(.system(size: 11.5, design: .monospaced)).opacity(0.75)
        }
    }

    // 4 Scoreboard
    private func scoreColumn<C: View>(_ slot: ProjectSlot, _ number: String, _ label: String, @ViewBuilder content: @escaping () -> C) -> some View {
        Tile(slot: slot) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                BigNumber(text: number, size: 76)
                Text(label).font(.system(size: 15, weight: .bold))
            }
            Divider().overlay(slot.ink.opacity(0.3))
            content()
        }
    }
    private var score: some View {
        let running = data.work.filter { $0.column == .working }
        let needs = data.needsYou.count + (data.needsYou.isEmpty && data.statusNeedsYou ? 1 : 0)
        return VStack(spacing: 10) {
            if on(.countdown) {
                scoreColumn(palette.slot(0), data.daysLeft.map(String.init) ?? "—", data.daysLeft == 1 ? "day left" : "days left") { DatesList(data: data) }
            }
            if on(.progress) {
                scoreColumn(palette.slot(2), "\(data.done)/\(data.total)", "done") {
                    ForEach(data.work.filter { $0.column == .done }.prefix(5)) { w in
                        Label(w.title, systemImage: "checkmark").font(.system(size: 14)).lineLimit(1)
                    }
                }
            }
            if on(.decision) {
                scoreColumn(palette.slot(1), "\(needs)", "needs you") {
                    if let d = decision, d.needs {
                        Text(d.text).font(.system(size: 20, weight: .bold)).fixedSize(horizontal: false, vertical: true)
                        PillButton(title: d.open == nil ? "Status page" : "Open", slot: palette.slot(1)) { (d.open ?? openStatus)?() }
                    } else {
                        EmptyNote("Nothing is waiting on you.")
                    }
                }
            }
            if on(.team) {
                scoreColumn(palette.slot(3), "\(running.count)", "working") {
                    if running.isEmpty { EmptyNote("Nothing is running.") }
                    ForEach(running.prefix(5)) { w in
                        VStack(alignment: .leading, spacing: 0) {
                            if let owner = w.owner { Text(owner).font(.system(size: 14, weight: .bold)) }
                            Text(w.title).font(.system(size: 14)).lineLimit(2)
                        }
                    }
                }
            }
        }
    }

    // 5 Cover story
    private var cover: some View {
        let hero = palette.slot(0)
        let headline = decision?.needs == true ? decision?.text ?? title : (data.nextAction.isEmpty ? (data.goal.isEmpty ? title : data.goal) : data.nextAction)
        return VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 10) {
                Text(data.closed ? "CLOSED" : (data.stateLabel ?? "Status").uppercased()).font(.system(size: 13, weight: .semibold))
                Text(headline).font(.system(size: 34, design: .serif)).lineLimit(4).fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(20)
            .background(hero.fill)
            .foregroundColor(hero.ink)
            VStack(alignment: .leading, spacing: 18) {
                story("Where things stand", data.summary.isEmpty ? "No status written yet." : data.summary)
                story("Already decided", data.decisions.isEmpty ? "No decisions recorded yet." : data.decisions)
                if on(.schedule) {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("What happens next").font(.system(size: 21, weight: .bold, design: .serif))
                        DatesList(data: data)
                    }
                }
                if let open = openStatus {
                    PillButton(title: "Open the status page", slot: palette.slot(0), action: open)
                }
                if on(.decision), decision?.needs == true { decisionTile(palette.slot(1), size: 22) }
                if on(.files), !data.made.isEmpty { MadeList(data: data) }
            }
            .padding(20)
        }
        .background(Color("BgPanel"))
        .foregroundColor(Color("TextPrimary"))
        .clipShape(RoundedRectangle(cornerRadius: 22, style: .continuous))
    }
    private func story(_ heading: String, _ text: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(heading).font(.system(size: 21, weight: .bold, design: .serif))
            Text(text).font(.system(size: 15)).lineSpacing(3).fixedSize(horizontal: false, vertical: true)
        }
    }

    // 6 Runway
    private var runway: some View {
        let now = Date()
        let stamps = data.work.compactMap(\.at)
        let dated = data.deadlines.compactMap(\.date)
        let start = (stamps + [now.addingTimeInterval(-7 * 86_400)]).min() ?? now
        let end = (dated + [now.addingTimeInterval(7 * 86_400)]).max() ?? now
        let span = max(end.timeIntervalSince(start), 1)
        let x: (Date) -> CGFloat = { CGFloat($0.timeIntervalSince(start) / span) }
        return VStack(spacing: 12) {
            banner
            Tile(slot: palette.plain) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    BigNumber(text: data.daysLeft.map { "\($0) \($0 == 1 ? "day" : "days")" } ?? "No deadline", size: 38).foregroundColor(palette.set.accent)
                    Text(data.nextDeadline.map { "to \($0.label)" } ?? "Add deadlines on the status page.").font(.system(size: 14)).opacity(0.8)
                }
                HStack {
                    Text(shortDate(start))
                    Spacer()
                    Text(shortDate(end))
                }
                .font(.system(size: 12)).opacity(0.75)
                if data.work.isEmpty { EmptyNote("Work shows on the runway once this project has some.") }
                ForEach(Array(data.work.prefix(10).enumerated()), id: \.element.id) { i, w in
                    VStack(alignment: .leading, spacing: 3) {
                        (Text(w.owner.map { "\($0) " } ?? "").bold() + Text(w.title)).font(.system(size: 12)).lineLimit(1)
                        GeometryReader { geo in
                            let s = min(x(w.at ?? now), 0.92)
                            let stop = w.column == .done ? s + 0.08 : max(x(now), s + 0.08)
                            ZStack(alignment: .topLeading) {
                                Capsule()
                                    .fill(palette.slot(i).fill.opacity(w.column == .done ? 0.5 : 1))
                                    .frame(width: max((min(stop, 1) - s) * geo.size.width, 6), height: 14)
                                    .offset(x: s * geo.size.width)
                                Rectangle().fill(Color("TextPrimary").opacity(0.7)).frame(width: 1.5, height: 14).offset(x: x(now) * geo.size.width)
                                ForEach(dated, id: \.self) { d in
                                    Rectangle().fill(palette.set.accent).frame(width: 3, height: 14).offset(x: min(x(d), 0.995) * geo.size.width)
                                }
                            }
                        }
                        .frame(height: 14)
                    }
                }
            }
            if on(.schedule), !data.deadlines.isEmpty {
                LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 12) {
                    ForEach(Array(data.deadlines.prefix(4).enumerated()), id: \.element.id) { i, d in
                        Tile(slot: palette.slot(i)) {
                            Text(d.date == nil ? "No date" : shortDate(d.date)).font(.system(size: 22, weight: .heavy))
                            Text(d.label).font(.system(size: 13.5)).lineLimit(2)
                        }
                    }
                }
            }
        }
    }

    // 7 Board
    private var board: some View {
        let tones: [ProjectBoardWork.Column: ProjectSlot] = [.queued: palette.slot(2), .working: palette.slot(0), .needsYou: palette.slot(1), .done: palette.slot(3)]
        return ScrollView(.horizontal, showsIndicators: false) {
            HStack(alignment: .top, spacing: 10) {
                ForEach(ProjectBoardWork.Column.allCases, id: \.self) { column in
                    let items = data.work.filter { $0.column == column }
                    Tile(slot: tones[column] ?? palette.plain, minHeight: 300) {
                        HStack(alignment: .firstTextBaseline) {
                            Text(column.rawValue).font(.system(size: 16, weight: .bold))
                            Spacer()
                            Text("\(items.count)").font(.system(size: 24, weight: .heavy))
                        }
                        ForEach(items.prefix(12)) { w in
                            Button { w.open?() } label: {
                                VStack(alignment: .leading, spacing: 4) {
                                    Text(w.title).font(.system(size: 14, weight: .semibold)).multilineTextAlignment(.leading)
                                    if let owner = w.owner { Text(owner).font(.system(size: 12.5)).foregroundColor(Color("TextSecondary")) }
                                }
                                .foregroundColor(Color("TextPrimary"))
                                .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                                .padding(10)
                                .background(RoundedRectangle(cornerRadius: 13).fill(Color("BgPanel")))
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .frame(width: 250)
                }
            }
        }
    }

    // 8 Crew
    private var crew: some View {
        VStack(spacing: 12) {
            banner
            if data.people.isEmpty {
                Tile(slot: palette.plain) { EmptyNote("Bots show here once this project has a team.") }
            } else {
                LazyVGrid(columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible())], spacing: 12) {
                    ForEach(Array(data.people.enumerated()), id: \.element.id) { i, p in
                        Button { p.open?() } label: {
                            Tile(slot: palette.slot(i % 4), minHeight: 250) {
                                if let bot = p.bot { BotAvatarView(bot: bot, size: 68).frame(maxWidth: .infinity) }
                                Text(p.name).font(.system(size: 24, design: .serif))
                                Text(p.doing ?? "Idle").font(.system(size: 14)).multilineTextAlignment(.leading).fixedSize(horizontal: false, vertical: true)
                                Spacer(minLength: 0)
                                Text(p.state == "needs_you" ? "Needs you" : p.state == "working" ? "Working" : "Idle").font(.system(size: 12.5, weight: .semibold))
                            }
                        }
                        .buttonStyle(.plain)
                        .disabled(p.open == nil)
                    }
                }
            }
            if on(.activity) { Tile(slot: palette.plain) { TileLabel(text: "Latest changes"); StreamList(data: data, limit: 3) } }
        }
    }

    // 9 Orbit
    private var orbit: some View {
        let groups: [(name: String, tasks: [ProjectBoardWork])] = data.people.isEmpty
            ? ProjectBoardWork.Column.allCases.compactMap { c in
                let t = data.work.filter { $0.column == c }
                return t.isEmpty ? nil : (c.rawValue, Array(t.prefix(3)))
            }
            : data.people.prefix(4).map { p in (p.name, Array(data.work.filter { $0.owner == p.name }.prefix(3))) }
        return VStack(spacing: 12) {
            banner
            GeometryReader { geo in
                let scale = geo.size.width / 390
                let center = CGPoint(x: 195 * scale, y: 340 * scale)
                let spots = [CGPoint(x: 100, y: 190), CGPoint(x: 290, y: 190), CGPoint(x: 290, y: 490), CGPoint(x: 100, y: 490)]
                ZStack(alignment: .topLeading) {
                    ForEach(Array(groups.prefix(4).enumerated()), id: \.offset) { i, group in
                        let spot = CGPoint(x: spots[i].x * scale, y: spots[i].y * scale)
                        Path { p in p.move(to: center); p.addLine(to: spot) }.stroke(Color.white.opacity(0.3), lineWidth: 2)
                        let top = i < 2
                        ForEach(Array(group.tasks.enumerated()), id: \.element.id) { j, task in
                            Button { task.open?() } label: {
                                Text(task.title)
                                    .font(.system(size: 12, weight: .semibold))
                                    .lineLimit(1)
                                    .padding(.horizontal, 8)
                                    .frame(width: 172 * scale, height: 28)
                                    .foregroundColor(.white)
                                    .background(Capsule().fill(Color.white.opacity(task.column == .done ? 0.1 : 0.2)))
                            }
                            .buttonStyle(.plain)
                            .position(x: (i == 0 || i == 3 ? 98 : 292) * scale, y: (top ? 26 + CGFloat(j) * 33 : 654 - CGFloat(j) * 33) * scale)
                        }
                        let slot = palette.slot(i)
                        Text(group.name)
                            .font(.system(size: 12, weight: .bold))
                            .lineLimit(1)
                            .minimumScaleFactor(0.6)
                            .padding(4)
                            .foregroundColor(slot.ink)
                            .frame(width: 58, height: 58)
                            .background(Circle().fill(slot.fill).overlay(Circle().stroke(Color.white.opacity(0.3))))
                            .position(spot)
                    }
                    ProgressRing(done: data.done, total: data.total, accent: palette.set.accent, size: 128)
                        .environment(\.colorScheme, .dark)
                        .foregroundColor(.white)
                        .position(center)
                }
            }
            .frame(height: 680)
            .background(RoundedRectangle(cornerRadius: 22, style: .continuous).fill(palette.deep))
        }
    }

    // 10 Gallery
    private var gallery: some View {
        VStack(spacing: 12) {
            banner
            if data.made.isEmpty {
                Tile(slot: palette.plain) { EmptyNote("Files and things made in this project show here.") }
            } else {
                LazyVGrid(columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible())], spacing: 12) {
                    ForEach(Array(data.made.prefix(12).enumerated()), id: \.element.id) { i, m in
                        Button { m.open?() } label: {
                            Tile(slot: palette.slot(i), minHeight: 150) {
                                Text(m.kind).font(.system(size: 12.5, weight: .semibold))
                                Spacer(minLength: 0)
                                Text(m.name).font(.system(size: 19, weight: .bold, design: .serif)).multilineTextAlignment(.leading).lineLimit(3)
                            }
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
    }
}

// MARK: - Customize

/// The one place for a project's look and settings. A sheet at half height, so
/// the dashboard behind it is the preview.
struct ProjectCustomizeSheet<Details: View, Manage: View>: View {
    @ObservedObject var store: ProjectLookStore
    let data: ProjectBoardData
    let title: String
    let scope: BackdropScope
    let backdropColors: [String?]
    @ViewBuilder var details: () -> Details
    @ViewBuilder var manage: () -> Manage

    private enum Tab: String, CaseIterable { case layout = "Layout", colors = "Colors", background = "Background", tiles = "Tiles", details = "Details", manage = "Manage" }
    @State private var tab: Tab = .layout
    @State private var prompt = ""
    @State private var pickingBackground = false
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            VStack(spacing: 10) {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(Tab.allCases, id: \.self) { t in
                            Button { tab = t } label: {
                                Text(t.rawValue)
                                    .font(.system(size: 13, weight: tab == t ? .semibold : .medium))
                                    .foregroundColor(tab == t ? Color("BgPanel") : Color("TextPrimary"))
                                    .padding(.horizontal, 13)
                                    .frame(minHeight: 44)
                                    .background(Capsule().fill(tab == t ? Color("TextPrimary") : Color("FillNeutral")))
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding(.horizontal, 16)
                }
                ScrollView { content.padding(.horizontal, 16).padding(.bottom, 20) }
            }
            .navigationTitle("Customize")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }
        .presentationDetents([.medium, .large])
        .presentationBackgroundInteraction(.enabled(upThrough: .medium))
        .sheet(isPresented: $pickingBackground) {
            StageBackdropScopeSheet(scope: scope, title: "Background", colors: backdropColors)
        }
    }

    @ViewBuilder private var content: some View {
        switch tab {
        case .layout: layouts
        case .colors: colors
        case .background:
            VStack(alignment: .leading, spacing: 8) {
                Text("The background fills the page behind every layout.").font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                Button("Choose a background") { pickingBackground = true }
                    .frame(minHeight: 44)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        case .tiles: tiles
        case .details: VStack(alignment: .leading, spacing: 8) { details() }.frame(maxWidth: .infinity, alignment: .leading)
        case .manage: VStack(alignment: .leading, spacing: 8) { manage() }.frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var layouts: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 6) {
                Label("Make one", systemImage: "sparkle").font(.system(size: 13, weight: .semibold))
                HStack {
                    TextField("Just decisions and dates", text: $prompt)
                        .textFieldStyle(.roundedBorder)
                    Button("Build it") {
                        let words = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
                        guard !words.isEmpty else { return }
                        store.look.customPrompt = words
                        store.look.customTiles = projectTiles(forPrompt: words)
                        store.look.layout = .custom
                    }
                    .disabled(prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    .frame(minHeight: 44)
                }
            }
            LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 14) {
                if store.look.customTiles != nil { layoutCell(.custom) }
                ForEach(ProjectLayoutKind.builtIn) { layoutCell($0) }
            }
        }
    }

    private func layoutCell(_ kind: ProjectLayoutKind) -> some View {
        let selected = store.look.layout == kind
        return Button { store.look.layout = kind } label: {
            VStack(alignment: .leading, spacing: 5) {
                ProjectDashboardView(layout: kind, look: store.look, data: data, title: title)
                    .frame(width: 390, height: 640, alignment: .top)
                    .padding(10)
                    .background(Color("FillNeutral"))
                    .scaleEffect(0.36, anchor: .topLeading)
                    .frame(width: 148, height: 238, alignment: .topLeading)
                    .clipShape(RoundedRectangle(cornerRadius: 10))
                    .overlay(RoundedRectangle(cornerRadius: 10).stroke(selected ? Color("TextPrimary") : Color("BorderSubtle"), lineWidth: selected ? 2.5 : 1))
                    .allowsHitTesting(false)
                    .accessibilityHidden(true)
                HStack(spacing: 4) {
                    if selected { Image(systemName: "checkmark").font(.system(size: 11, weight: .bold)) }
                    Text(kind.name).font(.system(size: 12.5, weight: selected ? .bold : .medium))
                }
                .foregroundColor(Color("TextPrimary"))
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(kind.name) layout")
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private var colors: some View {
        LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 10) {
            ForEach(ProjectColorSet.all) { set in
                let selected = store.look.colors == set.id
                let palette = ProjectPalette(set: set, dark: false)
                Button { store.look.colors = set.id } label: {
                    VStack(alignment: .leading, spacing: 6) {
                        HStack(spacing: 2) {
                            ForEach(0..<5, id: \.self) { palette.slot($0).fill.frame(height: 40) }
                        }
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                        Text(set.name).font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                    }
                    .padding(10)
                    .overlay(RoundedRectangle(cornerRadius: 14).stroke(selected ? Color("TextPrimary") : Color("BorderSubtle"), lineWidth: selected ? 2.5 : 1))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("\(set.name) colors")
                .accessibilityAddTraits(selected ? .isSelected : [])
            }
        }
    }

    private var tiles: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Show on this layout:").font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
            ForEach(store.look.layout.tiles) { tile in
                Toggle(tile.name, isOn: Binding(get: { store.look.isOn(tile) }, set: { _ in store.toggle(tile) }))
                    .font(.system(size: 14))
                    .frame(minHeight: 44)
            }
        }
    }
}
