import Foundation

/// Where an incoming link leads inside the app.
///
/// Two sources share this one parser:
/// - **Universal Links** — `https://ai.allternit.com/...` or
///   `https://m.allternit.com/...` (apple-app-site-association in allternit-ai
///   `public/.well-known/`): `/b/<botId>`, `/i/<code>`, `/settings[/<section>]`,
///   and any path with `?session=<id>`.
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
    /// Decisions: questions and approvals waiting on you (approval pushes).
    case decisions

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
            comps.query = URLComponents(url: url, resolvingAgainstBaseURL: false)?.query
            return parse(components: comps, origin: "m.allternit.com")
        }
        return nil
    }

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
            return .computers
        default:
            return nil
        }
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

    func enqueue(_ link: DeepLink) {
        // onOpenURL and onContinueUserActivity can both see one Universal Link.
        if link == lastLink, Date().timeIntervalSince(lastAt) < 1.5 { return }
        lastLink = link
        lastAt = Date()
        pending = link
    }

    /// Takes the pending link (the host calls this once it can act on it).
    func consume() -> DeepLink? {
        defer { pending = nil }
        return pending
    }
}
