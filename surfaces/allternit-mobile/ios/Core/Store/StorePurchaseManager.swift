import Combine
import Foundation
import StoreKit

/// What the purchase UI says after an action. One at a time; the view clears it.
enum StorePurchaseNotice: Equatable, Sendable {
    /// Granted (or granted before): the line to show.
    case purchased(String)
    case restored(Int)
    case nothingToRestore
    /// Ask to Buy: waiting for a parent. The purchase arrives later through
    /// `Transaction.updates`.
    case pending
    case cancelled
    /// Sandbox purchase by an account outside the server's allow-list (D8).
    case sandboxNotGranted
    /// A 200 that granted nothing for another reason (refunded, account closed).
    case notGranted(String)
    case signedOut
    /// 409 `already_subscribed`: another plan is open and someone else bills it.
    case alreadySubscribed(provider: String?, manageURL: URL?, message: String)
    /// 409 `owned_by_another_account`.
    case ownedByAnotherAccount(String)
    /// 403 `first_purchase_limit`.
    case firstPurchaseLimit(String)
    /// StoreKit can't reach the App Store or has no such product.
    case storeUnavailable
    /// The server refused or could not be reached. `canRetry` shows "Try again".
    case failed(String, canRetry: Bool)

    var isError: Bool {
        switch self {
        case .purchased, .restored, .nothingToRestore, .pending, .cancelled, .sandboxNotGranted: return false
        default: return true
        }
    }
}

enum StorePurchaseCopy {
    static let signIn = "Sign in to buy"
    static let signedOut = "Sign in to your Allternit account to buy. Purchases are added to the account you're signed in to."
    static let pending = "Waiting for approval. The purchase finishes when it's approved."
    static let cancelled = "Purchase cancelled."
    static let sandbox = "Test purchase. No credits were added."
    static let nothingToRestore = "No purchases to restore for this Apple Account."
    static let storeUnavailable = "The App Store isn't available right now. Try again later."
    static let serverError = "Your purchase went through, but Allternit couldn't add it yet. We'll keep trying. You won't be charged again."
    static let preflightError = "Couldn't start the purchase. Try again."
    static let unverified = "The App Store couldn't verify this purchase. Nothing was added."
    static let ownedByAnother = "This purchase belongs to a different Allternit account. Sign in to that account to use it."

    static func restored(_ count: Int) -> String {
        count == 1 ? "Restored 1 purchase." : "Restored \(count) purchases."
    }

    static func billedBy(_ provider: String?) -> String {
        switch provider {
        case "apple": return "Apple"
        case "google": return "Google Play"
        default: return "the web"
        }
    }

    static func text(_ notice: StorePurchaseNotice) -> String {
        switch notice {
        case .purchased(let line): return line
        case .restored(let count): return restored(count)
        case .nothingToRestore: return nothingToRestore
        case .pending: return pending
        case .cancelled: return cancelled
        case .sandboxNotGranted: return sandbox
        case .notGranted(let line): return line
        case .signedOut: return signedOut
        case .alreadySubscribed(_, _, let message): return message
        case .ownedByAnotherAccount(let message): return message
        case .firstPurchaseLimit(let message): return message
        case .storeUnavailable: return storeUnavailable
        case .failed(let message, _): return message
        }
    }
}

/// Apple In-App Purchase for plans and credit packs (StoreKit 2).
///
/// - Products come from `Product.products(for:)`, only for ids the server
///   has Apple switched on for (`store.apple.enabled`, D10).
/// - Every purchase runs preflight first and passes the server's account
///   token as `appAccountToken` (D4).
/// - A verified transaction is posted to the server. `finish()` runs only
///   after the server answers 2xx, `granted: false` included; anything else
///   leaves it unfinished, so StoreKit hands it back at the next launch and
///   the server gets it then.
/// - `Transaction.updates` (renewals, Ask to Buy approvals, refunds, other
///   devices) and `Transaction.unfinished` are posted from app launch.
@MainActor
final class StorePurchaseManager: ObservableObject {
    static let shared = StorePurchaseManager()

    enum CatalogState: Equatable {
        case idle
        case loading
        /// Store items and StoreKit products are loaded.
        case ready
        /// The server sells in the app, but StoreKit returned nothing or failed.
        case unavailable
        /// The server has Apple switched off (or is an old server): hide store buttons.
        case notConfigured
    }

    @Published private(set) var catalogState: CatalogState = .idle
    /// Items the server enables AND StoreKit returned a product for.
    @Published private(set) var items: [StoreCatalogItem] = []
    @Published private(set) var products: [String: Product] = [:]
    /// The product being bought; one purchase at a time.
    @Published private(set) var busyProductId: String? = nil
    @Published private(set) var isRestoring = false
    @Published var notice: StorePurchaseNotice? = nil

