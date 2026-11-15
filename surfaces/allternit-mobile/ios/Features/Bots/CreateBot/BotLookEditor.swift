import SwiftUI
import WebKit

// -----------------------------------------------------------------------------
// Create bot / Edit bot → Look. The look editor is the web's own AvatarEditor
// (Library, Describe, Your photo, Initials) at /editor/bot-look, allternit-ai
// src/pages/AppBotLookPage.tsx. Drawn characters render in its 3D scene, so one
// editor serves Desktop, the web phone layout and this app. It is signed in the
// same way as the artifact editors (ArtifactLiveWebView): window.allternit.auth
// plus AuthenticatedWebBridge. The form around it stays native.
// -----------------------------------------------------------------------------

@MainActor
final class BotLookModel: ObservableObject {
    enum Phase: Equatable { case loading, ready, failed }

    @Published private(set) var phase: Phase = .loading
    @Published private(set) var height: CGFloat = 420
    @Published private(set) var avatar: JSONValue?
    /// The person changed the look (Edit keeps the saved avatar otherwise).
    @Published private(set) var dirty = false
    /// A photo or character is being made.
    @Published private(set) var busy = false
    /// Your photo / a drawn character is picked but not applied yet (the wizard's gate).
    @Published private(set) var blocked = false
    @Published var error: String?
    @Published private(set) var reloadToken = 0

    fileprivate weak var webView: WKWebView?
    private var initial: [String: JSONValue] = [:]
    private var inited = false
    private var watchdog: Task<Void, Never>?

    /// What the page opens on. Set before the sheet shows, and again for Retry.
    func configure(avatar: JSONValue?, accent: String, displayName: String, vendorId: String?) {
        var payload: [String: JSONValue] = ["accent": .string(accent), "displayName": .string(displayName)]
        if let avatar { payload["avatar"] = avatar }
        if let vendorId { payload["vendorId"] = .string(vendorId) }
        initial = payload
        if phase == .ready, !inited { sendInit() }
    }

    func push(accent: String? = nil, displayName: String? = nil) {
        guard inited else {
            if let accent { initial["accent"] = .string(accent) }
            if let displayName { initial["displayName"] = .string(displayName) }
            return
        }
        var patch: [String: JSONValue] = [:]
        if let accent { patch["accent"] = .string(accent) }
        if let displayName { patch["displayName"] = .string(displayName) }
        call("update", .object(patch))
    }

    func retry() {
        phase = .loading
        inited = false
        error = nil
        reloadToken += 1
    }

    /// useCreateBotSubmit syncAvatarAsset: best effort, and never holds up the new bot.
    func syncAsset(botId: String) async {
        guard inited, let webView else { return }
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            let once = ResumeOnce(continuation)
            webView.callAsyncJavaScript("return await window.allternitBotLook.syncAsset(id)",
                                        arguments: ["id": botId], in: nil, in: .page) { _ in once.resume() }
            DispatchQueue.main.asyncAfter(deadline: .now() + 3) { once.resume() }
        }
    }

    @MainActor
    private final class ResumeOnce {
        private var continuation: CheckedContinuation<Void, Never>?
        init(_ continuation: CheckedContinuation<Void, Never>) { self.continuation = continuation }
        func resume() {
            continuation?.resume()
            continuation = nil
        }
    }

    fileprivate func startWatchdog() {
        watchdog?.cancel()
        watchdog = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 30_000_000_000)
            guard !Task.isCancelled, let self, !self.inited else { return }
            self.phase = .failed
        }
    }

    fileprivate func failed() {
        watchdog?.cancel()
        phase = .failed
    }

    fileprivate func handle(_ body: [String: Any]) {
        switch body["type"] as? String {
        case "ready":
            watchdog?.cancel()
            phase = .ready
            sendInit()
        case "change":
            if let text = body["avatarJson"] as? String,
               let value = try? JSONDecoder().decode(JSONValue.self, from: Data(text.utf8)) {
                avatar = value
            }
            dirty = body["dirty"] as? Bool ?? false
            busy = body["busy"] as? Bool ?? false
            blocked = body["blocked"] as? Bool ?? false
        case "height":
            if let value = body["height"] as? Double, value > 0 { height = CGFloat(value) }
        case "error":
            let message = body["message"] as? String ?? ""
            error = message.isEmpty ? nil : message
        default:
            break
        }
    }

    private func sendInit() {
        inited = true
        call("init", .object(initial))
    }

    private func call(_ method: String, _ argument: JSONValue) {
        guard let data = try? JSONEncoder().encode(argument) else { return }
        webView?.evaluateJavaScript("window.allternitBotLook.\(method)(\(String(decoding: data, as: UTF8.self)))", completionHandler: nil)
    }
}

/// The Look editor at its full height, with its real loading and failed states.
struct BotLookEditor: View {
    @ObservedObject var model: BotLookModel
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        ZStack {
            BotLookWebView(model: model, dark: colorScheme == .dark)
                .frame(height: model.phase == .failed ? 1 : model.height)
                .opacity(model.phase == .ready ? 1 : 0)
            switch model.phase {
            case .loading:
                VStack(spacing: 8) {
                    ProgressView()
                    Text("Loading looks…").font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                }
            case .failed:
                VStack(spacing: 8) {
                    Text("Couldn’t load the looks. The bot keeps the default Gizzi look until they load.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .multilineTextAlignment(.center)
                    Button("Try again") { model.retry() }
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(minHeight: 44)
                }
                .padding(.vertical, 16)
                .frame(maxWidth: .infinity)
            case .ready:
                EmptyView()
            }
        }
        .frame(minHeight: model.phase == .ready ? 0 : 160)
    }
}

