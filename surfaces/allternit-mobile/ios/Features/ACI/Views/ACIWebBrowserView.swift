import SwiftUI
import WebKit

/// Resolves free-form bar input to a URL — direct port of the web landing's
/// submit logic (surfaces/ai.allternit.com/src/shell/BrowserPane.tsx:254-258):
/// explicit `http(s)://` stays as-is; a dot without spaces is treated as a
/// host (`https://` prepended); anything else falls back to a Google search.
enum ACIWebInput {
    static func resolve(_ raw: String) -> URL {
        let value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        let lowered = value.lowercased()

        if lowered.hasPrefix("http://") || lowered.hasPrefix("https://"),
           let url = URL(string: value) {
            return url
        }
        if value.contains("."), !value.contains(" "),
           let url = URL(string: "https://\(value)") {
            return url
        }
        // Google fallback (same query URL the web builds).
        var components = URLComponents(string: "https://www.google.com/search")!
        components.queryItems = [URLQueryItem(name: "q", value: value)]
        return components.url!
    }
}

/// Snapshot of the web view's navigation state, pushed by the coordinator on
/// every delegate callback and rendered by the bar/controls.
struct ACINavigationState: Equatable, Sendable {
    var url: URL? = nil
    var canGoBack = false
    var canGoForward = false
    var isLoading = false
    /// 0-1 load progress. Delegate-driven (start/commit/finish milestones) —
    /// no KVO, so no cross-thread observation to annotate under Swift 6.
    var progress: Double = 0
}

/// Imperative bridge between the SwiftUI chrome (URL bar, toolbar) and the
/// `WKWebView` inside the representable. The artifact webview loads static
/// content from `updateUIView`; a real browser needs user-triggered actions
/// (back/forward/reload/load-new-URL), which don't map onto SwiftUI's
/// update pass — so they go through this @MainActor controller instead.
@MainActor
final class ACIBrowserController: ObservableObject {
    @Published private(set) var navigation = ACINavigationState()

    /// Set by the representable in `makeUIView`; weak so the controller can
    /// outlive a torn-down web view without holding it. Read by
    /// ACIBrowserBridge when a bot drives this browser.
    fileprivate(set) weak var webView: WKWebView?

    private var loadedInitialURL = false

    func loadIfNeeded(_ url: URL) {
        guard !loadedInitialURL else { return }
        loadedInitialURL = true
        load(url)
    }

    func load(_ url: URL) {
        Task {
            let tunneled = await InstanceConnection.tunnelLocalhost(url: url)
            webView?.load(URLRequest(url: tunneled))
        }
    }

    func goBack() { webView?.goBack() }
    func goForward() { webView?.goForward() }
    func reload() { webView?.reload() }

    /// Called by the navigation delegate (main actor) on every milestone.
    func update(_ mutate: (inout ACINavigationState) -> Void) {
        mutate(&navigation)
    }
}

/// In-app web browser for the ACI tab — plain browsing, full navigation
/// allowed (this is NOT the locked-down artifact sandbox). URL bar on top,
/// progress bar under it, back/forward/reload toolbar at the bottom.
/// Keyboard-safe: nothing ignores the keyboard safe area, so SwiftUI's
/// default avoidance lifts the toolbar and shrinks the web view.
struct ACIWebBrowserView: View {
    let initialURL: URL
    let onExit: () -> Void
    let onStartAgentRun: ((String) -> Void)?

    @StateObject private var controller = ACIBrowserController()
    @State private var urlText: String
    @FocusState private var urlFieldFocused: Bool
    
    @State private var isExtensionChatOpen = false
    @State private var extensionInput = ""
    @State private var runError: String?

    @ObservedObject private var bridge = ACIBrowserBridge.shared
    @ObservedObject private var phone = AppPhoneService.shared

    private let theme = ModeTheme(mode: .browser)

