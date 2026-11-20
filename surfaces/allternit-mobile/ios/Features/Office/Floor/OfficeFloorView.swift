import Combine
import SwiftUI
import WebKit

/// Bots mode → Office: the Workspace floor from 3dfacility.allternit.com
/// (`?embed=workspace&chrome=none`), with "Open the full facility" switching to
/// `?embed=facility&actions=chat` and back. Fed with the person's real bots — the native
/// counterpart of the web's OfficeFloor (allternit-ai src/views/office).
///
/// The facility only talks to a parent frame, so a tiny host page (served as
/// https://ai.allternit.com, an origin the facility accepts) frames it and
/// relays: Swift → `feed(json)` → the frame; the frame's actions → `office`
/// message handler. Picking a bot shows its main chat in the panel
/// (`allternit:workspace:thread`); a message written there is sent in the
/// background and its reply streams into the panel; answer/open push the chat.
struct OfficeFloorView: View {
    @ObservedObject private var auth = AuthManager.shared
    @ObservedObject private var store = BotModeStore.shared
    @ObservedObject private var hub = AgentHubStore.shared
    @ObservedObject private var colors = BotColorStore.shared
    @ObservedObject private var projectStore = ProjectStore.shared
    @EnvironmentObject private var navigator: BotNavigator
    @State private var ready = false
    @State private var failed = false
    @State private var attempt = 0
    @State private var thread = OfficePanelThread()
    @State private var computerList: [ComputerRecord] = []
    /// Workspace floor (default) or the full facility; changing it reloads the frame.
    @State private var mode: OfficeFloorMode = .workspace
    /// The `v` the Workspace frame announced in its ready message (1 when it omits it).
    @State private var frameVersion = 1
    @State private var floor: FloorStateV2?
    @ObservedObject private var status = BotStatusStore.shared
    @State private var bridge = OfficeFloorBridge()

