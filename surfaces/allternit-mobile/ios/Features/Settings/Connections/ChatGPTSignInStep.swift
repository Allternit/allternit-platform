import SwiftUI
import WebKit

/// Sign in with ChatGPT on this iPhone (contract §9.9): the note first, then
/// OpenAI's own page in a web view. The web view catches the redirect to
/// `http://127.0.0.1:1455/auth/callback`, the phone exchanges the code, and
/// the tokens go to the Keychain. Nothing goes to an Allternit server.
struct ChatGPTSignInStep: View {
    @ObservedObject var store: ConnectionsStore
    @ObservedObject private var device = ChatGPTDeviceStore.shared
    let onConnected: () -> Void
    let onFallback: (String) -> Void

    private enum Phase: Equatable { case disclosure, starting, web(ChatGPTPendingSignIn), exchanging, done, failed(String) }

    @State private var phase: Phase = .starting
    @State private var checking = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if device.signedIn, phase != .done, !isSigningIn {
                signedInState
            } else {
                switch phase {
                case .disclosure:
                    ConnDisclosureView(disclosure: ChatGPTDeviceDisclosure.text, onAccept: {
                        device.acceptDisclosure()
                        Task { await start() }
                    }, onDecline: {
                        onFallback("openai-key")
                    })
                case .starting:
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("Opening ChatGPT…").font(.system(size: 13.5)).foregroundColor(Color("TextSecondary"))
                    }
                case .web(let pending):
                    VStack(alignment: .leading, spacing: 8) {
                        ConnStepStrip(steps: ["Sign in to ChatGPT", "Allow Allternit", "Done"], at: 0)
                        Text("Sign in on OpenAI's page below. Your password goes to OpenAI only.")
                            .font(.system(size: 13.5))
                            .foregroundColor(Color("TextSecondary"))
                            .fixedSize(horizontal: false, vertical: true)
                        ChatGPTAuthWebView(url: pending.url) { callback in
                            Task { await finish(callback, pending: pending) }
                        }
                        .frame(minHeight: 520)
                        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                        .accessibilityIdentifier("chatgpt-signin-web")
                    }
                case .exchanging:
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("Finishing the sign-in on this iPhone…").font(.system(size: 13.5)).foregroundColor(Color("TextSecondary"))
                    }
                case .done:
                    HStack(spacing: 8) {
                        Image(systemName: "checkmark.circle.fill").foregroundColor(Theme.statusSuccess)
                        Text(store.statusLine(ConnectionsCatalog.entry("chatgpt")!))
                            .font(.system(size: 14, weight: .medium))
                            .foregroundColor(Color("TextPrimary"))
                    }
                case .failed(let message):
                    VStack(alignment: .leading, spacing: 8) {
                        Text(message).font(.system(size: 13.5)).foregroundColor(Color("TextPrimary"))
                        HStack(spacing: 8) {
                            Button("Try again") { Task { await start() } }.buttonStyle(ConnPrimaryButtonStyle())
                            Button("Use a key instead") { onFallback("openai-key") }.buttonStyle(ConnQuietButtonStyle())
                        }
                    }
                }
            }
        }
        .task {
            if device.signedIn { return }
            if device.disclosureAccepted { await start() } else { phase = .disclosure }
        }
    }

    private var isSigningIn: Bool {
        switch phase {
        case .web, .exchanging: return true
        default: return false
        }
    }

    /// Already signed in: the limit or expiry, and a way to check again.
    @ViewBuilder
    private var signedInState: some View {
        let line = store.statusLine(ConnectionsCatalog.entry("chatgpt")!)
        Text(line).font(.system(size: 14, weight: .medium)).foregroundColor(Color("TextPrimary"))
        if device.needs == .usageLimit {
            Text("Your ChatGPT plan hit its usage limit. It resets on OpenAI's schedule. Until then, pick another model in chat.")
                .font(.system(size: 13.5)).foregroundColor(Color("TextSecondary")).fixedSize(horizontal: false, vertical: true)
        }
        if let error = device.modelsError { SettingsErrorBanner(message: error) }
        HStack(spacing: 8) {
            Button {
                checking = true
                Task {
                    device.markWorking()
                    await device.loadModels(force: true)
                    checking = false
                }
            } label: {
                HStack(spacing: 6) {
                    if checking { ProgressView().controlSize(.small) }
                    Text("Check again")
                }
            }
            .buttonStyle(ConnQuietButtonStyle())
            .disabled(checking)
            Button("Sign in again") { Task { await start() } }.buttonStyle(ConnQuietButtonStyle())
        }
    }

    private func start() async {
        store.setConnecting("chatgpt", true)
        phase = .starting
        let pending = await ChatGPTAuthService.shared.beginSignIn()
        phase = .web(pending)
    }

    private func finish(_ callback: URL, pending: ChatGPTPendingSignIn) async {
        guard phase == .web(pending) else { return }
        phase = .exchanging
        defer { store.setConnecting("chatgpt", false) }
        do {
            let session = try await ChatGPTAuthService.shared.complete(callback: callback, pending: pending)
            await device.signedIn(session)
            phase = .done
            onConnected()
        } catch let error as ChatGPTAuthError {
            phase = .failed(error.plainText)
        } catch {
            phase = .failed("The sign-in didn't finish. Try again.")
        }
    }
}

/// OpenAI's authorize page. Navigations to the loopback redirect are
/// cancelled and handed back instead of loaded: nothing listens on the phone.
/// No cookies are kept between sign-ins (non-persistent data store).
struct ChatGPTAuthWebView: UIViewRepresentable {
    let url: URL
    let onCallback: (URL) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(onCallback: onCallback) }

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        let view = WKWebView(frame: .zero, configuration: config)
        view.navigationDelegate = context.coordinator
        view.allowsBackForwardNavigationGestures = true
        view.load(URLRequest(url: url))
        return view
    }

    func updateUIView(_ view: WKWebView, context: Context) {
        context.coordinator.onCallback = onCallback
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate {
        var onCallback: (URL) -> Void
        private var handled = false

        init(onCallback: @escaping (URL) -> Void) { self.onCallback = onCallback }

        func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction) async -> WKNavigationActionPolicy {
            guard let url = navigationAction.request.url, ChatGPTOAuth.isCallback(url) else { return .allow }
            if !handled {
                handled = true
                onCallback(url)
            }
            return .cancel
        }
    }
}
