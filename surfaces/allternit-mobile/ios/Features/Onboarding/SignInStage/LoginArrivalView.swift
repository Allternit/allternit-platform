import SwiftUI
import WebKit

/// A bundled, network-free illustration. Authentication remains native and never enters this view.
struct LoginArrivalView: View {
    var paused = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.scenePhase) private var scenePhase
    @State private var loaded = false

    var body: some View {
        ZStack {
            Color(hex: "#141413")
            ArrivalIllustration(paused: paused || scenePhase != .active,
                                reducedMotion: reduceMotion, loaded: $loaded)
                .opacity(loaded ? 1 : 0)
            if !loaded {
                VStack(spacing: 24) {
                    WordmarkView(height: 18)
                    Image("GizziMascot").resizable().scaledToFit().frame(width: 120, height: 120)
                }
                .accessibilityHidden(true)
            }
        }
        .aspectRatio(720.0 / 500.0, contentMode: .fit)
        .allowsHitTesting(false)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Allternit welcomes you. Gizzi and the bot team assemble, then the scene repeats.")
    }
}

private struct ArrivalIllustration: UIViewRepresentable {
    let paused: Bool
    let reducedMotion: Bool
    @Binding var loaded: Bool

    func makeCoordinator() -> Coordinator { Coordinator(loaded: $loaded) }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = true
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.isOpaque = false
        view.backgroundColor = UIColor(red: 20 / 255, green: 20 / 255, blue: 19 / 255, alpha: 1)
        view.scrollView.isScrollEnabled = false
        view.scrollView.bounces = false
        view.isUserInteractionEnabled = false
        view.navigationDelegate = context.coordinator
        context.coordinator.paused = paused
        context.coordinator.reducedMotion = reducedMotion
        if let url = Bundle.main.url(forResource: "login-arrival", withExtension: "html"),
           let html = try? String(contentsOf: url, encoding: .utf8) {
            // Inline data images + hash-locked inline script; CSP rejects all network requests.
            view.loadHTMLString(html, baseURL: nil)
        }
        return view
    }

    func updateUIView(_ view: WKWebView, context: Context) {
        context.coordinator.paused = paused
        context.coordinator.reducedMotion = reducedMotion
        context.coordinator.sendState(view)
    }

    static func dismantleUIView(_ view: WKWebView, coordinator: Coordinator) {
        view.evaluateJavaScript("window.allternitArrivalState?.({paused:true,reducedMotion:true})")
        view.stopLoading()
        view.navigationDelegate = nil
    }

    final class Coordinator: NSObject, WKNavigationDelegate {
        var loaded: Binding<Bool>
        var paused = false
        var reducedMotion = false
        private var finished = false
        init(loaded: Binding<Bool>) { self.loaded = loaded }

        func sendState(_ view: WKWebView) {
            guard finished else { return }
            view.evaluateJavaScript("window.allternitArrivalState?.({paused:\(paused),reducedMotion:\(reducedMotion)})")
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            finished = true
            sendState(webView)
            loaded.wrappedValue = true
        }

        func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
                     decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
            // loadHTMLString's initial about:blank is the only allowed navigation.
            decisionHandler(navigationAction.request.url?.absoluteString == "about:blank" ? .allow : .cancel)
        }

        func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
            finished = false
            loaded.wrappedValue = false
        }
    }
}
