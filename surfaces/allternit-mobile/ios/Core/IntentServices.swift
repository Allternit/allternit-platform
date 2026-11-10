import Foundation

/// The app's `IntentServicing`, over the existing clients. Registered once at
/// launch (`IntentServiceRegistry.register(IntentServices())`).
final class IntentServices: IntentServicing, @unchecked Sendable {
    private let bots = BotModeClient.shared
    private let inbox = InboxClient.shared
    private let routines = RoutinesClient.shared
    private let agents = AgentClient()
    private let chat = AgentChatClient()

    // MARK: Gate support

    func isSignedIn() -> Bool { AppGroupStore.signedIn }
    func hasComputer() -> Bool { AppGroupStore.hasComputer }

    func isSleepingError(_ error: Error) -> Bool {
        switch error {
        case RuntimeRelayError.unreachable, is NotificationActionTimeout: return true
        case APIError.httpError(let status, _): return [502, 503, 504].contains(status)
        default: return false
        }
    }

    func wakeComputer(timeout: TimeInterval) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        repeat {
            let online = await MainActor.run { () -> Task<Bool, Never> in
                Task { @MainActor in
                    await RuntimeRelayStore.shared.refresh()
                    return RuntimeRelayStore.shared.activeDevice?.isOnline == true
                }
            }.value
            if online { return true }
            try? await Task.sleep(for: .seconds(2))
        } while Date() < deadline
        return false
    }

    func classify(_ error: Error) -> IntentFailure {
        if let f = error as? IntentFailure { return f }
        switch error {
        case RuntimeRelayError.noComputer: return .noComputer
        case RuntimeRelayError.unreachable, is URLError, is NotificationActionTimeout: return .offline
        case APIError.httpError(let status, let message):
            switch status {
            case 401, 403: return .signedOut
            case 404: return .notFound("that")
            case 502, 503, 504: return .offline
            default: return .failed(message ?? "HTTP \(status)")
            }
        default: return .failed(error.localizedDescription)
        }
    }

    // MARK: Directory

    private func botRecords() async throws -> [AgentRecord] {
        try await agents.listAgents().filter { $0.isBot }
    }

    func listBots() async throws -> [BotInfo] {
        try await botRecords().map { BotInfo(id: $0.id, name: $0.name, status: $0.status) }
    }

    private func conversation(_ t: BotThread) -> ConversationInfo {
        ConversationInfo(id: t.id, sessionId: t.currentSessionId, title: t.displayTitle,
                         botId: t.botId, lastActivityAt: t.lastActivityAt)
    }

    func listConversations(matching: String?) async throws -> [ConversationInfo] {
        let all = try await bots.listAllThreads()
        let q = matching?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() ?? ""
        return all.filter { q.isEmpty || $0.displayTitle.lowercased().contains(q) }
            .sorted { $0.lastActivityAt > $1.lastActivityAt }.map(conversation)
    }

    func listRoutines() async throws -> [RoutineInfo] {
        try await routines.listRoutines().map { RoutineInfo(id: $0.id, name: $0.name, state: $0.state, schedule: $0.schedule) }
    }

    func listProjects() async throws -> [ProjectInfo] {
        try await bots.listBotProjects().filter { !$0.archived }.map { ProjectInfo(id: $0.id, title: $0.title) }
    }

    func listComputers() async throws -> [ComputerInfo] {
        await MainActor.run { () -> Task<[ComputerInfo], Never> in
            Task { @MainActor in
                let store = RuntimeRelayStore.shared
                await store.refresh()
                let active = store.activeRuntimeId
                return store.devices.map {
                    ComputerInfo(id: $0.id, name: $0.displayName, typeLabel: $0.typeLabel,
                                 isOnline: $0.isOnline, isActive: $0.id == active)
                }
            }
        }.value
    }

    func listModels() async throws -> [ModelInfo] {
        try await chat.listModels().map { ModelInfo(id: $0.id, name: $0.name, provider: $0.provider) }
    }

    // MARK: Sessions

    private func botName(_ botId: String) async throws -> String {
        guard let b = try await botRecords().first(where: { $0.id == botId }) else {
            throw IntentFailure.notFound("that bot")
        }
        return b.name
    }

    func mainConversation(botId: String) async throws -> ConversationInfo? {
        let pins = UserDefaults.standard.dictionary(forKey: "allternit-bot-mode-canonical-chats") as? [String: String]
        if let pin = pins?[botId] {
            return ConversationInfo(id: pin, sessionId: pin, title: "Main chat", botId: botId)
        }
        let main = try await bots.listAllThreads()
            .filter { $0.botId == botId && $0.isMainChat && $0.currentSessionId != nil }
            .max { $0.lastActivityAt < $1.lastActivityAt }
        return main.map(conversation)
    }

    func newChat(botId: String) async throws -> ConversationInfo {
        let name = try await botName(botId)
        let s = try await bots.createBotSession(botId: botId, botName: name, name: "Chat with \(name)", metadata: [:])
        return ConversationInfo(id: s.id, sessionId: s.id, title: s.name ?? name, botId: botId)
    }

    private func chatTarget(botId: String) async throws -> ConversationInfo {
        if let main = try await mainConversation(botId: botId), main.sessionId != nil { return main }
        return try await newChat(botId: botId)
    }

    // MARK: Talk

    func ask(botId: String, text: String, wait: TimeInterval) async throws -> AskResult {
        let name = try await botName(botId)
        let convo = try await chatTarget(botId: botId)
        guard let sid = convo.sessionId else { throw IntentFailure.notFound("that chat") }
        let before = try await chat.listMessages(sessionId: sid).count
        try await bots.sendUserMessage(sessionId: sid, text: text)
        let deadline = Date().addingTimeInterval(wait)
        var last = ""
        var stable = 0
        while Date() < deadline {
            try? await Task.sleep(for: .seconds(1.5))
            let msgs = try await chat.listMessages(sessionId: sid)
            let reply = msgs.dropFirst(before).last { $0.role == "assistant" && !$0.content.isEmpty }?.content ?? ""
            if !reply.isEmpty, reply == last { stable += 1 } else { stable = 0 }
            last = reply
            if stable >= 2 { return AskResult(botName: name, answer: reply, stillWorking: false, conversation: convo) }
        }
        return AskResult(botName: name, answer: last.isEmpty ? nil : last, stillWorking: true, conversation: convo)
    }

    // MARK: Delegate

    func handOff(text: String, title: String?) async throws -> ProjectInfo {
        let t = title?.trimmingCharacters(in: .whitespacesAndNewlines).nilIfBlank ?? String(text.prefix(60))
        let id = try await bots.createBotProject(title: t)
        try await bots.sendToCoordinator(projectId: id, text: text)
        return ProjectInfo(id: id, title: t)
    }

    func runRoutine(id: String) async throws -> RoutineStartResult {
        let r = try await routines.listRoutines().first { $0.id == id }
        try await routines.runRoutine(id: id)
        let info = RoutineInfo(id: id, name: r?.name ?? "Routine", state: "running", schedule: r?.schedule)
        return RoutineStartResult(routine: info, started: true)
    }

    func send(botId: String, text: String) async throws -> ConversationInfo {
        let convo = try await chatTarget(botId: botId)
        guard let sid = convo.sessionId else { throw IntentFailure.notFound("that chat") }
        try await bots.sendUserMessage(sessionId: sid, text: text)
        return convo
    }

    func reply(conversation: ConversationInfo, text: String) async throws -> ConversationInfo {
        guard let sid = conversation.sessionId else { throw IntentFailure.notFound("that chat") }
        try await bots.sendUserMessage(sessionId: sid, text: text)
        return conversation
    }

    func createRoutine(name: String, instructions: String, schedule: String?) async throws -> RoutineInfo {
        let r = try await routines.createRoutine(
            name: name, steps: [RoutineStep(command: instructions, status: "pending")],
            trigger: schedule == nil ? nil : "schedule", schedule: schedule)
        return RoutineInfo(id: r.id, name: r.name, state: r.state, schedule: r.schedule)
    }

    // MARK: Decide

    private func approvalInfo(_ i: InboxItem) -> ApprovalInfo {
        ApprovalInfo(id: i.id, kind: i.kind, botId: i.botId, botName: i.botName, summary: i.summary,
                     fullText: i.text ?? i.summary, recipient: i.person, contentHash: i.contentHash,
                     sessionId: i.threadId, isPeerHeld: i.isPeerHeld, canApprove: i.canApprove, at: i.at)
    }

    func listApprovals() async throws -> ApprovalList {
        do {
            let items = try await inbox.list() ?? []
            return ApprovalList(items: items.map(approvalInfo), asOf: Date(), fromSnapshot: false)
        } catch {
            // No reachable computer: show what the cloud summary last knew (read-only).
            let snap = (try? await Self.cloudSnapshot()) ?? AppGroupStore.snapshot
            guard let snap, !snap.needsYouRows.isEmpty else { throw error }
            let rows = snap.needsYouRows.map {
                ApprovalInfo(id: $0.id, kind: $0.kind, botId: "", botName: $0.botName, summary: $0.line,
                             fullText: $0.line, canApprove: false)
            }
            return ApprovalList(items: rows, asOf: snap.asOf, fromSnapshot: true)
        }
    }

    private func decide(_ item: ApprovalInfo, _ action: NotificationActionID, kind: DecisionKind,
                        success: DecisionOutcome, until: Date? = nil) async throws -> DecisionResult {
        let r = await NotificationActions.resolve(inboxId: item.id, action: action, contentHash: item.contentHash,
                                                  runtimeId: nil, sessionId: nil, replyText: nil)
        let outcome: DecisionOutcome
        switch r {
        case .done: outcome = success
        case .alreadyHandled: outcome = .alreadyHandled
        case .changed: outcome = .changed
        case .offline: outcome = .offline
        case .signedOut: throw IntentFailure.signedOut
        case .failed(let why): throw IntentFailure.failed(why)
        case .invalid: throw IntentFailure.notFound("that item")
        }
        return DecisionResult(kind: kind, outcome: outcome, itemId: item.id, snoozedUntil: outcome == .snoozed ? until : nil)
    }

    func approve(_ item: ApprovalInfo) async throws -> DecisionResult {
        try await decide(item, .approve, kind: .approve, success: item.isPeerHeld ? .sent : .approved)
    }

    func deny(_ item: ApprovalInfo) async throws -> DecisionResult {
        try await decide(item, .deny, kind: .deny, success: .denied)
    }

    /// The notification path snoozes for 60 minutes; other lengths use the inbox client.
    func snooze(_ item: ApprovalInfo, minutes: Int) async throws -> DecisionResult {
        let until = Date().addingTimeInterval(Double(minutes) * 60)
        if minutes == 60 { return try await decide(item, .snooze, kind: .snooze, success: .snoozed, until: until) }
        do { try await inbox.snooze(item.id, minutes: minutes) } catch { throw classify(error) }
        return DecisionResult(kind: .snooze, outcome: .snoozed, itemId: item.id, snoozedUntil: until)
    }

    // MARK: Status

    private static func cloudSnapshot() async throws -> StatusSnapshot {
        switch await StatusSummaryClient().fetch() {
        case .success(let s): return s
        case .failure(.signedOut): throw IntentFailure.signedOut
        case .failure: throw IntentFailure.offline
        }
    }

    func botStatus(filter: BotStatusFilter) async throws -> BotStatusList {
        let snap: StatusSnapshot
        do { snap = try await Self.cloudSnapshot() }
        catch { guard let cached = AppGroupStore.snapshot else { throw error }; snap = cached }
        func has(_ s: String, _ keys: [String]) -> Bool { keys.contains { s.lowercased().contains($0) } }
        let rows = snap.bots.filter { b in
            switch filter {
            case .all: return true
            case .working: return has(b.status, ["work", "run"])
            case .needsMe: return has(b.status, ["wait", "attention", "need", "block"])
            }
        }
        return BotStatusList(bots: rows.map { BotInfo(id: $0.botId, name: $0.name, status: $0.status, step: $0.step) },
                             asOf: snap.asOf)
    }

    func lastRoutineResult(routineId: String?) async throws -> LastRoutineResult {
        let snap: StatusSnapshot
        do { snap = try await Self.cloudSnapshot() }
        catch { guard let cached = AppGroupStore.snapshot else { throw error }; snap = cached }
        guard let r = snap.lastRoutine, routineId == nil || r.routineId == routineId else {
            throw IntentFailure.notFound("a routine result")
        }
        return LastRoutineResult(routineId: r.routineId, title: r.title, status: r.status,
                                 at: r.at.map { Date(timeIntervalSince1970: $0) })
    }

    /// Newest artifact the bot wrote (version `author_id` = the bot). The list
    /// rows carry no author, so the newest few are opened and matched. Text
    /// bodies only: the v2 API returns the body as a string, and binary
    /// artifacts (PDF, image) are not downloaded here.
    func latestArtifact(botId: String) async throws -> ArtifactInfo {
        let page = try await ArtifactsV2Client.shared.list(ArtifactV2ListQuery(scope: "all", limit: 12))
        var sawBinary = false
        for row in page.items.prefix(12) {
            let artifact = try await ArtifactsV2Client.shared.get(id: row.id)
            guard artifact.version.authorId == botId else { continue }
            let format = artifact.version.bodyFormat.lowercased()
            let ext: String
            if format.contains("html") { ext = "html" }
            else if format.contains("markdown") || format == "md" { ext = "md" }
            else if format.contains("json") { ext = "json" }
            else if format.contains("svg") { ext = "svg" }
            else if format.contains("csv") { ext = "csv" }
            else if format.hasPrefix("text") || format.isEmpty { ext = "txt" }
            else { sawBinary = true; continue }
            let safe = artifact.title.map { $0.isLetter || $0.isNumber ? $0 : "-" }
            let dir = FileManager.default.temporaryDirectory.appendingPathComponent("intent-artifacts", isDirectory: true)
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let url = dir.appendingPathComponent("\(String(safe.prefix(60))).\(ext)")
            try artifact.version.body.write(to: url, atomically: true, encoding: .utf8)
            return ArtifactInfo(id: artifact.id, title: artifact.title, mimeType: format, fileURL: url)
        }
        if sawBinary { throw IntentFailure.notAvailable("Downloading that kind of artifact") }
        throw IntentFailure.notFound("an artifact from that bot")
    }

    func watchBot(botId: String) async throws -> BotInfo {
        throw IntentFailure.notAvailable("Watching a bot")
    }

    // MARK: Control

    private func paused(_ s: BotModeClient.BotPauseState) -> PausedState {
        PausedState(paused: s.paused, since: s.since.map { Date(timeIntervalSince1970: $0) }, by: s.by)
    }

    func pausedState() async throws -> PausedState { paused(try await bots.botPauseState()) }
    func setPaused(_ paused: Bool) async throws -> PausedState {
        self.paused(try await bots.setBotPaused(paused, by: "shortcut"))
    }

    func stopRun(conversation: ConversationInfo) async throws -> StoppedResult {
        guard let sid = conversation.sessionId else { throw IntentFailure.notFound("that chat") }
        try await chat.abort(sessionId: sid)
        return StoppedResult(sessionId: sid)
    }

    /// Cloud route the Settings screen uses (works with no computer): read
    /// the saved value, flip `enabled`, PUT the whole object back.
    func setQuietHours(enabled: Bool) async throws -> QuietHoursState {
        let url = AppConfig.cloudAPIBaseURL.appendingPathComponent("api/v1/notifications/quiet-hours")
        let get = try await APIClient.shared.authorizedRequest(url: url, method: "GET")
        let (data, response) = try await URLSession.shared.data(for: get)
        var value: [String: Any] = [:]
        if (response as? HTTPURLResponse)?.statusCode == 404 { throw IntentFailure.notAvailable("Quiet hours") }
        try APIClient.shared.validate(response, data: data)
        if let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] { value = obj }
        if value["start"] == nil { value["start"] = "22:00" }
        if value["end"] == nil { value["end"] = "07:00" }
        if value["timeZone"] == nil { value["timeZone"] = TimeZone.current.identifier }
        if value["allowCalls"] == nil { value["allowCalls"] = true }
        if value["letThrough"] == nil { value["letThrough"] = "urgent" }
        value["enabled"] = enabled
        var put = try await APIClient.shared.authorizedRequest(url: url, method: "PUT")
        put.httpBody = try JSONSerialization.data(withJSONObject: value)
        let (out, resp) = try await URLSession.shared.data(for: put)
        try APIClient.shared.validate(resp, data: out)
        return QuietHoursState(enabled: enabled)
    }

    func switchComputer(id: String) async throws -> ComputerInfo {
        let info: ComputerInfo? = await MainActor.run { () -> Task<ComputerInfo?, Never> in
            Task { @MainActor in
                let store = RuntimeRelayStore.shared
                guard store.devices.contains(where: { $0.id == id }) else { return nil }
                store.select(id)
                guard let d = store.devices.first(where: { $0.id == id }) else { return nil }
                return ComputerInfo(id: d.id, name: d.displayName, typeLabel: d.typeLabel, isOnline: d.isOnline, isActive: true)
            }
        }.value
        guard let info else { throw IntentFailure.notFound("that computer") }
        return info
    }

    func setBrain(botId: String, modelId: String) async throws -> BrainChangeResult {
        guard let model = try await listModels().first(where: { $0.id == modelId }) else {
            throw IntentFailure.notFound("that model")
        }
        guard let bot = try await botRecords().first(where: { $0.id == botId }) else {
            throw IntentFailure.notFound("that bot")
        }
        try await agents.updateAgent(id: botId, model: model.id, provider: model.provider)
        return BrainChangeResult(bot: BotInfo(id: bot.id, name: bot.name, status: bot.status), model: model)
    }

    // MARK: Memory

    func remember(botId: String, text: String) async throws -> SavedResult {
        let name = try await botName(botId)
        try await bots.addMemory(botId: botId, content: text)
        return SavedResult(botName: name)
    }

    // MARK: - Talk/Delegate/Decide additions

    func openLink(_ url: URL) async -> Bool {
        await MainActor.run { DeepLinkRouter.shared.open(url: url) }
    }

    func startCodeTask(text: String, projectId: String?) async throws -> ConversationInfo {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let name = trimmed.isEmpty ? "Code task" : String(trimmed.prefix(60))
        // The same stamp the Code tab sends with the agent pill off
        // (AppMode.code.originSurface, AgentModeStore.sessionMode(for: .code)).
        let s = try await chat.createSession(name: name, originSurface: "code", sessionMode: "regular",
                                             projectId: projectId)
        try await bots.sendUserMessage(sessionId: s.id, text: text)
        return ConversationInfo(id: s.id, sessionId: s.id, title: s.name ?? name, botId: nil)
    }

    func sendFile(botId: String, data: Data, filename: String, note: String?) async throws -> ConversationInfo {
        try SendFileRules.check(byteCount: data.count)
        let up = try await chat.upload(name: filename, mediaType: SendFileRules.mediaType(for: filename),
                                       dataBase64: data.base64EncodedString())
        return try await send(botId: botId, text: SendFileRules.message(filename: filename, url: up.url, note: note))
    }
}

// MARK: - Status/Control additions

extension IntentServices: FocusFilterServicing {
    /// Focus filter: one cloud call; see `FocusFilterClient`.
    func setFocusFilter(_ request: FocusFilterRequest) async throws {
        var r = request
        r.deviceId = await FocusFilterClient.currentDeviceId()
        try await FocusFilterClient().send(r)
    }
}

private extension String {
    var nilIfBlank: String? { trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : self }
}
