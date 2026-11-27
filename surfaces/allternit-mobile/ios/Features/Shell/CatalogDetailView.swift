import SwiftUI
import UIKit
import WebKit

/// One Catalog entry (allternit-ai views/catalog detail): logo, name, byline,
/// Add / Connect through the existing connector flow, the connector URL, the
/// real app screen when the entry has a screen pack, and the facts.
struct CatalogDetailView: View {
    let entry: CatalogEntry
    var isConnected: Bool = false
    /// Called after the connect sheet closes so the Catalog can refresh.
    var onChanged: () -> Void = {}

    @Environment(\.colorScheme) private var scheme
    @State private var isConnectPresented = false
    @State private var copied = false
    @State private var browserURL: IdentifiableURL?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                header
                if !entry.blurb.isEmpty {
                    Text(entry.blurb)
                        .font(.body)
                        .foregroundColor(Color("TextPrimary"))
                        .fixedSize(horizontal: false, vertical: true)
                }
                if let pack = entry.screenPack, entry.interactive || !pack.isEmpty {
                    CatalogScreenFrame(pack: pack, dark: scheme == .dark)
                }
                facts
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
        }
        .background(Color("BgPrimary"))
        .navigationTitle("Catalog")
        .navigationBarTitleDisplayMode(.inline)
        .sheet(isPresented: $isConnectPresented, onDismiss: onChanged) {
            ConnectorsListView(initialConnectorId: entry.connectId)
        }
        .sheet(item: $browserURL) { SafariView(url: $0.url) }
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 14) {
            CatalogLogo(name: entry.name, url: entry.iconURL, size: 56)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(entry.name).font(.title2.weight(.semibold)).foregroundColor(Color("TextPrimary"))
                    if entry.isVerified {
                        Image(systemName: "checkmark.seal").foregroundColor(Color("TextSecondary"))
                            .accessibilityLabel("Verified")
                    }
                }
                if let author = entry.author, !author.isEmpty {
                    Text("By \(author)").font(.subheadline).foregroundColor(Color("TextSecondary"))
                }
            }
            Spacer(minLength: 8)
            action
        }
    }

    @ViewBuilder private var action: some View {
        if entry.isSkill {
            EmptyView()
        } else if isConnected {
            Text("Connected").font(.subheadline.weight(.medium)).foregroundColor(Color("TextSecondary"))
        } else if entry.available {
            Button { isConnectPresented = true } label: {
                Text("Add").font(.subheadline.weight(.semibold)).foregroundColor(.white)
                    .padding(.horizontal, 18).frame(minHeight: 44)
                    .background(Color("AccentPrimary")).clipShape(Capsule())
            }
            .buttonStyle(.plain)
        } else {
            Text("Unavailable").font(.subheadline).foregroundColor(Color("TextSecondary"))
        }
    }

    private var facts: some View {
        VStack(alignment: .leading, spacing: 0) {
            if !entry.available, let reason = entry.unavailableReason {
                fact("Status", Text(reason))
            }
            if let url = entry.serverURL, !url.isEmpty {
                fact("Connector URL", HStack {
                    Text(url).font(.footnote.monospaced()).lineLimit(1).truncationMode(.middle)
                    Spacer(minLength: 8)
                    Button(copied ? "Copied" : "Copy") {
                        UIPasteboard.general.string = url
                        copied = true
                        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { copied = false }
                    }
                    .font(.footnote.weight(.medium)).frame(minHeight: 44).buttonStyle(.borderless)
                })
            }
            if entry.interactive {
                fact("In-chat UI", Text("Shows its own screen inside the chat."))
            }
            fact("Developer", HStack(spacing: 6) {
                Text(entry.author ?? "Unknown")
                if entry.isVerified { Image(systemName: "checkmark.seal").font(.footnote) }
            })
            if !entry.categories.isEmpty {
                fact("Categories", Text(entry.categories.joined(separator: ", ")))
            }
            if let site = entry.authorURL, let u = URL(string: site) {
                Button { browserURL = IdentifiableURL(url: u) } label: {
                    Text("Developer website \u{2197}").font(.subheadline.weight(.medium))
                }
                .frame(minHeight: 44).buttonStyle(.borderless)
            }
        }
    }

    private func fact<V: View>(_ label: String, _ value: V) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label).font(.caption).foregroundColor(Color("TextSecondary"))
            value.font(.subheadline).foregroundColor(Color("TextPrimary"))
            Divider().background(Color("BorderSubtle")).padding(.top, 8)
        }
        .padding(.vertical, 8)
    }
}

/// The app's real screen: one small embed page of ai.allternit.com (ai PR D5)
/// in an in-app web view, sample data only. Until the page ships it shows
/// nothing, never a broken frame.
private struct CatalogScreenFrame: View {
    let pack: String
    let dark: Bool
    @State private var loaded = false

    private var url: URL? {
        var c = URLComponents(string: "https://ai.allternit.com/embed/app-screen.html")
        c?.queryItems = [URLQueryItem(name: "app", value: pack), URLQueryItem(name: "theme", value: dark ? "dark" : "light")]
        return c?.url
    }

    var body: some View {
        if let url {
            VStack(alignment: .leading, spacing: 6) {
                CatalogWebFrame(url: url, loaded: $loaded)
                    .frame(height: loaded ? 360 : 1)
                    .opacity(loaded ? 1 : 0)
                    .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
                        .stroke(Color("BorderSubtle"), lineWidth: loaded ? 1 : 0))
                if loaded {
                    Text("Sample data").font(.caption).foregroundColor(Color("TextSecondary"))
                }
            }
        }
    }
}

/// Reports `loaded` only on a 2xx navigation, so a missing page stays hidden.
private struct CatalogWebFrame: UIViewRepresentable {
    let url: URL
    @Binding var loaded: Bool

    func makeCoordinator() -> Coordinator { Coordinator(loaded: $loaded) }

    func makeUIView(context: Context) -> WKWebView {
        let view = WKWebView(frame: .zero, configuration: WKWebViewConfiguration())
        view.navigationDelegate = context.coordinator
        view.isOpaque = false
        view.backgroundColor = .clear
        view.scrollView.isScrollEnabled = false
        view.load(URLRequest(url: url))
        return view
    }

    func updateUIView(_ uiView: WKWebView, context: Context) {
        if uiView.url?.absoluteString != url.absoluteString, !uiView.isLoading { uiView.load(URLRequest(url: url)) }
    }

    final class Coordinator: NSObject, WKNavigationDelegate {
        var loaded: Binding<Bool>
        private var ok = false
        init(loaded: Binding<Bool>) { self.loaded = loaded }

        func webView(_ webView: WKWebView, decidePolicyFor navigationResponse: WKNavigationResponse,
                     decisionHandler: @escaping (WKNavigationResponsePolicy) -> Void) {
            if let http = navigationResponse.response as? HTTPURLResponse { ok = (200...299).contains(http.statusCode) }
            decisionHandler(.allow)
        }
        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { loaded.wrappedValue = ok }
        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { loaded.wrappedValue = false }
        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
            loaded.wrappedValue = false
        }
    }
}
