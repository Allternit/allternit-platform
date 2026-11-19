import SwiftUI

/// Response-style choices for `PUT /api/v1/agent-preferences`
/// (agent_preferences_routes.rs — the raw values are the wire contract;
/// the backend 400s anything outside this set).
enum ResponseStyle: String, CaseIterable, Sendable {
    case concise, balanced, detailed, custom, learning

    var label: String {
        switch self {
        case .concise: return "Concise"
        case .balanced: return "Balanced"
        case .detailed: return "Detailed"
        case .custom: return "Custom"
        case .learning: return "Learning"
        }
    }

    /// Directive injected into the composed system prompt at send time.
    /// "balanced" and "custom" add nothing: balanced is the platform
    /// default, and custom is carried entirely by the instructions text.
    var promptDirective: String? {
        switch self {
        case .concise:
            return "Response style: keep responses brief and to the point — no preamble, no recap, no filler."
        case .balanced:
            return nil
        case .detailed:
            return "Response style: give thorough, detailed responses — full context, reasoning, and examples where they help."
        case .custom:
            return nil
        case .learning:
            return "Response style: The person wants to learn, not only get the answer. Give a little context, then ask one guiding question per turn. Let them try; after two or three wrong tries, or when they ask, give the answer and explain it. Show the reasoning before any verdict. Praise good method plainly, with no superlatives. If they drift off task, steer back gently."
        }
    }
}

/// User-level agent preferences (response style + custom instructions),
/// backed by `GET/PUT /api/v1/agent-preferences` — the same endpoint whose
/// PUT best-effort syncs a managed STYLE.md into each of the user's agent
/// workspaces (agent_preferences_routes.rs sync_style_md).
///
/// Fetched once per launch; saves are optimistic with rollback on failure.
/// The agent-chat bridge composes the directive into the system prompt
/// SERVER-SIDE from this same row (v1_routes.rs agent_chat_bridge) — the
/// client never injects it.
@MainActor
final class PreferencesStore: ObservableObject {
    static let shared = PreferencesStore()

    @Published private(set) var responseStyle: ResponseStyle = .balanced
    @Published private(set) var customInstructions: String = ""
    @Published private(set) var isLoaded = false
    @Published private(set) var isSaving = false
    /// Fetch/save failures surface in Settings as plain text (never silent).
    @Published var saveError: String? = nil
    /// Set on every successful save so views can flash a brief confirmation.
    @Published private(set) var lastSavedAt: Date? = nil

    private let client: PreferencesClient
    private var fetchTask: Task<Void, Never>? = nil

    init(client: PreferencesClient = PreferencesClient()) {
        self.client = client
    }

    /// Loads the preference row once per launch unless forced; concurrent
    /// callers share the in-flight request (UsageStore idiom).
    func fetchIfNeeded(force: Bool = false) {
        guard force || !isLoaded, fetchTask == nil else { return }
        fetchTask = Task { [weak self] in
            guard let self else { return }
            defer { self.fetchTask = nil }
            do {
                let prefs = try await self.client.get()
                self.responseStyle = ResponseStyle(rawValue: prefs.responseStyle) ?? .balanced
                self.customInstructions = prefs.customInstructions
                self.isLoaded = true
            } catch is CancellationError {
                // View went away mid-flight — keep current state.
            } catch {
                // Unreachable backend: keep defaults and mark loaded so the
                // endpoint isn't hammered on every Settings open.
                self.isLoaded = true
                self.saveError = error.localizedDescription
            }
        }
    }

    /// Optimistic save: the UI flips immediately; a failed PUT rolls back
    /// and reports the error. The backend's STYLE.md sync rides the PUT —
    /// no extra call from here.
    func save(style: ResponseStyle, instructions: String) {
        let previousStyle = responseStyle
        let previousInstructions = customInstructions
        responseStyle = style
        customInstructions = instructions
        saveError = nil
        isSaving = true
        Task { [weak self] in
            guard let self else { return }
            defer { self.isSaving = false }
            do {
                try await self.client.put(responseStyle: style.rawValue, customInstructions: instructions)
                self.lastSavedAt = Date()
            } catch is CancellationError {
                // A newer save superseded this one — leave state alone.
            } catch {
                self.responseStyle = previousStyle
                self.customInstructions = previousInstructions
                self.saveError = error.localizedDescription
            }
        }
    }

    /// The style directive + the user's verbatim instructions as one block.
    /// NOTE: the SEND path no longer uses this — the agent-chat bridge
    /// composes persona + preferences server-side from the agents and
    /// user_agent_preferences rows (v1_routes.rs agent_chat_bridge). Kept
    /// as the canonical client-side rendering of the preference (e.g. for
    /// a future Settings preview of "what the agent is told").
    var systemPromptDirective: String? {
        let instructions = customInstructions.trimmingCharacters(in: .whitespacesAndNewlines)
        var parts: [String] = []
        if let directive = responseStyle.promptDirective { parts.append(directive) }
        if !instructions.isEmpty {
            parts.append("Custom instructions from the user:\n\(instructions)")
        }
        return parts.isEmpty ? nil : parts.joined(separator: "\n\n")
    }
}