    var body: some View {
        ZStack {
            if failed {
                VStack(spacing: 12) {
                    ContentUnavailableView("The office didn't load", systemImage: "wifi.exclamationmark",
                                           description: Text("Check your connection and try again."))
                    Button("Try again") {
                        failed = false
                        ready = false
                        attempt += 1
                    }
                    .buttonStyle(.borderedProminent)
                }
            } else {
                OfficeFloorWeb(feed: feedJSON, thread: thread, bridge: bridge, mode: mode, attempt: attempt, visitorKey: "allternit.facility.visitor.\(auth.currentUserId ?? "local")", onReady: { version in frameVersion = version; ready = true }, onAction: handle)
                    .id(auth.currentUserId ?? "local")
                    .ignoresSafeArea(edges: .bottom)
                if !ready {
                    ProgressView("Opening the office…")
                        .padding(16)
                        .background(RoundedRectangle(cornerRadius: 12).fill(Color("BgPanel")))
                }
            }
        }
        .background(Color("BgPanel"))
        .navigationTitle("Office")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(mode == .workspace ? "Open the full facility" : "Back to Workspace") {
                    // A new frame loads; its ready message sets the version again.
                    ready = false
                    failed = false
                    frameVersion = 1
                    mode = mode == .workspace ? .facility : .workspace
                    attempt += 1
                }
                .font(.footnote)
            }
        }
        .task(id: attempt) {
            // A cold 3D load on a slow phone can take a while (web: 25 s).
            try? await Task.sleep(for: .seconds(25))
            if !ready { failed = true }
        }
        .task {
            // Names only for the facility's Commons studios and Compute Row; a failed load just omits the field.
            projectStore.fetchProjectsIfNeeded()
            if let list = try? await BotModeClient.shared.listComputers() { computerList = list }
        }
        .task(id: frameVersion) {
            // Feed v2 only when the frame said v:2; a failed load keeps the last good state.
            guard mode == .workspace, frameVersion >= 2 else { return }
            while !Task.isCancelled {
                if let latest = try? await BotModeClient.shared.floorState() { floor = latest }
                try? await Task.sleep(for: .seconds(15))
            }
        }
        .task {
            // Statuses and questions come from the threads; keep them fresh while the floor is open.
            while !Task.isCancelled {
                await store.refreshThreads()
                try? await Task.sleep(for: .seconds(15))
            }
        }
    }

    private func handle(action: String, botId: String?, text: String?, podId: String?) {
        if action == "pod" {
            if let podId { bridge.focus(podId: podId) }
            return
        }
        guard let botId, store.bot(botId) != nil else { return }
        switch action {
        case "select":
            // The full facility opens real native bot work, then returns to the same world.
            navigator.open(.chat(botId: botId, sessionId: nil))
        case "message":
            // Written in the panel: sent into the bot's main chat here, the reply streams back into the panel.
            guard let text = text?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty else { return }
            Task { await thread.send(text, to: botId) }
        case "answer", "open":
            // A waiting bot's question is at the top of its main chat.
            navigator.open(.chat(botId: botId, sessionId: nil))
        default:
            break
        }
    }

    /// The live roster in the facility's `allternit:workspace` shape (workspace-feed.ts parseBots).
    private var feedJSON: String {
        let bots: [[String: Any]] = store.bots.map { bot in
            let threads = store.threads(for: bot.id).filter { $0.archived != true }
            let waiting = threads.first { $0.group == "waiting" }
            let working = threads.first { $0.group == "working" }
            var entry: [String: Any] = [
                "id": bot.id,
                "name": bot.botDisplayName,
                "role": bot.botTagline,
                "color": colors.hex(for: bot) ?? "#9CA3AF",
                "status": waiting != nil ? "waiting" : working != nil ? "working" : "idle",
                "task": (working ?? threads.first).map { $0.statusLine ?? $0.displayTitle } ?? "",
                "chief": bot.isPrimaryTemplateBot,
            ]
            if let waiting { entry["question"] = waiting.statusLine ?? waiting.displayTitle }
            // A:// is the Allternit orb (BotAvatarView): the facility sees this picture and draws the living orb.
            if bot.isPrimaryTemplateBot {
                entry["avatar"] = ["sheet": Self.orbSheet, "layout": "still"]
                return entry
            }
            switch BotAvatarSource.resolve(bot) {
            case .sprite(let url) where Self.allowedAsset(url):
                entry["avatar"] = ["sheet": url.absoluteString]
            case .image(let url) where Self.allowedAsset(url):
                entry["avatar"] = ["sheet": url.absoluteString, "layout": "still"]
            default:
                break
            }
            return entry
        }
        var message: [String: Any] = ["type": "allternit:workspace", "bots": bots]
        if mode == .workspace, frameVersion >= 2 {
            var waiting: [String: Int] = [:]
            var approvals: [String: Int] = [:]
            for bot in store.bots {
                waiting[bot.id] = store.threads(for: bot.id).filter { $0.archived != true && $0.group == "waiting" }.count
                approvals[bot.id] = status.entries[bot.id]?.state.pendingApprovalsCount ?? 0
            }
            OfficeFloorFeedV2.decorate(message: &message, bots: bots, floor: floor, approvals: approvals, waiting: waiting)
        }
        if auth.currentUserId != nil {
            let projects = Self.workspaceProjects(projectStore.projects)
            if !projects.isEmpty { message["projects"] = projects }
            let computers = Self.workspaceComputers(computerList)
            if !computers.isEmpty { message["computers"] = computers }
        }
        let data = (try? JSONSerialization.data(withJSONObject: message)) ?? Data("{}".utf8)
        return String(decoding: data, as: UTF8.self)
    }

    /// Projects for the facility (web toWorkspaceProjects): names only, newest first, max 24.
    static func workspaceProjects(_ list: [CoworkProject]) -> [[String: Any]] {
        list.filter { !$0.id.isEmpty && !$0.title.isEmpty }
            .sorted { $0.updatedAt > $1.updatedAt }
            .prefix(24)
            .map { ["id": $0.id, "name": $0.title, "updatedAt": $0.updatedAt] }
    }

    /// Computers for the facility (web toWorkspaceComputers): name and state only, running first, max 12.
    static func workspaceComputers(_ list: [ComputerRecord]) -> [[String: Any]] {
        func state(_ s: String) -> String? {
            switch s {
            case "running", "stopped", "error": return s
            case "creating": return "starting"
            default: return nil
            }
        }
        // The server list is already ordered; a stable sort keeps that order inside each group.
        let kept = list.enumerated().filter { !$0.element.id.isEmpty && !$0.element.name.isEmpty && state($0.element.status) != nil }
        let sorted = kept.sorted { a, b in
            let ar = a.element.isRunning, br = b.element.isRunning
            return ar != br ? ar : a.offset < b.offset
        }
        return sorted.prefix(12).map { ["id": $0.element.id, "name": $0.element.name, "status": state($0.element.status)!] }
    }

    /// The A-block orb mark the web app hosts; the facility draws A:// as the orb when its avatar is this.
    private static let orbSheet = "https://ai.allternit.com/brand/allternit-orb.svg"

    /// The facility loads pictures from allternit.com hosts only.
    private static func allowedAsset(_ url: URL) -> Bool {
        guard url.scheme == "https", let host = url.host?.lowercased() else { return false }
        return host == "allternit.com" || host.hasSuffix(".allternit.com")
    }
}

