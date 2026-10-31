import SwiftUI

// Hermes parity, bot page (SPEC-hermes-parity-2026-10-31.md sections 3 and 4):
// the four tiles above the rows (Soul, Memory, Skills, Usage) and the pages
// they open. Data comes from `GET /bot-hub/:id/summary`; a value with no real
// source is hidden, a loading tile shows a quiet placeholder, an errored tile
// shows no value.

// MARK: - Summary loading

@MainActor
final class BotSummaryModel: ObservableObject {
    enum Phase { case loading, loaded(BotSummary), failed }
    @Published private(set) var phase: Phase = .loading

    var summary: BotSummary? {
        if case .loaded(let s) = phase { return s }
        return nil
    }

    func load(botId: String) async {
        if summary == nil { phase = .loading }
        do {
            phase = .loaded(try await HermesClient.shared.summary(botId: botId))
        } catch is CancellationError {
        } catch {
            if summary == nil { phase = .failed }
        }
    }
}

// MARK: - Tiles

struct BotSummaryTiles: View {
    let bot: AgentRecord
    @ObservedObject var model: BotSummaryModel
    @EnvironmentObject private var navigator: BotNavigator

    var body: some View {
        LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)], spacing: 10) {
            tile("Soul", icon: "heart.text.square", value: soulValue, route: .soul(bot.id))
            tile("Memory", icon: "brain", value: memoryValue, route: .memory(bot.id))
            tile("Skills", icon: "sparkles", value: skillsValue, route: .skills(bot.id))
            tile("Usage", icon: "chart.bar", value: usageValue, route: .usage(bot.id))
        }
        .padding(.horizontal, 14)
    }

    private var soulValue: String? {
        guard let at = model.summary?.soul?.updatedAt, !at.isEmpty else { return nil }
        let rel = BotTime.relative(at)
        return rel.isEmpty ? nil : "Edited \(rel)"
    }

    private var memoryValue: String? {
        model.summary?.memory?.count.map { "\($0) saved" }
    }

    private var skillsValue: String? {
        model.summary?.skills?.on.map { "\($0) on" }
    }

    private var usageValue: String? {
        guard let usage = model.summary?.usage else { return nil }
        if let spend = usage.spendUsd30d {
            return spend.formatted(.currency(code: "USD")) + " · 30 days"
        }
        return usage.sessions30d.map { "\($0) \($0 == 1 ? "session" : "sessions") · 30 days" }
    }

    private func tile(_ title: String, icon: String, value: String?, route: BotRoute) -> some View {
        Button { navigator.open(route) } label: {
            VStack(alignment: .leading, spacing: 6) {
                Image(systemName: icon)
                    .font(.system(size: 17))
                    .foregroundColor(Color("TextSecondary"))
                    .accessibilityHidden(true)
                Text(title)
                    .font(.body.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                Group {
                    if let value {
                        Text(value).foregroundColor(Color("TextTertiary"))
                    } else if case .loading = model.phase {
                        RoundedRectangle(cornerRadius: 4).fill(Color("FillNeutral")).frame(width: 64, height: 10)
                    } else {
                        Text(" ")
                    }
                }
                .font(.footnote)
                .lineLimit(2)
                .frame(minHeight: 14, alignment: .leading)
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle")))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(value.map { "\(title), \($0)" } ?? title)
    }
}

// MARK: - Soul

/// Description and the SOUL.md editor on one screen, plus Edit with chat
/// (the existing editor). Saves when you leave and with Save.
struct BotSoulPage: View {
    let bot: AgentRecord

    var body: some View {
        WorkspaceFileEditorView(
            agentId: bot.id,
            agentName: bot.botDisplayName,
            file: WorkspaceFileInfo(path: "SOUL.md", sizeBytes: 0, modifiedAt: nil),
            initialDescription: bot.description ?? ""
        )
        .navigationTitle("Soul")
    }
}

// MARK: - Skills

struct BotSkillsPage: View {
    let bot: AgentRecord
    var body: some View {
        CapabilitiesSettingsView(kind: .skill)
    }
}

// MARK: - Usage

struct BotUsagePage: View {
    let bot: AgentRecord
    @StateObject private var model = BotSummaryModel()

    var body: some View {
        List {
            switch model.phase {
            case .loading:
                HStack(spacing: 10) {
                    ProgressView()
                    Text("Loading usage").font(.subheadline).foregroundColor(Color("TextSecondary"))
                }
            case .failed:
                FriendlyInlineStateView(
                    style: .error, icon: "exclamationmark.triangle",
                    title: "Couldn't load usage", message: "Check your connection and try again.",
                    actionTitle: "Retry", action: { Task { await model.load(botId: bot.id) } }
                )
            case .loaded(let summary):
                let rows = usageRows(summary)
                if rows.isEmpty {
                    FriendlyInlineStateView(
                        style: .empty, icon: "chart.bar",
                        title: "No usage yet",
                        message: "\(bot.botDisplayName) has not been used in the last 30 days."
                    )
                } else {
                    Section {
                        ForEach(rows, id: \.0) { row in
                            LabeledContent(row.0) {
                                Text(row.1).monospacedDigit().foregroundColor(Color("TextPrimary"))
                            }
                        }
                    } header: {
                        Text("Last 30 days")
                    }
                }
                if let model = summary.model, !model.isEmpty {
                    Section {
                        LabeledContent("Model") { Text(model).foregroundColor(Color("TextPrimary")) }
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Usage")
        .navigationBarTitleDisplayMode(.inline)
        .refreshable { await model.load(botId: bot.id) }
        .task { await model.load(botId: bot.id) }
    }

    private func usageRows(_ summary: BotSummary) -> [(String, String)] {
        var rows: [(String, String)] = []
        if let n = summary.usage?.sessions30d { rows.append(("Sessions", "\(n)")) }
        if let n = summary.usage?.apiCalls30d { rows.append(("API calls", "\(n)")) }
        if let spend = summary.usage?.spendUsd30d { rows.append(("Spend", spend.formatted(.currency(code: "USD")))) }
        return rows
    }
}

// MARK: - Opening a map item

enum BotMapOpen {
    /// What Open does for a map item inside a bot, or nil when there is nothing to open.
    @MainActor
    static func action(bot: AgentRecord, navigator: BotNavigator, item: MemoryMapItem) -> (() -> Void)? {
        switch item.kind {
        case "thread":
            guard let ref = item.ref, !ref.isEmpty else { return nil }
            return { navigator.open(.chat(botId: bot.id, sessionId: ref)) }
        case "routine": return { navigator.open(.routines(bot.id)) }
        case "skill": return { navigator.open(.skills(bot.id)) }
        case "artifact": return { navigator.open(.botHome(bot.id, .artifacts)) }
        case "soul": return { navigator.open(.soul(bot.id)) }
        default: return nil
        }
    }
}

// MARK: - Stored memory sizes

struct StoredMemorySizes: View {
    let summary: BotSummary?

    var body: some View {
        let rows = sizes
        if !rows.isEmpty {
            ForEach(rows, id: \.0) { row in
                LabeledContent(row.0) { Text(row.1).monospacedDigit().foregroundColor(Color("TextPrimary")) }
            }
        }
    }

    private var sizes: [(String, String)] {
        var rows: [(String, String)] = []
        if let n = summary?.memory?.count { rows.append(("Saved memories", "\(n)")) }
        if let b = summary?.soul?.bytes {
            rows.append(("Soul", ByteCountFormatter.string(fromByteCount: Int64(b), countStyle: .file)))
        }
        if let s = summary?.skills, let on = s.on, let total = s.total { rows.append(("Skills", "\(on) of \(total) on")) }
        return rows
    }
}
