import SwiftUI

/// Agent Activity's thread list — Factory mail's real `GET /mail/threads`
/// (`AgentActivityStore.shared`), presented as a sheet from
/// `ComposerPlusSheet` (this app has no dedicated Agent Activity screen to
/// push onto, same situation `CoworkTasksListView` solves) with its own
/// internal `NavigationStack` for the list → detail **push** — never a
/// `.sheet` for the detail, which is the whole point: the web phase's
/// product feedback was specifically about a drill-down living in a modal,
/// and a pushed detail view already isn't one.
///
/// Row `topic` is the raw `threadId` string, displayed as-is — there is no
/// separate human-readable title in the real backend yet (map doc's
/// finding, still true). Review/guard/reservation tags come from
/// `AgentActivityStore`'s ledger-tail heuristic, not a real per-thread
/// status field (none exists).
struct AgentActivityListView: View {
    @StateObject private var store = AgentActivityStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var searchText = ""
    @State private var tagFilter: ActivityTagFilter = .all
    /// Pushed detail (nil = list).
    @State private var detailThread: AgentActivityThreadSummary? = nil

    private var visibleThreads: [AgentActivityThreadSummary] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        let tagFiltered = store.threads.filter { thread in
            guard tagFilter != .all else { return true }
            let state = store.state(for: thread.threadId)
            switch tagFilter {
            case .review: return state.hasPendingReview
            case .guardActivity: return state.hasGuardActivity
            case .reservation: return state.hasReservationActivity
            case .all: return true
            }
        }
        guard !query.isEmpty else { return tagFiltered }
        return tagFiltered.filter { $0.threadId.localizedCaseInsensitiveContains(query) }
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                // Header (CoworkTasksListView's sheet chrome: title + dismiss
                // — this surface has no create action, unlike Automation
                // Tasks / Cowork Tasks' "+").
                HStack {
                    Text("Agent Activity")
                        .font(.system(.title3, design: .serif))
                        .fontWeight(.medium)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    tagFilterMenu
                    Button(action: { dismiss() }) {
                        Image(systemName: "xmark")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Color("TextSecondary"))
                            .frame(width: 32, height: 32)
                            .background(Color("BgPanel"))
                            .clipShape(Circle())
                    }
                    .accessibilityLabel("Close")
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 14)

                Divider().background(Color("BorderSubtle"))

                content
            }
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .toolbar(.hidden, for: .navigationBar)
            .navigationDestination(item: $detailThread) { thread in
                AgentActivityDetailView(thread: thread)
            }
        }
        .task {
            store.fetchIfNeeded()
        }
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        if store.isLoading && store.threads.isEmpty {
            Spacer()
            ProgressView()
            Spacer()
        } else if let loadError = store.loadError, store.threads.isEmpty {
            Spacer()
            FriendlyStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "Couldn't load agent activity",
                message: FriendlyErrorMessage.from(loadError),
                actionTitle: "Retry",
                action: { store.fetchIfNeeded(force: true) }
            )
            Spacer()
        } else if store.threads.isEmpty {
            Spacer()
            emptyState
            Spacer()
        } else {
            listContent
        }
    }

    private var listContent: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                TextField("Search threads", text: $searchText)
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                if !searchText.isEmpty {
                    Button(action: { searchText = "" }) {
                        Image(systemName: "xmark.circle.fill")
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 9)
            .background(Color("BgSecondary"))
            .cornerRadius(10)
            .padding(.horizontal, 20)
            .padding(.bottom, 12)

            ScrollView {
                LazyVStack(spacing: 8) {
                    ForEach(visibleThreads) { thread in
                        threadRow(thread)
                    }
                    if visibleThreads.isEmpty {
                        VStack(spacing: 10) {
                            Image(systemName: "magnifyingglass")
                                .font(.system(size: 20, weight: .medium))
                                .foregroundColor(Color("TextSecondary"))
                            Text("No threads match.")
                                .font(.subheadline)
                                .foregroundColor(Color("TextSecondary"))
                        }
                        .padding(.top, 24)
                    }
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 20)
            }
            .scrollDismissesKeyboard(.interactively)
            .refreshable {
                await store.refresh()
            }
        }
    }

    private func threadRow(_ thread: AgentActivityThreadSummary) -> some View {
        let state = store.state(for: thread.threadId)
        let unread = store.isUnread(thread.threadId)
        return Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            detailThread = thread
        }) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: "bubble.left.and.bubble.right")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Color("AccentPrimary"))
                    .frame(width: 28)
                    .padding(.top, 2)
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        Text("Thread")
                            .font(.system(size: 14, weight: unread ? .bold : .medium))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                        if unread {
                            Circle()
                                .fill(Color("AccentPrimary"))
                                .frame(width: 6, height: 6)
                                .accessibilityLabel("Unread")
                        }
                        Spacer()
                        if let relativeText = Self.relativeText(thread.lastActivityAt) {
                            Text(relativeText)
                                .font(.caption2)
                                .foregroundColor(Color("TextSecondary"))
                        }
                    }
                    Text("\(Self.shortThreadId(thread.threadId)) · \(thread.messageCount == 1 ? "1 message" : "\(thread.messageCount) messages")")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                    if state.hasPendingReview || state.hasGuardActivity || state.hasReservationActivity {
                        HStack(spacing: 6) {
                            if state.hasPendingReview {
                                tag("Needs review", color: Theme.statusWarning)
                            }
                            if state.hasGuardActivity {
                                tag("Guard activity", color: .red)
                            }
                            if state.hasReservationActivity {
                                tag("Reservation", color: Color("TextSecondary"))
                            }
                        }
                    }
                }
                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.top, 3)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusMD)
                    .stroke(Theme.borderWarmDefault, lineWidth: 1)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func tag(_ label: String, color: Color) -> some View {
        Text(label)
            .font(.caption2)
            .fontWeight(.semibold)
            .foregroundColor(color)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(color.opacity(0.15))
            .clipShape(Capsule())
    }

    private var tagFilterMenu: some View {
        Menu {
            ForEach(ActivityTagFilter.allCases) { filter in
                Button(action: { tagFilter = filter }) {
                    HStack {
                        if tagFilter == filter {
                            Image(systemName: "checkmark")
                        }
                        Text(filter.label)
                    }
                }
            }
        } label: {
            HStack(spacing: 4) {
                Image(systemName: "line.3.horizontal.decrease.circle")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(tagFilter == .all ? Color("TextSecondary") : Color("AccentPrimary"))
                if tagFilter != .all {
                    Circle()
                        .fill(Color("AccentPrimary"))
                        .frame(width: 6, height: 6)
                }
            }
            .frame(width: 32, height: 32)
            .background(Color("BgPanel"))
            .clipShape(Circle())
        }
        .accessibilityLabel("Filter by tag")
    }

    enum ActivityTagFilter: String, CaseIterable, Identifiable {
        case all, review, guardActivity, reservation

        var id: String { rawValue }

        var label: String {
            switch self {
            case .all: return "All threads"
            case .review: return "Needs review"
            case .guardActivity: return "Guard activity"
            case .reservation: return "Reservation"
            }
        }
    }

    private var emptyState: some View {
        FriendlyStateView(
            style: .empty,
            icon: "bubble.left.and.bubble.right",
            title: "No agent activity yet",
            message: "Threads show up here as agents send mail through the Factory."
        )
    }

    // MARK: - Formatting (shared with AgentActivityDetailView)

    /// Backend timestamps are ISO-8601. Mirrored from
    /// AutomationTasksListView's private copy — no shared date-parsing
    /// helper exists in this codebase (CodeModeView keeps its own too).
    private static func parseTimestamp(_ value: String) -> Date? {
        if let date = try? Date(value, strategy: Date.ISO8601FormatStyle(includingFractionalSeconds: true)) {
            return date
        }
        return try? Date(value, strategy: Date.ISO8601FormatStyle())
    }

    static func relativeText(_ value: String?) -> String? {
        guard let value, let date = parseTimestamp(value) else { return nil }
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: date, relativeTo: Date())
    }

    /// The backend has no human-readable thread subject yet, so the row shows
    /// a fixed "Thread" title and uses the first 8 characters of the id as a
    /// stable, scannable subtitle.
    private static func shortThreadId(_ threadId: String) -> String {
        String(threadId.prefix(8))
    }
}
