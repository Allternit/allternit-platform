import SwiftUI
import WebKit

/// Loads the minted preview URL and tracks navigation state, mirroring
/// `ACIBrowserController`'s shape — the initial load carries `token` as a
/// query param; the relay's `Set-Cookie` response handles every request
/// after that automatically, since `WKWebViewConfiguration()`'s default
/// (non-ephemeral) data store persists cookies for the view's lifetime. No
/// explicit cookie-store code needed here at all.
@MainActor
final class DevServerPreviewController: ObservableObject {
    @Published private(set) var isLoading = false
    @Published private(set) var progress: Double = 0
    @Published private(set) var canGoBack = false
    @Published private(set) var loadError: String? = nil

    fileprivate weak var webView: WKWebView?
    private var loadedInitialURL = false

    func loadIfNeeded(_ url: URL) {
        guard !loadedInitialURL else { return }
        loadedInitialURL = true
        webView?.load(URLRequest(url: url))
    }

    func reload() { webView?.reload() }
    func goBack() { webView?.goBack() }

    func sync(_ webView: WKWebView, isLoading: Bool, progress: Double, error: String? = nil) {
        self.isLoading = isLoading
        self.progress = progress
        self.canGoBack = webView.canGoBack
        if let error { loadError = error } else if isLoading { loadError = nil }
    }
}

/// Full-screen in-app browser for one dev-server preview session — a
/// deliberately smaller surface than `ACIWebBrowserView` (no URL bar, no
/// arbitrary navigation chrome): the whole point is "preview the one thing
/// I just detected," not general browsing. Static preview only in v1 — no
/// WebSocket relay yet, so dev-server live-reload/HMR won't fire; a manual
/// reload picks up changes.
struct DevServerPreviewView: View {
    let client: DevPreviewClient
    let ptyID: String
    let port: Int
    let token: String

    @StateObject private var controller = DevServerPreviewController()
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ZStack(alignment: .top) {
                DevServerPreviewWebView(controller: controller)
                    .ignoresSafeArea(edges: .bottom)
                // Band dot field over the page until the real didFinish, then it fades out.
                DotLoadingPane(loading: controller.isLoading)
                if controller.isLoading {
                    ProgressView(value: controller.progress)
                        .progressViewStyle(.linear)
                        .tint(Theme.accentCode)
                }
            }
            .navigationTitle(":\(port)")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Close") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    HStack(spacing: 16) {
                        if controller.canGoBack {
                            Button {
                                controller.goBack()
                            } label: {
                                Image(systemName: "chevron.left")
                            }
                        }
                        Button {
                            controller.reload()
                        } label: {
                            Image(systemName: "arrow.clockwise")
                        }
                        .accessibilityLabel("Refresh")
                    }
                }
            }
            .safeAreaInset(edge: .bottom) {
                if let loadError = controller.loadError {
                    Text(loadError)
                        .font(.caption)
                        .foregroundColor(Theme.statusError)
                        .frame(maxWidth: .infinity)
                        .padding(8)
                        .background(Color("BgPanel"))
                }
            }
        }
        .task {
            controller.loadIfNeeded(client.previewURL(ptyID: ptyID, port: port, token: token))
        }
    }
}

// MARK: - WKWebView representable

/// Thin `WKWebView` wrapper, same idiom as `ACIBrowserWebView`
/// (`Features/ACI/Views/ACIWebBrowserView.swift`) — no scheme handler, no
/// custom navigation policy, plain default configuration so the persistent
/// cookie store does its job.
private struct DevServerPreviewWebView: UIViewRepresentable {
    let controller: DevServerPreviewController

    func makeCoordinator() -> Coordinator {
        Coordinator(controller: controller)
    }

    func makeUIView(context: Context) -> WKWebView {
        let webView = WKWebView(frame: .zero, configuration: WKWebViewConfiguration())
        webView.navigationDelegate = context.coordinator
        webView.allowsBackForwardNavigationGestures = true
        controller.webView = webView
        return webView
    }

    func updateUIView(_ uiView: WKWebView, context: Context) {
        // Nothing to sync — loads are imperative via the controller.
    }

    final class Coordinator: NSObject {
        private let controller: DevServerPreviewController

        init(controller: DevServerPreviewController) {
            self.controller = controller
        }

        @MainActor func sync(_ webView: WKWebView, isLoading: Bool, progress: Double, error: String? = nil) {
            controller.sync(webView, isLoading: isLoading, progress: progress, error: error)
        }
    }
}

extension DevServerPreviewWebView.Coordinator: WKNavigationDelegate {
    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        sync(webView, isLoading: true, progress: 0.15)
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        sync(webView, isLoading: true, progress: 0.6)
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        sync(webView, isLoading: false, progress: 1)
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        sync(webView, isLoading: false, progress: 0, error: error.localizedDescription)
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        sync(webView, isLoading: false, progress: 0, error: error.localizedDescription)
    }
}