/// Which 3dfacility embed the Office tab shows.
enum OfficeFloorMode { case workspace, facility }

/// The host page around the facility frame.
private struct OfficeFloorWeb: UIViewRepresentable {
    static let origin = "https://3dfacility.allternit.com"
    let feed: String
    let thread: OfficePanelThread
    let bridge: OfficeFloorBridge
    let mode: OfficeFloorMode
    let attempt: Int
    let visitorKey: String
    let onReady: (Int) -> Void
    let onAction: (String, String?, String?, String?) -> Void

    // One WKWebView is not kept alive after leaving Office: the view is torn down with its
    // script handler and visitor/identity plumbing, and a stale WebGL frame held in memory
    // for 60 s costs more than the cold load saves; the feed replays on every ready anyway.
    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.userContentController.add(context.coordinator, name: "office")
        let web = WKWebView(frame: .zero, configuration: config)
        web.isOpaque = false
        web.backgroundColor = .clear
        web.scrollView.isScrollEnabled = false
        context.coordinator.web = web
        thread.post = { [weak coordinator = context.coordinator] json in coordinator?.sendThread(json) }
        bridge.post = { [weak coordinator = context.coordinator] json in coordinator?.sendRaw(json) }
        load(web, context: context)
        return web
    }

    func updateUIView(_ web: WKWebView, context: Context) {
        let c = context.coordinator
        c.onReady = onReady
        c.bridge = bridge
        c.onAction = onAction
        if c.attempt != attempt { load(web, context: context) }
        c.send(feed)
    }

    static func dismantleUIView(_ web: WKWebView, coordinator: Coordinator) {
        web.configuration.userContentController.removeScriptMessageHandler(forName: "office")
    }

    private func load(_ web: WKWebView, context: Context) {
        let c = context.coordinator
        c.attempt = attempt
        c.mode = mode
        c.visitorKey = visitorKey
        c.ready = false
        c.lastSent = nil
        c.pending = feed
        c.pendingThread = nil
        let dark = UITraitCollection.current.userInterfaceStyle == .dark
        // The complete facility: character creation, roaming, and the existing live bot room.
        // This app handles chat only (open, answer, message), so the page leaves hire and share off.
        let src = mode == .workspace
            ? "\(Self.origin)/?embed=workspace&chrome=none\(dark ? "&theme=dark" : "")"
            : "\(Self.origin)/?embed=facility&actions=chat\(dark ? "&theme=dark" : "")"
        let html = """
        <!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1,maximum-scale=1">
        <style>html,body{margin:0;height:100%;background:transparent}iframe{border:0;width:100%;height:100%;display:block}</style></head>
        <body><iframe id="f" src="\(src)" allow="autoplay"></iframe><script>
        const f = document.getElementById('f'), O = '\(Self.origin)';
        window.feed = (m) => f.contentWindow && f.contentWindow.postMessage(m, O);
        let connected = false;
        addEventListener('message', (e) => {
          if (e.origin !== O || e.source !== f.contentWindow || !e.data) return;
          const d = e.data;
          if (d.type === 'allternit:facility:ready' && d.version === 1 && !connected) { connected = true; webkit.messageHandlers.office.postMessage({ type: 'ready' }); }
          else if (d.type === 'allternit:workspace:ready' && !connected) { connected = true; webkit.messageHandlers.office.postMessage({ type: 'ready', v: Number(d.v) >= 2 ? Number(d.v) : 1 }); }
          else if (d.type === 'allternit:workspace:anchors' && Array.isArray(d.items)) webkit.messageHandlers.office.postMessage({ type: 'anchors', items: d.items.slice(0, 100).map((i) => ({ botId: String(i.botId || ''), x: +i.x, y: +i.y, w: +i.w, h: +i.h, visible: i.visible !== false })) });
          else if (d.type === 'allternit:facility:visitor' && d.version === 1) webkit.messageHandlers.office.postMessage({ type: 'visitor', visitor: d.visitor });
          else if (d.type === 'allternit:workspace:action') webkit.messageHandlers.office.postMessage({ type: 'action', action: String(d.action || ''), botId: typeof d.botId === 'string' ? d.botId : null, text: typeof d.text === 'string' ? d.text.slice(0, 4000) : null, podId: typeof d.podId === 'string' ? d.podId : (d.pod && typeof d.pod.podId === 'string' ? d.pod.podId : null) });
        });
        </script></body></html>
        """
        web.loadHTMLString(html, baseURL: URL(string: "https://ai.allternit.com/office-host"))
    }

    @MainActor
    final class Coordinator: NSObject, WKScriptMessageHandler {
        weak var web: WKWebView?
        var attempt = -1
        var mode: OfficeFloorMode = .workspace
        var visitorKey = ""
        var ready = false
        var pending: String?
        /// Dedupe for the roster feed only; every thread post goes out.
        var lastSent: String?
        /// The latest thread post, replayed when the frame (re)loads.
        var pendingThread: String?
        var onReady: (Int) -> Void = { _ in }
        var onAction: (String, String?, String?, String?) -> Void = { _, _, _, _ in }
        /// Anchors are stored on the bridge; set by the owner of the web view.
        weak var bridge: OfficeFloorBridge?

        func send(_ feed: String) {
            pending = feed
            guard ready, feed != lastSent else { return }
            lastSent = feed
            web?.evaluateJavaScript("window.feed(\(feed))")
        }

        /// A ready-made message (focus) for the frame; dropped until the frame is ready.
        func sendRaw(_ json: String) {
            guard ready else { return }
            web?.evaluateJavaScript("window.feed(\(json))")
        }

        func sendThread(_ json: String) {
            pendingThread = json
            guard ready else { return }
            web?.evaluateJavaScript("window.feed(\(json))")
        }

        func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
            // Only the validated top-level relay may reach native actions or persistence.
            guard message.frameInfo.isMainFrame, let body = message.body as? [String: Any] else { return }
            switch body["type"] as? String {
            case "visitor":
                guard ready, let visitor = body["visitor"] as? [String: Any],
                      let config = visitor["config"] as? [String: Any], config["name"] is String,
                      let data = try? JSONSerialization.data(withJSONObject: visitor), data.count < 3_200_000 else { return }
                UserDefaults.standard.set(data, forKey: visitorKey)
            case "ready":
                // Identity state is owned by the app, never the frame's partitioned storage.
                // The Workspace frame has no visitor to restore, so `allternit:facility:init` is facility-only.
                if mode == .facility {
                    let stored = UserDefaults.standard.data(forKey: visitorKey)
                    let visitor = stored.flatMap { String(data: $0, encoding: .utf8) } ?? "null"
                    web?.evaluateJavaScript("window.feed({type:'allternit:facility:init',version:1,visitor:\(visitor)})")
                }
                ready = true
                lastSent = nil
                // Replay the feed (and thread) on every ready, in either mode.
                if let pending { send(pending) }
                if let pendingThread { sendThread(pendingThread) }
                onReady(body["v"] as? Int ?? 1)
            case "anchors":
                if let items = body["items"] as? [[String: Any]] { bridge?.setAnchors(items) }
            case "action":
                if let action = body["action"] as? String { onAction(action, body["botId"] as? String, body["text"] as? String, body["podId"] as? String) }
            default:
                break
            }
        }
    }
}

