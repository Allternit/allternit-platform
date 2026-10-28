import Foundation
import StoreKit

/// Decides whether the app may show links that lead to purchases on the web
/// ("Buy credits", "Get Pro", the usage wall's upgrade buttons).
///
/// App Store rule: on the United States storefront an app may link out to its
/// own website to buy. Everywhere else it may not, so outside the US the app
/// shows plan status only, with no link and no price. The storefront comes
/// from StoreKit 2 (`Storefront.current`), which follows the App Store
/// account's country, not the device region. Until StoreKit answers (or when
/// it can't, e.g. no App Store account) links stay hidden.
@MainActor
final class StorefrontGate: ObservableObject {
    static let shared = StorefrontGate()

    /// ISO 3166-1 alpha-3 code, e.g. "USA"; nil until known.
    @Published private(set) var countryCode: String? = nil

    /// True only on the US storefront.
    var allowsPurchaseLinks: Bool { countryCode == "USA" }

    /// Where "Get Pro" goes (the backend's `upgradeUrl` default).
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
