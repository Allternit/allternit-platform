import SwiftUI
import UIKit
import WebKit

/// Runs a v2 artifact in the web's own viewer, signed in as the user.
///
/// Kinds the web draws with React and live runtimes — dashboards, ops
/// sheets, motion, cards, canvas designs, video projects and pages that use
/// stored data, AI or connectors — open here instead of in a native
/// renderer. There is no signed-in render URL on the server
/// (`/api/v2/artifacts/:id/render` is the motion video export), so this
/// hosts the web app's single-artifact route `ai.allternit.com/a/<id>`
/// (PublicArtifactPage: the kind's Viewer, nothing else) and hands it two
/// things before any page script runs:
///
/// 1. **The artifact.** The route reads `GET /api/v2/public/artifacts/:id`,
///    which only serves link-shared artifacts. The bridge answers that one
///    request with the record the app already fetched with the user's
///    bearer (`GET /api/v2/artifacts/:id`), in the public shape, with
///    `capabilities` folded into `meta` so PageViewer starts the page
///    runtime for pages that use storage/AI/connectors.
/// 2. **The sign-in.** `window.allternit.auth.getClerkToken()` is the
///    Desktop bridge the web's `hasCloudClerkSession` / `buildAuthHeaders`
///    already read, so the page runtime (`/api/v2/artifact-runtime/:id/*`:
///    storage, consents, AI billed to the viewer), dashboard/ops connector
///    calls and every other cloud call go out with the user's Clerk JWT.
///    `startLogin` is left out on purpose, so the web never switches into
///    Desktop-shell mode. `AuthenticatedWebBridge` also signs the page's
///    own-origin and cloud-API requests. Tokens come from the app on demand
///    over reply handlers, so they stay fresh past a Clerk JWT's ~60 s life.
///
/// The page runtime itself (sandbox-origin iframe + postMessage, the web's
/// PageRuntimeFrame) runs unchanged inside the web view. Links that leave
/// Allternit open in Safari. Data lives in an in-memory store shared by
/// these views for the app session, so nothing persists across launches.
struct ArtifactLiveWebView: View {
    let artifact: ArtifactV2

    enum Phase: Equatable {
        case loading
        case ready
        case failed(String)
    }

    @State private var phase: Phase = .loading
    @State private var reloadToken = 0

    /// The web app that serves `/a/<id>` (DEFAULT_PUBLIC_BASE_URL in
    /// artifacts_v2.rs).
    static let webOrigin = URL(string: "https://ai.allternit.com")!

    var body: some View {
        ZStack {
            ArtifactLiveWebContainer(artifact: artifact, reloadToken: reloadToken) { next in
                phase = next
            }
            .opacity(phase == .ready ? 1 : 0.001)

            switch phase {
            case .loading:
                ProgressView("Opening \(artifact.knownKind?.noun.lowercased() ?? "artifact")…")
            case .ready:
                EmptyView()
            case .failed(let message):
                FriendlyStateView(
                    style: .offline,
                    icon: "wifi.slash",
                    title: "Couldn't open this \(artifact.knownKind?.noun.lowercased() ?? "artifact")",
                    message: message,
                    actionTitle: "Retry",
                    action: {
                        phase = .loading
                        reloadToken += 1
                    }
                )
                .background(Color("BgPrimary"))
            }
        }
    }

    /// Whether a page body needs the page runtime (port of the web's
    /// `pageCapabilities` + `hasCapabilities` checks in PageViewer).
    static func pageNeedsRuntime(_ a: ArtifactV2) -> Bool {
        if hasCapabilities(a.capabilities) { return true }
        if let meta = a.version.meta, hasCapabilities(meta["capabilities"]) { return true }
        let body = a.version.body
        if body.range(of: #"<meta\b[^>]*\bname\s*=\s*["']allternit:capabilities["']"#, options: [.regularExpression, .caseInsensitive]) != nil {
            return true
        }
        if body.range(of: #"\ballternit\s*\.\s*storage\s*\.\s*(get|set|delete|list|getJSON|setJSON)\b"#, options: .regularExpression) != nil {
            return true
        }
        return body.range(of: #"\ballternit\s*\.\s*ai\s*\.\s*complete\b"#, options: .regularExpression) != nil
    }

    private static func hasCapabilities(_ value: JSONValue?) -> Bool {
        guard case .object(let caps)? = value else { return false }
        if case .bool(true)? = caps["storage"] { return true }
        if case .bool(true)? = caps["ai"] { return true }
        if case .array(let list)? = caps["connectors"], !list.isEmpty { return true }
        return false
    }
}

/// The record in the public route's shape (`public_artifact` in
/// artifacts_v2.rs / PublicArtifact in PublicArtifactPage.tsx).
private struct ArtifactLiveSeed: Encodable {
    let id: String
    let kind: String
    let title: String
    let icon: String?
    let body: String
    let bodyFormat: String
    let meta: [String: JSONValue]
    let version: Int
    let updatedAt: String
    let runtimeVersion: Int?
    let ownerName: String?