/// The Office panel's thread: the picked bot's main chat, sent and streamed
/// through the shared chat stack (`ChatViewModel`, the one BotChatSessionView
/// uses), posted to the facility as `allternit:workspace:thread`. One view
/// model per bot, so picking another bot never stops a reply in progress.
@MainActor
final class OfficePanelThread {
    /// Hands a thread post (JSON) to the host page.
    var post: (String) -> Void = { _ in }
    private let store = BotModeStore.shared
    private var chats: [String: ChatViewModel] = [:]
    private var botId: String?
    private var sendError: String?
    private var watch: AnyCancellable?
    private var throttle: Task<Void, Never>?
    private var lastPostAt = Date.distantPast

    /// Shows a bot's main chat in the panel. False when it can't be opened.
    @discardableResult
    func select(_ botId: String) async -> Bool {
        guard let bot = store.bot(botId) else { return false }
        if self.botId != botId {
            self.botId = botId
            sendError = nil
        }
        let chat = chats[botId] ?? ChatViewModel()
        chats[botId] = chat
        watch = chat.objectWillChange.sink { [weak self] _ in
            MainActor.assumeIsolated { self?.schedule() }
        }
        do {
            let sessionId = try await store.mainChatSessionId(for: bot)
            // Fresh history each pick, unless a reply is still coming in.
            if !chat.isStreaming && !chat.isCreatingSession { chat.loadSession(sessionId) }
            schedule()
            return true
        } catch is CancellationError {
            return false
        } catch {
            if self.botId == botId { fail(FriendlyErrorMessage.from(error.localizedDescription)) }
            return false
        }
    }

