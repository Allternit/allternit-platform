import StoreKit
import StoreKitTest
import XCTest
@testable import Allternit

/// The server, faked at the network edge. StoreKit is real: an SKTestSession
/// on Config/Allternit.storekit (the 7 App Store Connect products).
actor FakeStoreBilling: StoreBillingClienting {
    static let plus = StoreCatalogItem(kind: .plan, id: "plus", label: "Plus",
                                       productId: "com.allternit.mobile.plan.plus.monthly", creditsUsd: 22, rolloverCapUsd: 44)
    static let pack10 = StoreCatalogItem(kind: .pack, id: "credits_10", label: "$10",
                                         productId: "com.allternit.mobile.credits.10", creditsUsd: 10)

    let token = UUID()
    private var items: [StoreCatalogItem] = [FakeStoreBilling.plus, FakeStoreBilling.pack10]
    private var preflightError: StoreBillingError?
    private var postFailures: [StoreBillingError] = []
    private var answers: [StorePurchaseAnswer] = []
    private(set) var preflights: [String] = []
    private(set) var posted: [String] = []

    func catalog() async throws -> [StoreCatalogItem] { items }

    func preflight(kind: StoreKind, id: String, environment: String) async throws -> StorePreflight {
        preflights.append("\(kind.rawValue):\(id):\(environment)")
        if let preflightError { throw preflightError }
        let productId = items.first { $0.kind == kind && $0.id == id }?.productId ?? ""
        return StorePreflight(accountToken: token.uuidString, productId: productId)
    }

    func postAppleTransaction(id: String) async throws -> StorePurchaseAnswer {
        posted.append(id)
        if !postFailures.isEmpty { throw postFailures.removeFirst() }
        return answers.isEmpty ? StorePurchaseAnswer(granted: true) : answers.removeFirst()
    }

    func refusePreflight(_ error: StoreBillingError) { preflightError = error }
    func failNextPosts(_ errors: [StoreBillingError]) { postFailures = errors }
    func answerNext(_ next: [StorePurchaseAnswer]) { answers = next }
    func sellNothing() { items = [] }
}

@MainActor
final class StorePurchaseTests: XCTestCase {
    private var session: SKTestSession!
    private var fake: FakeStoreBilling!
    private var accountReloads = 0

    override func setUp() async throws {
        let url = try XCTUnwrap(Bundle(for: Self.self).url(forResource: "Allternit", withExtension: "storekit"))
        session = try SKTestSession(contentsOf: url)
        session.resetToDefaultState()
        session.disableDialogs = true
        session.clearTransactions()
        fake = FakeStoreBilling()
        accountReloads = 0
    }

    override func tearDown() async throws {
        session.clearTransactions()
        session = nil
    }

    private func makeManager(signedIn: Bool = true) async -> StorePurchaseManager {
        let manager = StorePurchaseManager(
            client: fake,
            isSignedIn: { signedIn },
            environment: { "sandbox" },
            onAccountChanged: { [weak self] in self?.accountReloads += 1 }
        )
        await manager.loadCatalog(force: true)
        return manager
    }

    private func unfinishedIds() async -> [UInt64] {
        var ids: [UInt64] = []
        for await result in StoreKit.Transaction.unfinished {
            if case .verified(let transaction) = result { ids.append(transaction.id) }
        }
        return ids
    }

    private func latest(_ productId: String) async throws -> StoreKit.Transaction {
        let result = await StoreKit.Transaction.latest(for: productId)
        guard case .verified(let transaction)? = result else {
            XCTFail("no verified transaction for \(productId)")
            throw CancellationError()
        }
        return transaction
    }

