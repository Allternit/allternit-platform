import SwiftUI

/// Mounts the island bar over the workspace (`MainWorkspaceView`). No rail
/// tab (iOS rule): the entries are the island pill, the composer's "+"
/// sheet ("Do with an app") and `allternit://bar` (DeepLinkHost sets
/// `DeepLinkRouter.barRequest`, the "Do with Allternit" App Intent sends it
/// when Siri cannot ask for every value).
struct IslandBarHost: ViewModifier {
    @ObservedObject private var model = IslandBarModel.shared
    @ObservedObject private var router = DeepLinkRouter.shared
    @AppStorage(IslandBarFlag.key) private var flagDefault = false

    func body(content: Content) -> some View {
        content
            .overlay {
                GeometryReader { geo in
                    IslandBarOverlay(model: model,
                                     topInset: geo.safeAreaInsets.top,
                                     width: geo.size.width,
                                     showsPill: flagDefault || launchArgumentEnabled("bar-island"))
                }
            }
            // Fires for a request that arrived before the workspace existed too
            // (`@Published` sends its current value on subscribe).
            .onReceive(router.$barRequest) { request in
                guard request != nil, let taken = router.takeBarRequest() else { return }
                model.open(with: taken)
            }
            .onReceive(NotificationCenter.default.publisher(for: .islandBarOpen)) { _ in
                model.open()
            }
    }
}

extension Notification.Name {
    /// Opens the island bar empty (composer "+" sheet, "Do with an app").
    static let islandBarOpen = Notification.Name("allternit.islandBar.open")
}

extension View {
    func islandBar() -> some View { modifier(IslandBarHost()) }
}