    /// Sends a message from the panel into the bot's main chat, no navigation.
    func send(_ text: String, to botId: String) async {
        // Usually already picked (the panel posts select first); otherwise open it now.
        var ready = self.botId == botId && chats[botId]?.currentSessionId != nil
        if !ready { ready = await select(botId) }
        guard ready, let chat = chats[botId] else {
            if self.botId == botId, sendError == nil { fail(nil) }
            return
        }
        // Let the history land first so it can't replace the new message.
        for _ in 0..<50 where chat.messages.isEmpty && chat.transientError == nil {
            try? await Task.sleep(for: .milliseconds(100))
        }
        guard !chat.isStreaming, !chat.isCreatingSession, chat.currentSessionId != nil else {
            if self.botId == botId { fail(nil) }
            return
        }
        sendError = nil
        let models = ModelStore.shared
        chat.sendMessage(text, runtimeModelId: models.selectedModelId, effort: models.effortForSend)
        schedule()
    }

    private func fail(_ message: String?) {
        sendError = message.map { "Couldn't send. \($0)" } ?? "Couldn't send. Try again."
        schedule()
    }

    /// At most about 4 posts a second; the trailing post carries the latest state.
    private func schedule() {
        guard throttle == nil else { return }
        let wait = max(0, 0.25 - Date().timeIntervalSince(lastPostAt))
        throttle = Task { [weak self] in
            try? await Task.sleep(for: .seconds(wait))
            guard let self else { return }
            self.throttle = nil
            self.lastPostAt = Date()
            self.postNow()
        }
    }

    private func postNow() {
        guard let botId else { return }
        let chat = chats[botId]
        var lines: [[String: Any]] = (chat?.messages ?? []).compactMap { message in
            guard message.role == "user" || message.role == "assistant" else { return nil }
            var text = message.content.trimmingCharacters(in: .whitespacesAndNewlines)
            if text.isEmpty, message.error != nil, !message.isStreaming { text = "Couldn't send. Try again." }
            guard !text.isEmpty else { return nil }
            return [
                "id": message.id,
                "from": message.role == "user" ? "you" : "bot",
                "text": String(text.prefix(2000)),
                "at": Int(message.createdAt.timeIntervalSince1970 * 1000),
            ]
        }
        lines = Array(lines.suffix(20))
        if let error = sendError ?? chat?.transientError {
            lines.append(["id": "office-error", "from": "bot", "text": error,
                          "at": Int(Date().timeIntervalSince1970 * 1000)])
        }
        let payload: [String: Any] = [
            "type": "allternit:workspace:thread",
            "botId": botId,
            "lines": lines,
            "working": chat?.isStreaming ?? false,
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: payload) else { return }
        post(String(decoding: data, as: UTF8.self))
    }
}