    init(initialURL: URL, onExit: @escaping () -> Void, onStartAgentRun: ((String) -> Void)? = nil) {
        self.initialURL = initialURL
        self.onExit = onExit
        self.onStartAgentRun = onStartAgentRun
        _urlText = State(initialValue: initialURL.absoluteString)
    }

    var body: some View {
        VStack(spacing: 0) {
            urlBar
            progressBar
            if bridge.isBotActive {
                botBanner
            }
            ACIBrowserWebView(controller: controller)
                .onAppear { controller.loadIfNeeded(initialURL) }
            toolbar
        }
        .background(Color("BgPrimary"))
        .animation(.easeOut(duration: 0.2), value: bridge.isBotActive)
        .onAppear { bridge.attach(controller) }
        .onDisappear { bridge.detach(controller) }
        .alert("Couldn't start the run", isPresented: Binding(
            get: { runError != nil },
            set: { if !$0 { runError = nil } }
        )) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(runError ?? "")
        }
        .onChange(of: controller.navigation.url) { _, newURL in
            // Follow navigations unless the user is mid-edit in the bar.
            if !urlFieldFocused, let newURL {
                urlText = newURL.absoluteString
            }
        }
        .sheet(isPresented: $isExtensionChatOpen) {
            extensionChatSheet
                .presentationDetents([.height(380), .large])
                .presentationDragIndicator(.visible)
        }
        .onAppear {
            if launchArgumentEnabled("extension") {
                isExtensionChatOpen = true
            }
        }
    }

    // MARK: - URL bar

    private var urlBar: some View {
        HStack(spacing: 8) {
            Button(action: onExit) {
                Image(systemName: "chevron.left")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 32, height: 32)
            }
            .buttonStyle(.plain)

            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
                TextField("Search or enter URL", text: $urlText)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .keyboardType(.URL)
                    .submitLabel(.go)
                    .focused($urlFieldFocused)
                    .foregroundColor(Color("TextPrimary"))
                    .font(.system(size: 14))
                    .onSubmit {
                        let input = urlText
                        urlFieldFocused = false
                        controller.load(ACIWebInput.resolve(input))
                    }
                if controller.navigation.isLoading {
                    ProgressView()
                        .controlSize(.small)
                }
            }
            .padding(.horizontal, 10)
            .frame(height: 36)
            .background(Color("BgSecondary"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusSM)
                    .stroke(urlFieldFocused ? theme.accentGlow : Color("BorderSubtle"), lineWidth: 1)
            )
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    // MARK: - Bot banner

    /// Shown while a bot is driving this browser over the control socket
    /// (guideline 2.5.14: a visible indicator, and a way to stop it).
    private var botBanner: some View {
        HStack(spacing: 8) {
            Image(systemName: "cursorarrow.motionlines")
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(theme.accent)
            Text("Bot is using the browser")
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
            Spacer()
            Button("Stop") { bridge.stopBot() }
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Theme.statusError)
                .buttonStyle(.plain)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .background(theme.accentSoft)
        .accessibilityElement(children: .contain)
    }

    /// With this iPhone paired and the control socket up, the run drives
    /// THIS browser (target `app:<phoneId>`) and the page stays on screen;
    /// otherwise the caller's run view takes over as before.
    private func startAgentRun(_ goal: String) {
        if let credentials = phone.credentials, phone.isControlConnected {
            bridge.allowBot()
            Task {
                do {
                    let run = try await ACIAgentClient().startRun(goal: goal, targetDevice: credentials.serial)
                    bridge.activeRunId = run.sessionId
                } catch {
                    runError = error.localizedDescription
                }
            }
        } else {
            onStartAgentRun?(goal)
        }
    }

    // MARK: - Progress bar

    private var progressBar: some View {
        GeometryReader { geometry in
            Rectangle()
                .fill(theme.accent)
                .frame(width: geometry.size.width * controller.navigation.progress, height: 2)
                .opacity(controller.navigation.isLoading ? 1 : 0)
                .animation(.easeOut(duration: 0.2), value: controller.navigation.progress)
        }
        .frame(height: 2)
    }

    // MARK: - Toolbar

    private var toolbar: some View {
        HStack(spacing: 0) {
            toolbarButton("chevron.left", enabled: controller.navigation.canGoBack) {
                controller.goBack()
            }
            toolbarButton("chevron.right", enabled: controller.navigation.canGoForward) {
                controller.goForward()
            }
            Spacer()
            
            // Allternit Extension chat panel button
            Button(action: {
                let generator = UIImpactFeedbackGenerator(style: .medium)
                generator.impactOccurred()
                isExtensionChatOpen = true
            }) {
                HStack(spacing: 4) {
                    Image(systemName: "puzzlepiece.extension.fill")
                        .font(.system(size: 14, weight: .bold))
                    Text("Extension Chat")
                        .font(.system(size: 12, weight: .semibold))
                }
                .foregroundColor(theme.accent)
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .background(theme.accentSoft)
                .cornerRadius(8)
                .overlay(
                    RoundedRectangle(cornerRadius: 8)
                        .stroke(theme.accentGlow, lineWidth: 1)
                )
            }
            .buttonStyle(.plain)
            
            Spacer()
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 6)
        .background(Color("BgPrimary"))
    }

    private func toolbarButton(_ icon: String, enabled: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(.system(size: 16, weight: .medium))
                .foregroundColor(enabled ? theme.accent : Color("TextSecondary").opacity(0.4))
                .frame(width: 44, height: 36)
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
    }

    // MARK: - Extension Chat Sheet
    
    private var extensionChatSheet: some View {
        VStack(alignment: .leading, spacing: 16) {
            // Header
            HStack {
                Image(systemName: "puzzlepiece.extension.fill")
                    .font(.title3)
                    .foregroundColor(theme.accent)
                
                VStack(alignment: .leading, spacing: 2) {
                    Text("Allternit Extension")
                        .font(.headline)
                        .foregroundColor(Color("TextPrimary"))
                    Text(controller.navigation.url?.absoluteString ?? "No active page")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
                
                Spacer()
                
                Button(action: { isExtensionChatOpen = false }) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.title2)
                        .foregroundColor(Color("TextSecondary"))
                }
                .buttonStyle(.plain)
            }
            .padding(.top, 20)
            .padding(.horizontal, 20)
            
            Divider().background(Color("BorderSubtle"))
            
            // Illustration / Chat Intro
            VStack(spacing: 8) {
                Image(systemName: "sparkles.bubble.left.fill")
                    .font(.system(size: 28))
                    .foregroundColor(theme.accent)
                    .padding(12)
                    .background(theme.accentSoft)
                    .clipShape(Circle())
                
                Text("What should the Computer Agent do?")
                    .font(.subheadline)
                    .fontWeight(.semibold)
                    .foregroundColor(Color("TextPrimary"))
                
                Text("Describe a page automation task, and the agent will run ACI to automate this browser tab.")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 24)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 8)
            
            // Predefined Quick Suggestions
            VStack(alignment: .leading, spacing: 8) {
                Text("Suggested Tasks")
                    .font(.system(size: 10, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
                    .textCase(.uppercase)
                    .padding(.horizontal, 20)
                
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach([
                            "Extract page contact details",
                            "Find all links on this page",
                            "Search for specific tutorials",
                            "Auto-fill dummy data on this form"
                        ], id: \.self) { suggestion in
                            Button(action: {
                                extensionInput = suggestion
                            }) {
                                Text(suggestion)
                                    .font(.system(size: 12, weight: .medium))
                                    .padding(.horizontal, 12)
                                    .padding(.vertical, 8)
                                    .background(Color("BgSecondary"))
                                    .foregroundColor(Color("TextPrimary"))
                                    .cornerRadius(8)
                                    .overlay(
                                        RoundedRectangle(cornerRadius: 8)
                                            .stroke(Color("BorderSubtle"), lineWidth: 1)
                                    )
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding(.horizontal, 20)
                }
            }
            
            Spacer()
            
            // Input composer
            HStack(spacing: 8) {
                TextField("Describe task for the browser agent...", text: $extensionInput)
                    .padding(.horizontal, 14)
                    .frame(height: 44)
                    .background(Color("BgSecondary"))
                    .foregroundColor(Color("TextPrimary"))
                    .cornerRadius(10)
                    .overlay(
                        RoundedRectangle(cornerRadius: 10)
                            .stroke(Color("BorderSubtle"), lineWidth: 1)
                    )
                
                Button(action: {
                    let goal = extensionInput.trimmingCharacters(in: .whitespacesAndNewlines)
                    guard !goal.isEmpty else { return }
                    isExtensionChatOpen = false
                    startAgentRun(goal)
                }) {
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.system(size: 32))
                        .foregroundColor(theme.accent)
                }
                .buttonStyle(.plain)
                .disabled(extensionInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
            .padding(.horizontal, 20)
            .padding(.bottom, 20)
        }
        .background(Color("BgPrimary"))
    }
}

// MARK: - WKWebView representable

/// Thin `WKWebView` wrapper. Unlike `SandboxedArtifactWebView` there is no
/// scheme handler, no CSP and no navigation policy — full browsing is the
/// point. Delegate conformances live in an extension, matching the existing
/// representable idiom: `WKNavigationDelegate` is main-actor-annotated in
/// the SDK, so the conformance is inferred main-actor-isolated and can push
/// straight into the @MainActor controller.
private struct ACIBrowserWebView: UIViewRepresentable {
    let controller: ACIBrowserController

    func makeCoordinator() -> Coordinator {
        Coordinator(controller: controller)
    }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        // The bot bridge's element reader, in its own content world.
        configuration.userContentController.addUserScript(ACIBrowserBridge.userScript)
        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.navigationDelegate = context.coordinator
        webView.allowsBackForwardNavigationGestures = true
        controller.webView = webView
        return webView
    }

    func updateUIView(_ uiView: WKWebView, context: Context) {
        // Nothing to sync — loads are imperative (ACIBrowserController), and
        // re-loading here would reset the page on every SwiftUI pass (the bug
        // the artifact webview's lastLoaded guard exists against).
    }

    final class Coordinator: NSObject {
        private let controller: ACIBrowserController

        init(controller: ACIBrowserController) {
            self.controller = controller
        }

        @MainActor func sync(_ webView: WKWebView, isLoading: Bool, progress: Double) {
            controller.update { state in
                state.url = webView.url ?? state.url
                state.canGoBack = webView.canGoBack
                state.canGoForward = webView.canGoForward
                state.isLoading = isLoading
                state.progress = progress
            }
        }
    }
}

// MARK: - WKNavigationDelegate

extension ACIBrowserWebView.Coordinator: WKNavigationDelegate {
    @MainActor func webView(_ webView: WKWebView,
                 decidePolicyFor navigationAction: WKNavigationAction,
                 decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void) {
        guard let url = navigationAction.request.url else {
            decisionHandler(.allow)
            return
        }
        let host = url.host ?? ""
        if host == "localhost" || host == "127.0.0.1" {
            decisionHandler(.cancel)
            Task {
                let tunneled = await InstanceConnection.tunnelLocalhost(url: url)
                webView.load(URLRequest(url: tunneled))
            }
        } else {
            decisionHandler(.allow)
        }
    }

    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        sync(webView, isLoading: true, progress: 0.15)
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        sync(webView, isLoading: true, progress: 0.6)
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        sync(webView, isLoading: false, progress: 1)
    }

    func webView(_ webView: WKWebView,
                 didFailProvisionalNavigation navigation: WKNavigation!,
                 withError error: Error) {
        sync(webView, isLoading: false, progress: 0)
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        sync(webView, isLoading: false, progress: 0)
    }
}
