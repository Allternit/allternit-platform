import SwiftUI

/// A bot chat (allternit-ai src/views/bots/BotChatSessionView.tsx), hosted
/// on the shared chat stack: `ChatContentView` + `ChatViewModel` (agent
/// session SSE via AgentChatClient). With no explicit session it opens the
/// bot's main chat, recovering it before ever creating one
/// (BotModeStore.mainChatSessionId).
struct BotChatSessionView: View {
    let bot: AgentRecord
    let explicitSessionId: String?

    @StateObject private var viewModel = ChatViewModel()
    @ObservedObject private var store = BotModeStore.shared
    @ObservedObject private var status = BotStatusStore.shared
    @EnvironmentObject private var navigator: BotNavigator
    @State private var sessionId: String? = nil
    @State private var resolveError: String? = nil
    @State private var isStartingThread = false

    private var isMainChat: Bool { explicitSessionId == nil }

    var body: some View {
        Group {
            if let sessionId {
                VStack(spacing: 0) {
                    if let line = liveLine {
                        Text(line)
                            .font(.caption)
                            .foregroundColor(status.needsAttention(for: bot.id) ? Theme.statusWarning : Color("TextSecondary"))
                            .lineLimit(1)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 6)
                            .background(Color("BgSecondary"))
                    }
                    ChatContentView(sessionId: sessionId, viewModel: viewModel)
                }
            } else if let resolveError {
                FriendlyStateView(
                    style: .error,
                    icon: "exclamationmark.bubble",
                    title: "Couldn't open \(bot.botDisplayName)",
                    message: resolveError,
                    actionTitle: "Try again",
                    action: { Task { await resolve() } }
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                VStack(spacing: 12) {
                    BotAvatarView(bot: bot, size: 72, live: true)
                    ProgressView()
                    Text("Opening \(bot.botDisplayName)…")
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .background(Color("BgPrimary"))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar { toolbar }
        .task {
            status.subscribe(botId: bot.id)
            await resolve()
        }
        .onDisappear { status.unsubscribe(botId: bot.id) }
        .onChange(of: viewModel.transientError) { _, error in
            // A main-chat pin whose session is gone: drop it so the next
            // open recovers or recreates (web resolveCanonicalChatSession).
            if isMainChat, let error, error.hasPrefix("Couldn't load the conversation") {
                store.forgetMainChat(for: bot.id)
            }
        }
    }

    /// The thread's status line while the bot works or waits.
    private var liveLine: String? {
        guard let sessionId else { return nil }
        if let thread = store.threads.first(where: { $0.currentSessionId == sessionId }),
           thread.group == "waiting" || thread.group == "working" {
            return thread.statusLine ?? (thread.group == "waiting" ? "Needs you" : "Working…")
        }
        if status.needsAttention(for: bot.id) { return "Needs you" }
        return nil
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .principal) {
            Button {
                navigator.open(.page(bot.id))
            } label: {
                HStack(spacing: 8) {
                    BotStatusAvatar(bot: bot, size: 26,
                                    working: status.isWorking(for: bot.id),
                                    waiting: status.needsAttention(for: bot.id))
                    VStack(alignment: .leading, spacing: 0) {
                        Text(bot.botDisplayName)
                            .font(.subheadline.weight(.semibold))
                            .foregroundColor(Color("TextPrimary"))
                        Text(isMainChat ? "Main chat" : "Thread")
                            .font(.caption2)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
            }
            .accessibilityLabel("\(bot.botDisplayName) profile")
        }
        ToolbarItemGroup(placement: .topBarTrailing) {
            Button {
                navigator.open(.stage(bot.id))
            } label: {
                Image(systemName: "sparkles")
            }
            .accessibilityLabel("Live stage")
            Button {
                startThread()
            } label: {
                Image(systemName: isStartingThread ? "hourglass" : "square.and.pencil")
            }
            .disabled(isStartingThread)
            .accessibilityLabel("New thread")
        }
    }

    private func resolve() async {
        resolveError = nil
        if let explicitSessionId {
            sessionId = explicitSessionId
            return
        }
        do {
            sessionId = try await store.mainChatSessionId(for: bot)
        } catch is CancellationError {
        } catch {
            resolveError = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    private func startThread() {
        isStartingThread = true
        Task {
            defer { isStartingThread = false }
            do {
                let id = try await store.newThreadSession(for: bot)
                navigator.open(.chat(botId: bot.id, sessionId: id))
            } catch {
                viewModel.transientError = FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
    }
}
