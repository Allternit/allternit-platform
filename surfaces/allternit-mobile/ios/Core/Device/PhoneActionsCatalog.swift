import Foundation

/// The Phone Use action catalog shipped with the app (contract §7):
/// `phone-actions-catalog.json` beside this file, trimmed from the 2026-11-16
/// research catalog by `Scripts/trim-phone-actions-catalog.py`.
///
/// Only usable App Intents are in the file: widget and configuration intents
/// (`isDiscoverable: false`) are dropped at trim time, so nothing here can
/// list or run them. Web-research entries are kept with their confidence and
/// are runnable only through a shortcut the person added.
///
/// Apple gives apps no list of other apps' intents and no list of installed
/// shortcuts, so each third-party app gets one optional Allternit shortcut
/// ("Allternit Slack" …) the person adds with one tap. `link` stays nil until
/// the owner publishes that shortcut (gate G3); Settings shows "Link coming".
struct PhoneActionsCatalog: Decodable, Sendable {
    struct Intent: Decodable, Sendable, Hashable {
        let id: String
        let title: String
        let kind: String
        let params: [String]
        let opensApp: Bool
    }

    struct WebAction: Decodable, Sendable, Hashable {
        let action: String
        let kind: String
        let params: [String]
        let confidence: String
    }

    struct App: Decodable, Sendable, Identifiable, Hashable {
        let id: String
        let name: String
        let category: String
        /// The optional Allternit shortcut for this app.
        let shortcut: String
        /// Its iCloud link, nil until published.
        let link: String?
        let intents: [Intent]
        let web: [WebAction]

        var actionCount: Int { intents.count + web.count }
        var linkURL: URL? { link.flatMap(URL.init(string:)) }
    }

    struct BuiltIn: Decodable, Sendable, Hashable {
        let id: String
        let title: String
        let kind: String
        let params: [String]
    }

    /// The one "Allternit Actions" shortcut: built-in Apple actions only,
    /// dispatched on the JSON `action` field.
    struct ActionsShortcut: Decodable, Sendable, Hashable {
        let name: String
        let link: String?
        let actions: [BuiltIn]

        var linkURL: URL? { link.flatMap(URL.init(string:)) }
    }

    let built: String
    let actions: ActionsShortcut
    let apps: [App]

    static let empty = PhoneActionsCatalog(
        built: "", actions: ActionsShortcut(name: "Allternit Actions", link: nil, actions: []), apps: [])

    /// The bundled catalog, or an empty one if the file is missing or broken
    /// (Settings then shows only the Allternit Actions row).
    static let shared: PhoneActionsCatalog = load(from: .main)

    static func load(from bundle: Bundle) -> PhoneActionsCatalog {
        guard let url = bundle.url(forResource: "phone-actions-catalog", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let catalog = try? JSONDecoder().decode(PhoneActionsCatalog.self, from: data) else { return .empty }
        return catalog
    }

    /// Every Allternit-authored shortcut name, Actions first.
    var shortcutNames: [String] { [actions.name] + apps.map(\.shortcut) }
}
