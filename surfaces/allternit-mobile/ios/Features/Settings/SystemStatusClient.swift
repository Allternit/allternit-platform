import Foundation

/// Public system status for Settings. Reads `GET /api/v1/status.json` from
/// status.allternit.com (contract: services/status/contract/types.ts,
/// `allternit.status/v1`). Only `overall` is decoded; unknown values map to
/// `.unknown`. Any failure returns nil so Settings shows just the row.
enum SystemStatusLevel: Equatable, Sendable {
    case operational, degraded, down, maintenance, unknown

    init(raw: String) {
        switch raw {
        case "operational": self = .operational
        case "degraded": self = .degraded
        case "down": self = .down
        case "maintenance": self = .maintenance
        default: self = .unknown
        }
    }

    /// Plain wording, no uptime promise. Nil for `.unknown` (show nothing).
    var line: String? {
        switch self {
        case .operational: return "All systems operational"
        case .degraded: return "Some systems degraded"
        case .down: return "Major outage"
        case .maintenance: return "Scheduled maintenance"
        case .unknown: return nil
        }
    }

    /// Shape as well as colour, so state never rests on colour alone.
    var symbol: String {
        switch self {
        case .operational: return "checkmark.circle"
        case .degraded: return "exclamationmark.triangle"
        case .down: return "xmark.octagon"
        case .maintenance: return "wrench.and.screwdriver"
        case .unknown: return "questionmark.circle"
        }
    }
}

struct SystemStatusDoc: Decodable, Sendable {
    let schema: String
    let lastCheckedAt: String?
    let overallRaw: String

    private enum CodingKeys: String, CodingKey {
        case schema
        case lastCheckedAt = "last_checked_at"
        case overallRaw = "overall"
    }

    var overall: SystemStatusLevel { SystemStatusLevel(raw: overallRaw) }
}

enum SystemStatusClient {
    static let pageURL = URL(string: "https://status.allternit.com")!
    static let jsonURL = URL(string: "https://status.allternit.com/api/v1/status.json")!

    static func decode(_ data: Data) -> SystemStatusLevel? {
        guard let doc = try? JSONDecoder().decode(SystemStatusDoc.self, from: data),
              doc.schema == "allternit.status/v1" else { return nil }
        return doc.overall
    }

    /// 3 s timeout; nil on any failure (no DNS yet, offline, bad payload).
    static func fetch() async -> SystemStatusLevel? {
        var request = URLRequest(url: jsonURL, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 3)
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 3
        config.timeoutIntervalForResource = 3
        let session = URLSession(configuration: config)
        guard let (data, response) = try? await session.data(for: request),
              (response as? HTTPURLResponse)?.statusCode == 200 else { return nil }
        return decode(data)
    }
}
