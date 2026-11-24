import SwiftUI
import SafariServices

/// Shared thin `SFSafariViewController` wrapper for in-app browsing (export
/// data, support links). Mirrors the private copy in ConnectorsListView,
/// which stays file-scoped to its OAuth flow.
struct SafariView: UIViewControllerRepresentable {
    let url: URL

    func makeUIViewController(context: Context) -> SFSafariViewController {
        SFSafariViewController(url: url)
    }

    func updateUIViewController(_ uiViewController: SFSafariViewController, context: Context) {}
}

/// `URL` isn't `Identifiable`; wrap it for `.sheet(item:)`.
struct IdentifiableURL: Identifiable {
    let url: URL
    var id: String { url.absoluteString }
}

// MARK: - Open links in (phone calm I13)

/// Presents a web page over whatever is on screen (sheets included), the way
/// the app's in-app pages open.
@MainActor
enum InAppBrowser {
    /// False when there is nothing to present from (the caller then hands
    /// the link to the system).
    static func present(_ url: URL) -> Bool {
        let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        let windows = scenes.filter { $0.activationState == .foregroundActive }.flatMap(\.windows)
        guard let root = (windows.first(where: \.isKeyWindow) ?? windows.first)?.rootViewController else { return false }
        var top = root
        while let next = top.presentedViewController, !next.isBeingDismissed { top = next }
        // A page already open over the app: let the system take the next one.
        if top is SFSafariViewController { return false }
        let page = SFSafariViewController(url: url)
        page.dismissButtonStyle = .done
        top.present(page, animated: !ReducedMotion.current)
        return true
    }
}

extension View {
    /// The one root `openURL` handler: every `openURL(_:)`, `Link` and
    /// Markdown link below it follows Settings → App → Open links in
    /// (`openLinkTarget(for:preference:)`).
    func openLinksHandler() -> some View {
        environment(\.openURL, OpenURLAction { url in
            switch openLinkTarget(for: url, preference: SettingsStore.openLinksPreference) {
            case .inApp:
                let shown = MainActor.assumeIsolated { InAppBrowser.present(url) }
                return shown ? .handled : .systemAction
            case .system:
                return .systemAction
            }
        })
    }
}
