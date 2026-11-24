import SwiftUI

/// A structured, renderable version of a stream failure — replaces the old
/// behavior of appending the backend's raw error string ("gizzi message
/// failed (400 Bad Request): {…json…}") into the message text.
///
/// Built from the agent-chat `finish` frame's `metadata.errorDetails` (the
/// runtime's structured error, passed through by v1_routes.rs).
struct ChatError: Equatable, Sendable {
    /// Runtime error name when known (e.g. "ProviderModelNotFoundError").
    var name: String?
    /// Short human title ("Model not available").
    var title: String
    /// One or two sentences of guidance.
    var detail: String
    /// The full raw payload, shown behind the disclosure row for debugging.
    var raw: String
    /// True for provider/model resolution failures — the card's primary
    /// action becomes "Choose a model" instead of "Retry".
    var isModelError: Bool
    /// Extra buttons for the card (e.g. the Claude organization refusals).
    /// Empty for every other error.
    var actions: [ChatErrorAction] = []
    /// The typed reason (usage limit, signed out, network, server) when the
    /// server's error frame or a refusal code said which; nil for plain text.
    var info: ChatErrorInfo? = nil

    /// A typed failure: the title says what happened (and when a usage limit
    /// resets), the detail is the server's reason, the action is the way out.
    init(info: ChatErrorInfo, raw: String, lane: ChatErrorInfo.Lane = .chat) {
        self.name = info.code
        self.info = info
        self.raw = raw
        self.isModelError = info.action == .pickModel
        self.title = info.title(lane: lane)
        self.detail = info.reason
        if info.action == .connect {
            self.actions = [info.isChatGPT
                ? ChatErrorAction(kind: .connectChatGPT, title: info.actionLabel ?? "Connect ChatGPT", systemImage: "link")
                : ChatErrorAction(kind: .openConnections, title: info.actionLabel ?? "Open Connections", systemImage: "gearshape")]
        }
    }

    /// Builds a card from the finish frame's metadata fields.
    init(name: String?, message: String?, raw: String) {
        let isNoBrain = name == "no_brain"
        let isModelError = name == "ProviderModelNotFoundError" || isNoBrain
        self.name = name
        self.isModelError = isModelError
        self.raw = raw

        if isNoBrain {
            self.title = AgentChatClientError.noBrainMessage
            self.detail = "Choose the brain this chat runs on, then send again."
        } else if isModelError {
            self.title = "Model not available"
            self.detail = "The model for this chat couldn't be found. Pick another model and try again."
        } else {
            self.title = "Something went wrong"
            let trimmed = message?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            self.detail = trimmed.isEmpty ? "The response failed before it could complete." : trimmed
        }
    }

    /// Transport/creation failures (no runtime payload to parse).
    static func connectionInterrupted(_ reason: String) -> ChatError {
        ChatError(
            name: nil,
            message: reason,
            raw: reason
        )
    }
}

/// A button a specific error adds to its card. The chat decides what each
/// kind does; the card only draws it.
struct ChatErrorAction: Equatable, Sendable, Identifiable {
    enum Kind: Equatable, Sendable {
        /// Re-send the same message once on an Allternit Cloud model, on tap.
        case useCredits
        /// Open Settings › Connections (the Claude organization row).
        case manageClaudeOrg
        /// Open Settings › Connections to link the organization again.
        case linkAgain
        /// Open the ChatGPT sign-in (the connect sheet) from a signed-out error.
        case connectChatGPT
        /// Open Settings › Connections from a signed-out error that is not ChatGPT's.
        case openConnections
    }
    let kind: Kind
    let title: String
    let systemImage: String
    var id: Kind { kind }

