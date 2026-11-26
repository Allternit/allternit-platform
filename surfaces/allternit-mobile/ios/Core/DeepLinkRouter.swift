import Foundation

/// Where an incoming link leads inside the app.
///
/// Two sources share this one parser:
/// - **Universal Links** — `https://ai.allternit.com/...` or
///   `https://m.allternit.com/...` (apple-app-site-association in allternit-ai
///   `public/.well-known/`): `/b/<botId>`, `/i/<code>`, `/settings[/<section>]`,
///   `/inbox`, and any path with `?session=<id>`.
/// - **`allternit://` links** — the same paths on the app's own scheme
///   (`allternit://inbox`, `allternit://b/<botId>`), registered in
///   Info.plist `CFBundleURLTypes` for widgets, Shortcuts and Spotlight.
/// - **Push taps** — the cloud's notification `url` is a path on the app's
///   own origin, the same one the Web Push service worker opens
///   (`/?allternit_thread=…&bot=…`, `/?allternit_call=…`,
///   `/?allternit_channel=…`, `/settings/billing`; allternit-ai
///   `src/lib/gateway/push-links.ts`).
enum DeepLink: Equatable, Sendable {
    /// A chat session (`?session=<id>`).
    case session(String)
    /// A bot's page (`/b/<botId>`).
    case bot(String)
    /// A bot thread (push `allternit_thread`).
    case thread(botId: String, threadId: String)
    /// A ringing Allternit call (push `allternit_call=<room>&from=&thread=&bot=`):
    /// the incoming-call screen, or the thread when the ring already ended.
    case call(room: String, from: String, threadId: String, botId: String)
    /// A relayed channel message (push `allternit_channel`) — lands on the bots.
    case channel(String)
    /// Settings, optionally a section (`/settings/billing`).
    case settings(String?)
    /// A phone invite page (`/i/<code>`), shown with the web flow in-app.
    case invite(URL)
    /// The Computers view (`/computers`).
    case computers
    /// One computer's console detail (`/computers/<id>`, a `computer_alert` push).
    case computerConsole(String)
    /// An agent browser run on one computer (`/computers/<id>?run=<run_id>`,
    /// the `aci.attention` push): the live run view with its gates.
    case computerRun(computerId: String, runId: String)
    /// Decisions: questions and approvals waiting on you (approval pushes).
    case decisions
    /// The "Needs you" inbox across all bots (`/inbox`, `allternit://inbox`).
    case inbox
    /// The inbox with one item in view (`/inbox/<id>`, a push's `data.inboxId`).
    case inboxItem(String)
    /// A bot's chat with `prompt` in the composer, never sent
    /// (`/bot/<id>/chat?prompt=`).
    case botChat(botId: String, prompt: String?)
    /// The main bot (`/call/main`): its chat, where the call button is.
    case callMain
    /// A routine's page (`/routine/<id>`).
    case routine(String)
    /// The team hand-off composer on the bots launchpad (`allternit://handoff`,
    /// `/handoff`, or the web shortcut's `?allternit_open=handoff`).
    case handoff
    /// Start a phone task on the linked phone (`allternit://phone-task?goal=&source=`),
    /// sent by the iMessage app, which has no session of its own.
    case phoneTask(goal: String, source: String)
    /// One phone or computer run: its status, steps and Review in a sheet
    /// (`allternit://phone-run/<runId>`, the iMessage card's
    /// `https://ai.allternit.com/?allternit_phone_run=<runId>`).
    case phoneRun(String)
    /// Meetings (`/meetings`), or one meeting's live page / recap
    /// (`/meetings/<id>`: the MCP `page_url`, notification taps, the Live Activity).
    case meetings(String?)
    /// The in-app bar with an app, a tool and values filled in
    /// (`allternit://bar?app=<kind>:<ref>&tool=<name>&bot=<id>&v.<key>=<value>`), sent
    /// by "Do with Allternit" when Siri can't ask for every value.
    case bar(BarLinkRequest)

    static let webHosts: Set<String> = ["ai.allternit.com", "m.allternit.com"]

