import SwiftUI

/// Per-bot desktop VM status + control state (the Desktop card on the bot
/// detail screen).
///
/// iOS counterpart of the web's BotDesktopView state
/// (surfaces/ai.allternit.com/src/views/bots/BotDesktopView.tsx), backed by
/// BotDesktopClient (`/api/v1/bots/:bot_id/desktop`,
/// cmd/allternit-api/src/bot_desktop_routes.rs).
///
/// The API has no route that LISTS a bot's sandbox — status and the control
/// actions all require the caller to supply `sandbox_id` — so the store
/// remembers the sandbox id each bot was provisioned with (provision is
/// idempotent server-side and returns the existing record, bot_desktop_
/// routes.rs:149-160). A bot with no remembered sandbox queries status with
/// an empty id and the server answers `off` — the card then offers
/// Provision.
///
/// Fetch posture follows the CronJobStore/WebhookTriggersStore convention:
/// one-shot fetch on appear, explicit refresh after each mutation, no
/// polling (the web view polls every 5s only to drive its live VNC canvas,
/// which iOS does not render). On failure the last known snapshot is kept
/// and `error` carries the server's message so the view can show it instead
/// of spinning forever.
@MainActor
final class BotDesktopStore: ObservableObject {
    static let shared = BotDesktopStore()

    /// One bot's desktop snapshot plus request state.
    struct Entry: Sendable {
        /// Last status answer; nil until the first successful fetch.
        var status: BotDesktopStatus?
        /// Sandbox id learned from provision/status — the control actions
        /// address the sandbox by this id.
        var sandboxId: String?
        /// Status fetch in flight.
        var isLoading: Bool
        /// Provision request in flight.
        var isProvisioning: Bool
        /// Observe/take-over/hand-back request in flight.
        var isControlActionInFlight: Bool
        /// Last load or action failure (server `{error}` body when present).
        var error: String?
    }

    @Published private(set) var entries: [String: Entry] = [:]

    private let client: BotDesktopClient
    /// Bot ids whose status request is in flight — duplicate concurrent
    /// fetches are dropped (CronJobStore's fetchTask pattern, keyed).
    private var fetchingBotIds: Set<String> = []

    init(client: BotDesktopClient = BotDesktopClient()) {
        self.client = client
    }

    func entry(for botId: String) -> Entry? {
        entries[botId]
    }

    // MARK: - Fetch

    /// Fetches the desktop status once unless forced; concurrent callers
    /// share the in-flight request (WebhookTriggersStore's
    /// `fetchTriggersIfNeeded` pattern).
    func fetchStatusIfNeeded(botId: String, force: Bool = false) {
        guard force || entries[botId]?.status == nil else { return }
        refresh(botId: botId)
    }

    /// Unconditional status refresh (re-appear, post-mutation resync).
    func refresh(botId: String) {
        guard !fetchingBotIds.contains(botId) else { return }
        fetchingBotIds.insert(botId)
        update(botId) { $0.isLoading = true; $0.error = nil }
        let sandboxId = entries[botId]?.sandboxId
        Task { [weak self] in
            guard let self else { return }
            defer {
                self.fetchingBotIds.remove(botId)
                self.update(botId) { $0.isLoading = false }
            }
            do {
                let status = try await self.client.status(botId: botId, sandboxId: sandboxId)
                self.update(botId) {
                    $0.status = status
                    if !status.sandboxId.isEmpty {
                        $0.sandboxId = status.sandboxId
                    }
                }
            } catch is CancellationError {
                // View went away mid-flight — keep current state.
            } catch {
                // A bot that never had a desktop is "off", not an error: the
                // screen then offers to set one up (web resolveLiveBotDesktop).
                if error.localizedDescription.contains("no persisted desktop sandbox") {
                    self.update(botId) {
                        $0.status = BotDesktopStatus(status: .off, controlState: .unknown, wsURL: nil,
                                                     streamProtocol: .none, sandboxId: "",
                                                     takenOverByUserId: nil, takenOverAt: nil)
                    }
                } else {
                    self.update(botId) { $0.error = error.localizedDescription }
                }
            }
        }
    }

    // MARK: - Mutations

    /// Provisions the bot's persistent virtual computer (idempotent
    /// server-side), then resyncs the status so the card reflects the fresh
    /// sandbox. The 503 no-driver case reaches the view via `entry.error`
    /// with the server's own message.
    func provision(botId: String) async {
        guard entries[botId]?.isProvisioning != true else { return }
        update(botId) { $0.isProvisioning = true; $0.error = nil }
        defer { update(botId) { $0.isProvisioning = false } }
        do {
            let provisioned = try await client.provision(botId: botId)
            update(botId) { $0.sandboxId = provisioned.sandboxId }
            refresh(botId: botId)
        } catch is CancellationError {
            // View went away mid-flight — keep current state.
        } catch {
            update(botId) { $0.error = error.localizedDescription }
        }
    }

    /// `POST …/desktop/observe` — watch without pausing the bot.
    func observe(botId: String) async {
        await controlAction(botId: botId, action: client.observe)
    }

    /// `POST …/desktop/take-over` — human drives, the bot's autonomous
    /// computer use pauses.
    func takeOver(botId: String) async {
        await controlAction(botId: botId, action: client.takeOver)
    }

    /// `POST …/desktop/hand-back` — control returns to the bot.
    func handBack(botId: String) async {
        await controlAction(botId: botId, action: client.handBack)
    }

    /// Shared control-action flow: guard against double taps, POST, then
    /// resync the full status (the action answers only
    /// `{control_state, sandbox_id}` — the refetch picks up
    /// `taken_over_at`/`ws_url`, matching the web's `loadStatus()` after
    /// each action, BotDesktopView.tsx:160-197).
    private func controlAction(
        botId: String,
        action: (String, String) async throws -> BotDesktopControlResponse
    ) async {
        guard let sandboxId = entries[botId]?.sandboxId, !sandboxId.isEmpty,
              entries[botId]?.isControlActionInFlight != true else { return }
        update(botId) { $0.isControlActionInFlight = true; $0.error = nil }
        defer { update(botId) { $0.isControlActionInFlight = false } }
        do {
            _ = try await action(botId, sandboxId)
            refresh(botId: botId)
        } catch is CancellationError {
            // View went away mid-flight — keep current state.
        } catch {
            update(botId) { $0.error = error.localizedDescription }
        }
    }

    private func update(_ botId: String, _ mutate: (inout Entry) -> Void) {
        var entry = entries[botId] ?? Entry(
            status: nil,
            sandboxId: nil,
            isLoading: false,
            isProvisioning: false,
            isControlActionInFlight: false,
            error: nil
        )
        mutate(&entry)
        entries[botId] = entry
    }
}
