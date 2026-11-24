import Foundation

/// A row of `GET /api/v1/memory/documents`
/// (cmd/allternit-api/src/memory_routes.rs — `MemoryDoc`). Field casing is
/// snake_case on the wire, matching the rusqlite column names.
struct MemoryDocument: Decodable, Sendable, Identifiable {
    let id: String
    let agentId: String?
    let title: String
    let sourceType: String
    let sourceUrl: String?
    let chunkCount: Int
    let isIndexed: Bool
    let createdAt: String
    let updatedAt: String

    enum CodingKeys: String, CodingKey {
        case id
        case agentId = "agent_id"
        case title
        case sourceType = "source_type"
        case sourceUrl = "source_url"
        case chunkCount = "chunk_count"
        case isIndexed = "is_indexed"
        case createdAt = "created_at"
        case updatedAt = "updated_at"
    }
}

/// `GET /api/v1/memory/stats` — counts across the memory stores. `memories`
/// is a nested `{ "total": N }` object on the wire; every field is optional
/// so older/newer builds with partial shapes still decode.
struct MemoryStats: Decodable, Sendable {
    let connections: Int?
    let insights: Int?
    let memories: MemoryCount?
    let vectors: Int?

    struct MemoryCount: Decodable, Sendable {
        let total: Int?
    }

    /// Flattened totals for the settings header.
    var memoryTotal: Int { memories?.total ?? 0 }
}

/// `GET /api/v1/memory/health` — subsystem reachability. Only `status` is
/// read by the UI ("healthy" / "degraded").
struct MemoryHealth: Decodable, Sendable {
    let status: String?
}

// MARK: - Allternit Sleep

/// `GET /api/v1/memory/sleep/status` (cmd/allternit-api/src/sleep/types.rs
/// `SleepStatus`). Every field is optional so a runtime with a partial shape
/// still decodes; the accessors give the defaults the UI reads.
struct SleepStatus: Decodable, Sendable, Equatable {
    var available: Bool?
    /// The master switch. A stage runs only when this and its own flag are on.
    var enabled: Bool?
    var stages: [String: Bool]?
    /// New memories since the last finished nap, over `debt_scale`. 1.0 = behind.
    var debt: Double?
    var lastCycle: SleepCycleSummary?
    var spendTodayUsd: Double?
    var dailySpendCapUsd: Double?
    var manifestId: String?
    var openDisputes: Int?

    enum CodingKeys: String, CodingKey {
        case available, enabled, stages, debt
        case lastCycle = "last_cycle"
        case spendTodayUsd = "spend_today_usd"
        case dailySpendCapUsd = "daily_spend_cap_usd"
        case manifestId = "manifest_id"
        case openDisputes = "open_disputes"
    }

    var isAvailable: Bool { available ?? true }
    var isEnabled: Bool { enabled ?? false }
    func stage(_ name: String) -> Bool { stages?[name] == true }
    var debtValue: Double { debt ?? 0 }
    var spendToday: Double { spendTodayUsd ?? 0 }
    var dailyCap: Double { dailySpendCapUsd ?? 0 }
}

/// sleep/types.rs `CycleSummary`: one sleep (a nap, daydream, deep or REM).
struct SleepCycleSummary: Decodable, Sendable, Equatable, Identifiable {
    let id: String
    var trigger: String?
    /// `daydream`, `nap`, `deep` or `rem`.
    var kind: String?
    /// `running`, `finished`, `stopped`, `failed` or `undone`.
    var state: String?
    /// The stage running now, or the last one that ran.
    var stage: String?
    var spendUsd: Double?
    var startedAt: String?
    var finishedAt: String?
    var undoneAt: String?
    /// One line on what the cycle did; nil while it runs.
    var summary: String?

    enum CodingKeys: String, CodingKey {
        case id, trigger, kind, state, stage, summary
        case spendUsd = "spend_usd"
        case startedAt = "started_at"
        case finishedAt = "finished_at"
        case undoneAt = "undone_at"
    }
}

/// A count in a stage report. Counts are numbers on the wire, but the map is
/// open (`Record<string, unknown>`), so anything else reads as 0 instead of
/// failing the whole report.
struct SleepCount: Decodable, Sendable, Equatable {
    let value: Double

