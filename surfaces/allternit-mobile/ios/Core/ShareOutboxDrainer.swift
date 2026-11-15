import Foundation

/// Sends what the share extension left in `OutboxStore` (plan step I7).
/// Runs on launch and every foreground (`AppDelegate`). Files go through
/// `AgentChatClient.upload` first, then one `BotModeClient.sendUserMessage`
/// carries the note, the text or link and the uploaded file links. An item is
/// removed only after the send succeeds; any failure leaves it for the next
/// run.
@MainActor
final class ShareOutboxDrainer {
    static let shared = ShareOutboxDrainer()

    private var draining = false
    private let chat: AgentChatClient
    private let bots: BotModeClient
    private let agents: AgentClient

    init(chat: AgentChatClient = AgentChatClient(),
         bots: BotModeClient = .shared,
         agents: AgentClient = AgentClient()) {
        self.chat = chat
        self.bots = bots
        self.agents = agents
    }

    func drain() async {
        guard !draining else { return }
        draining = true
        defer { draining = false }

        // Record each bot's model provider for the share sheet's consent text
        // (the snapshot does not carry it). Best effort.
        var records: [String: AgentRecord] = [:]
        if let all = try? await agents.listAgents() {
            records = Dictionary(all.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
            var providers = OutboxStore.botProviders
            for r in all where r.isBot && !r.provider.isEmpty { providers[r.id] = r.provider }
            OutboxStore.botProviders = providers
        }

        for var item in OutboxStore.pending() {
            if await send(&item, records: records) {
                OutboxStore.remove(item)
            } else {
                item.attempts += 1
                OutboxStore.save(item)
            }
        }
    }

    /// True once the message is accepted. Uploaded links are written back to
    /// the item so a retry does not upload twice.
    private func send(_ item: inout OutboxStore.Item, records: [String: AgentRecord]) async -> Bool {
        do {
            guard let sessionId = try await sessionId(for: item.botId, records: records) else { return false }
            for i in item.files.indices where item.files[i].uploadedURL == nil {
                guard let url = OutboxStore.fileURL(item.files[i]),
                      let data = try? Data(contentsOf: url) else { return false }
                let up = try await chat.upload(
                    name: item.files[i].name,
                    mediaType: item.files[i].mediaType,
                    dataBase64: data.base64EncodedString()
                )
                item.files[i].uploadedURL = up.url
                OutboxStore.save(item)
            }
            try await bots.sendUserMessage(sessionId: sessionId, text: Self.message(for: item))
            return true
        } catch {
            return false
        }
    }

    private func sessionId(for botId: String, records: [String: AgentRecord]) async throws -> String? {
        let store = BotModeStore.shared
        if let known = store.canonicalChatIds[botId] { return known }
        guard let record = records[botId] else { return nil }
        return try await store.mainChatSessionId(for: record)
    }

    /// The note first, then the shared text or link, then one link per file.
    static func message(for item: OutboxStore.Item) -> String {
        var parts: [String] = []
        if let n = item.note?.trimmingCharacters(in: .whitespacesAndNewlines), !n.isEmpty { parts.append(n) }
        if let t = item.text, !t.isEmpty { parts.append(t) }
        if let u = item.url, !u.isEmpty { parts.append(u) }
        for f in item.files {
            if let u = f.uploadedURL { parts.append("[\(f.name)](\(u))") }
        }
        return parts.joined(separator: "\n\n")
    }
}
