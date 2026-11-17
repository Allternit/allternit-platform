import Foundation

// The phone's side of the store purchase contract (cloud-api
// routes/billing_store.rs, docs api/cloud-billing.mdx "Store purchases").
// The server never trusts the phone: it reads every transaction from the App
// Store Server API itself and grants credits once.
//
// - GET  /billing/plans, /billing/packs   `store.apple.{enabled, product_id}`
// - POST /billing/store/preflight         `{account_token, product_id}` or 409/403/503
// - POST /billing/store/apple/transactions `{granted, reason?, subscription?, balance_usd?, idempotent_replay}`

/// What a store item is on the server.
enum StoreKind: String, Sendable, Codable, Identifiable {
    case plan
    case pack

    var id: String { rawValue }
}

/// One plan or credit pack the App Store can sell right now: the server has
/// Apple switched on for it (`store.apple.enabled`). Prices come from
/// StoreKit, never from here (D11).
struct StoreCatalogItem: Identifiable, Equatable, Sendable {
    let kind: StoreKind
    /// Server id: `plus`, `super` (Max), `ultra`, `credits_10` …
    let id: String
    let label: String
    /// `com.allternit.mobile.plan.plus.monthly`, `com.allternit.mobile.credits.10` …
    let productId: String
    /// Monthly credits for a plan, the pack's credits for a pack.
    let creditsUsd: Double
    /// Plans only: how far unused credits roll over.
    var rolloverCapUsd: Double? = nil
}

/// `POST /billing/store/preflight` 200.
struct StorePreflight: Decodable, Equatable, Sendable {
    /// The account's UUID, passed to StoreKit as `appAccountToken` (D4).
    let accountToken: String
    let productId: String

    enum CodingKeys: String, CodingKey {
        case accountToken = "account_token"
        case productId = "product_id"
    }
}

/// The plan as the purchase answer reports it.
struct StorePlanState: Decodable, Equatable, Sendable {
    let planId: String
    let label: String
    let status: String
    let billingProvider: String?
    var renewsAt: String? = nil
    var manageUrl: String? = nil
    var conflict: Bool = false

    enum CodingKeys: String, CodingKey {
        case planId = "plan_id"
        case label, status
        case billingProvider = "billing_provider"
        case renewsAt = "renews_at"
        case manageUrl = "manage_url"
        case conflict
    }
}

/// `POST /billing/store/apple/transactions` 200. Every 200 means the server
/// recorded the transaction, so the phone finishes it, `granted` or not.
struct StorePurchaseAnswer: Decodable, Equatable, Sendable {
    let granted: Bool
    /// When nothing was granted: `sandbox_not_granted`, `refunded`,
    /// `unknown_account`, `account_closed`.
    var reason: String? = nil
    var message: String? = nil
    var subscription: StorePlanState? = nil
    var balanceUsd: Double? = nil
    var idempotentReplay: Bool = false

    enum CodingKeys: String, CodingKey {
        case granted, reason, message, subscription
        case balanceUsd = "balance_usd"
        case idempotentReplay = "idempotent_replay"
    }

    init(granted: Bool, reason: String? = nil, message: String? = nil, subscription: StorePlanState? = nil,
         balanceUsd: Double? = nil, idempotentReplay: Bool = false) {
        self.granted = granted
        self.reason = reason
        self.message = message
        self.subscription = subscription
        self.balanceUsd = balanceUsd
        self.idempotentReplay = idempotentReplay
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        granted = try c.decodeIfPresent(Bool.self, forKey: .granted) ?? false
        reason = try c.decodeIfPresent(String.self, forKey: .reason)
        message = try c.decodeIfPresent(String.self, forKey: .message)
        subscription = try c.decodeIfPresent(StorePlanState.self, forKey: .subscription)
        balanceUsd = try c.decodeIfPresent(Double.self, forKey: .balanceUsd)
        idempotentReplay = try c.decodeIfPresent(Bool.self, forKey: .idempotentReplay) ?? false
    }
}

/// A non-2xx answer from a store route: `{ "error": "<code>", "message": … }`
/// plus the 409 `already_subscribed` fields.
struct StoreRefusal: Equatable, Sendable {
    let status: Int
    /// `already_subscribed`, `owned_by_another_account`, `first_purchase_limit`,
    /// `store_billing_not_configured`, `store_unavailable`, … or empty.
    let code: String
    var message: String? = nil
    var billingProvider: String? = nil
    var manageURL: URL? = nil
    var currentPlanId: String? = nil

    /// Worth trying again later with the same transaction (5xx, a store or
    /// network outage). 4xx answers won't change on retry.
    var isRetryable: Bool { status >= 500 || status == 429 || status == 408 }

    static func parse(status: Int, body: Data?) -> StoreRefusal {
        struct Wire: Decodable {
            let error: String?
            let message: String?
            let billingProvider: String?
            let manageUrl: String?
            let currentPlanId: String?
            enum CodingKeys: String, CodingKey {
                case error, message
                case billingProvider = "billing_provider"
                case manageUrl = "manage_url"
                case currentPlanId = "current_plan_id"
            }
        }
        let wire = body.flatMap { try? JSONDecoder().decode(Wire.self, from: $0) }
        return StoreRefusal(
            status: status,
            code: wire?.error ?? "",
            message: wire?.message,
            billingProvider: wire?.billingProvider,
            manageURL: wire?.manageUrl.flatMap(URL.init(string:)),
            currentPlanId: wire?.currentPlanId
        )
    }
}

enum StoreBillingError: Error, Equatable, Sendable {
    /// 401: no Allternit session.
    case signedOut
    case refused(StoreRefusal)
    /// The network or the API route is down.
    case unreachable(String)
    case invalidResponse