    /// Parses a full URL. Returns nil for anything that isn't an Allternit
    /// web link (file URLs keep going to the document viewer).
    static func parse(url: URL) -> DeepLink? {
        guard let scheme = url.scheme?.lowercased() else { return nil }
        if scheme == "https" || scheme == "http" {
            guard let host = url.host?.lowercased(), webHosts.contains(host) else { return nil }
            return parse(components: URLComponents(url: url, resolvingAgainstBaseURL: false), origin: host)
        }
        if scheme == "allternit" {
            // allternit://b/<id> → host is the first segment.
            var path = "/" + (url.host ?? "")
            if !url.path.isEmpty { path += url.path }
            var comps = URLComponents()
            comps.path = path
            // The encoded query, so an encoded `&` or `=` inside a value stays in it.
            comps.percentEncodedQuery = URLComponents(url: url, resolvingAgainstBaseURL: false)?.percentEncodedQuery
            return parse(components: comps, origin: "m.allternit.com")
        }
        return nil
    }

    /// Where a tapped push leads, from its `url` and its `data`.
    ///
    /// The `url` wins when it names a place (a bot thread, a call, …). An
    /// agent's message to its owner (`agent.message.sent`, send_user_message)
    /// or a needs-you nudge (`thread.needs_user`) from a session with no bot
    /// carries no thread link (`url` is "/"), so it opens the chat session
    /// by `data.sessionId`.
    static func forPush(urlPath: String?, event: String?, sessionId: String?) -> DeepLink? {
        if let urlPath, !urlPath.isEmpty, let link = parse(path: urlPath) { return link }
        guard let event, sessionRoutedEvents.contains(event),
              let sessionId = sessionId?.trimmingCharacters(in: .whitespacesAndNewlines),
              !sessionId.isEmpty else { return nil }
        return .session(sessionId)
    }

    /// Push events that fall back to their session when no link is given.
    static let sessionRoutedEvents: Set<String> = ["agent.message.sent", "thread.needs_user"]

    /// Parses an app path such as a push notification's `url`.
    static func parse(path: String) -> DeepLink? {
        guard let comps = URLComponents(string: path) else { return nil }
        return parse(components: comps, origin: "m.allternit.com")
    }

    private static func parse(components: URLComponents?, origin: String) -> DeepLink? {
        guard let components else { return nil }
        let query = Dictionary(
            (components.queryItems ?? []).compactMap { item -> (String, String)? in
                guard let value = item.value, !value.isEmpty else { return nil }
                return (item.name, value)
            },
            uniquingKeysWith: { first, _ in first }
        )
        if let thread = query["allternit_thread"], let bot = query["bot"] {
            return .thread(botId: bot, threadId: thread)
        }
        if let room = query["allternit_call"] {
            return .call(room: room, from: query["from"] ?? "", threadId: query["thread"] ?? "", botId: query["bot"] ?? "")
        }
        if let provider = query["allternit_channel"] {
            return .channel(provider)
        }
        if let run = query["allternit_phone_run"] {
            return .phoneRun(run)
        }
        if query["allternit_open"] == "handoff" {
            return .handoff
        }
        if let session = query["session"] {
            return .session(session)
        }

        let segments = components.path.split(separator: "/").map(String.init)
        guard let first = segments.first else { return nil }
        switch first {
        case "b":
            guard segments.count >= 2 else { return nil }
            return .bot(segments[1])
        case "i":
            guard segments.count >= 2 else { return nil }
            var web = URLComponents()
            web.scheme = "https"
            web.host = origin
            web.path = components.path
            web.query = components.query
            return web.url.map(DeepLink.invite)
        case "settings":
            return .settings(segments.count >= 2 ? segments[1] : nil)
        case "computers":
            if segments.count >= 2, let id = segments[1].removingPercentEncoding, !id.isEmpty, id != "console" {
                if let run = query["run"] { return .computerRun(computerId: id, runId: run) }
                return .computerConsole(id)
            }
            return .computers
        case "phone-task":
            guard let goal = query["goal"], !goal.isEmpty else { return nil }
            return .phoneTask(goal: goal, source: query["source"] ?? "imessage_ext")
        case "phone-run":
            guard segments.count >= 2, let id = segments[1].removingPercentEncoding, !id.isEmpty else { return nil }
            return .phoneRun(id)
        case "inbox":
            if segments.count >= 2, let id = segments[1].removingPercentEncoding, !id.isEmpty {
                return .inboxItem(id)
            }
            return .inbox
        case "bot":
            guard segments.count >= 2 else { return nil }
            if segments.count >= 3, segments[2] == "chat" {
                return .botChat(botId: segments[1], prompt: query["prompt"])
            }
            return .bot(segments[1])
        case "call":
            guard segments.count >= 2, segments[1] == "main" else { return nil }
            return .callMain
        case "routine":
            guard segments.count >= 2 else { return nil }
            return .routine(segments[1])
        case "handoff":
            return .handoff
        case "bar":
            return .bar(BarLinkRequest(query: query))
        case "meetings":
            let id = segments.count >= 2 ? segments[1].removingPercentEncoding : nil
            // `allternit://meetings/calendar-connected` is the calendar consent return.
            return .meetings(id == "calendar-connected" || id == "zoom-connected" ? nil : id)
        default:
            return nil
        }
    }
}

