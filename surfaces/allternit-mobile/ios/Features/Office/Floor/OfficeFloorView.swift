import SwiftUI
import WebKit

/// Bots mode → Office: the bots' floor from 3dfacility.allternit.com
/// (`?embed=workspace`), fed with the person's real bots — the native
/// counterpart of the web's OfficeFloor (allternit-ai src/views/office).
///
/// The facility only talks to a parent frame, so a tiny host page (served as
/// https://ai.allternit.com, an origin the facility accepts) frames it and
/// relays: Swift → `feed(json)` → the frame; the frame's actions (answer,
/// open) → `office` message handler → the bot's chat.
struct OfficeFloorView: View {
    @ObservedObject private var store = BotModeStore.shared
    @ObservedObject private var hub = AgentHubStore.shared
    @ObservedObject private var colors = BotColorStore.shared
    @EnvironmentObject private var navigator: BotNavigator
    @State private var ready = false
    @State private var failed = false
    @State private var attempt = 0

    var body: some View {
        ZStack {
            if store.bots.isEmpty && !hub.agents.isEmpty {
                ContentUnavailableView("No bots yet", systemImage: "building.2",
                                       description: Text("Make a bot and it gets a desk here."))
            } else if failed {
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
                OfficeFloorWeb(feed: feedJSON, attempt: attempt, onReady: { ready = true }, onAction: handle)
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
        .task(id: attempt) {
            // A cold 3D load on a slow phone can take a while (web: 25 s).
            try? await Task.sleep(for: .seconds(25))
            if !ready { failed = true }
        }
        .task {
            // Statuses and questions come from the threads; keep them fresh while the floor is open.
            while !Task.isCancelled {
                await store.refreshThreads()
                try? await Task.sleep(for: .seconds(15))
            }
        }
    }

    private func handle(action: String, botId: String?) {
        guard let botId, store.bot(botId) != nil, action == "answer" || action == "open" else { return }
        // A waiting bot's question is at the top of its main chat.
        navigator.open(.chat(botId: botId, sessionId: nil))
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
                "task": (working ?? threads.first).map { $0.statusLine ?? $0.title } ?? "",
                "chief": bot.isPrimaryTemplateBot,
            ]
            if let waiting { entry["question"] = waiting.statusLine ?? waiting.title }
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
        let data = (try? JSONSerialization.data(withJSONObject: ["type": "allternit:workspace", "bots": bots])) ?? Data("{}".utf8)
        return String(decoding: data, as: UTF8.self)
    }

    /// The facility loads pictures from allternit.com hosts only.
    private static func allowedAsset(_ url: URL) -> Bool {
        guard url.scheme == "https", let host = url.host?.lowercased() else { return false }
        return host == "allternit.com" || host.hasSuffix(".allternit.com")
    }
}

/// The host page around the facility frame.
private struct OfficeFloorWeb: UIViewRepresentable {
    static let origin = "https://3dfacility.allternit.com"
    let feed: String
    let attempt: Int
    let onReady: () -> Void
    let onAction: (String, String?) -> Void

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.userContentController.add(context.coordinator, name: "office")
        let web = WKWebView(frame: .zero, configuration: config)
        web.isOpaque = false
        web.backgroundColor = .clear
        web.scrollView.isScrollEnabled = false
        context.coordinator.web = web
        load(web, context: context)
        return web
    }

    func updateUIView(_ web: WKWebView, context: Context) {
        let c = context.coordinator
        c.onReady = onReady
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
        c.ready = false
        c.lastSent = nil
        c.pending = feed
        let dark = UITraitCollection.current.userInterfaceStyle == .dark
        let src = "\(Self.origin)/?embed=workspace\(dark ? "&theme=dark" : "")"
        let html = """
        <!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1,maximum-scale=1">
        <style>html,body{margin:0;height:100%;background:transparent}iframe{border:0;width:100%;height:100%;display:block}</style></head>
        <body><iframe id="f" src="\(src)" allow="autoplay"></iframe><script>
        const f = document.getElementById('f'), O = '\(Self.origin)';
        window.feed = (m) => f.contentWindow && f.contentWindow.postMessage(m, O);
        addEventListener('message', (e) => {
          if (e.origin !== O || e.source !== f.contentWindow || !e.data) return;
          const d = e.data;
          if (d.type === 'allternit:workspace:ready') webkit.messageHandlers.office.postMessage({ type: 'ready' });
          else if (d.type === 'allternit:workspace:action') webkit.messageHandlers.office.postMessage({ type: 'action', action: String(d.action || ''), botId: typeof d.botId === 'string' ? d.botId : null });
        });
        </script></body></html>
        """
        web.loadHTMLString(html, baseURL: URL(string: "https://ai.allternit.com/office-host"))
    }

    @MainActor
    final class Coordinator: NSObject, WKScriptMessageHandler {
        weak var web: WKWebView?
        var attempt = -1
        var ready = false
        var pending: String?
        var lastSent: String?
        var onReady: () -> Void = {}
        var onAction: (String, String?) -> Void = { _, _ in }

        func send(_ feed: String) {
            pending = feed
            guard ready, feed != lastSent else { return }
            lastSent = feed
            web?.evaluateJavaScript("window.feed(\(feed))")
        }

        func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
            guard let body = message.body as? [String: Any] else { return }
            switch body["type"] as? String {
            case "ready":
                ready = true
                lastSent = nil
                if let pending { send(pending) }
                onReady()
            case "action":
                if let action = body["action"] as? String { onAction(action, body["botId"] as? String) }
            default:
                break
            }
        }
    }
}