    var isRetryable: Bool {
        switch self {
        case .signedOut: return false
        case .refused(let refusal): return refusal.isRetryable
        case .unreachable, .invalidResponse: return true
        }
    }
}

/// The calls StorePurchaseManager makes. A protocol so tests run the real
/// StoreKit flow against a fake server.
protocol StoreBillingClienting: Sendable {
    /// Plans and packs with Apple switched on. Empty when the server has no
    /// Apple secrets yet (dark by default, D10).
    func catalog() async throws -> [StoreCatalogItem]
    func preflight(kind: StoreKind, id: String, environment: String) async throws -> StorePreflight
    func postAppleTransaction(id: String) async throws -> StorePurchaseAnswer
}

/// The real client on the app's authenticated API client (same token and
/// route as AccountPlanStore's billing reads).
struct StoreBillingClient: StoreBillingClienting {
    var api: APIClient = .shared

    func catalog() async throws -> [StoreCatalogItem] {
        async let plans: PlansWire = get("billing/plans")
        async let packs: PacksWire = get("billing/packs")
        let (p, k) = try await (plans, packs)
        return Self.items(plans: p, packs: k)
    }

    func preflight(kind: StoreKind, id: String, environment: String) async throws -> StorePreflight {
        try await post("billing/store/preflight", body: PreflightBody(provider: "apple", kind: kind.rawValue, id: id, environment: environment))
    }

    func postAppleTransaction(id: String) async throws -> StorePurchaseAnswer {
        try await post("billing/store/apple/transactions", body: TransactionBody(transactionId: id))
    }

    // MARK: Wire

    struct OfferWire: Decodable, Sendable {
        let enabled: Bool
        let productId: String
        enum CodingKeys: String, CodingKey {
            case enabled
            case productId = "product_id"
        }
    }

    struct OffersWire: Decodable, Sendable {
        let apple: OfferWire?
    }

    struct PlanWire: Decodable, Sendable {
        let id: String
        let label: String
        let monthlyCreditsUsd: Double
        let rolloverCapUsd: Double?
        let store: OffersWire?
        enum CodingKeys: String, CodingKey {
            case id, label, store
            case monthlyCreditsUsd = "monthly_credits_usd"
            case rolloverCapUsd = "rollover_cap_usd"
        }
    }

    struct PackWire: Decodable, Sendable {
        let id: String
        let label: String
        let creditsUsd: Double
        let store: OffersWire?
        enum CodingKeys: String, CodingKey {
            case id, label, store
            case creditsUsd = "credits_usd"
        }
    }

    struct PlansWire: Decodable, Sendable { let plans: [PlanWire] }
    struct PacksWire: Decodable, Sendable { let packs: [PackWire] }

    private struct PreflightBody: Encodable {
        let provider: String
        let kind: String
        let id: String
        let environment: String
    }

    private struct TransactionBody: Encodable {
        let transactionId: String
        enum CodingKeys: String, CodingKey { case transactionId = "transaction_id" }
    }

    /// Only items Apple is switched on for. An old server sends no `store`
    /// block: nothing is sold in the app.
    static func items(plans: PlansWire, packs: PacksWire) -> [StoreCatalogItem] {
        let planItems = plans.plans.compactMap { plan -> StoreCatalogItem? in
            guard let apple = plan.store?.apple, apple.enabled, !apple.productId.isEmpty else { return nil }
            return StoreCatalogItem(kind: .plan, id: plan.id, label: plan.label, productId: apple.productId,
                                    creditsUsd: plan.monthlyCreditsUsd, rolloverCapUsd: plan.rolloverCapUsd)
        }
        let packItems = packs.packs.compactMap { pack -> StoreCatalogItem? in
            guard let apple = pack.store?.apple, apple.enabled, !apple.productId.isEmpty else { return nil }
            return StoreCatalogItem(kind: .pack, id: pack.id, label: pack.label, productId: apple.productId,
                                    creditsUsd: pack.creditsUsd)
        }
        return planItems + packItems
    }

    // MARK: Transport (keeps the error body: APIClient.validate drops it)

    private func get<T: Decodable>(_ path: String) async throws -> T {
        let request: URLRequest
        do { request = try await api.authorizedRequest(path: path) } catch { throw StoreBillingError.unreachable(error.localizedDescription) }
        return try await send(request)
    }

    private func post<T: Decodable, B: Encodable>(_ path: String, body: B) async throws -> T {
        var request: URLRequest
        do { request = try await api.authorizedRequest(path: path, method: "POST") } catch { throw StoreBillingError.unreachable(error.localizedDescription) }
        request.httpBody = try JSONEncoder().encode(body)
        return try await send(request)
    }

    private func send<T: Decodable>(_ request: URLRequest) async throws -> T {
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await api.send(request)
        } catch let error as APIError {
            if case .httpError(let status, let message) = error {
                throw status == 401 ? StoreBillingError.signedOut
                    : StoreBillingError.refused(StoreRefusal(status: status, code: "", message: message))
            }
            throw StoreBillingError.unreachable(error.localizedDescription)
        } catch {
            throw StoreBillingError.unreachable(error.localizedDescription)
        }
        guard let http = response as? HTTPURLResponse else { throw StoreBillingError.invalidResponse }
        guard (200...299).contains(http.statusCode) else {
            NSLog("[StoreBilling] %d %@", http.statusCode, http.url?.path ?? "-")
            if http.statusCode == 401 { throw StoreBillingError.signedOut }
            throw StoreBillingError.refused(StoreRefusal.parse(status: http.statusCode, body: data))
        }
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw StoreBillingError.invalidResponse
        }
    }
}
