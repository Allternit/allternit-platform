import SwiftUI

// -----------------------------------------------------------------------------
// Group chats (allternit-ai src/views/bots/GroupsListView.tsx,
// GroupChatView.tsx, components/bots/stage/group/GroupStage.tsx). Rooms
// persist through `/api/v1/bot-groups`; member turns run on this device the
// way the web's group-chat-turn-runner does: each responding bot answers in
// an ephemeral agent session, sees the room history, may "(pass)", and may
// @mention another member into the next round.
// -----------------------------------------------------------------------------

struct GroupRowLabel: View {
    let group: BotGroupChat
    @ObservedObject private var store = BotModeStore.shared

    var body: some View {
        HStack(spacing: 10) {
            GroupAvatarStack(members: group.members, size: 30)
            VStack(alignment: .leading, spacing: 2) {
                Text(group.name).lineLimit(1)
                Text(group.log.last.map { "\($0.displayName ?? "You"): \($0.text)" }
                     ?? group.members.map(\.displayName).joined(separator: ", "))
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
            }
        }
    }
}

/// Overlapping member avatars (GroupChatAvatar).
struct GroupAvatarStack: View {
    let members: [GroupChatMember]
    var size: CGFloat = 30
    @ObservedObject private var store = BotModeStore.shared

    var body: some View {
        HStack(spacing: -size * 0.35) {
            ForEach(members.prefix(3)) { member in
                Group {
                    if let bot = store.bot(member.botId) {
                        BotAvatarView(bot: bot, size: size)
                    } else {
                        AgentIdenticonView(seed: member.botId, size: size)
                    }
                }
                .background(Circle().fill(Color("BgPrimary")).padding(-1))
            }
        }
        .frame(minWidth: size)
    }
}

struct GroupsListView: View {
    @ObservedObject private var store = BotModeStore.shared
    @EnvironmentObject private var navigator: BotNavigator
    @State private var isCreating = false

    var body: some View {
        List {
            if let error = store.groupsError, store.groups.isEmpty {
                FriendlyStateView(style: .error, icon: "exclamationmark.triangle", title: "Couldn't load group chats",
                                  message: error, actionTitle: "Retry", action: { Task { await store.refreshGroups() } })
            } else if store.groups.isEmpty {
                FriendlyStateView(style: .empty, icon: "person.3", title: "No group chats yet",
                                  message: "Put two or more bots in one room and talk to them together.",
                                  actionTitle: "New group chat", action: { isCreating = true })
            } else {
                ForEach(store.groups) { group in
                    NavigationLink(value: BotRoute.group(group.id)) { GroupRowLabel(group: group) }
                        .swipeActions {
                            Button("Delete", role: .destructive) {
                                Task { try? await store.deleteGroup(group.id) }
                            }
                        }
                }
            }
        }
        .navigationTitle("Groups")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button { isCreating = true } label: { Image(systemName: "plus") }
                    .accessibilityLabel("New group chat")
            }
        }
        .refreshable { await store.refreshGroups() }
        .task { await store.refreshGroups() }
        .sheet(isPresented: $isCreating) {
            NewGroupChatSheet { group in
                navigator.open(.group(group.id))
            }
        }
    }
}

struct NewGroupChatSheet: View {
    var onCreated: (BotGroupChat) -> Void

    @ObservedObject private var store = BotModeStore.shared
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var selected: Set<String> = []
    @State private var isSaving = false
    @State private var error: String? = nil

    private var canCreate: Bool {
        !name.trimmingCharacters(in: .whitespaces).isEmpty && (2...6).contains(selected.count) && !isSaving
    }