    enum CodingKeys: String, CodingKey {
        case id, kind, title, icon, body, meta, version
        case bodyFormat = "body_format"
        case updatedAt = "updated_at"
        case runtimeVersion = "runtime_version"
        case ownerName = "owner_name"
    }

    init(_ a: ArtifactV2) {
        id = a.id
        kind = a.kind
        title = a.title
        icon = a.icon
        body = a.version.body
        bodyFormat = a.version.bodyFormat
        var meta = a.version.meta ?? [:]
        if meta["capabilities"] == nil, let caps = a.capabilities, caps != .null {
            meta["capabilities"] = caps
        }
        self.meta = meta
        version = a.version.version
        updatedAt = a.updatedAt
        runtimeVersion = a.runtimeVersion
        ownerName = a.owner.name
    }
}

private struct ArtifactLiveWebContainer: UIViewRepresentable {
    let artifact: ArtifactV2
    let reloadToken: Int
    let onPhase: (ArtifactLiveWebView.Phase) -> Void

    static let clerkHandlerName = "allternitClerk"

    /// One in-memory store for every live artifact view this launch: the web
    /// bundle stays cached between opens, and nothing outlives the process.
    @MainActor private static let dataStore = WKWebsiteDataStore.nonPersistent()

    func makeCoordinator() -> Coordinator { Coordinator(onPhase: onPhase) }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = Self.dataStore
        configuration.allowsInlineMediaPlayback = true
        configuration.mediaTypesRequiringUserActionForPlayback = []
        let preferences = WKWebpagePreferences()
        preferences.allowsContentJavaScript = true
        configuration.defaultWebpagePreferences = preferences

        let controller = configuration.userContentController
        AuthenticatedWebBridge.install(
            into: controller,
            handler: context.coordinator.authHandler,
            trustedOrigins: [ArtifactLiveWebView.webOrigin, AppConfig.cloudAPIBaseURL, AppConfig.apiBaseURL]
        )
        controller.addScriptMessageHandler(context.coordinator.clerkHandler, contentWorld: .page, name: Self.clerkHandlerName)
        controller.addUserScript(WKUserScript(
            source: Self.seedScript(for: artifact),
            injectionTime: .atDocumentStart,
            forMainFrameOnly: true,
            in: .page
        ))

        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.navigationDelegate = context.coordinator
        webView.uiDelegate = context.coordinator
        webView.isOpaque = false
        webView.backgroundColor = .clear
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        context.coordinator.reloadToken = reloadToken
        context.coordinator.load(Self.pageURL(for: artifact), into: webView)
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        context.coordinator.onPhase = onPhase
        if context.coordinator.reloadToken != reloadToken {
            context.coordinator.reloadToken = reloadToken
            context.coordinator.load(Self.pageURL(for: artifact), into: webView)
        }
    }

    static func dismantleUIView(_ webView: WKWebView, coordinator: Coordinator) {
        let controller = webView.configuration.userContentController
        controller.removeScriptMessageHandler(forName: AuthenticatedWebBridge.handlerName, contentWorld: .page)
        controller.removeScriptMessageHandler(forName: clerkHandlerName, contentWorld: .page)
        controller.removeAllUserScripts()
    }