    private func waitUntil(_ what: String, timeout: TimeInterval = 10, _ condition: () async -> Bool) async throws {
        let end = Date().addingTimeInterval(timeout)
        while Date() < end {
            if await condition() { return }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        XCTFail("timed out waiting for \(what)")
    }

    // MARK: - Catalog

    func testCatalogShowsOnlyProductsTheServerEnablesAndStoreKitPrices() async {
        let manager = await makeManager()
        XCTAssertEqual(manager.catalogState, .ready)
        XCTAssertEqual(manager.items.map(\.id), ["plus", "credits_10"])
        // D11: the App Store's price, not the server's.
        XCTAssertEqual(manager.displayPrice(FakeStoreBilling.plus), "$24.99")
        XCTAssertEqual(manager.displayPrice(FakeStoreBilling.pack10), "$11.99")
        XCTAssertTrue(StorefrontGate.shared.showsStoreButtons(for: .plan))
        XCTAssertTrue(StorefrontGate.shared.showsStoreButtons(for: .pack))
    }

    func testServerWithAppleOffHidesStoreButtons() async {
        await fake.sellNothing()
        let manager = await makeManager()
        XCTAssertEqual(manager.catalogState, .notConfigured)
        XCTAssertTrue(manager.items.isEmpty)
        XCTAssertFalse(StorefrontGate.shared.showsStoreButtons)
    }

    func testCatalogWireKeepsOnlyAppleEnabledItems() throws {
        let plans = try JSONDecoder().decode(StoreBillingClient.PlansWire.self, from: Data(#"""
        {"plans":[
          {"id":"plus","label":"Plus","price_usd":20,"monthly_credits_usd":22,"rollover_cap_usd":44,
           "store":{"reference_price_usd":24.99,"apple":{"enabled":true,"product_id":"com.allternit.mobile.plan.plus.monthly"},"google":{"enabled":false,"product_id":"plan_plus"}}},
          {"id":"super","label":"Max","price_usd":100,"monthly_credits_usd":110,"rollover_cap_usd":220,
           "store":{"reference_price_usd":124.99,"apple":{"enabled":false,"product_id":"com.allternit.mobile.plan.max.monthly"}}},
          {"id":"ultra","label":"Ultra","price_usd":200,"monthly_credits_usd":220,"rollover_cap_usd":440}
        ]}
        """#.utf8))
        let packs = try JSONDecoder().decode(StoreBillingClient.PacksWire.self, from: Data(#"""
        {"packs":[{"id":"credits_10","label":"$10","credits_usd":10,"price_usd":10,
          "store":{"reference_price_usd":11.99,"apple":{"enabled":true,"product_id":"com.allternit.mobile.credits.10"}}}]}
        """#.utf8))
        let items = StoreBillingClient.items(plans: plans, packs: packs)
        XCTAssertEqual(items.map(\.id), ["plus", "credits_10"])
        XCTAssertEqual(items.first?.rolloverCapUsd, 44)
    }

    // MARK: - Buy

    func testBuyPostsTheTransactionWithTheAccountTokenThenFinishesIt() async throws {
        await fake.answerNext([StorePurchaseAnswer(
            granted: true,
            subscription: StorePlanState(planId: "plus", label: "Plus", status: "active", billingProvider: "apple"))])
        let manager = await makeManager()

        await manager.purchase(FakeStoreBilling.plus)

        let transaction = try await latest(FakeStoreBilling.plus.productId)
        // D4: the server's account token rides on the purchase.
        XCTAssertEqual(transaction.appAccountToken, fake.token)
        let preflights = await fake.preflights
        XCTAssertEqual(preflights, ["plan:plus:sandbox"])
        let posted = await fake.posted
        XCTAssertEqual(posted, [String(transaction.id)])
        XCTAssertEqual(manager.notice, .purchased("Plus is active. Its credits are on your account."))
        let unfinished = await unfinishedIds()
        XCTAssertFalse(unfinished.contains(transaction.id), "finished after the server's 200")
        XCTAssertEqual(accountReloads, 1)
    }

    func testSandboxPurchaseOutsideTheAllowListIsFinishedWithANotice() async throws {
        await fake.answerNext([StorePurchaseAnswer(granted: false, reason: "sandbox_not_granted",
                                                   message: "Test purchase. No credits were added.")])
        let manager = await makeManager()

        await manager.purchase(FakeStoreBilling.pack10)

        XCTAssertEqual(manager.notice, .sandboxNotGranted)
        XCTAssertEqual(StorePurchaseCopy.text(manager.notice!), "Test purchase. No credits were added.")
        let transaction = try await latest(FakeStoreBilling.pack10.productId)
        let unfinished = await unfinishedIds()
        XCTAssertFalse(unfinished.contains(transaction.id), "a 200 with granted:false is still finished")
    }

    func testServerErrorKeepsTheTransactionUnfinishedAndRetryDeliversIt() async throws {
        await fake.failNextPosts([.refused(StoreRefusal(status: 502, code: "store_unavailable"))])
        let manager = await makeManager()

        await manager.purchase(FakeStoreBilling.pack10)

        XCTAssertEqual(manager.notice, .failed(StorePurchaseCopy.serverError, canRetry: true))
        XCTAssertTrue(manager.canRetry)
        let transaction = try await latest(FakeStoreBilling.pack10.productId)
        var unfinished = await unfinishedIds()
        XCTAssertTrue(unfinished.contains(transaction.id), "not finished without a 2xx")

        await manager.retry()

        XCTAssertEqual(manager.notice, .purchased("$10 in credits added to your account."))
        unfinished = await unfinishedIds()
        XCTAssertFalse(unfinished.contains(transaction.id))
        let posted = await fake.posted
        XCTAssertEqual(posted, [String(transaction.id), String(transaction.id)])
    }

    func testAlreadySubscribedOnTheWebStopsBeforeTheAppStoreCharges() async {
        await fake.refusePreflight(.refused(StoreRefusal(
            status: 409, code: "already_subscribed",
            message: "This account already has a plan. Change or cancel it in Manage billing.",
            billingProvider: "stripe", manageURL: URL(string: "https://platform.allternit.com/billing"), currentPlanId: "plus")))
        let manager = await makeManager()

        await manager.purchase(FakeStoreBilling.plus)

        XCTAssertEqual(manager.notice, .alreadySubscribed(
            provider: "stripe", manageURL: URL(string: "https://platform.allternit.com/billing"),
            message: "This account already has a plan. Change or cancel it in Manage billing."))
        XCTAssertTrue(session.allTransactions().isEmpty, "no App Store purchase was made")
        let posted = await fake.posted
        XCTAssertTrue(posted.isEmpty)
    }

    func testSignedOutAsksToSignInAndBuysNothing() async {
        let manager = await makeManager(signedIn: false)

        await manager.purchase(FakeStoreBilling.plus)

        XCTAssertEqual(manager.notice, .signedOut)
        let preflights = await fake.preflights
        XCTAssertTrue(preflights.isEmpty)
        XCTAssertTrue(session.allTransactions().isEmpty)
    }

    // MARK: - Cancel

    func testCancelledPurchasePostsNothing() async {
        session.failTransactionsEnabled = true
        session.failureError = .paymentCancelled
        let manager = await makeManager()

        await manager.purchase(FakeStoreBilling.pack10)

        XCTAssertEqual(manager.notice, .cancelled)
        let posted = await fake.posted
        XCTAssertTrue(posted.isEmpty)
    }

    // MARK: - Pending (Ask to Buy)

    func testAskToBuyIsPendingThenTheApprovalIsPostedFromUpdates() async throws {
        session.askToBuyEnabled = true
        let manager = await makeManager()
        manager.start()
        defer { manager.stop() }

        await manager.purchase(FakeStoreBilling.pack10)

        XCTAssertEqual(manager.notice, .pending)
        var posted = await fake.posted
        XCTAssertTrue(posted.isEmpty, "nothing to post until a parent approves")

        let pending = try XCTUnwrap(session.allTransactions().last)
        try session.approveAskToBuyTransaction(identifier: pending.identifier)

        try await waitUntil("the approved purchase to be posted") { await !self.fake.posted.isEmpty }
        try await waitUntil("the success notice") { manager.notice == .purchased("$10 in credits added to your account.") }
        posted = await fake.posted
        XCTAssertEqual(posted.count, 1)
    }

    // MARK: - Restore

    func testRestoreReDeliversAPlanTheServerMissed() async throws {
        await fake.failNextPosts([.unreachable("offline")])
        let manager = await makeManager()
        await manager.purchase(FakeStoreBilling.plus)
        let transaction = try await latest(FakeStoreBilling.plus.productId)
        var unfinished = await unfinishedIds()
        XCTAssertTrue(unfinished.contains(transaction.id))

        await manager.restore()

        XCTAssertEqual(manager.notice, .restored(1))
        let posted = await fake.posted
        XCTAssertEqual(posted, [String(transaction.id), String(transaction.id)])
        unfinished = await unfinishedIds()
        XCTAssertFalse(unfinished.contains(transaction.id))
    }

    func testRestoreWithNothingBoughtSaysSo() async {
        let manager = await makeManager()

        await manager.restore()

        XCTAssertEqual(manager.notice, .nothingToRestore)
    }

    // MARK: - Refund

    func testRefundIsPostedSoTheServerTakesTheCreditsBack() async throws {
        let manager = await makeManager()
        await manager.purchase(FakeStoreBilling.pack10)
        let transaction = try await latest(FakeStoreBilling.pack10.productId)
        XCTAssertEqual(accountReloads, 1)
        manager.start()
        defer { manager.stop() }
        await fake.answerNext([StorePurchaseAnswer(granted: false, reason: "refunded")])

        let bought = try XCTUnwrap(session.allTransactions().first { $0.productIdentifier == FakeStoreBilling.pack10.productId })
        try session.refundTransaction(identifier: bought.identifier)

        try await waitUntil("the refund to be posted") { await self.fake.posted.count == 2 }
        let posted = await fake.posted
        XCTAssertEqual(posted, [String(transaction.id), String(transaction.id)])
        // The balance is read again after the server removed the credits.
        try await waitUntil("the account to reload") { self.accountReloads == 2 }
    }

    // MARK: - Wire

    func testAlreadySubscribedRefusalCarriesWhoBillsAndTheManageLink() {
        let refusal = StoreRefusal.parse(status: 409, body: Data(#"""
        {"error":"already_subscribed","current_plan_id":"plus","billing_provider":"apple",
         "manage_url":"https://apps.apple.com/account/subscriptions",
         "message":"This account already has a plan billed by Apple. Change or cancel it in your App Store subscriptions."}
        """#.utf8))
        XCTAssertEqual(refusal.code, "already_subscribed")
        XCTAssertEqual(refusal.billingProvider, "apple")
        XCTAssertEqual(refusal.manageURL, URL(string: "https://apps.apple.com/account/subscriptions"))
        XCTAssertEqual(refusal.currentPlanId, "plus")
        XCTAssertFalse(refusal.isRetryable)
        XCTAssertTrue(StoreRefusal.parse(status: 502, body: Data(#"{"error":"store_unavailable"}"#.utf8)).isRetryable)
    }

    func testPlanPurchaseIsBlockedWhileTheWebOrGoogleBillsAPlan() {
        XCTAssertTrue(StorePurchaseManager.planPurchaseAllowed(nil))
        XCTAssertTrue(StorePurchaseManager.planPurchaseAllowed(BillingSubscription(planId: "free", label: "Free", planTier: "free", status: "none")))
        XCTAssertTrue(StorePurchaseManager.planPurchaseAllowed(BillingSubscription(planId: "plus", label: "Plus", planTier: "plus", status: "active", billingProvider: "apple")))
        XCTAssertFalse(StorePurchaseManager.planPurchaseAllowed(BillingSubscription(planId: "plus", label: "Plus", planTier: "plus", status: "active", billingProvider: "stripe")))
        XCTAssertFalse(StorePurchaseManager.planPurchaseAllowed(BillingSubscription(planId: "super", label: "Max", planTier: "max", status: "past_due", billingProvider: "google")))
    }
}