    var body: some View {
        NavigationStack {
            Form {
                Section("Name") {
                    TextField("Launch crew", text: $name)
                }
                Section {
                    ForEach(store.bots) { bot in
                        Button {
                            if selected.contains(bot.id) { selected.remove(bot.id) } else if selected.count < 6 { selected.insert(bot.id) }
                        } label: {
                            HStack(spacing: 10) {
                                BotAvatarView(bot: bot, size: 30)
                                Text(bot.botDisplayName).foregroundColor(Color("TextPrimary"))
                                Spacer()
                                if selected.contains(bot.id) {
                                    Image(systemName: "checkmark.circle.fill").foregroundColor(Color("AccentPrimary"))
                                }
                            }
                        }
                    }
                } header: {
                    Text("Members")
                } footer: {
                    Text("Pick 2 to 6 bots.")
                }
                if let error {
                    Text(error).foregroundColor(Theme.statusError)
                }
            }
            .navigationTitle("New group chat")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(isSaving ? "Creating…" : "Create") { create() }.disabled(!canCreate)
                }
            }
        }
    }

    private func create() {
        isSaving = true
        let members = store.bots.filter { selected.contains($0.id) }
        Task {
            defer { isSaving = false }
            do {
                let group = try await store.createGroup(name: name.trimmingCharacters(in: .whitespaces), members: members)
                dismiss()
                onCreated(group)
            } catch {
                self.error = FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
    }
}

/// One room: the group stage (who's in, who's talking) over the log, and a
/// composer. `@name` targets members; `@everyone` asks all.
struct GroupChatView: View {
    let groupId: String

    @ObservedObject private var store = BotModeStore.shared
    @State private var draft = ""
    @State private var runner = GroupTurnRunner()
    @State private var speakingBotId: String? = nil
    @State private var isRunning = false
    @State private var runError: String? = nil

    var body: some View {
        if let group = store.group(groupId) {
            VStack(spacing: 0) {
                GroupStageView(group: group, speakingBotId: speakingBotId)
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 10) {
                            if group.log.isEmpty {
                                Text("Say hi. Everyone answers, or @mention one of them.")
                                    .font(.footnote)
                                    .foregroundColor(Color("TextSecondary"))
                                    .frame(maxWidth: .infinity)
                                    .padding(.top, 24)
                            }
                            ForEach(group.log) { message in
                                GroupMessageRow(message: message).id(message.id)
                            }
                            if let speakingBotId, let bot = store.bot(speakingBotId) {
                                HStack(spacing: 8) {
                                    BotAvatarView(bot: bot, size: 22, live: true)
                                    Text("\(bot.botDisplayName) is typing…")
                                        .font(.caption)
                                        .foregroundColor(Color("TextSecondary"))
                                }
                            }
                            if let runError {
                                Text(runError).font(.caption).foregroundColor(Theme.statusError)
                            }
                        }
                        .padding(14)
                    }
                    .onChange(of: group.log.count) { _, _ in
                        if let last = group.log.last?.id {
                            withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo(last, anchor: .bottom) }
                        }
                    }
                }
                composer(group)
            }
            .background(Color("BgPrimary"))
            .navigationTitle(group.name)
            .navigationBarTitleDisplayMode(.inline)
        } else {
            FriendlyStateView(style: .empty, icon: "person.3", title: "Group not found",
                              message: "This room was deleted on another device.")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .task { await store.refreshGroups() }
        }
    }

    private func composer(_ group: BotGroupChat) -> some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField("Message \(group.name)", text: $draft, axis: .vertical)
                .lineLimit(1...5)
                .padding(.horizontal, 12)
                .padding(.vertical, 9)
                .background(Color("BgSecondary"))
                .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
            Button {
                send(group)
            } label: {
                Image(systemName: isRunning ? "stop.circle.fill" : "arrow.up.circle.fill")
                    .font(.system(size: 32))
                    .foregroundColor(Color("AccentPrimary"))
            }
            .disabled(!isRunning && draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            .accessibilityLabel(isRunning ? "Stop" : "Send")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(Color("BgPrimary"))
    }

    private func send(_ group: BotGroupChat) {
        if isRunning {
            runner.cancel()
            return
        }
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        draft = ""
        runError = nil
        isRunning = true
        var room = group
        room.log.append(GroupChatMessage(id: UUID().uuidString, from: "user", botId: nil, displayName: "You",
                                         text: text, timestamp: ISO8601DateFormatter().string(from: Date()), threadId: nil))
        room.updatedAt = ISO8601DateFormatter().string(from: Date())
        let start = room
        Task {
            await store.saveGroup(start)
            let finished = await runner.run(
                room: start,
                userText: text,
                bots: store.bots,
                onSpeaker: { speakingBotId = $0 },
                onReply: { updated in
                    Task { await store.saveGroup(updated) }
                }
            )
            speakingBotId = nil
            isRunning = false
            if finished.replies == 0 && !finished.cancelled {
                runError = "Nobody answered. Check that the members' brains are set up."
            }
        }
    }
}

