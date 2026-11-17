import Foundation

/// Select mode on the phone session overview ("View all ›" → ⋯ → Select chats):
/// which view is showing (All / Pinned / Archived), which rows are ticked, and
/// what the bulk actions do. Pure, so the rules are tested without UI.
/// Port of the web `overview-select.ts` (ai #1151); keep the two in step.

enum OverviewView: String, CaseIterable, Equatable {
    case all, pinned, archived

    /// The ⋯ menu's view labels (web `OVERVIEW_VIEW_LABEL`).
    var label: String {
        switch self {
        case .all: return "All"
        case .pinned: return "Pinned"
        case .archived: return "Archived"
        }
    }
}

let OVERVIEW_VIEW_LABEL: [OverviewView: String] = Dictionary(
    uniqueKeysWithValues: OverviewView.allCases.map { ($0, $0.label) })

struct OverviewSelectState: Equatable {
    var view: OverviewView = .all
    var selecting: Bool = false
    /// Ticked row ids, in the order they were ticked.
    var selected: [String] = []
}

let INITIAL_OVERVIEW_SELECT = OverviewSelectState()

enum OverviewSelectEvent: Equatable {
    case start
    case cancel
    case toggle(String)
    case selectAll([String])
    case view(OverviewView)
    /// The visible rows changed: drop ticks on rows that are gone.
    case visible([String])
    /// A bulk action ran: leave select mode.
    case done
}

func reduceOverviewSelect(_ state: OverviewSelectState, _ event: OverviewSelectEvent) -> OverviewSelectState {
    var s = state
    switch event {
    case .start:
        s.selecting = true
        s.selected = []
    case .cancel, .done:
        s.selecting = false
        s.selected = []
    case .toggle(let id):
        guard state.selecting else { return state }
        if let i = s.selected.firstIndex(of: id) { s.selected.remove(at: i) } else { s.selected.append(id) }
    case .selectAll(let ids):
        guard state.selecting else { return state }
        // A second "Select all" with everything ticked clears the ticks.
        let all = !ids.isEmpty && ids.allSatisfy { state.selected.contains($0) }
        s.selected = all ? [] : ids
    case .view(let v):
        // Ticks belong to the list they were made on.
        guard v != state.view else { return state }
        s.view = v
        s.selected = []
    case .visible(let ids):
        let keep = state.selected.filter { ids.contains($0) }
        guard keep.count != state.selected.count else { return state }
        s.selected = keep
    }
    return s
}

enum BulkPinAction: Equatable { case pin, unpin }

/// Pin when any ticked row is unpinned; Unpin only when every ticked row is already pinned.
func bulkPinAction(_ selected: [String], pinnedIds: Set<String>) -> BulkPinAction {
    !selected.isEmpty && selected.allSatisfy { pinnedIds.contains($0) } ? .unpin : .pin
}
