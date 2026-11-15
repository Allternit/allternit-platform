import AppIntents
import Foundation
import WidgetKit

/// Shared pieces of the one flipping home-screen widget (plan REVISION 2,
/// "Flip widget mechanism"). Compiled into the app and every extension
/// (`SystemSurfaces/`), so the intents below can run in the widget process or,
/// for the ones that open the app, in the app process. No app-only imports.

enum StatusWidgetKind {
    static let id = "AllternitStatusWidget"
}

enum WidgetFace: String, Sendable, Equatable {
    case status
    case actions

    var other: WidgetFace { self == .status ? .actions : .status }
}

enum WidgetFlip {
    /// A flipped widget returns to its front face after this long.
    static let returnAfter: TimeInterval = 10 * 60

    /// The slot key: family plus configured front face. Two identical
    /// widgets share a slot and flip together.
    static func slot(family: String, front: WidgetFace) -> String {
        "\(family)-\(front.rawValue)"
    }

    /// True while `slot` is flipped (inside the return window).
    static func isFlipped(slot: String, now: Date = Date()) -> Bool {
        guard let at = AppGroupStore.flippedAt(slot: slot) else { return false }
        let age = now.timeIntervalSince(at)
        return age >= 0 && age < returnAfter
    }
}

/// Small flags the widget reads that the app writes.
enum WidgetFlags {
    private static let reduceMotionKey = "surfaces.widget.reduceMotion"
    private static let pendingLinkKey = "surfaces.widget.pendingLink"

    /// The app's Reduce Motion setting, for when the widget environment does
    /// not carry `accessibilityReduceMotion` (device question 4).
    static var reduceMotion: Bool {
        get { AppGroup.defaults?.bool(forKey: reduceMotionKey) ?? false }
        set { AppGroup.defaults?.set(newValue, forKey: reduceMotionKey) }
    }

    /// A deep link an intent wants the app to open, waiting for the app.
    static var pendingLink: String? {
        get { AppGroup.defaults?.string(forKey: pendingLinkKey) }
        set {
            if let newValue { AppGroup.defaults?.set(newValue, forKey: pendingLinkKey) }
            else { AppGroup.defaults?.removeObject(forKey: pendingLinkKey) }
        }
    }
}

/// Flip button. Toggles `flippedAt` for the slot in the App Group, then asks
/// WidgetKit for a new timeline; the provider picks the face.
struct FlipWidgetIntent: AppIntent {
    static let title: LocalizedStringResource = "Flip widget"
    static let isDiscoverable = false

    @Parameter(title: "Slot") var slot: String

    init() { slot = "" }
    init(slot: String) { self.slot = slot }

    func perform() async throws -> some IntentResult {
        if WidgetFlip.isFlipped(slot: slot) {
            AppGroupStore.setFlippedAt(nil, slot: slot)
        } else {
            AppGroupStore.setFlippedAt(Date(), slot: slot)
        }
        WidgetCenter.shared.reloadTimelines(ofKind: StatusWidgetKind.id)
        return .result()
    }
}

/// Opens the app at an `allternit://` URL from a widget button (no `Link` in
/// the small size). Runs in the app process (`openAppWhenRun`): it hands the
/// URL to `WidgetLinkOpener.handler` when the app has set one and always
/// leaves it in `WidgetFlags.pendingLink` for the app to take on foreground.
struct OpenAllternitWidgetLinkIntent: AppIntent {
    static let title: LocalizedStringResource = "Open Allternit"
    static let isDiscoverable = false
    static let openAppWhenRun = true

    @Parameter(title: "URL") var urlString: String

    init() { urlString = "allternit://inbox" }
    init(urlString: String) { self.urlString = urlString }

    @MainActor
    func perform() async throws -> some IntentResult {
        WidgetFlags.pendingLink = urlString
        if let url = URL(string: urlString) { WidgetLinkOpener.handler?(url) }
        return .result()
    }
}

/// App side hook for `OpenAllternitWidgetLinkIntent`: the app sets `handler` at
/// launch to route the URL through `DeepLinkRouter`.
enum WidgetLinkOpener {
    @MainActor static var handler: (@MainActor (URL) -> Void)?
}

/// iOS 18 and later: the system opens the URL itself.
@available(iOS 18.0, *)
struct OpenAllternitURLIntent: AppIntent {
    static let title: LocalizedStringResource = "Open Allternit"
    static let isDiscoverable = false

    @Parameter(title: "URL") var urlString: String

    init() { urlString = "allternit://inbox" }
    init(urlString: String) { self.urlString = urlString }

    func perform() async throws -> some IntentResult & OpensIntent {
        let url = URL(string: urlString) ?? URL(string: "allternit://inbox")!
        return .result(opensIntent: OpenURLIntent(url))
    }
}