struct GroupMessageRow: View {
    let message: GroupChatMessage
    @ObservedObject private var store = BotModeStore.shared

    var body: some View {
        if message.from == "user" {
            HStack {
                Spacer(minLength: 40)
                Text(message.text)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(Color("AccentPrimary").opacity(0.14))
                    .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
            }
        } else {
            HStack(alignment: .top, spacing: 8) {
                if let botId = message.botId, let bot = store.bot(botId) {
                    BotAvatarView(bot: bot, size: 28)
                } else {
                    AgentIdenticonView(seed: message.botId ?? "bot", size: 28)
                }
                VStack(alignment: .leading, spacing: 2) {
                    Text(message.displayName ?? "Bot")
                        .font(.caption.weight(.semibold))
                        .foregroundColor(Color("TextSecondary"))
                    Text(LocalizedStringKey(message.text))
                        .foregroundColor(Color("TextPrimary"))
                        .textSelection(.enabled)
                }
                Spacer(minLength: 20)
            }
        }
    }
}

/// The group stage: every member on a strip, the one talking lifted and
/// animated (GroupStage.tsx, phone size).
struct GroupStageView: View {
    let group: BotGroupChat
    let speakingBotId: String?
    @ObservedObject private var store = BotModeStore.shared
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 16) {
                ForEach(group.members) { member in
                    let speaking = member.botId == speakingBotId
                    VStack(spacing: 4) {
                        if let bot = store.bot(member.botId) {
                            BotAvatarView(bot: bot, size: speaking ? 56 : 44, live: speaking)
                        } else {
                            AgentIdenticonView(seed: member.botId, size: 44)
                        }
                        Text(member.displayName)
                            .font(.caption2.weight(speaking ? .semibold : .regular))
                            .foregroundColor(speaking ? Color("TextPrimary") : Color("TextSecondary"))
                            .lineLimit(1)
                    }
                    .offset(y: speaking && !reduceMotion ? -4 : 0)
                    .animation(reduceMotion ? nil : .spring(response: 0.35, dampingFraction: 0.8), value: speakingBotId)
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel(speaking ? "\(member.displayName), talking" : member.displayName)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
        }
        .frame(height: 96)
        .background(Color("BgSecondary"))
    }
}

/// Runs member turns (lib/bots/group-chat-turn-runner.ts, native subset).
@MainActor
final class GroupTurnRunner {
    struct Outcome { let replies: Int; let cancelled: Bool }

    private static let maxRounds = 3
    private static let maxMessagesPerSend = 10
    private static let maxHistory = 30

    private let chatClient = AgentChatClient()
    private var cancelled = false

    func cancel() { cancelled = true }