    private let client: StoreBillingClienting
    private let isSignedIn: @MainActor () -> Bool
    private let environment: @Sendable () async -> String
    private let onAccountChanged: @MainActor () -> Void

    /// A transaction the server didn't take yet; "Try again" re-posts it.
    private var retryTransaction: StoreKit.Transaction? = nil
    /// The item whose preflight failed; "Try again" starts it over.
    private var retryItem: StoreCatalogItem? = nil
    /// Transactions being posted right now (purchase result and the updates
    /// stream can both deliver one).
    private var inFlight: Set<UInt64> = []
    private var updatesTask: Task<Void, Never>? = nil
    private var signInObserver: AnyCancellable? = nil
    private var catalogLoadedAt: Date? = nil

    init(client: StoreBillingClienting = StoreBillingClient(),
         isSignedIn: @escaping @MainActor () -> Bool = { AuthManager.shared.isSignedIn },
         environment: @escaping @Sendable () async -> String = { await StorePurchaseManager.currentEnvironment() },
         onAccountChanged: @escaping @MainActor () -> Void = { AccountPlanStore.shared.load(force: true) }) {
        self.client = client
        self.isSignedIn = isSignedIn
        self.environment = environment
        self.onAccountChanged = onAccountChanged
    }

    // MARK: - Lifecycle

    /// Call once at launch: listens for transactions and posts the ones left
    /// unfinished. Also replays them after each sign-in, since a transaction
    /// posted while signed out was refused and kept.
    func start() {
        guard updatesTask == nil else { return }
        // In the unit-test host the tests run their own manager against a
        // StoreKit test session; the app's shared one stays out of the way.
        if self === Self.shared, ProcessInfo.processInfo.environment["XCTestConfigurationFilePath"] != nil { return }
        updatesTask = Task { [weak self] in
            for await update in StoreKit.Transaction.updates {
                await self?.handleUpdate(update)
            }
        }
        // Loads the store block so every entry point knows whether to show
        // store buttons, then posts what was left unfinished.
        Task { await loadCatalog() }
        signInObserver = AuthManager.shared.$isSignedIn
            .removeDuplicates()
            .dropFirst()
            .filter { $0 }
            .sink { [weak self] _ in
                Task { @MainActor in await self?.replayUnfinished() }
            }
    }

    /// Stops the listener (tests; the shared manager runs for the app's life).
    func stop() {
        updatesTask?.cancel()
        updatesTask = nil
        signInObserver = nil
    }

    /// Posts every unfinished transaction quietly. No notice on failure: they
    /// stay unfinished and come back next time.
    func replayUnfinished() async {
        guard isSignedIn() else { return }
        for await result in StoreKit.Transaction.unfinished {
            guard case .verified(let transaction) = result else { continue }
            _ = await deliver(transaction)
        }
    }

    private func handleUpdate(_ result: VerificationResult<StoreKit.Transaction>) async {
        // Signed out: leave it unfinished; it is posted after sign-in.
        guard isSignedIn(), case .verified(let transaction) = result else { return }
        let wasPending = notice == .pending
        if case .delivered(let answer) = await deliver(transaction), answer.granted, transaction.revocationDate == nil, wasPending {
            notice = successNotice(answer, productId: transaction.productID)
        }
    }

    // MARK: - Catalog