    /// The actions for a `claude_link_*` refusal code; none for other codes.
    static func forClaudeLink(code: String) -> [ChatErrorAction] {
        guard code.hasPrefix("claude_link_") else { return [] }
        var list = [ChatErrorAction(kind: .useCredits, title: "Use Allternit credits for this message", systemImage: "sparkles")]
        if code == "claude_link_disconnected" {
            list.append(ChatErrorAction(kind: .linkAgain, title: "Link again", systemImage: "link"))
        }
        list.append(ChatErrorAction(kind: .manageClaudeOrg, title: "Manage Claude organization", systemImage: "gearshape"))
        return list
    }
}

/// Which Allternit Cloud model the credits button re-sends on: the person's
/// most recent Cloud model when it is still listed, else the first listed
/// Cloud model, else none (the button is hidden).
enum CreditsFallback {
    static func modelId(recent: String?, listed: [String]) -> String? {
        if let recent, listed.contains(recent) { return recent }
        return listed.first
    }
}

/// Inline error card rendered under an assistant message (Claude-style
/// status card instead of raw JSON in the bubble). Primary action is
/// contextual: model errors offer "Choose a model", everything else offers
/// "Retry". The raw payload lives behind a disclosure row.
struct ChatErrorCardView: View {
    let error: ChatError
    let onChooseModel: () -> Void
    let onRetry: () -> Void
    /// Draws only these kinds from `error.actions` (the credits button is
    /// left out when no Cloud model is listed).
    var availableActions: Set<ChatErrorAction.Kind> = []
    var onAction: (ChatErrorAction) -> Void = { _ in }

    private var visibleActions: [ChatErrorAction] {
        error.actions.filter { availableActions.contains($0.kind) }
    }

    @State private var showRaw = false

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundColor(Theme.statusWarning)
                Text(error.title)
                    .font(.subheadline)
                    .fontWeight(.semibold)
                    .foregroundColor(Color("TextPrimary"))
            }

            Text(error.detail)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .fixedSize(horizontal: false, vertical: true)

            ForEach(visibleActions) { item in
                actionButton(title: item.title, systemImage: item.systemImage,
                             primary: item.kind == .useCredits || item.kind == .connectChatGPT || item.kind == .openConnections, action: { onAction(item) })
                    .accessibilityLabel(item.title)
            }

            HStack(spacing: 10) {
                if error.isModelError {
                    actionButton(title: "Choose a model", systemImage: "cpu", action: onChooseModel)
                }
                actionButton(title: "Retry", systemImage: "arrow.clockwise",
                             primary: !error.isModelError && visibleActions.isEmpty, action: onRetry)
            }

            Button(action: {
                withAnimation(.easeInOut(duration: 0.2)) { showRaw.toggle() }
            }) {
                HStack(spacing: 4) {
                    Text("Details")
                        .font(.caption2)
                    Image(systemName: showRaw ? "chevron.up" : "chevron.down")
                        .font(.system(size: 8, weight: .bold))
                }
                .foregroundColor(Color("TextSecondary"))
            }
            .buttonStyle(.plain)

            if showRaw {
                Text(error.raw)
                    .font(.system(.caption2, design: .monospaced))
                    .foregroundColor(Color("TextSecondary"))
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(12)
        .frame(maxWidth: 320, alignment: .leading)
        .background(Color("BgPrimary"))
        .cornerRadius(12)
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(Theme.statusWarning.opacity(0.4), lineWidth: 1)
        )
    }

    private func actionButton(title: String, systemImage: String, primary: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: {
            Haptics.light()
            action()
        }) {
            HStack(spacing: 5) {
                Image(systemName: systemImage)
                    .font(.system(size: 10, weight: .semibold))
                Text(title)
                    .font(.system(size: 12, weight: .semibold))
            }
            .foregroundColor(primary ? .black : Color("TextPrimary"))
            .padding(.horizontal, 12)
            .frame(height: 30)
            .background(primary ? Color("AccentPrimary") : Color("BgSecondary"))
            .clipShape(Capsule())
            .overlay(
                Capsule().stroke(primary ? .clear : Theme.borderWarmDefault, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
    }
}
