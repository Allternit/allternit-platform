import Foundation
import StoreKit

/// Decides whether the app may show links that lead to purchases on the web
/// ("Buy credits", "Choose a plan", the usage wall's upgrade buttons), and
/// whether it shows In-App Purchase buttons.
///
/// App Store rule: on the United States storefront an app may link out to its
/// own website to buy. Everywhere else it may not, so outside the US the app
/// shows plan status only, with no link and no price. The storefront comes
/// from StoreKit 2 (`Storefront.current`), which follows the App Store
/// account's country, not the device region. Until StoreKit answers (or when
/// it can't, e.g. no App Store account) links stay hidden.
///
/// In-App Purchase buttons follow the server, not the storefront: they show
/// on every storefront once the server has Apple switched on for that kind
/// of item and StoreKit returned its products (StorePurchaseManager sets
/// `storeKinds`). The US storefront shows them next to the web link.
@MainActor
final class StorefrontGate: ObservableObject {
    static let shared = StorefrontGate()

    /// ISO 3166-1 alpha-3 code, e.g. "USA"; nil until known.
    @Published private(set) var countryCode: String? = nil

    /// True only on the US storefront.
    var allowsPurchaseLinks: Bool { countryCode == "USA" }

    /// Kinds of items (plans, packs) the app can sell through the App Store
    /// right now. Empty until StorePurchaseManager loads, and while the
    /// server keeps Apple off (D10).
    @Published private(set) var storeKinds: Set<StoreKind> = []

    /// Any In-App Purchase button to show.
    var showsStoreButtons: Bool { !storeKinds.isEmpty }

    /// In-App Purchase buttons for plans or for packs.
    func showsStoreButtons(for kind: StoreKind) -> Bool { storeKinds.contains(kind) }

    /// Some way to buy here: the App Store, or the web link (US only).
    func canBuy(_ kind: StoreKind) -> Bool { allowsPurchaseLinks || showsStoreButtons(for: kind) }

    func setStoreKinds(_ kinds: Set<StoreKind>) {
        if storeKinds != kinds { storeKinds = kinds }
    }

    /// Where "Choose a plan" goes (the backend's `upgradeUrl` default).
    static let upgradeURL = URL(string: "https://allternit.com/pricing")!
    /// Where "Buy credits" goes: the web Billing page with the credit packs.
    static let creditsURL = URL(string: "https://ai.allternit.com/settings/billing")!

    private var updatesTask: Task<Void, Never>? = nil

    private init() {
        refresh()
        // The storefront can change while the app runs (account switch in
        // the App Store); follow it.
        updatesTask = Task { [weak self] in
            for await storefront in Storefront.updates {
                // Task inherits @MainActor from init, so this runs on main.
                self?.countryCode = storefront.countryCode
            }
        }
    }

    func refresh() {
        Task { [weak self] in
            let code = await Storefront.current?.countryCode
            self?.countryCode = code
        }
    }
}
