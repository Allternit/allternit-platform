import Foundation

// -----------------------------------------------------------------------------
// Loop REST models — base path `v1/automations` on gizzi-code's own server
// (`cmd/gizzi-code/src/runtime/server/server.ts:367,455`), same host as
// CronClient/RoutinesClient/PtyClient/PermissionClient, NOT `allternit-api`.
//
// Mirrors the row shape built in the POST insert
// (`cmd/gizzi-code/src/runtime/server/routes/automations.ts:257-267`). Like
// `Routine`, the loop row's JS object literal keys are snake_case as written
// (`agent_id`, `exit_condition`, `max_iterations`, `iteration_log`,
// `time_created`, `time_updated`), so explicit `CodingKeys` map them.
//
// Timestamps are `Date.now()` ms-epoch numbers, same as Routine — decoded as
// `Double` and converted with `Date(timeIntervalSince1970: ms / 1000)` at
// render time (`RoutinesListView.relativeText`, reused here).
// -----------------------------------------------------------------------------

/// One entry of `iteration_log` — `LoopLogEntry` (`loop-engine.ts:6-11`),
/// confirmed by reading the engine source rather than assumed: pushed once
/// per iteration in `LoopEngine.startLoop` (`loop-engine.ts:39-45`) with
/// `output` already being `stdout + stderr` concatenated (`loop-engine.ts:
/// 41`), `exitCode` from the spawned process, and `timestamp` as an ISO-8601
/// string (`new Date().toISOString()`, unlike the row's own ms-epoch
/// timestamps). Unlike the row itself, `LoopLogEntry` is a plain TS
/// interface (not a drizzle-mapped column), so its own field names are
/// already camelCase on the wire — no `CodingKeys` needed here.
struct LoopIteration: Decodable, Sendable, Hashable {
    let iteration: Int
    let output: String
    let exitCode: Int
    let timestamp: String
}

/// Loop Card measurement (`metric` column). Present only on measured loops.
/// Wire keys are already camelCase (`LoopMetricSchema`, automations.ts).
struct LoopMetric: Codable, Sendable, Hashable {
    let command: String
    /// "lower" or "higher" is better.
    let direction: String
    /// Fractional gain needed to keep a change (0.05 = 5%).
    let keepBar: Double
    let noiseRuns: Int
    let minPairs: Int
}

/// One ledger row: an attempt, rejects included (`ledger` column,
/// loop-engine.ts `runMeasured`). `delta` is the median fractional change,
/// positive is better; null when the attempt could not be measured.
struct LoopLedgerRow: Decodable, Sendable, Hashable {
    let iteration: Int
    let hypothesis: String
    let delta: Double?
    let n: Int
    let noise: Double
    /// KEEP, MINOR, DROP or INCONCLUSIVE.
    let verdict: String
    let why: String
    /// ACTED, NOOP or BLOCKED.
    let outcome: String
}

/// A loop row (insert shape, automations.ts:257-267). `state` moves
/// running -> succeeded | max_iterations (loop-engine.ts:72-105), or can be
/// set to something else entirely by a `PUT` (e.g. a manual "stopped") —
/// rendered generically rather than assuming a closed set. There's no
/// dedicated pause/resume endpoint, same as Routine.
struct Loop: Decodable, Sendable, Identifiable, Hashable {
    let id: String
    let agentId: String?
    let command: String
    let exitCondition: String?
    let maxIterations: Int
    let iterationLog: [LoopIteration]
    let state: String
    let timeCreated: Double
    let timeUpdated: Double
    let metric: LoopMetric?
    let ledger: [LoopLedgerRow]?

    private enum CodingKeys: String, CodingKey {
        case id, command, state, metric, ledger
        case agentId = "agent_id"
        case exitCondition = "exit_condition"
        case maxIterations = "max_iterations"
        case iterationLog = "iteration_log"
        case timeCreated = "time_created"
        case timeUpdated = "time_updated"
    }
}

/// Body of `POST v1/automations/loops` (`LoopCreateSchema`,
/// automations.ts:33-39, narrowed to what `CreateLoopSheet` collects —
/// `id`/`agent_id` are left server-generated/unset). Wire keys are
/// snake_case per the zod schema, unlike Routine's create body (whose field
/// names happen to be identical in both cases).
struct CreateLoopRequest: Encodable, Sendable {
    let command: String
    let exitCondition: String?
    let maxIterations: Int
    /// Optional Loop Card; nil makes a plain loop.
    let metric: LoopMetric?

    private enum CodingKeys: String, CodingKey {
        case command, metric
        case exitCondition = "exit_condition"
        case maxIterations = "max_iterations"
    }
}
