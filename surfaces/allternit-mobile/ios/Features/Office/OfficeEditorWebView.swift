import SwiftUI
import WebKit

/// Interactive office editor in a WKWebView — loads the platform surface's
/// editor route (`/docs|/sheets|/slides|/pdf/:artifactId`) for a record in
/// the sectioned artifact service (Office records imported into Artifacts
/// v2 keep that id as `origin.legacy_id`).
///
/// Authentication: the first navigation carries the Bearer, and every
/// request the page itself makes is signed too. A user script installed at
/// document start wraps `fetch` and `XMLHttpRequest` so calls to the page's
/// own origin and the cloud API get `Authorization: Bearer <token>` (unless
/// the page set one), and seeds `localStorage.allternit_token`, which the
/// web's `buildAuthHeaders` reads. The token comes from the app on demand
/// over a reply-capable message handler, so it stays fresh past a Clerk
/// JWT's ~60 s lifetime. Third-party origins never see it.
struct OfficeEditorWebView: UIViewRepresentable {
    let family: OfficeFamily
    let artifactId: String

    /// Platform web root: the gateway serves the platform's static UI at its
    /// root, so strip `/api...` from the configured API base.
    static func platformBaseURL() -> URL {
        var components = URLComponents(url: AppConfig.apiBaseURL, resolvingAgainstBaseURL: false)
        components?.path = ""
        components?.query = nil
        return components?.url ?? AppConfig.apiBaseURL
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        AuthenticatedWebBridge.install(
            into: configuration.userContentController,
            handler: context.coordinator,
            trustedOrigins: [Self.platformBaseURL(), AppConfig.cloudAPIBaseURL]
        )
        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.navigationDelegate = context.coordinator
        let url = Self.platformBaseURL()
            .appendingPathComponent(family.editorPath)
            .appendingPathComponent(artifactId)
        context.coordinator.load(url: url, into: webView)
        return webView
    }

    func updateUIView(_ uiView: WKWebView, context: Context) {}

    static func dismantleUIView(_ uiView: WKWebView, coordinator: Coordinator) {
        uiView.configuration.userContentController.removeScriptMessageHandler(
            forName: AuthenticatedWebBridge.handlerName, contentWorld: .page
        )
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate, WKScriptMessageHandlerWithReply {
        func load(url: URL, into webView: WKWebView) {
            Task {
                var request = URLRequest(url: url)
                if let token = await ArtifactsV2Client.bearerToken() {
                    request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
                }
                webView.load(request)
            }
        }

        /// `window.webkit.messageHandlers.allternitAuth.postMessage(null)`
        /// resolves to the current bearer (or null when signed out).
        func userContentController(_ userContentController: WKUserContentController,
                                   didReceive message: WKScriptMessage) async -> (Any?, String?) {
            (await ArtifactsV2Client.bearerToken(), nil)
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
            logFailure(error)
        }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
            logFailure(error)
        }

        private func logFailure(_ error: Error) {
            NSLog("[OfficeEditorWebView] navigation failed: \(error.localizedDescription)")
        }
    }
}

/// Signs a web page's own API calls with the app's bearer (see
/// OfficeEditorWebView). Origins are fixed at install time.
enum AuthenticatedWebBridge {
    static let handlerName = "allternitAuth"

    @MainActor
    static func install(into controller: WKUserContentController,
                        handler: WKScriptMessageHandlerWithReply,
                        trustedOrigins: [URL]) {
        controller.addScriptMessageHandler(handler, contentWorld: .page, name: handlerName)
        let origins = trustedOrigins.compactMap { url -> String? in
            guard let scheme = url.scheme, let host = url.host else { return nil }
            return url.port.map { "\(scheme)://\(host):\($0)" } ?? "\(scheme)://\(host)"
        }
        let originsJSON = (try? JSONEncoder().encode(origins)).map { String(decoding: $0, as: UTF8.self) } ?? "[]"
        let script = """
        (function () {
          var handler = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.\(handlerName);
          if (!handler) return;
          var trusted = \(originsJSON);
          var cached = null, cachedAt = 0;
          function token() {
            if (cached && Date.now() - cachedAt < 30000) return Promise.resolve(cached);
            return handler.postMessage(null).then(function (t) {
              cached = t || null; cachedAt = Date.now();
              try { if (cached) localStorage.setItem('allternit_token', cached); } catch (e) {}
              return cached;
            }, function () { return null; });
          }
          function isTrusted(url) {
            try { var u = new URL(url, location.href); return u.origin === location.origin || trusted.indexOf(u.origin) >= 0; }
            catch (e) { return false; }
          }
          token();
          var origFetch = window.fetch;
          window.fetch = function (input, init) {
            var url = typeof input === 'string' ? input : (input && input.url) || String(input);
            if (!isTrusted(url)) return origFetch.apply(this, arguments);
            var self = this;
            return token().then(function (t) {
              if (!t) return origFetch.call(self, input, init);
              var opts = Object.assign({}, init || {});
              var headers = new Headers(opts.headers || (typeof input !== 'string' && input && input.headers) || {});
              if (!headers.has('Authorization')) headers.set('Authorization', 'Bearer ' + t);
              opts.headers = headers;
              return origFetch.call(self, input, opts);
            });
          };
          var origOpen = XMLHttpRequest.prototype.open;
          var origSend = XMLHttpRequest.prototype.send;
          var origSet = XMLHttpRequest.prototype.setRequestHeader;
          XMLHttpRequest.prototype.open = function (method, url) {
            this.__allternitTrusted = isTrusted(url); this.__allternitHasAuth = false;
            return origOpen.apply(this, arguments);
          };
          XMLHttpRequest.prototype.setRequestHeader = function (name) {
            if (String(name).toLowerCase() === 'authorization') this.__allternitHasAuth = true;
            return origSet.apply(this, arguments);
          };
          XMLHttpRequest.prototype.send = function () {
            if (this.__allternitTrusted && !this.__allternitHasAuth && cached) origSet.call(this, 'Authorization', 'Bearer ' + cached);
            return origSend.apply(this, arguments);
          };
        })();
        """
        controller.addUserScript(WKUserScript(source: script, injectionTime: .atDocumentStart, forMainFrameOnly: true, in: .page))
    }
}