    /// Loads the server's store block, then the StoreKit products for it.
    func loadCatalog(force: Bool = false) async {
        if catalogState == .loading { return }
        if !force, catalogState == .ready { return }
        // While Apple is off on the server (the default until its secrets
        // are set), don't ask again on every screen.
        if !force, catalogState == .notConfigured, let at = catalogLoadedAt, Date().timeIntervalSince(at) < 600 { return }
        catalogState = .loading
        let enabled: [StoreCatalogItem]
        do {
            enabled = try await client.catalog()
        } catch {
            // Server unreachable: keep store buttons hidden, as when off.
            publish(items: [], products: [:], state: .unavailable)
            return
        }
        guard !enabled.isEmpty else {
            publish(items: [], products: [:], state: .notConfigured)
            return
        }
        do {
            let found = try await Product.products(for: Set(enabled.map(\.productId)))
            let byId = Dictionary(found.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
            let sellable = enabled.filter { byId[$0.productId] != nil }
            publish(items: sellable, products: byId, state: sellable.isEmpty ? .unavailable : .ready)
        } catch {
            publish(items: [], products: [:], state: .unavailable)
        }
        if isSignedIn() { await replayUnfinished() }
    }

    private func publish(items: [StoreCatalogItem], products: [String: Product], state: CatalogState) {
        catalogLoadedAt = Date()
        self.items = items
        self.products = products
        self.catalogState = state
        StorefrontGate.shared.setStoreKinds(Set(items.map(\.kind)))
    }

    var plans: [StoreCatalogItem] { items.filter { $0.kind == .plan } }
    var packs: [StoreCatalogItem] { items.filter { $0.kind == .pack } }

    /// The App Store's localized price ("$24.99", "24,99 €"), D11.
    func displayPrice(_ item: StoreCatalogItem) -> String? {
        products[item.productId]?.displayPrice
    }

    /// A plan can be bought in the app when there is no paid plan, or when
    /// Apple already bills it (a change inside the subscription group bills
    /// once). A plan billed on the web or by Google is managed there (D5).
    static func planPurchaseAllowed(_ subscription: BillingSubscription?) -> Bool {
        guard let subscription, subscription.isPaid else { return true }
        return subscription.billingProvider == "apple"
    }

    // MARK: - Purchase

    func purchase(_ item: StoreCatalogItem) async {
        guard busyProductId == nil else { return }
        guard isSignedIn() else {
            notice = .signedOut
            return
        }
        guard let product = products[item.productId] else {
            notice = .storeUnavailable
            return
        }
        busyProductId = item.productId
        notice = nil
        retryItem = nil
        retryTransaction = nil
        defer { busyProductId = nil }

        let preflight: StorePreflight
        do {
            preflight = try await client.preflight(kind: item.kind, id: item.id, environment: await environment())
        } catch let error as StoreBillingError {
            notice = refusalNotice(error, preflight: true)
            if error.isRetryable { retryItem = item }
            return
        } catch {
            notice = .failed(StorePurchaseCopy.preflightError, canRetry: true)
            retryItem = item
            return
        }
        guard let token = UUID(uuidString: preflight.accountToken) else {
            notice = .failed(StorePurchaseCopy.preflightError, canRetry: true)
            retryItem = item
            return
        }
        // The server names the product to buy; it should match the catalog.
        let toBuy = products[preflight.productId] ?? product

        let result: Product.PurchaseResult
        do {
            result = try await toBuy.purchase(options: [.appAccountToken(token)])
        } catch {
            notice = Self.isCancellation(error) ? .cancelled : .storeUnavailable
            return
        }
        switch result {
        case .success(.verified(let transaction)):
            switch await deliver(transaction) {
            case .delivered(let answer):
                notice = answerNotice(answer, productId: transaction.productID)
            case .refused(let error):
                notice = refusalNotice(error, preflight: false)
                if error.isRetryable { retryTransaction = transaction }
            case .busy:
                break
            }
        case .success(.unverified):
            // Not posted, not finished: StoreKit offers it again and the
            // server checks it with Apple then.
            notice = .failed(StorePurchaseCopy.unverified, canRetry: false)
        case .pending:
            notice = .pending
        case .userCancelled:
            notice = .cancelled
        @unknown default:
            notice = nil
        }
    }

    /// "Try again" after a server error.
    func retry() async {
        if let transaction = retryTransaction {
            retryTransaction = nil
            notice = nil
            switch await deliver(transaction) {
            case .delivered(let answer):
                notice = answerNotice(answer, productId: transaction.productID)
            case .refused(let error):
                notice = refusalNotice(error, preflight: false)
                if error.isRetryable { retryTransaction = transaction }
            case .busy:
                break
            }
        } else if let item = retryItem {
            await purchase(item)
        }
    }

    var canRetry: Bool { retryTransaction != nil || retryItem != nil }

    // MARK: - Restore

    /// "Restore Purchases": `AppStore.sync()`, then posts the current
    /// entitlements and anything left unfinished.
    func restore() async {
        guard !isRestoring else { return }
        guard isSignedIn() else {
            notice = .signedOut
            return
        }
        isRestoring = true
        notice = nil
        defer { isRestoring = false }
        do {
            try await AppStore.sync()
        } catch {
            if Self.isCancellation(error) {
                notice = .cancelled
                return
            }
            // Still post what this device knows.
            NSLog("[StorePurchase] AppStore.sync failed: %@", error.localizedDescription)
        }
        var restored = 0
        var failure: StorePurchaseNotice? = nil
        var seen: Set<UInt64> = []
        for sequence in [StoreKit.Transaction.currentEntitlements, StoreKit.Transaction.unfinished] {
            for await result in sequence {
                guard case .verified(let transaction) = result, seen.insert(transaction.id).inserted else { continue }
                switch await deliver(transaction) {
                case .delivered(let answer):
                    if answer.granted { restored += 1 }
                    else if answer.reason == "sandbox_not_granted" { failure = failure ?? .sandboxNotGranted }
                case .refused(let error):
                    failure = refusalNotice(error, preflight: false)
                    if error.isRetryable { retryTransaction = transaction }
                case .busy:
                    break
                }
            }
        }
        if restored > 0 {
            notice = .restored(restored)
        } else {
            notice = failure ?? .nothingToRestore
        }
    }

    // MARK: - Delivery

    enum Delivery: Equatable {
        case delivered(StorePurchaseAnswer)
        case refused(StoreBillingError)
        /// Already being posted by another path.
        case busy
    }

    /// Posts one transaction. Finishes it only on a 2xx.
    @discardableResult
    func deliver(_ transaction: StoreKit.Transaction) async -> Delivery {
        guard inFlight.insert(transaction.id).inserted else { return .busy }
        defer { inFlight.remove(transaction.id) }
        do {
            let answer = try await client.postAppleTransaction(id: String(transaction.id))
            await transaction.finish()
            if answer.granted || answer.reason == "refunded" { onAccountChanged() }
            return .delivered(answer)
        } catch let error as StoreBillingError {
            NSLog("[StorePurchase] transaction %llu kept unfinished: %@", transaction.id, String(describing: error))
            return .refused(error)
        } catch {
            return .refused(.unreachable(error.localizedDescription))
        }
    }

    // MARK: - Notices

    private func item(for productId: String) -> StoreCatalogItem? {
        items.first { $0.productId == productId }
    }

    private func successNotice(_ answer: StorePurchaseAnswer, productId: String) -> StorePurchaseNotice {
        if answer.idempotentReplay { return .purchased("This purchase is already on your account.") }
        if let plan = answer.subscription {
            return .purchased("\(plan.label) is active. Its credits are on your account.")
        }
        let label = item(for: productId)?.label ?? "Credits"
        return .purchased("\(label) in credits added to your account.")
    }

    func answerNotice(_ answer: StorePurchaseAnswer, productId: String) -> StorePurchaseNotice {
        if answer.granted { return successNotice(answer, productId: productId) }
        switch answer.reason {
        case "sandbox_not_granted": return .sandboxNotGranted
        case "refunded": return .notGranted("This purchase was refunded. Nothing was added.")
        default: return .notGranted(answer.message ?? "Nothing was added to your account.")
        }
    }

    func refusalNotice(_ error: StoreBillingError, preflight: Bool) -> StorePurchaseNotice {
        switch error {
        case .signedOut:
            return .signedOut
        case .refused(let refusal):
            switch refusal.code {
            case "already_subscribed":
                return .alreadySubscribed(
                    provider: refusal.billingProvider,
                    manageURL: refusal.manageURL,
                    message: refusal.message ?? "This account already has a plan billed by \(StorePurchaseCopy.billedBy(refusal.billingProvider))."
                )
            case "owned_by_another_account":
                return .ownedByAnotherAccount(refusal.message ?? StorePurchaseCopy.ownedByAnother)
            case "first_purchase_limit":
                return .firstPurchaseLimit(refusal.message ?? "This purchase is over the limit for a first purchase.")
            case "store_billing_not_configured":
                // The server switched Apple off: hide the buttons.
                publish(items: [], products: [:], state: .notConfigured)
                return .failed(refusal.message ?? "Buying in the app isn't available yet.", canRetry: false)
            default:
                let fallback = preflight ? StorePurchaseCopy.preflightError : StorePurchaseCopy.serverError
                // After a purchase the server's own message ("Couldn't reach
                // the App Store") is less useful than "we'll keep trying".
                let message = preflight ? (refusal.message ?? fallback) : fallback
                return .failed(message, canRetry: refusal.isRetryable)
            }
        case .unreachable, .invalidResponse:
            return .failed(preflight ? StorePurchaseCopy.preflightError : StorePurchaseCopy.serverError, canRetry: true)
        }
    }

    // MARK: - Helpers

    nonisolated static func isCancellation(_ error: Error) -> Bool {
        if let storeKit = error as? StoreKitError, case .userCancelled = storeKit { return true }
        if let sk = error as? SKError, sk.code == .paymentCancelled { return true }
        return false
    }

    /// `production` for App Store installs, `sandbox` for TestFlight, Xcode
    /// and the StoreKit test environment. Unknown counts as production, so
    /// the server's first-purchase limit still applies.
    nonisolated static func currentEnvironment() async -> String {
        guard let result = try? await AppTransaction.shared else { return "production" }
        return result.unsafePayloadValue.environment == .production ? "production" : "sandbox"
    }
}
