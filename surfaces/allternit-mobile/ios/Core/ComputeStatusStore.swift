import Foundation
import SwiftUI
import UIKit

// The compute gate (contract: CONTRACT-compute-gate-2026-11-13.md).
// A signed-in account with no computer does not reach agent work until it
// links its own Desktop (A), subscribes (B), buys credits (C) or confirms
// chat-only. Server truth: GET /api/v1/account/compute-status. Nothing here
// hard-codes a price; every number comes from `paths`.

struct ComputeStatus: Decodable, Equatable, Sendable {
    struct Computer: Decodable, Equatable, Sendable {
        let id: String
        let kind: String
        let provider: String?
        let status: String
        let sleepsWhenIdle: Bool?
        /// Plan computers with included hours (Plus). Absent on older servers.
        let hours: Hours?

        enum CodingKeys: String, CodingKey {
            case id, kind, provider, status, hours
            case sleepsWhenIdle = "sleeps_when_idle"
        }
    }

    /// `computer.hours` (platform account_compute.rs): this UTC month.
    struct Hours: Decodable, Equatable, Sendable {
        let includedHours: Int
        let usedHours: Double
        let overageHours: Double?
        let overagePricePerHourUsd: Double?
        let stoppedForHours: Bool?
        let resumesAt: String?

        enum CodingKeys: String, CodingKey {
            case includedHours = "included_hours"
            case usedHours = "used_hours"
            case overageHours = "overage_hours"
            case overagePricePerHourUsd = "overage_price_per_hour_usd"
            case stoppedForHours = "stopped_for_hours"
            case resumesAt = "resumes_at"
        }
    }

    struct Size: Decodable, Equatable, Sendable {
        let id: String
        let cpu: Int?
        let memoryGb: Int?
        let diskGb: Int?
        let pricePerHourUsd: Double?
        let stoppedPricePerHourUsd: Double?

        enum CodingKeys: String, CodingKey {
            case id, cpu
            case memoryGb = "memory_gb"
            case diskGb = "disk_gb"
            case pricePerHourUsd = "price_per_hour_usd"
            case stoppedPricePerHourUsd = "stopped_price_per_hour_usd"
        }
    }

    struct Credits: Decodable, Equatable, Sendable {
        let pricePerHourUsd: Double
        let stoppedPricePerHourUsd: Double?
        let balanceUsd: Double
        let minStartUsd: Double
        let sizes: [Size]?

        enum CodingKeys: String, CodingKey {
            case pricePerHourUsd = "price_per_hour_usd"
            case stoppedPricePerHourUsd = "stopped_price_per_hour_usd"
            case balanceUsd = "balance_usd"
            case minStartUsd = "min_start_usd"
            case sizes
        }
    }

    struct Own: Decodable, Equatable, Sendable {
        let downloadUrl: String?
        let pairingSupported: Bool?

        enum CodingKeys: String, CodingKey {
            case downloadUrl = "download_url"
            case pairingSupported = "pairing_supported"
        }
    }

    struct PlanOption: Decodable, Equatable, Sendable {
        let id: String
        let name: String
        let priceUsdMonth: Double
        let includedHours: Int?
        let overagePricePerHourUsd: Double?

        enum CodingKeys: String, CodingKey {
            case id, name
            case priceUsdMonth = "price_usd_month"
            case includedHours = "included_hours"
            case overagePricePerHourUsd = "overage_price_per_hour_usd"
        }
    }

    struct PlanPath: Decodable, Equatable, Sendable {
        let plans: [PlanOption]
    }

    struct Paths: Decodable, Equatable, Sendable {
        let own: Own?
        let plan: PlanPath?
        let credits: Credits?
    }

    /// none | paired | provisioning | hosted | suspended
    let state: String
    let online: Bool?
    let gate: Bool
    let acknowledgedAt: String?
    let computer: Computer?
    let paths: Paths?

    enum CodingKeys: String, CodingKey {
        case state, online, gate, computer, paths
        case acknowledgedAt = "acknowledged_at"
    }

    var isAcknowledged: Bool { acknowledgedAt != nil }
    var isSuspended: Bool { state == "suspended" }
}

enum ComputeGateError: Error, Equatable {
    /// 402 insufficient_credits; `minStartUsd` from the body.
    case insufficientCredits(minStartUsd: Double?)
    /// 410 free_computers_retired.
    case freeRetired
    case other(String)
}

@MainActor
final class ComputeStatusStore: ObservableObject {
    static let shared = ComputeStatusStore()