/// What `allternit://bar` asks the in-app bar to show. Every field may be
/// empty: a bare `allternit://bar` just opens the bar.
struct BarLinkRequest: Equatable, Sendable {
    /// `<kind>:<ref>` (as `BarAppInfo.id`), nil for no app.
    var app: String?
    var tool: String?
    /// The bot that does it; nil = the person themself.
    var bot: String?
    /// Blank key -> value as text, from `v.<key>` query items.
    var values: [String: String]

    init(app: String?, tool: String?, bot: String? = nil, values: [String: String]) {
        self.app = app
        self.tool = tool
        self.bot = bot
        self.values = values
    }

    init(query: [String: String]) {
        app = query["app"]
        tool = query["tool"]
        bot = query["bot"]
        var values: [String: String] = [:]
        for (name, value) in query where name.hasPrefix("v.") && name.count > 2 {
            values[String(name.dropFirst(2))] = value
        }
        self.values = values
    }
}

/// Holds the link the workspace should open next. Links can arrive before
/// the workspace exists (a cold launch from a notification tap or a
/// Universal Link, or while signed out / in onboarding), so the link waits
/// in `pending` until `DeepLinkHost` (on `MainWorkspaceView`) takes it.
@MainActor
final class DeepLinkRouter: ObservableObject {
    static let shared = DeepLinkRouter()

    @Published private(set) var pending: DeepLink?

    private var lastLink: DeepLink?
    private var lastAt: Date = .distantPast

    private init() {}

    /// Returns true when `url` is an Allternit link (handled here).
    @discardableResult
    func open(url: URL) -> Bool {
        guard let link = DeepLink.parse(url: url) else { return false }
        enqueue(link)
        return true
    }

    /// A push notification's `url` path.
    func open(path: String) {
        guard let link = DeepLink.parse(path: path) else { return }
        enqueue(link)
    }

    /// A tapped push: its `url`, else its session (see `DeepLink.forPush`).
    func openPush(urlPath: String?, event: String?, sessionId: String?) {
        guard let link = DeepLink.forPush(urlPath: urlPath, event: event, sessionId: sessionId) else { return }
        enqueue(link)
    }

    func enqueue(_ link: DeepLink) {
        // onOpenURL and onContinueUserActivity can both see one Universal Link.
        if link == lastLink, Date().timeIntervalSince(lastAt) < 1.5 { return }
        lastLink = link
        lastAt = Date()
        pending = link
    }

    /// Text a `botChat` link puts in the next bot composer that appears.
    /// Never sent by itself.
    private(set) var composerPrefill: String?

    func setComposerPrefill(_ text: String?) { composerPrefill = text }

    /// Takes the prefill (the composer calls this when it appears).
    func takeComposerPrefill() -> String? {
        defer { composerPrefill = nil }
        return composerPrefill
    }

    /// What the last `allternit://bar` link asked for. The in-app bar
    /// (Features/IslandBar) watches this, opens and takes it.
    @Published private(set) var barRequest: BarLinkRequest?

    func setBarRequest(_ request: BarLinkRequest) { barRequest = request }

    /// Takes the bar request (the bar calls this when it opens with it).
    func takeBarRequest() -> BarLinkRequest? {
        defer { barRequest = nil }
        return barRequest
    }

    /// Takes the pending link (the host calls this once it can act on it).
    func consume() -> DeepLink? {
        defer { pending = nil }
        return pending
    }
}
