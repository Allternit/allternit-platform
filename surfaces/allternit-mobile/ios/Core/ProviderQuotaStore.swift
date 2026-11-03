import Foundation

/// One rolling usage window of a provider account (5-hour, weekly, per-model weekly).
struct ProviderLimitWindow: Decodable, Sendable, Equatable, Identifiable {
    let id: String
    let label: String
    let usedRatio: Double
    let resetAt: String?

    var resetDate: Date? {
        guard let resetAt else { return nil }
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fractional.date(from: resetAt) ?? ISO8601DateFormatter().date(from: resetAt)
    }

    /// "5-hour", "Weekly", "Weekly · Fable" — same naming as the web (provider-limits.ts).
    var displayLabel: String {
        let text = "\(id) \(label)".lowercased()
        let base = id == "5h" ? "5-hour" : id.hasPrefix("7d") ? "Weekly" : id.hasPrefix("month") ? "Monthly" : label
        for family in ["opus", "sonnet", "haiku", "fable"] where text.contains(family) {
            return "\(base) · \(family.capitalized)"
        }
        return base
    }
}


/// `GET /api/v1/providers/:id/quota`: the account's plan name and usage
/// windows, relayed from the runtime (gizzi quota). Same source as the web's
/// Usage cards and model picker rings. Refreshed at most once a minute.
@MainActor
final class ProviderQuotaStore: ObservableObject {
    static let shared = ProviderQuotaStore()

    struct Quota: Equatable {
        var plan: String?
        var windows: [ProviderLimitWindow]
    }

    @Published private(set) var byProvider: [String: Quota] = [:]
    private var fetchedAt: [String: Date] = [:]

    private struct Wire: Decodable {
        struct Inner: Decodable {
            let plan: String?
            let windows: [ProviderLimitWindow]
        }
        let status: String
        let quota: Inner?
    }

    func load(_ providerIds: [String]) {
        for id in Set(providerIds) where id != "allternit" {
            if let at = fetchedAt[id], Date().timeIntervalSince(at) < 60 { continue }
            fetchedAt[id] = Date()
            Task { [weak self] in
                guard let wire: Wire = try? await APIClient.shared.get(path: "providers/\(id)/quota") else { return }
                guard wire.status == "ok", let quota = wire.quota else { return }
                self?.byProvider[id] = Quota(plan: quota.plan, windows: quota.windows.map {
                    ProviderLimitWindow(id: $0.id, label: $0.label, usedRatio: min(1, max(0, $0.usedRatio)), resetAt: $0.resetAt)
                })
            }
        }
    }

    /// The window closest to its limit.
    func worst(_ providerId: String) -> ProviderLimitWindow? {
        if providerId == "allternit" { return Self.cloudWindow() }
        return byProvider[providerId]?.windows.max { $0.usedRatio < $1.usedRatio }
    }

    /// Allternit Cloud's monthly allowance, from `me/usage`.
    static func cloudWindow() -> ProviderLimitWindow? {
        guard let percent = UsageStore.shared.percentUsed else { return nil }
        return ProviderLimitWindow(id: "month", label: "Monthly", usedRatio: min(1, max(0, percent / 100)), resetAt: nil)
    }
}
