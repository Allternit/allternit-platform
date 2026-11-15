import SwiftUI
import WebKit

// -----------------------------------------------------------------------------
// An app's screen on its MCP Apps page: the web's own ConnectorScreenSection
// (and, for a connected MCP app whose server ships views, "From <app>") at
// /editor/app-screen, allternit-ai src/pages/AppScreenEmbedPage.tsx. One
// renderer draws every app's screen on Desktop, ai.allternit.com, the PWA and
// this app, in each vendor's own look, so they stay the same everywhere. The
// page around it (logo, Connect, tools) stays native. Signed in the same way
// as the artifact editors: window.allternit.auth plus AuthenticatedWebBridge.
// -----------------------------------------------------------------------------

/// Which app's screen to show.
struct AppScreenTarget: Equatable {
    /// Catalog id (`linear`, `googledrive`); a vendor server's `mcp:` prefix is fine.
    let app: String
    let name: String
    var connected = false
    var account: String? = nil
    /// The installed MCP connector, for the vendor's own views.
    var install: String? = nil

    @MainActor
    func url(dark: Bool, full: Bool) -> URL {
        var components = URLComponents(url: ArtifactLiveWebView.webOrigin, resolvingAgainstBaseURL: false)!
        components.path = SampleWorkspace.isEnabled ? "/embed/editor/app-screen" : "/editor/app-screen"
        var items = [
            URLQueryItem(name: "app", value: app),
            URLQueryItem(name: "name", value: name),
            URLQueryItem(name: "theme", value: dark ? "dark" : "light"),
        ]
        if connected { items.append(URLQueryItem(name: "connected", value: "1")) }
        if let account, !account.isEmpty { items.append(URLQueryItem(name: "account", value: account)) }
        if let install, !install.isEmpty { items.append(URLQueryItem(name: "install", value: install)) }
        if full { items.append(URLQueryItem(name: "view", value: "full")) }
        components.queryItems = items
        return components.url!
    }
}

/// The "Screens" section of an app's page. Hides itself when the app has no
/// screen; "Open" shows the whole screen full screen.
struct AppScreenSection: View {
    let target: AppScreenTarget
    var onConnect: (() -> Void)? = nil

    @Environment(\.colorScheme) private var colorScheme
    @State private var height: CGFloat = 0
    @State private var hidden = false
    @State private var failed = false
    @State private var full = false
    @State private var reload = 0

    var body: some View {
        if !hidden {
            ZStack {
                AppScreenWebView(
                    url: target.url(dark: colorScheme == .dark, full: false),
                    scrolls: false,
                    reloadToken: reload,
                    onMessage: handle,
                    onFail: { failed = true }
                )
                .frame(height: max(height, 1))
                .opacity(height > 0 ? 1 : 0)
                if failed {
                    VStack(spacing: 8) {
                        Text("Couldn’t load \(target.name)’s screen.")
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextSecondary"))
                        Button("Try again") { failed = false; height = 0; reload += 1 }
                            .font(.system(size: 14, weight: .medium))
                            .foregroundColor(Color("TextPrimary"))
                            .frame(minHeight: 44)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 16)
                } else if height == 0 {
                    ProgressView().frame(maxWidth: .infinity, minHeight: 120)
                }
            }
            .fullScreenCover(isPresented: $full) {
                AppScreenFullView(target: target, onConnect: onConnect)
            }
        }
    }

    private func handle(_ body: [String: Any]) {
        switch body["type"] as? String {
        case "ready":
            if (body["hasScreen"] as? Bool) == false { hidden = true }
        case "height":
            if let h = body["height"] as? Double { height = CGFloat(h) }
            else if let h = body["height"] as? Int { height = CGFloat(h) }
        case "openFull":
            full = true
        case "connect":
            onConnect?()
        default:
            break
        }
    }
}

/// The whole screen, edge to edge (the web's FullScreen route).
struct AppScreenFullView: View {
    let target: AppScreenTarget
    var onConnect: (() -> Void)? = nil

    @Environment(\.dismiss) private var dismiss
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        AppScreenWebView(
            url: target.url(dark: colorScheme == .dark, full: true),
            scrolls: true,
            reloadToken: 0,
            onMessage: { body in
                switch body["type"] as? String {
                case "close": dismiss()
                case "connect": dismiss(); onConnect?()
                default: break
                }
            },
            onFail: {}
        )
        .background(Color("BgPrimary").ignoresSafeArea())
        .ignoresSafeArea(edges: .bottom)
    }
}