private struct BotLookWebView: UIViewRepresentable {
    @ObservedObject var model: BotLookModel
    let dark: Bool

    private static let lookHandler = "allternitBotLook"
    private static let clerkHandler = "allternitClerk"

    private var url: URL {
        var components = URLComponents(url: ArtifactLiveWebView.webOrigin, resolvingAgainstBaseURL: false)!
        components.path = SampleWorkspace.isEnabled ? "/embed/editor/bot-look" : "/editor/bot-look"
        components.queryItems = [URLQueryItem(name: "theme", value: dark ? "dark" : "light")]
        return components.url!
    }

    func makeCoordinator() -> Coordinator { Coordinator(model: model) }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        // "Yours" (characters you describe) stays in the page's storage, as on the web.
        configuration.websiteDataStore = .default()
        let controller = configuration.userContentController
        AuthenticatedWebBridge.install(
            into: controller,
            handler: context.coordinator.authHandler,
            trustedOrigins: [ArtifactLiveWebView.webOrigin, AppConfig.cloudAPIBaseURL, AppConfig.apiBaseURL]
        )
        controller.addScriptMessageHandler(context.coordinator.clerkReply, contentWorld: .page, name: Self.clerkHandler)
        controller.add(context.coordinator, contentWorld: .page, name: Self.lookHandler)
        controller.addUserScript(WKUserScript(source: Self.signInScript, injectionTime: .atDocumentStart,
                                              forMainFrameOnly: true, in: .page))

        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.navigationDelegate = context.coordinator
        webView.isOpaque = false
        webView.backgroundColor = .clear
        webView.scrollView.backgroundColor = .clear
        webView.scrollView.isScrollEnabled = false
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        model.webView = webView
        context.coordinator.reloadToken = model.reloadToken
        context.coordinator.load(url, into: webView)
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        if context.coordinator.reloadToken != model.reloadToken {
            context.coordinator.reloadToken = model.reloadToken
            context.coordinator.load(url, into: webView)
        }
    }

    static func dismantleUIView(_ webView: WKWebView, coordinator: Coordinator) {
        let controller = webView.configuration.userContentController
        controller.removeScriptMessageHandler(forName: AuthenticatedWebBridge.handlerName, contentWorld: .page)
        controller.removeScriptMessageHandler(forName: clerkHandler, contentWorld: .page)
        controller.removeScriptMessageHandler(forName: lookHandler, contentWorld: .page)
        controller.removeAllUserScripts()
    }

    /// The Desktop-style Clerk getter and the app's API origin, as for the editors.
    private static var signInScript: String {
        let api = AppConfig.apiBaseURL.absoluteString.replacingOccurrences(of: "/api/v1", with: "")
        let apiLine = SampleWorkspace.isEnabled ? "" : "window.allternitAppApi = \(ArtifactHTML.jsString(api));"
        return """
        (function () {
          var handlers = window.webkit && window.webkit.messageHandlers;
          var clerk = handlers && handlers.\(clerkHandler);
          if (!clerk) return;
          \(apiLine)
          document.addEventListener('DOMContentLoaded', function () {
            var v = document.querySelector('meta[name=viewport]');
            if (v) v.setAttribute('content', 'width=device-width, initial-scale=1, maximum-scale=1, viewport-fit=cover');
          });
          window.allternit = window.allternit || {};
          if (!window.allternit.auth) {
            window.allternit.auth = {
              getClerkToken: function () {
                return clerk.postMessage(null).then(function (t) { return t || null; }, function () { return null; });
              }
            };
          }
        })();
        """
    }

    /// Bearer for AuthenticatedWebBridge: Clerk first, then the device token.
    @MainActor
    final class AuthReply: NSObject, WKScriptMessageHandlerWithReply {
        func userContentController(_ userContentController: WKUserContentController,
                                   didReceive message: WKScriptMessage) async -> (Any?, String?) {
            (await ArtifactsV2Client.bearerToken(), nil)
        }
    }

    /// The Clerk session JWT alone: what the Desktop bridge returns.
    @MainActor
    final class ClerkReply: NSObject, WKScriptMessageHandlerWithReply {
        func userContentController(_ userContentController: WKUserContentController,
                                   didReceive message: WKScriptMessage) async -> (Any?, String?) {
            let token: String? = (try? await AuthManager.shared.getToken()) ?? nil
            let value: Any? = (token?.isEmpty == false) ? token : nil
            return (value, nil)
        }
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate, WKScriptMessageHandler {
        let authHandler = AuthReply()
        let clerkReply = ClerkReply()
        var reloadToken = 0
        private let model: BotLookModel

        init(model: BotLookModel) { self.model = model }

        func load(_ url: URL, into webView: WKWebView) {
            model.startWatchdog()
            webView.load(URLRequest(url: url))
        }

        func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
            guard message.frameInfo.isMainFrame, let body = message.body as? [String: Any] else { return }
            model.handle(body)
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { model.failed() }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) { model.failed() }

        func webViewWebContentProcessDidTerminate(_ webView: WKWebView) { model.failed() }
    }
}