    @Published private(set) var status: ComputeStatus? = nil
    @Published private(set) var isLoading = false
    /// The check failed (404 until deployed, offline, signed out). The UI says
    /// "Couldn't check your computer." with Retry and never opens agent screens.
    @Published private(set) var loadFailed = false
    /// A 409 compute_required came back from an agent route: show the gate sheet.
    @Published var showGateSheet = false
    /// Set by views that wait on something (pairing, provisioning): polls every 5 s.
    @Published private(set) var isWaiting = false

    private var waiters = 0
    private var pollTask: Task<Void, Never>? = nil
    private var foregroundObserver: NSObjectProtocol? = nil

    private init() {
        foregroundObserver = NotificationCenter.default.addObserver(
            forName: UIApplication.willEnterForegroundNotification, object: nil, queue: .main
        ) { [weak self] _ in
            Task { @MainActor in await self?.refresh() }
        }
    }

    /// True when agent work must be blocked and the full gate shown.
    var needsFullGate: Bool {
        guard let s = status else { return false }
        return s.gate && !s.isAcknowledged
    }

    /// Gate is on but the person chose chat-only: show the small banner.
    var needsBanner: Bool {
        guard let s = status else { return false }
        return s.gate && s.isAcknowledged
    }

    func refresh() async {
        guard !isLoading else { return }
        isLoading = true
        defer { isLoading = false }
        do {
            let (status, data) = try await ComputeHTTP.call("GET", "api/v1/account/compute-status")
            guard (200...299).contains(status) else {
                loadFailed = true
                return
            }
            self.status = try JSONDecoder().decode(ComputeStatus.self, from: data)
            loadFailed = false
        } catch is CancellationError {
        } catch {
            loadFailed = true
        }
    }

    func refreshIfNeeded() async {
        if status == nil || loadFailed { await refresh() }
    }

    /// Chat-only: records the choice. The gate for agents stays on server-side.
    func acknowledgeChatOnly() async throws {
        let body = try JSONEncoder().encode(["choice": "chat_only"])
        let (code, _) = try await ComputeHTTP.call("POST", "api/v1/account/compute-status/acknowledge", body: body)
        guard (200...299).contains(code) else { throw ComputeGateError.other("Something went wrong. Try again.") }
        await refresh()
    }

    /// Path C: create or start the credit computer.
    func startCreditComputer(size: String) async throws {
        let body = try JSONEncoder().encode(["size": size])
        let (code, data) = try await ComputeHTTP.call("POST", "api/v1/provisioned-instances/credit", body: body)
        switch code {
        case 200...299:
            await refresh()
        case 402:
            let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
            let err = json?["error"] as? [String: Any]
            throw ComputeGateError.insufficientCredits(minStartUsd: err?["min_start_usd"] as? Double)
        case 410:
            throw ComputeGateError.freeRetired
        default:
            throw ComputeGateError.other("Something went wrong. Try again.")
        }
    }

    /// An agent route answered 409 compute_required: refresh and open the gate.
    func noteComputeRequired() {
        showGateSheet = true
        Task { await refresh() }
    }

    // MARK: - Waiting (5 s poll, only while a view waits)

    func beginWaiting() {
        waiters += 1
        isWaiting = true
        guard pollTask == nil else { return }
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 5_000_000_000)
                if Task.isCancelled { break }
                await self?.refresh()
            }
        }
    }

    func endWaiting() {
        waiters = max(0, waiters - 1)
        guard waiters == 0 else { return }
        isWaiting = false
        pollTask?.cancel()
        pollTask = nil
    }
}

/// One cloud-api call that hands back the status code and body, so the caller
/// can read 402/409/410 bodies. Straight to cloud-api, same auth as the rest.
enum ComputeHTTP {
    static func call(_ method: String, _ path: String, body: Data? = nil) async throws -> (Int, Data) {
        var request = try await APIClient.shared.authorizedRequest(
            url: AppConfig.cloudAPIBaseURL.appendingPathComponent(path), method: method)
        request.timeoutInterval = 20
        if let body { request.httpBody = body }
        let (data, response) = try await APIClient.shared.session.data(for: request)
        return ((response as? HTTPURLResponse)?.statusCode ?? 0, data)
    }

    /// `error.code` of a JSON error body, if any.
    static func errorCode(_ data: Data?) -> String? {
        guard let data,
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let err = json["error"] as? [String: Any] else { return nil }
        return err["code"] as? String
    }
}
