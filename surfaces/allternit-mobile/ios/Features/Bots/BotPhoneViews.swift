import SwiftUI

// -----------------------------------------------------------------------------
// A bot's phone (allternit-ai src/lib/bot-phone/phone-data.ts): its messaging
// channels, the conversations on them, its number and email address, and
// which conversations are unread. Read marks are shared by every device of
// the owner through `/api/v1/bot-phone/seen` (a conversation is unread when
// it changed after its mark), so opening one here clears it on the web too.
//
// Endpoints (all through APIClient.shared):
//   GET  gateway/bots/:id/channels
//   GET  gateway/channel-accounts/:accountId/conversations
//   GET  phone/numbers                      (cloud: the account's numbers)
//   GET  agents/:id                         (the bot's own email address)
//   GET  bot-phone/seen   PUT bot-phone/seen
//   GET  threads/:id                        (a conversation's thread → session)
// -----------------------------------------------------------------------------

/// `GET gateway/channel-accounts/:id/conversations` row (lib/gateway/api.ts ChannelConversation).
struct ChannelConversationRecord: Decodable, Hashable, Sendable {
    let conversationId: String
    let conversation: String
    let name: String?
    let threadId: String
    let updatedAt: String?
    let lastText: String?

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        conversationId = try c.decode(String.self, forKey: .conversationId)
        conversation = try c.decodeIfPresent(String.self, forKey: .conversation) ?? conversationId
        name = try c.decodeIfPresent(String.self, forKey: .name)
        threadId = try c.decodeIfPresent(String.self, forKey: .threadId) ?? ""
        updatedAt = try c.decodeIfPresent(String.self, forKey: .updatedAt)
        lastText = try c.decodeIfPresent(String.self, forKey: .lastText)
    }

    enum CodingKeys: String, CodingKey {
        case conversationId, conversation, name, threadId, updatedAt, lastText
    }
}

private struct ChannelConversationList: Decodable {
    let conversations: [ChannelConversationRecord]?
}

/// One of the account's phone numbers (lib/gateway/phone-outbound.ts PhoneNumberRow).
struct BotPhoneNumber: Hashable, Sendable {
    let id: String
    let e164: String
    let botId: String?
    let smsState: String?

    var isPendingRegistration: Bool { smsState == "pending_registration" }
}

/// `GET phone/numbers` answers either a bare array or `{ numbers: [...] }`.
private struct PhoneNumberList: Decodable {
    let numbers: [BotPhoneNumber]

    private struct Row: Decodable {
        let id: String?
        let numberId: String?
        let e164: String?
        let botId: String?
        let smsState: String?
    }

    private enum Keys: String, CodingKey { case numbers }

    init(from decoder: Decoder) throws {
        let rows: [Row]
        if let list = try? decoder.singleValueContainer().decode([Row].self) {
            rows = list
        } else {
            let c = try decoder.container(keyedBy: Keys.self)
            rows = (try? c.decode([Row].self, forKey: .numbers)) ?? []
        }
        numbers = rows.compactMap { row in
            guard let e164 = row.e164, !e164.isEmpty else { return nil }
            return BotPhoneNumber(id: row.id ?? row.numberId ?? e164, e164: e164, botId: row.botId, smsState: row.smsState)
        }
    }
}

private struct SeenEnvelope: Codable {
    let seen: [String: Double]?
}

private struct AgentEmailProbe: Decodable {
    struct Channels: Decodable {
        struct Email: Decodable { let address: String? }
        let email: Email?
    }
    let identityChannels: Channels?
}

private struct ThreadEnvelope: Decodable {
    struct Thread: Decodable {
        let id: String
        let currentSessionId: String?
    }
    let thread: Thread
}

/// One conversation on the bot's phone.
struct BotPhoneConversation: Identifiable, Hashable, Sendable {
    let id: String
    let threadId: String
    let provider: String
    let channelLabel: String
    let name: String
    let lastText: String?
    let updatedAt: Date?
    let unread: Bool
}

/// Everything the phone screen and the drawer's Phones list need for one bot.
struct BotPhoneSnapshot: Sendable {
    var channels: [BotChannel] = []
    var conversations: [BotPhoneConversation] = []
    var numbers: [BotPhoneNumber] = []
    var emailAddress: String? = nil

    /// "Bots that have any channel, number or address" (hasAnyChannel).
    var hasPhone: Bool { !channels.isEmpty || !numbers.isEmpty || emailAddress != nil }
    var unreadCount: Int { conversations.filter(\.unread).count }
}