/// Bearer for `AuthenticatedWebBridge`: Clerk first, then the device token.
@MainActor
final class WebPageAuthReply: NSObject, WKScriptMessageHandlerWithReply {
    func userContentController(_ userContentController: WKUserContentController,
                               didReceive message: WKScriptMessage) async -> (Any?, String?) {
        (await ArtifactsV2Client.bearerToken(), nil)
    }
}

/// The Clerk session JWT alone: what the web's Desktop bridge returns.
@MainActor
final class WebPageClerkReply: NSObject, WKScriptMessageHandlerWithReply {
    func userContentController(_ userContentController: WKUserContentController,
                               didReceive message: WKScriptMessage) async -> (Any?, String?) {
        let token: String? = (try? await AuthManager.shared.getToken()) ?? nil
        return ((token?.isEmpty == false) ? token : nil, nil)
    }
}

private struct AppScreenWebView: UIViewRepresentable {
    let url: URL
    let scrolls: Bool
    let reloadToken: Int
    let onMessage: ([String: Any]) -> Void
    let onFail: () -> Void

    static let handlerName = "allternitAppScreen"
    static let clerkHandlerName = "allternitClerk"

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .default()
        let controller = configuration.userContentController
        AuthenticatedWebBridge.install(
            into: controller,
            handler: context.coordinator.authReply,
            trustedOrigins: [ArtifactLiveWebView.webOrigin, AppConfig.cloudAPIBaseURL, AppConfig.apiBaseURL]
        )
        controller.addScriptMessageHandler(context.coordinator.clerkReply, contentWorld: .page, name: Self.clerkHandlerName)
        controller.add(context.coordinator, contentWorld: .page, name: Self.handlerName)
        controller.addUserScript(WKUserScript(source: Self.signInScript, injectionTime: .atDocumentStart,
                                              forMainFrameOnly: true, in: .page))

        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.navigationDelegate = context.coordinator
        webView.uiDelegate = context.coordinator
        webView.isOpaque = false
        webView.backgroundColor = .clear
        webView.scrollView.backgroundColor = .clear
        webView.scrollView.isScrollEnabled = scrolls
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        context.coordinator.parent = self
        context.coordinator.reloadToken = reloadToken
        webView.load(URLRequest(url: url))
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        context.coordinator.parent = self
        if context.coordinator.reloadToken != reloadToken {
            context.coordinator.reloadToken = reloadToken
            webView.load(URLRequest(url: url))
        }
    }

    /// Before the page loads: the Desktop-style Clerk getter and the app's API origin (as BotLookEditor).
    private static var signInScript: String {
        let api = AppConfig.apiBaseURL.absoluteString.replacingOccurrences(of: "/api/v1", with: "")
        let apiLine = SampleWorkspace.isEnabled ? "" : "window.allternitAppApi = \(ArtifactHTML.jsString(api));"
        return """
        (function () {
          var handlers = window.webkit && window.webkit.messageHandlers;
          var clerk = handlers && handlers.\(clerkHandlerName);
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

    static func dismantleUIView(_ webView: WKWebView, coordinator: Coordinator) {
        let controller = webView.configuration.userContentController
        controller.removeScriptMessageHandler(forName: AuthenticatedWebBridge.handlerName, contentWorld: .page)
        controller.removeScriptMessageHandler(forName: clerkHandlerName, contentWorld: .page)
        controller.removeScriptMessageHandler(forName: handlerName, contentWorld: .page)
        controller.removeAllUserScripts()
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate, WKUIDelegate, WKScriptMessageHandler {
        let authReply = WebPageAuthReply()
        let clerkReply = WebPageClerkReply()
        var parent: AppScreenWebView?
        var reloadToken = 0

        func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
            guard message.frameInfo.isMainFrame, let body = message.body as? [String: Any] else { return }
            parent?.onMessage(body)
        }

        /// Links out of a screen ("Open in Linear") go to Safari, never into the page.
        func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
                     decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void) {
            if navigationAction.navigationType == .linkActivated, let url = navigationAction.request.url,
               url.host != ArtifactLiveWebView.webOrigin.host {
                UIApplication.shared.open(url)
                decisionHandler(.cancel)
                return
            }
            decisionHandler(.allow)
        }

        func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                     for navigationAction: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
            if let url = navigationAction.request.url, let scheme = url.scheme?.lowercased(), ["http", "https"].contains(scheme) {
                UIApplication.shared.open(url)
            }
            return nil
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { parent?.onFail() }
        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) { parent?.onFail() }
        func webViewWebContentProcessDidTerminate(_ webView: WKWebView) { parent?.onFail() }
    }
}