    func run(room start: BotGroupChat, userText: String, bots: [AgentRecord],
             onSpeaker: (String?) -> Void, onReply: (BotGroupChat) -> Void) async -> Outcome {
        cancelled = false
        var room = start
        var replies = 0
        var responders = Self.responders(for: userText, members: room.members, sinceUser: [])
        for _ in 0..<Self.maxRounds {
            guard !responders.isEmpty, !cancelled else { break }
            var roundReplies: [GroupChatMessage] = []
            for member in responders {
                guard !cancelled, replies < Self.maxMessagesPerSend,
                      let bot = bots.first(where: { $0.id == member.botId }) else { continue }
                onSpeaker(member.botId)
                guard let text = await reply(as: member, bot: bot, in: room, userText: userText) else { continue }
                let message = GroupChatMessage(id: UUID().uuidString, from: "bot", botId: member.botId,
                                               displayName: member.displayName, text: text,
                                               timestamp: ISO8601DateFormatter().string(from: Date()), threadId: nil)
                room.log.append(message)
                room.updatedAt = message.timestamp
                roundReplies.append(message)
                replies += 1
                onReply(room)
            }
            // Only members @mentioned by this round's replies go again.
            let mentioned = Set(roundReplies.flatMap { Self.mentions($0.text) })
            responders = mentioned.isEmpty ? [] : room.members.filter {
                mentioned.contains($0.handle.lowercased()) || mentioned.contains($0.displayName.lowercased())
            }
        }
        onSpeaker(nil)
        return Outcome(replies: replies, cancelled: cancelled)
    }

    private func reply(as member: GroupChatMember, bot: AgentRecord, in room: BotGroupChat, userText: String) async -> String? {
        let names = room.members.map(\.displayName).joined(separator: ", ")
        let history = room.log.suffix(Self.maxHistory).map { "\($0.displayName ?? "User"): \($0.text)" }.joined(separator: "\n")
        let prompt = [
            "You are \(member.displayName), a member of the group chat \"\(room.name)\".",
            "Other members in this room: \(names).",
            "",
            "Recent conversation:",
            history,
            "",
            "The user said: \(userText)",
            "",
            "Reply briefly as \(member.displayName), or say \"(pass)\" to stay silent.",
            "You may @mention another member to pull them into the next round.",
            "Escalate to @user if a real human judgment is needed.",
        ].joined(separator: "\n")
        let system = ["You are \(member.displayName). You must ALWAYS identify yourself as \(member.displayName).",
                      bot.systemPrompt ?? ""].filter { !$0.isEmpty }.joined(separator: "\n\n")
        var sessionId: String? = nil
        defer {
            if let sessionId {
                Task { try? await APIClient.shared.delete(path: "agent-sessions/\(sessionId)") }
            }
        }
        do {
            let session = try await chatClient.createSession(
                name: member.displayName, originSurface: "chat", sessionMode: "agent",
                agentId: bot.id, agentName: member.displayName, ephemeral: true
            )
            sessionId = session.id
            var text = ""
            for try await event in chatClient.sendMessageStream(sessionId: session.id, text: prompt,
                                                                agentId: bot.id, systemPrompt: system) {
                if cancelled { break }
                if case .textDelta(let delta) = event { text += delta.text }
            }
            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            let lowered = trimmed.lowercased()
            if trimmed.isEmpty || ["(pass)", "pass", "pass."].contains(lowered) { return nil }
            return trimmed
        } catch {
            return nil
        }
    }

    static func mentions(_ text: String) -> [String] {
        text.split(whereSeparator: { $0.isWhitespace }).compactMap { word in
            guard word.hasPrefix("@") else { return nil }
            let name = word.dropFirst().prefix { $0.isLetter || $0.isNumber || $0 == "_" || $0 == "-" }
            return name.isEmpty ? nil : name.lowercased()
        }
    }

    static func responders(for userText: String, members: [GroupChatMember], sinceUser: [String]) -> [GroupChatMember] {
        let named = Set(mentions(userText))
        if named.isEmpty || named.contains("everyone") || named.contains("all") { return members }
        let picked = members.filter {
            named.contains($0.handle.lowercased()) || named.contains($0.displayName.lowercased())
                || named.contains($0.botId.lowercased())
        }
        return picked.isEmpty ? members : picked
    }
}