enum BotPhoneAPI {
    private static func escape(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))) ?? value
    }

    /// The account's numbers; empty when the cloud has none or can't be reached.
    static func numbers() async -> [BotPhoneNumber] {
        let list: PhoneNumberList? = try? await APIClient.shared.get(path: "phone/numbers")
        return list?.numbers ?? []
    }

    /// The owner's read marks (conversationId → epoch ms), merged with this device's.
    static func seen() async -> [String: Double] {
        let local = (UserDefaults.standard.dictionary(forKey: seenKey) as? [String: Double]) ?? [:]
        let envelope: SeenEnvelope? = try? await APIClient.shared.get(path: "bot-phone/seen")
        let merged = local.merging(envelope?.seen ?? [:]) { max($0, $1) }
        UserDefaults.standard.set(merged, forKey: seenKey)
        return merged
    }

    static let seenKey = "allternit:bot-phone:seen"

    /// Marks a conversation read here and on the owner's other devices.
    static func markSeen(_ conversationId: String, at date: Date = Date()) {
        var local = (UserDefaults.standard.dictionary(forKey: seenKey) as? [String: Double]) ?? [:]
        let at = max(date.timeIntervalSince1970 * 1000, local[conversationId] ?? 0)
        local[conversationId] = at
        UserDefaults.standard.set(local, forKey: seenKey)
        Task {
            try? await APIClient.shared.put(path: "bot-phone/seen", body: SeenEnvelope(seen: [conversationId: at]))
        }
    }

    private static func emailAddress(botId: String) async -> String? {
        let probe: AgentEmailProbe? = try? await APIClient.shared.get(path: "agents/\(escape(botId))")
        let address = probe?.identityChannels?.email?.address?.trimmingCharacters(in: .whitespacesAndNewlines)
        return (address?.isEmpty ?? true) ? nil : address
    }

    private static func conversations(accountId: String) async -> [ChannelConversationRecord] {
        let list: ChannelConversationList? = try? await APIClient.shared.get(
            path: "gateway/channel-accounts/\(escape(accountId))/conversations"
        )
        return list?.conversations ?? []
    }

    /// loadBotPhone: channels, the conversations on the enabled ones, the
    /// bot's numbers and (when the email channel is on) its address. Pass
    /// `numbers`/`seen` to share one read across many bots. Throws only when
    /// the channel list itself can't be read.
    static func load(botId: String, numbers shared: [BotPhoneNumber]? = nil,
                     seen sharedSeen: [String: Double]? = nil) async throws -> BotPhoneSnapshot {
        async let channelsLoad = BotModeClient.shared.listChannels(botId: botId)
        async let emailLoad = emailAddress(botId: botId)
        let allNumbers: [BotPhoneNumber]
        if let shared { allNumbers = shared } else { allNumbers = await numbers() }
        let marks: [String: Double]
        if let sharedSeen { marks = sharedSeen } else { marks = await seen() }
        let channels = try await channelsLoad
        let email = await emailLoad

        var rows: [BotPhoneConversation] = []
        await withTaskGroup(of: [BotPhoneConversation].self) { group in
            for channel in channels where channel.enabled {
                group.addTask {
                    await conversations(accountId: channel.accountId).map { record in
                        let updated = record.updatedAt.flatMap { BotTime.date($0) }
                        let updatedMs = (updated?.timeIntervalSince1970 ?? 0) * 1000
                        let trimmed = record.name?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
                        return BotPhoneConversation(
                            id: record.conversationId,
                            threadId: record.threadId,
                            provider: channel.provider,
                            channelLabel: channel.label,
                            name: trimmed.isEmpty ? record.conversation : trimmed,
                            lastText: record.lastText,
                            updatedAt: updated,
                            unread: updatedMs > (marks[record.conversationId] ?? 0)
                        )
                    }
                }
            }
            for await chunk in group { rows.append(contentsOf: chunk) }
        }
        rows.sort { ($0.updatedAt ?? .distantPast) > ($1.updatedAt ?? .distantPast) }
        let hasEmail = channels.contains { $0.provider == "email" && $0.enabled }
        return BotPhoneSnapshot(
            channels: channels,
            conversations: rows,
            numbers: allNumbers.filter { $0.botId == botId },
            emailAddress: hasEmail ? email : nil
        )
    }

    /// The session a conversation's thread is on (to open it as a bot chat).
    static func sessionId(threadId: String) async -> String? {
        guard !threadId.isEmpty else { return nil }
        let envelope: ThreadEnvelope? = try? await APIClient.shared.get(path: "threads/\(escape(threadId))")
        return envelope?.thread.currentSessionId
    }

    static func providerIcon(_ provider: String) -> String {
        switch provider {
        case "email": return "envelope"
        case "sms": return "message"
        case "telegram": return "paperplane"
        case "slack", "discord", "teams": return "number"
        case "whatsapp": return "phone.bubble"
        default: return "bubble.left.and.bubble.right"
        }
    }
}

