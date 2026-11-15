import SwiftUI
import UIKit
import WebKit

/// Renders a v2 page/design/diagram/doc body the way the web's HtmlPreview
/// iframe does: scripts on, CDN subresources allowed (generated pages use
/// Tailwind, web fonts, chart libraries), but the page can't navigate the
/// view away — a tapped link opens in Safari. Each view gets its own
/// non-persistent data store, so a page can't read another page's storage
/// or any Allternit session. (In-chat cards keep SandboxedArtifactWebView's
/// stricter no-network sandbox.)
struct ArtifactPageWebView: UIViewRepresentable {
    enum Source: Equatable {
        case html(String)
        case url(URL)
    }

    let source: Source

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.allowsInlineMediaPlayback = true
        let preferences = WKWebpagePreferences()
        preferences.allowsContentJavaScript = true
        configuration.defaultWebpagePreferences = preferences
        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.navigationDelegate = context.coordinator
        webView.uiDelegate = context.coordinator
        webView.isOpaque = false
        webView.backgroundColor = .clear
        context.coordinator.load(source, into: webView)
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        context.coordinator.load(source, into: webView)
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate, WKUIDelegate {
        private var loaded: Source?
        private var initialURL: URL?

        func load(_ source: Source, into webView: WKWebView) {
            guard loaded != source else { return }
            loaded = source
            switch source {
            case .html(let html):
                initialURL = nil
                webView.loadHTMLString(html, baseURL: nil)
            case .url(let url):
                initialURL = url
                webView.load(URLRequest(url: url))
            }
        }

        func webView(_ webView: WKWebView,
                     decidePolicyFor navigationAction: WKNavigationAction,
                     decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void) {
            let isMainFrame = navigationAction.targetFrame?.isMainFrame ?? false
            if isMainFrame, navigationAction.navigationType == .linkActivated,
               let url = navigationAction.request.url {
                if let scheme = url.scheme?.lowercased(), ["http", "https", "mailto", "tel"].contains(scheme) {
                    UIApplication.shared.open(url)
                }
                decisionHandler(.cancel)
                return
            }
            decisionHandler(.allow)
        }

        /// `target=_blank` / `window.open`: hand the URL to Safari instead
        /// of opening a second web view.
        func webView(_ webView: WKWebView,
                     createWebViewWith configuration: WKWebViewConfiguration,
                     for navigationAction: WKNavigationAction,
                     windowFeatures: WKWindowFeatures) -> WKWebView? {
            if let url = navigationAction.request.url, let scheme = url.scheme?.lowercased(), ["http", "https"].contains(scheme) {
                UIApplication.shared.open(url)
            }
            return nil
        }
    }
}

/// Small HTML documents for bodies that aren't pages themselves.
enum ArtifactHTML {
    /// JSON string literal safe to drop inside a <script>.
    static func jsString(_ value: String) -> String {
        let data = (try? JSONEncoder().encode(value)) ?? Data("\"\"".utf8)
        return String(decoding: data, as: UTF8.self).replacingOccurrences(of: "</", with: "<\\/")
    }

    private static let baseStyle = """
    :root { color-scheme: light dark; --fg:#1d1d1f; --muted:#6e6e73; --bg:#ffffff; --line:#e5e5ea; --code:#f5f5f7; }
    @media (prefers-color-scheme: dark) { :root { --fg:#ececec; --muted:#9b9b9b; --bg:#1c1c1e; --line:#38383a; --code:#2c2c2e; } }
    html,body { margin:0; background:var(--bg); color:var(--fg); }
    body { font: 16px/1.55 -apple-system, system-ui, sans-serif; padding: 20px 16px 48px; -webkit-text-size-adjust: 100%; }
    img, svg { max-width: 100%; height: auto; }
    pre, code { font-family: ui-monospace, Menlo, monospace; font-size: 13px; background: var(--code); border-radius: 6px; }
    pre { padding: 12px; overflow-x: auto; } code { padding: 1px 4px; } pre code { padding: 0; }
    table { border-collapse: collapse; } td, th { border: 1px solid var(--line); padding: 6px 10px; }
    blockquote { margin: 0; padding-left: 12px; border-left: 3px solid var(--line); color: var(--muted); }
    a { color: #0a84ff; }
    """

    private static func page(head: String = "", body: String) -> String {
        """
        <!doctype html><html><head><meta charset="utf-8">
        <meta name="viewport" content="width=device-width, initial-scale=1">
        <style>\(baseStyle)</style>\(head)</head><body>\(body)</body></html>
        """
    }

    /// Markdown rendered with marked (pinned) and sanitized with DOMPurify.
    static func markdown(_ markdown: String) -> String {
        page(
            head: """
            <script src="https://cdn.jsdelivr.net/npm/marked@12.0.2/marked.min.js"></script>
            <script src="https://cdn.jsdelivr.net/npm/dompurify@3.1.6/dist/purify.min.js"></script>
            """,
            body: """
            <article id="doc"></article>
            <script>
            (function(){ var md = \(jsString(markdown)); var el = document.getElementById('doc');
              try { el.innerHTML = DOMPurify.sanitize(marked.parse(md)); }
              catch (e) { el.textContent = md; el.style.whiteSpace = 'pre-wrap'; } })();
            </script>
            """
        )
    }

    /// Mermaid source rendered by mermaid (pinned), with the source shown
    /// if it doesn't parse.
    static func mermaid(_ source: String) -> String {
        page(
            head: "<script src=\"https://cdn.jsdelivr.net/npm/mermaid@10.9.1/dist/mermaid.min.js\"></script>",
            body: """
            <div id="d" style="display:flex;justify-content:center"></div>
            <script>
            (function(){ var src = \(jsString(source)); var el = document.getElementById('d');
              var dark = window.matchMedia('(prefers-color-scheme: dark)').matches;
              try { mermaid.initialize({ startOnLoad:false, securityLevel:'strict', theme: dark ? 'dark' : 'default' });
                mermaid.render('m', src).then(function(r){ el.innerHTML = r.svg; })
                  .catch(function(){ el.innerHTML = '<pre></pre>'; el.firstChild.textContent = src; }); }
              catch (e) { el.innerHTML = '<pre></pre>'; el.firstChild.textContent = src; } })();
            </script>
            """
        )
    }

    /// An SVG drawn as an image (scripts inside it never run).
    static func svg(_ svg: String) -> String {
        let encoded = svg.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? ""
        return page(body: "<div style=\"display:flex;justify-content:center;background:#fff;border-radius:8px\"><img alt=\"\" src=\"data:image/svg+xml;charset=utf-8,\(encoded)\"></div>")
    }
}
