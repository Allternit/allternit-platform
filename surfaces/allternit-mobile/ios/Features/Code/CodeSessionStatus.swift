import Foundation

// Code session status for the Code sessions view (phone calm I9, brief item
// 7, contract C6). Pure: no store, no network, so tests need nothing.
//
// Mirrors the web's two modules:
// - `src/lib/code-session-status.ts` `codeSessionFilterStatus`
// - `src/components/session-overview/code-session-filter.ts` (filters,
//   labels, empty texts, counts)
//
// The server owns `needsInput` and `readyForReview` (allternit-api
// `code_session_status.rs`, read on iOS as `AgentSession.needsInput` /
// `.readyForReview`). Order, first match wins: Archived, Needs input,
// Working, Ready for review, Done, then idle.

/// One session's C6 status.
enum CodeSessionFilterStatus: String, CaseIterable, Sendable {
    case archived
    case needsInput = "needs_input"
    case working
    case readyForReview = "ready_for_review"
    case done
    case idle

    /// The row's status word ("Working · 2m"). Idle has none
    /// (`CODE_SESSION_STATUS_LABEL`).
    var label: String {
        switch self {
        case .archived: return "Archived"
        case .needsInput: return "Needs input"
        case .working: return "Working"
        case .readyForReview: return "Ready for review"
        case .done: return "Done"
        case .idle: return ""
        }
    }

    /// The row icon (SF Symbol). Working draws a spinner instead, so it has
    /// none here; idle and archived draw a plain dot.
    var systemImage: String? {
        switch self {
        case .needsInput: return "hand.raised.fill"
        case .readyForReview: return "eye"
        case .done: return "checkmark.circle"
        case .working, .archived, .idle: return nil
        }
    }

    /// The drawer's recents show only the two states that ask something of
    /// the user (step 25 already draws Working as the spinner).
    var asksForUser: Bool { self == .needsInput || self == .readyForReview }
}

/// The stored signals a status is read from. On iOS, archive is
/// `PATCH active:false` as well as `metadata.archived`, so both count.
struct CodeSessionSignals: Equatable, Sendable {
    var archived: Bool = false
    var needsInput: Bool = false
    var readyForReview: Bool = false
    var completed: Bool = false
}

/// `codeSessionFilterStatus(session, { streaming })`.
func codeSessionFilterStatus(_ signals: CodeSessionSignals, streaming: Bool = false) -> CodeSessionFilterStatus {
    if signals.archived { return .archived }
    if signals.needsInput { return .needsInput }
    if streaming { return .working }
    if signals.readyForReview { return .readyForReview }
    if signals.completed { return .done }
    return .idle
}

extension AgentSession {
    /// The C6 signals on this session.
    var codeSignals: CodeSessionSignals {
        CodeSessionSignals(archived: archived || !active,
                           needsInput: needsInput,
                           readyForReview: readyForReview,
                           completed: completed)
    }
}

/// `codeSessionFilterStatus` for a session record.
func codeSessionFilterStatus(_ session: AgentSession, streaming: Bool = false) -> CodeSessionFilterStatus {
    codeSessionFilterStatus(session.codeSignals, streaming: streaming)
}

// MARK: - Filters

/// The filter menu (`CodeSessionFilter`), in the mockup's order: All first,
/// Archived last. "All" is every session that is not archived (idle ones
/// too); every other filter is exactly one status.
enum CodeSessionFilter: String, CaseIterable, Identifiable, Sendable {
    case all
    case needsInput = "needs_input"
    case readyForReview = "ready_for_review"
    case working
    case done
    case archived

    var id: String { rawValue }

    /// `CODE_SESSION_FILTER_LABEL`
    var label: String {
        switch self {
        case .all: return "All"
        case .needsInput: return "Needs input"
        case .readyForReview: return "Ready for review"
        case .working: return "Working"
        case .done: return "Done"
        case .archived: return "Archived"
        }
    }

    /// What the empty list says under each filter (`CODE_SESSION_FILTER_EMPTY`).
    var emptyText: String {
        switch self {
        case .all: return "No code sessions yet. Start one and it shows here."
        case .needsInput: return "No session is waiting on you."
        case .readyForReview: return "Nothing to review."
        case .working: return "No session is working right now."
        case .done: return "No finished sessions."
        case .archived: return "Nothing archived."
        }
    }

    /// `matchesCodeSessionFilter`
    func matches(_ status: CodeSessionFilterStatus) -> Bool {
        switch self {
        case .all: return status != .archived
        case .needsInput: return status == .needsInput
        case .readyForReview: return status == .readyForReview
        case .working: return status == .working
        case .done: return status == .done
        case .archived: return status == .archived
        }
    }

    /// A launch argument's value: the web id ("needs_input"), or the same
    /// with dashes, camel case or the label ("Needs input"). nil when unknown.
    init?(argument: String) {
        let key = argument.lowercased()
            .replacingOccurrences(of: "-", with: "_")
            .replacingOccurrences(of: " ", with: "_")
        switch key {
        case "all": self = .all
        case "needs_input", "needsinput": self = .needsInput
        case "ready_for_review", "readyforreview", "review": self = .readyForReview
        case "working": self = .working
        case "done": self = .done
        case "archived": self = .archived
        default: return nil
        }
    }
}

/// Rows per filter (`CodeSessionFilterCounts`). A filter with no rows reads 0.
struct CodeSessionFilterCounts: Equatable, Sendable {
    private(set) var values: [CodeSessionFilter: Int] = [:]

    subscript(filter: CodeSessionFilter) -> Int { values[filter] ?? 0 }

    mutating func add(_ status: CodeSessionFilterStatus) {
        for filter in CodeSessionFilter.allCases where filter.matches(status) {
            values[filter, default: 0] += 1
        }
    }
}

/// `countCodeSessionFilters`
func countCodeSessionFilters(_ statuses: [CodeSessionFilterStatus]) -> CodeSessionFilterCounts {
    var counts = CodeSessionFilterCounts()
    for status in statuses { counts.add(status) }
    return counts
}

/// `filterCodeSessionRows`: the rows a filter shows (input order kept) and
/// the counts for every filter.
func filterCodeSessionRows<Row>(
    _ rows: [Row],
    filter: CodeSessionFilter,
    status: (Row) -> CodeSessionFilterStatus
) -> (items: [Row], counts: CodeSessionFilterCounts) {
    let statuses = rows.map(status)
    let items = zip(rows, statuses).filter { filter.matches($0.1) }.map { $0.0 }
    return (items, countCodeSessionFilters(statuses))
}

// MARK: - Connect

/// `newlyOnlineRuntimes`: online computers that were not online when the
/// connect sheet opened. Empty until the first list has been read (`before`
/// nil), so "Connected" comes only from the real computer list, never from
/// the typing animation.
func newlyOnlineComputers<Device>(
    before: Set<String>?,
    now: [Device],
    id: (Device) -> String,
    isOnline: (Device) -> Bool
) -> [Device] {
    guard let before else { return [] }
    return now.filter { isOnline($0) && !before.contains(id($0)) }
}