/// The drawer's Phones roster (useBotPhoneRoster): one row per bot, whether
/// it has a phone and how many unread. Shared so the drawer badge and the
/// sheet agree; refreshed when the drawer opens and every minute while the
/// sheet is up.
@MainActor
final class BotPhonesStore: ObservableObject {
    static let shared = BotPhonesStore()

    struct Entry: Identifiable, Hashable {
        let botId: String
        let hasPhone: Bool
        let unread: Int
        var id: String { botId }
    }

    @Published private(set) var entries: [Entry] = []
    @Published private(set) var isLoading = false
    @Published private(set) var hasLoaded = false
    @Published private(set) var loadError: String? = nil

    var totalUnread: Int { entries.reduce(0) { $0 + $1.unread } }
    var withPhone: [Entry] { entries.filter(\.hasPhone) }

    private var inFlight: Task<Void, Never>?

    func refresh() async {
        if let inFlight { return await inFlight.value }
        let botIds = BotModeStore.shared.bots.map(\.id)
        guard !botIds.isEmpty else {
            entries = []
            hasLoaded = !AgentHubStore.shared.isLoading
            return
        }
        isLoading = true
        let task = Task { @MainActor in
            let numbers = await BotPhoneAPI.numbers()
            let seen = await BotPhoneAPI.seen()
            var out: [Entry] = []
            var failures = 0
            await withTaskGroup(of: (String, BotPhoneSnapshot?).self) { group in
                for botId in botIds {
                    group.addTask {
                        (botId, try? await BotPhoneAPI.load(botId: botId, numbers: numbers, seen: seen))
                    }
                }
                for await (botId, snapshot) in group {
                    guard let snapshot else { failures += 1; continue }
                    out.append(Entry(botId: botId, hasPhone: snapshot.hasPhone, unread: snapshot.unreadCount))
                }
            }
            let order = Dictionary(uniqueKeysWithValues: botIds.enumerated().map { ($1, $0) })
            entries = out.sorted { (order[$0.botId] ?? 0) < (order[$1.botId] ?? 0) }
            loadError = (failures == botIds.count) ? "Couldn't reach your bots' phones." : nil
            hasLoaded = true
            isLoading = false
        }
        inFlight = task
        await task.value
        inFlight = nil
    }

    /// A conversation was opened: drop it from the bot's unread count at once.
    func noteSeen(botId: String) {
        entries = entries.map { $0.botId == botId ? Entry(botId: $0.botId, hasPhone: $0.hasPhone, unread: max(0, $0.unread - 1)) : $0 }
    }
}

/// One bot's phone (the floating Allternit phone's Messages, natively): its
/// number and address, then every conversation on its channels, newest
/// first, unread ones marked. Tapping a conversation opens its thread as a
/// bot chat. Channels (switch connections on/off) is one tap away.
struct BotPhoneView: View {
    let bot: AgentRecord

    @EnvironmentObject private var navigator: BotNavigator
    @State private var snapshot = BotPhoneSnapshot()
    @State private var isLoading = true
    @State private var hasLoaded = false
    @State private var loadError: String? = nil
    @State private var openingId: String? = nil
    @State private var openError: String? = nil
    @State private var givingPhone = false

