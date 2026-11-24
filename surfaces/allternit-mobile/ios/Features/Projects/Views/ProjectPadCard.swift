import SwiftUI

/// The project page's tab card and what it holds: the pad, the list, or
/// today's dashboard for a project that still has an old Style. Shared by the
/// chat project page and the bot project page.
struct ProjectPadCard: View {
    @ObservedObject var store: ProjectPadStore
    @ObservedObject var looks: ProjectLookStore
    let data: ProjectBoardData
    let threads: [PadThread]
    let title: String
    let modeLabel: String
    /// The folder color as `#rrggbb`.
    let colorHex: String
    let onEdit: () -> Void
    /// Opens the Needs-you target; the page decides what that is.
    let onNeeds: () -> Void
    let onStatus: () -> Void

    @State private var viewOverride: PadView? = nil

    var body: some View {
        let pad = store.pad
        let canEdit = store.status != .signedOut
        let needsCount = data.needsYou.count + (data.needsYou.isEmpty && data.statusNeedsYou ? 1 : 0)
        let needsText: String = {
            if let first = data.needsYou.first { return first.title }
            return data.nextAction.isEmpty ? (data.blockers.isEmpty ? (data.stateLabel ?? "Check the status page") : data.blockers) : data.nextAction
        }()
        let shownView = viewOverride ?? pad.view
        ProjectTabCard(
            title: title,
            modeLabel: modeLabel,
            colorHex: colorHex,
            view: shownView,
            onViewChange: { newView in
                viewOverride = newView
                if canEdit { store.save { $0.view = newView } }
            },
            onEdit: canEdit ? onEdit : nil,
            needsCount: needsCount,
            needsText: needsText,
            onNeeds: onNeeds,
            statusLine: store.statusMessage ?? store.error
        ) {
            if shownView == .list {
                ProjectPadListView(data: data, threads: threads, openStatus: onStatus)
            } else if pad.layout.style != nil {
                ProjectDashboardView(layout: looks.look.layout, look: looks.look, data: data, title: title, openStatus: onStatus)
            } else {
                ProjectPadView(store: store, data: data, threads: threads, title: title, openStatus: onStatus)
            }
        }
    }
}