    init(_ value: Double) { self.value = value }

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        value = (try? container.decode(Double.self)) ?? 0
    }
}

/// One stage of `GET /memory/sleep/cycles/:id` → `report.stages`.
struct SleepStageReport: Decodable, Sendable, Equatable {
    let stage: String
    /// `ran`, `skipped`, `failed`, `held` or `not_run`.
    var outcome: String?
    var reason: String?
    var spendUsd: Double?
    var counts: [String: SleepCount]?

    enum CodingKeys: String, CodingKey {
        case stage, outcome, reason, counts
        case spendUsd = "spend_usd"
    }

    func count(_ key: String) -> Int { Int(counts?[key]?.value ?? 0) }
}

struct SleepUndoInfo: Decodable, Sendable, Equatable {
    var available: Bool?
    var undoneAt: String?
    /// What this cycle did that has no undo (kept, not reversed).
    var cannotUndo: [String]?

    enum CodingKeys: String, CodingKey {
        case available
        case undoneAt = "undone_at"
        case cannotUndo = "cannot_undo"
    }
}

struct SleepCycleReport: Decodable, Sendable, Equatable {
    var cycleId: String?
    var kind: String?
    var state: String?
    var spendUsd: Double?
    var summary: String?
    var stopReason: String?
    var error: String?
    var stages: [SleepStageReport]?
    var undo: SleepUndoInfo?

    enum CodingKeys: String, CodingKey {
        case kind, state, summary, error, stages, undo
        case cycleId = "cycle_id"
        case spendUsd = "spend_usd"
        case stopReason = "stop_reason"
    }
}

/// `GET /memory/sleep/cycles/:id` → `{ cycle, report }`.
struct SleepCycleDetail: Decodable, Sendable, Equatable {
    let cycle: SleepCycleSummary
    var report: SleepCycleReport?
}

/// `PUT /memory/sleep/settings` answer (and `GET`).
struct SleepSettings: Decodable, Sendable, Equatable {
    var enabled: Bool?
    var stages: [String: Bool]?
    var dailySpendCapUsd: Double?

    enum CodingKeys: String, CodingKey {
        case enabled, stages
        case dailySpendCapUsd = "daily_spend_cap_usd"
    }
}

/// `PUT /memory/sleep/settings` body. Fields left nil are not sent, so a
/// change touches only what it names (the runtime writes it all-or-nothing).
struct SleepSettingsUpdate: Encodable, Sendable, Equatable {
    var enabled: Bool? = nil
    var stages: [String: Bool]? = nil
    var dailySpendCapUsd: Double? = nil

    enum CodingKeys: String, CodingKey {
        case enabled, stages
        case dailySpendCapUsd = "daily_spend_cap_usd"
    }
}

/// sleep/types.rs `DisputeSide`: one of two memories that disagree.
struct MemoryDisputeSide: Decodable, Sendable, Equatable {
    var kind: String?
    var id: String?
    var text: String?
    /// False once the side was replaced.
    var current: Bool?
}

/// sleep/types.rs `DisputeView`. Side a is the older memory, side b the newer.
struct MemoryDispute: Decodable, Sendable, Equatable, Identifiable {
    let id: String
    var topicId: String?
    let sideA: MemoryDisputeSide
    let sideB: MemoryDisputeSide
    var scope: String?
    var pick: String?
    var reason: String?
    var pickedBy: String?
    /// `open`, `needs_user` or `resolved`.
    var state: String?
    var createdAt: String?
    var resolvedAt: String?

    enum CodingKeys: String, CodingKey {
        case id, scope, pick, reason, state
        case topicId = "topic_id"
        case sideA = "side_a"
        case sideB = "side_b"
        case pickedBy = "picked_by"
        case createdAt = "created_at"
        case resolvedAt = "resolved_at"
    }
}

/// The four picks `POST /memory/disputes/:id/resolve` takes.
enum MemoryDisputeChoice: String, Encodable, Sendable, CaseIterable {
    case a
    case b
    case bothScoped = "both_scoped"
    case neither
}