    var body: some View {
        List {
            if isLoading && !hasLoaded {
                HStack { Spacer(); ProgressView(); Spacer() }
                    .listRowBackground(Color.clear)
            } else if let loadError, !hasLoaded {
                FriendlyStateView(style: .offline, icon: "wifi.slash", title: "Couldn't load \(bot.botDisplayName)'s phone",
                                  message: loadError, actionTitle: "Retry", action: { Task { await load() } })
                    .listRowBackground(Color.clear)
            } else if !snapshot.hasPhone {
                FriendlyStateView(style: .empty, icon: "iphone", title: "No phone yet",
                                  message: "Give \(bot.botDisplayName) a number, an email address or a messaging channel.",
                                  actionTitle: "Give it a phone", action: { givingPhone = true })
                    .listRowBackground(Color.clear)
            } else {
                identitySection
                conversationsSection
            }

            Section {
                NavigationLink(value: BotRoute.channels(bot.id)) {
                    Label("Channels", systemImage: "dot.radiowaves.left.and.right")
                }
                if snapshot.hasPhone {
                    Button { givingPhone = true } label: {
                        Label("Add a number, email or app", systemImage: "plus")
                    }
                }
            }
        }
        .navigationTitle("\(bot.botDisplayName)'s phone")
        .navigationBarTitleDisplayMode(.inline)
        .refreshable { await load() }
        .task {
            await load()
            // The web phone polls every 20 s while it's open.
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 20_000_000_000)
                if Task.isCancelled { break }
                await load()
            }
        }
        .sheet(isPresented: $givingPhone, onDismiss: { Task { await load() } }) {
            GiveBotPhoneSheet(botId: bot.id)
        }
        .alert("Couldn't open that conversation", isPresented: Binding(
            get: { openError != nil }, set: { if !$0 { openError = nil } }
        )) {
            Button("OK", role: .cancel) { openError = nil }
        } message: {
            Text(openError ?? "")
        }
    }

    @ViewBuilder
    private var identitySection: some View {
        if !snapshot.numbers.isEmpty || snapshot.emailAddress != nil {
            Section("Number and address") {
                ForEach(snapshot.numbers, id: \.id) { number in
                    VStack(alignment: .leading, spacing: 2) {
                        Label(number.e164, systemImage: "phone")
                            .textSelection(.enabled)
                        if number.isPendingRegistration {
                            Text("Texts out are pending carrier registration — replies send once it's approved.")
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
                        }
                    }
                }
                if let email = snapshot.emailAddress {
                    Label(email, systemImage: "envelope")
                        .textSelection(.enabled)
                }
            }
        }
    }

    @ViewBuilder
    private var conversationsSection: some View {
        Section {
            if snapshot.conversations.isEmpty {
                Text("No conversations yet. When someone messages \(bot.botDisplayName), it shows up here.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
            } else {
                ForEach(snapshot.conversations) { conversation in
                    Button { open(conversation) } label: { conversationRow(conversation) }
                        .buttonStyle(.plain)
                        .disabled(openingId != nil)
                }
            }
        } header: {
            HStack {
                Text("Messages")
                Spacer()
                if snapshot.unreadCount > 0 {
                    Text("\(snapshot.unreadCount) unread")
                        .font(.caption.weight(.semibold))
                        .foregroundColor(Theme.statusError)
                }
            }
        }
    }

    private func conversationRow(_ conversation: BotPhoneConversation) -> some View {
        HStack(spacing: 12) {
            Image(systemName: BotPhoneAPI.providerIcon(conversation.provider))
                .font(.system(size: 15))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 30, height: 30)
                .background(Circle().fill(Color("BgSecondary")))
            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    Text(conversation.name)
                        .font(.subheadline.weight(conversation.unread ? .semibold : .regular))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Spacer(minLength: 6)
                    if let date = conversation.updatedAt {
                        Text(date, style: .relative)
                            .font(.caption2)
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(1)
                    }
                }
                Text(conversation.lastText ?? conversation.channelLabel)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
            }
            if openingId == conversation.id {
                ProgressView()
            } else if conversation.unread {
                Circle().fill(Color("AccentPrimary")).frame(width: 9, height: 9)
                    .accessibilityHidden(true)
            }
        }
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(conversation.name), \(conversation.channelLabel)\(conversation.unread ? ", unread" : "")")
    }

    private func open(_ conversation: BotPhoneConversation) {
        openingId = conversation.id
        BotPhoneAPI.markSeen(conversation.id)
        if conversation.unread { BotPhonesStore.shared.noteSeen(botId: bot.id) }
        Task {
            defer { openingId = nil }
            let known = BotModeStore.shared.threads(for: bot.id).first { $0.id == conversation.threadId }?.currentSessionId
            let sessionId: String?
            if let known { sessionId = known } else { sessionId = await BotPhoneAPI.sessionId(threadId: conversation.threadId) }
            guard let sessionId else {
                openError = "This conversation's thread isn't on this computer yet. Try again in a moment."
                return
            }
            navigator.open(.chat(botId: bot.id, sessionId: sessionId))
            await load()
        }
    }

    private func load() async {
        if !hasLoaded { isLoading = true }
        do {
            snapshot = try await BotPhoneAPI.load(botId: bot.id)
            hasLoaded = true
            loadError = nil
        } catch is CancellationError {
        } catch {
            loadError = FriendlyErrorMessage.from(error.localizedDescription)
        }
        isLoading = false
    }
}