    static func pageURL(for a: ArtifactV2) -> URL {
        let escaped = a.id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))) ?? a.id
        return URL(string: "\(ArtifactLiveWebView.webOrigin.absoluteString)/a/\(escaped)")!
    }

    /// Runs before the web bundle: the Desktop-style Clerk getter, the
    /// answer to the route's public fetch, and the in-app chrome tweaks.
    static func seedScript(for a: ArtifactV2) -> String {
        let seedJSON = (try? JSONEncoder().encode(ArtifactLiveSeed(a))).map { String(decoding: $0, as: UTF8.self) } ?? "null"
        let pathJSON = ArtifactHTML.jsString("/api/v2/public/artifacts/" + (a.id.addingPercentEncoding(withAllowedCharacters: .alphanumerics.union(CharacterSet(charactersIn: "-_.~"))) ?? a.id))
        return """
        (function () {
          var handlers = window.webkit && window.webkit.messageHandlers;
          var clerk = handlers && handlers.\(clerkHandlerName);
          if (!clerk) return;
          var seed = \(seedJSON);
          var seedPath = \(pathJSON);
          window.allternit = window.allternit || {};
          if (!window.allternit.auth) {
            window.allternit.auth = {
              getClerkToken: function () {
                return clerk.postMessage(null).then(function (t) { return t || null; }, function () { return null; });
              }
            };
          }
          var origFetch = window.fetch;
          window.fetch = function (input, init) {
            try {
              var raw = typeof input === 'string' ? input : (input && input.url) || String(input);
              var u = new URL(raw, location.href);
              if (seed && u.pathname === seedPath) {
                return Promise.resolve(new Response(JSON.stringify(seed), {
                  status: 200, headers: { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' }
                }));
              }
            } catch (e) {}
            return origFetch.apply(this, arguments);
          };
          // The app's navigation bar names the artifact; drop the public
          // page's own title strip ("user-generated" notice + "Made with").
          function hideHeader() {
            var s = document.createElement('style');
            s.textContent = 'main > header:first-child{display:none!important}';
            (document.head || document.documentElement).appendChild(s);
          }
          if (document.documentElement) hideHeader(); else document.addEventListener('DOMContentLoaded', hideHeader);
        })();
        """
    }

    /// Bearer for `AuthenticatedWebBridge` (Clerk first, then the runtime
    /// device token, like every other v2 call).
    @MainActor
    final class AuthHandler: NSObject, WKScriptMessageHandlerWithReply {
        @MainActor
        func userContentController(_ userContentController: WKUserContentController,
                                   didReceive message: WKScriptMessage) async -> (Any?, String?) {
            (await ArtifactsV2Client.bearerToken(), nil)
        }
    }

    /// Clerk session JWT only — what the web's Desktop bridge returns. A
    /// device token is not a session, so a device-only app reads as signed
    /// out to the page and its AI/storage calls say so.
    @MainActor
    final class ClerkHandler: NSObject, WKScriptMessageHandlerWithReply {
        @MainActor
        func userContentController(_ userContentController: WKUserContentController,
                                   didReceive message: WKScriptMessage) async -> (Any?, String?) {
            let token: String? = (try? await AuthManager.shared.getToken()) ?? nil
            let value: Any? = (token?.isEmpty == false) ? token : nil
            return (value, nil)
        }
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate, WKUIDelegate {
        let authHandler = AuthHandler()
        let clerkHandler = ClerkHandler()
        var onPhase: (ArtifactLiveWebView.Phase) -> Void
        var reloadToken = 0
        private var pageURL: URL?

        init(onPhase: @escaping (ArtifactLiveWebView.Phase) -> Void) {
            self.onPhase = onPhase
        }

        func load(_ url: URL, into webView: WKWebView) {
            pageURL = url
            onPhase(.loading)
            webView.load(URLRequest(url: url, cachePolicy: .useProtocolCachePolicy, timeoutInterval: 30))
        }

        private func isAppPage(_ url: URL) -> Bool {
            url.scheme?.lowercased() == "https" && url.host == ArtifactLiveWebView.webOrigin.host
                && url.path.hasPrefix("/a/")
        }

        func webView(_ webView: WKWebView,
                     decidePolicyFor navigationAction: WKNavigationAction,
                     decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void) {
            // Frames (the page runtime's sandbox origin, embeds) load freely;
            // the main frame stays on the artifact route.
            guard navigationAction.targetFrame?.isMainFrame ?? false, let url = navigationAction.request.url else {
                decisionHandler(.allow)
                return
            }
            if isAppPage(url) || url.absoluteString == "about:blank" {
                decisionHandler(.allow)
                return
            }
            if let scheme = url.scheme?.lowercased(), ["http", "https", "mailto", "tel"].contains(scheme) {
                UIApplication.shared.open(url)
            }
            decisionHandler(.cancel)
        }

        func webView(_ webView: WKWebView,
                     createWebViewWith configuration: WKWebViewConfiguration,
                     for navigationAction: WKNavigationAction,
                     windowFeatures: WKWindowFeatures) -> WKWebView? {
            if let url = navigationAction.request.url, let scheme = url.scheme?.lowercased(), ["http", "https"].contains(scheme) {
                UIApplication.shared.open(url)
            }
            return nil
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            // The route paints its own skeleton while the viewer chunk
            // loads; show the web view as soon as the document is in.
            onPhase(.ready)
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
            fail(error)
        }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
            fail(error)
        }

        func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
            if let pageURL { load(pageURL, into: webView) }
        }

        private func fail(_ error: Error) {
            let nsError = error as NSError
            // A cancelled navigation (our own policy, or a quick reload) isn't a failure.
            if nsError.domain == NSURLErrorDomain, nsError.code == NSURLErrorCancelled { return }
            if nsError.domain == "WebKitErrorDomain", nsError.code == 102 { return }
            onPhase(.failed(FriendlyErrorMessage.from(error.localizedDescription)))
        }
    }
}
