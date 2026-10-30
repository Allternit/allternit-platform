import SwiftUI

/// The bot screen's tabs (allternit-ai src/views/bots/home/bot-home-tabs.ts).
/// Ids are stable (deep links use them); `phoneLabel` is what the phone's
/// segmented control shows.
enum BotHomeTab: String, CaseIterable, Hashable {
    case threads, artifacts, routines, memory, config

    var phoneLabel: String {
        switch self {
        case .threads: return "Chat"
        case .artifacts: return "Artifacts"
        case .routines: return "Routines"
        case .memory: return "Memory"
        case .config: return "Settings"
        }
    }

    /// A deep-link tab value → a tab (web `normalizeBotHomeTab`); Computer,
    /// Performance and usage live in Settings now.
    static func normalize(_ value: String?) -> BotHomeTab {
        switch value?.lowercased() {
        case "artifacts": return .artifacts
        case "routines": return .routines
        case "memory": return .memory
        case "computer", "runtime", "performance", "usage", "config", "settings": return .config
        default: return .threads
        }
    }
}

/// A bot's screen on a phone (BotHomeView with its segmented tabs, as
/// PhoneBotPage pushes it): Chat · Artifacts · Routines · Memory · Settings.
/// Each tab hosts the view that already exists for it.
struct BotHomeTabsView: View {
    let bot: AgentRecord
    @State private var tab: BotHomeTab
    @EnvironmentObject private var navigator: BotNavigator
    @ObservedObject private var colors = BotColorStore.shared

    init(bot: AgentRecord, initialTab: BotHomeTab) {
        self.bot = bot
        _tab = State(initialValue: initialTab)
    }

    var body: some View {
        VStack(spacing: 0) {
            Picker("Bot", selection: $tab) {
                ForEach(BotHomeTab.allCases, id: \.self) { Text($0.phoneLabel).tag($0) }
            }
            .pickerStyle(.segmented)
            .padding(.horizontal, 14)
            .padding(.vertical, 8)

            Group {
                switch tab {
                case .threads:
                    VStack(spacing: 0) {
                        mainChatCard
                        BotThreadsOverview(botId: bot.id)
                    }
                case .artifacts: BotArtifactsList(bot: bot)
                case .routines: BotRoutinesView(bot: bot)
                case .memory: BotMemoryView(bot: bot)
                case .config: AgentDetailView(initialAgent: bot)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background { BotWashedBackground(color: .some(colors.hex(for: bot))).ignoresSafeArea() }
        .claimsBotTint(bot.id)
        .navigationTitle(bot.botDisplayName)
        .navigationBarTitleDisplayMode(.inline)
    }

    /// The bot's main chat as the first card on the Chat tab (BotMainChatCard).
    private var mainChatCard: some View {
        Button { navigator.open(.chat(botId: bot.id, sessionId: nil)) } label: {
            HStack(spacing: 12) {
                BotAvatarView(bot: bot, size: 36)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Main chat")
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text(bot.botTagline)
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
                Spacer()
                Image(systemName: "chevron.right")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextTertiary"))
            }
            .padding(14)
            .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle")))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 14)
        .padding(.top, 4)
    }
}

/// The Artifacts tab: what the bot made in its chats (the library's
/// artifacts whose session belongs to this bot).
struct BotArtifactsList: View {
    let bot: AgentRecord
    @ObservedObject private var library = ArtifactLibraryStore.shared
    @State private var sessionIds: Set<String> = []
    @State private var isLoading = true
    @State private var error: String?
    @State private var selected: ArtifactRecord?

    private var artifacts: [SavedArtifact] {
        library.artifacts.filter { $0.sessionId.map(sessionIds.contains) ?? false }
    }

    var body: some View {
        List {
            if isLoading && artifacts.isEmpty {
                ProgressView().frame(maxWidth: .infinity)
            } else if let error, artifacts.isEmpty {
                FriendlyStateView(style: .error, icon: "exclamationmark.triangle", title: "Couldn't load artifacts",
                                  message: error, actionTitle: "Retry", action: { Task { await load() } })
            } else if artifacts.isEmpty {
                Text("Artifacts \(bot.botDisplayName) makes in its chats show here.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
            } else {
                ForEach(artifacts) { saved in
                    Button { selected = saved.record } label: {
                        HStack(spacing: 12) {
                            Image(systemName: saved.record.isPreviewable ? "safari" : "doc.text")
                                .font(.system(size: 14))
                                .foregroundColor(Color("TextSecondary"))
                                .frame(width: 28, height: 28)
                                .background(RoundedRectangle(cornerRadius: 8).fill(Color("FillNeutral")))
                            VStack(alignment: .leading, spacing: 2) {
                                Text(saved.record.title)
                                    .foregroundColor(Color("TextPrimary"))
                                    .lineLimit(1)
                                Text(saved.record.fileType.uppercased())
                                    .font(.caption)
                                    .foregroundColor(Color("TextSecondary"))
                            }
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .refreshable { await load() }
        .task { await load() }
        .sheet(item: $selected) { ArtifactDetailsView(artifact: $0) }
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            let sessions = try await AgentChatClient().listSessions()
            sessionIds = Set(sessions.filter { $0.agentId == bot.id }.map(\.id))
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
        await library.refreshFromBackend()
    }
}
