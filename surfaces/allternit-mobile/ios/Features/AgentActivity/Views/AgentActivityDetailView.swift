import SwiftUI

/// Agent Activity thread detail — pushed from `AgentActivityListView`'s
/// internal `NavigationStack` (a native push, never a `.sheet`; see
/// `AgentActivityListView`'s doc comment for why that matters here).
/// Structural pattern from `AutomationTaskDetailView`: header, then a
/// scrollable body — but unlike a cron job's fixed run-history list, this is
/// full message history with NO height cap (item 5's explicit constraint;
/// the web phase's whole reason for existing was fixing a height-capped
/// drill-down), a typed review card when the ledger-tail heuristic finds a
/// pending review, read-only guard/reservation cards, and a reply box.
struct AgentActivityDetailView: View {
    let thread: AgentActivityThreadSummary

    @StateObject private var store = AgentActivityStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var messages: [AgentActivityMessage] = []
    @State private var isLoadingMessages = false
    @State private var messagesError: String? = nil

    @State private var replyText = ""
    @State private var isSending = false
    @State private var isDeciding = false
    @State private var actionError: String? = nil

    private var threadState: AgentActivityThreadState {
        store.state(for: thread.threadId)
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                if let actionError {
                    Text(actionError)
                        .font(.caption)
                        .foregroundColor(Theme.statusWarning)
                }

                headerSection
                contextCards
                if threadState.hasPendingReview {
                    reviewCard
                }
                messagesSection
                replyRow
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 20)
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .navigationTitle(thread.threadId)
        .navigationBarTitleDisplayMode(.inline)
        .task {
            store.markViewed(thread.threadId)
            await loadMessages()
        }
    }

    // MARK: - Header

    private var headerSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(thread.messageCount == 1 ? "1 message" : "\(thread.messageCount) messages")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                Spacer()
                if let relativeText = AgentActivityListView.relativeText(thread.lastActivityAt) {
                    Text("Last activity \(relativeText)")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
        }
    }

    // MARK: - Guard / reservation context (read-only)

    /// The same last-4-relevant-events window `AgentActivityStore` derived
    /// its tags from — rendered honestly as raw `event_type` + relative
    /// time. There's no documented payload schema to summarize further
    /// (no "N paths reserved, Nm left"-style structured reservation object
    /// exists in the real data, matching the web phase's own finding), so
    /// this deliberately stays a plain read-only log rather than inventing
    /// richer copy the backend doesn't support.
    @ViewBuilder
    private var contextCards: some View {
        let events = store.relevantEvents(for: thread.threadId).filter {
            $0.eventType.range(of: "guard", options: .caseInsensitive) != nil
                || $0.eventType.range(of: "reserve", options: .caseInsensitive) != nil
        }
        if !events.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                Text("Guard & reservation activity")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                ForEach(events) { event in
                    HStack(spacing: 10) {
                        Image(systemName: event.eventType.range(of: "guard", options: .caseInsensitive) != nil
                              ? "lock.shield" : "lock")
                            .font(.system(size: 13, weight: .medium))
                            .foregroundColor(.red)
                        Text(event.eventType)
                            .font(.caption)
                            .foregroundColor(Color("TextPrimary"))
                        Spacer()
                        if let relativeText = AgentActivityListView.relativeText(event.timestamp) {
                            Text(relativeText)
                                .font(.caption2)
                                .foregroundColor(Color("TextSecondary"))
                        }
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(Color("BgPanel"))
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM))
                    .overlay(
                        RoundedRectangle(cornerRadius: Theme.radiusSM)
                            .stroke(Theme.borderWarmDefault, lineWidth: 1)
                    )
                }
            }
        }
    }

    // MARK: - Review card

    /// `decide()` is strictly boolean server-side (`MailDecideRequest`
    /// resolves to `"accepted"`/`"rejected"` either way — no N-way
    /// decisions), so this is Approve/Reject, not a richer decision UI. The
    /// heuristic that surfaces this card has no structured "what's being
    /// asked" payload to show (see `AgentActivityStore`'s derivation doc
    /// comment), so the card points at the message history above instead of
    /// fabricating a summary.
    private var reviewCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("REVIEW REQUESTED")
                .font(.caption2)
                .fontWeight(.bold)
                .foregroundColor(Theme.statusWarning)
            Text("Recent ledger activity on this thread includes a review request with no matching decision yet.")
                .font(.caption)
                .foregroundColor(Color("TextPrimary"))
            HStack(spacing: 10) {
                reviewButton(label: "Approve", color: Theme.statusSuccess) { decide(approve: true) }
                reviewButton(label: "Reject", color: .red) { decide(approve: false) }
            }
        }
        .padding(12)
        .background(Theme.statusWarning.opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusMD)
                .stroke(Theme.statusWarning.opacity(0.3), lineWidth: 1)
        )
        .disabled(isDeciding)
    }

    private func reviewButton(label: String, color: Color, action: @escaping () -> Void) -> some View {
        Button(action: {
            Haptics.light()
            action()
        }) {
            Text(label)
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(color)
                .frame(maxWidth: .infinity)
                .frame(height: 34)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusSM)
                        .stroke(color.opacity(0.4), lineWidth: 1)
                )
        }
        .buttonStyle(.plain)
    }

    // MARK: - Messages (full history, no height cap)

    private var messagesSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            if isLoadingMessages && messages.isEmpty {
                HStack {
                    Spacer()
                    ProgressView()
                    Spacer()
                }
                .padding(.vertical, 12)
            } else if let messagesError {
                HStack(spacing: 10) {
                    Image(systemName: "wifi.slash")
                        .font(.system(size: 14))
                        .foregroundColor(Theme.statusWarning)
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Couldn't load messages")
                            .font(.subheadline)
                            .foregroundColor(Color("TextPrimary"))
                        Text(FriendlyErrorMessage.from(messagesError))
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                        Button("Retry") {
                            Task { await loadMessages() }
                        }
                        .font(.caption)
                        .fontWeight(.semibold)
                        .foregroundColor(Color("AccentPrimary"))
                    }
                }
                .padding(.vertical, 4)
            } else if messages.isEmpty {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "bubble.left",
                    title: "No messages yet",
                    message: "Messages from this thread will appear here."
                )
            } else {
                VStack(spacing: 8) {
                    ForEach(messages) { message in
                        messageRow(message)
                    }
                }
            }
        }
    }

    private func messageRow(_ message: AgentActivityMessage) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(message.fromAgent)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(Color("AccentPrimary"))
                Spacer()
                if let relativeText = AgentActivityListView.relativeText(message.timestamp) {
                    Text(relativeText)
                        .font(.caption2)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            Text(message.body ?? "(no plain-text body — \(message.eventType))")
                .font(.subheadline)
                .foregroundColor(message.body != nil ? Color("TextPrimary") : Color("TextSecondary"))
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusMD)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    // MARK: - Reply

    private var replyRow: some View {
        HStack(spacing: 8) {
            TextField("Reply in this thread…", text: $replyText)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 12)
                .padding(.vertical, 9)
                .background(Color("BgSecondary"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
            Button(action: sendReply) {
                Image(systemName: "arrow.up.circle.fill")
                    .font(.system(size: 28))
                    .foregroundColor(replyText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                                      ? Color("TextSecondary") : Color("AccentPrimary"))
            }
            .buttonStyle(.plain)
            .disabled(isSending || replyText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
    }

    // MARK: - Actions

    private func loadMessages() async {
        if messages.isEmpty {
            isLoadingMessages = true
        }
        messagesError = nil
        do {
            messages = try await store.getThreadMessages(threadId: thread.threadId)
        } catch is CancellationError {
            // View went away mid-flight — keep current state.
        } catch {
            messagesError = error.localizedDescription
        }
        isLoadingMessages = false
    }

    private func sendReply() {
        let text = replyText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        isSending = true
        Task {
            do {
                try await store.send(threadId: thread.threadId, body: text)
                replyText = ""
                await loadMessages()
            } catch {
                actionError = "Couldn't send: \(error.localizedDescription)"
            }
            isSending = false
        }
    }

    private func decide(approve: Bool) {
        isDeciding = true
        Task {
            do {
                try await store.decide(threadId: thread.threadId, approve: approve)
                await loadMessages()
            } catch {
                actionError = "Couldn't record the decision: \(error.localizedDescription)"
            }
            isDeciding = false
        }
    }
}
