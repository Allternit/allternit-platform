import SwiftUI

/// Automation Tasks tab surface — Routines sub-view (Phase 2; Goals under
/// `/v1/automations` is a later phase). Structural pattern mirrors
/// `AutomationTasksListView`: search, row -> detail push, toolbar "+" opens
/// a creation sheet. The Cron/Routines/Loops segmented control (top of all
/// three list views, bound to `modeStore.automationKind`) is what makes this
/// reachable from the same "Automation Tasks" tab as Cron/Loops — see
/// ChatView.swift's `.automation` case.
///
/// Data: `RoutineStore.shared` over `GET v1/automations/routines` on
/// gizzi-code's own server (RoutinesClient — same host as PtyClient/
/// PermissionClient/CronClient, not the `allternit-api` relay ProjectsClient
/// uses).
struct RoutinesListView: View {
    @Binding var isSidebarOpen: Bool

    @EnvironmentObject private var modeStore: AppModeStore
    @StateObject private var routineStore = RoutineStore.shared

    @State private var searchText = ""
    @State private var statusFilter: StatusFilter = .all
    /// Pushed detail (nil = list).
    @State private var detailRoutine: Routine? = nil
    @State private var isCreateSheetPresented = false
    @State private var actionError: String? = nil

    private enum StatusFilter: String, CaseIterable {
        case all = "All"
        case running = "Running"
        case completed = "Completed"
        case failed = "Failed"
    }

    private var visibleRoutines: [Routine] {
        var routines = routineStore.routines
        switch statusFilter {
        case .all: break
        case .running: routines = routines.filter { $0.state == "running" }
        case .completed: routines = routines.filter { $0.state == "completed" }
        case .failed: routines = routines.filter { $0.state == "failed" }
        }
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return routines }
        return routines.filter { $0.name.localizedCaseInsensitiveContains(query) }
    }

    private var filterMenu: some View {
        Menu {
            ForEach(StatusFilter.allCases, id: \.self) { filter in
                Button(action: { statusFilter = filter }) {
                    Label(filter.rawValue, systemImage: statusFilter == filter ? "checkmark" : "")
                }
            }
        } label: {
            Image(systemName: "line.3.horizontal.decrease")
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 32, height: 32)
                .background(Color("BgPanel"))
                .clipShape(Circle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Filter routines")
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                // Header (AutomationTasksListView's standalone-tab chrome:
                // sidebar toggle + title + "+").
                HStack {
                    Button(action: {
                        let generator = UIImpactFeedbackGenerator(style: .medium)
                        generator.impactOccurred()
                        withAnimation(.spring(response: 0.35, dampingFraction: 0.86, blendDuration: 0)) {
                            isSidebarOpen.toggle()
                        }
                    }) {
                        Image(systemName: "line.3.horizontal")
                            .font(.title3)
                            .foregroundColor(Color("TextPrimary"))
                            .frame(width: 44, height: 44)
                    }
                    .accessibilityLabel("Open sidebar")

                    Menu {
                        ForEach(AutomationKind.allCases, id: \.self) { kind in
                            Button(action: { modeStore.automationKind = kind }) {
                                Label(
                                    kind.rawValue,
                                    systemImage: modeStore.automationKind == kind ? "checkmark" : ""
                                )
                            }
                        }
                    } label: {
                        HStack(spacing: 4) {
                            Text(modeStore.automationKind.rawValue)
                                .font(.system(.title3, design: .serif))
                                .fontWeight(.medium)
                                .foregroundColor(Color("TextPrimary"))
                            Image(systemName: "chevron.down")
                                .font(.system(size: 10, weight: .bold))
                                .foregroundColor(Color("TextSecondary"))
                        }
                    }

                    Spacer()

                    filterMenu

                    Button(action: {
                        let generator = UIImpactFeedbackGenerator(style: .light)
                        generator.impactOccurred()
                        isCreateSheetPresented = true
                    }) {
                        Image(systemName: "plus")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Color("TextSecondary"))
                            .frame(width: 32, height: 32)
                            .background(Color("BgPanel"))
                            .clipShape(Circle())
                    }
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 10)
                .background(Color("BgPrimary"))

                Divider().background(Color("BorderSubtle"))

                // Runs on the user's computer: "Connect a computer" when none answers.

                ConnectComputerView()

                content
            }
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .toolbar(.hidden, for: .navigationBar)
            .navigationDestination(item: $detailRoutine) { routine in
                RoutineDetailView(routine: routine)
            }
        }
        .sheet(isPresented: $isCreateSheetPresented) {
            CreateRoutineSheet { name, steps, trigger, schedule in
                createRoutine(name: name, steps: steps, trigger: trigger, schedule: schedule)
            }
        }
        .task {
            routineStore.fetchRoutinesIfNeeded()
        }
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        VStack(spacing: 0) {
            if routineStore.isLoading && routineStore.routines.isEmpty {
                Spacer()
                ProgressView()
                Spacer()
            } else if let loadError = routineStore.loadError, routineStore.routines.isEmpty {
                Spacer()
                FriendlyStateView(
                    style: errorStyle(loadError),
                    icon: "wifi.slash",
                    title: "Couldn't load routines",
                    message: FriendlyErrorMessage.from(loadError),
                    actionTitle: "Retry",
                    action: { routineStore.fetchRoutinesIfNeeded(force: true) }
                )
                Spacer()
            } else if routineStore.routines.isEmpty {
                Spacer()
                emptyState
                Spacer()
            } else {
                listContent
            }
        }
    }

    private var listContent: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                TextField("Search routines", text: $searchText)
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
            .padding(.top, 12)
            .padding(.bottom, 12)

            if let actionError {
                FriendlyInlineStateView(
                    style: .error,
                    icon: "exclamationmark.triangle",
                    title: "Action failed",
                    message: actionError
                )
            }

            ScrollView {
                LazyVStack(spacing: 8) {
                    ForEach(visibleRoutines) { routine in
                        routineRow(routine)
                    }
                    if visibleRoutines.isEmpty {
                        FriendlyInlineStateView(
                            style: .empty,
                            icon: "magnifyingglass",
                            title: "No matches",
                            message: "No routines match."
                        )
                        .padding(.top, 24)
                    }
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 20)
            }
            .scrollDismissesKeyboard(.interactively)
            .refreshable {
                await routineStore.refresh()
            }
        }
    }

    private func routineRow(_ routine: Routine) -> some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            detailRoutine = routine
        }) {
            HStack(spacing: 12) {
                Image(systemName: "list.bullet.rectangle")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Color("AccentPrimary"))
                    .frame(width: 28)
                VStack(alignment: .leading, spacing: 2) {
                    Text(routine.name)
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text(Self.subtitleText(routine))
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
                Spacer()
                Self.statusBadge(routine.state)
                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
            }
            .padding(.horizontal, 14)
            .frame(minHeight: 64)
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

    private var emptyState: some View {
        FriendlyStateView(
            style: .empty,
            icon: "list.bullet.rectangle",
            title: "No routines",
            message: "Chain a few commands into a named routine you can run on demand, anytime.",
            actionTitle: "Create routine",
            action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isCreateSheetPresented = true
            }
        )
    }

    // MARK: - Actions

    private func createRoutine(name: String, steps: [RoutineStep], trigger: String?, schedule: String?) {
        Task {
            do {
                try await routineStore.createRoutine(name: name, steps: steps, trigger: trigger, schedule: schedule)
            } catch {
                actionError = "Couldn't create the routine: \(error.localizedDescription)"
            }
        }
    }

    // MARK: - Formatting (shared with RoutineDetailView)

    /// Status badge colors mirroring AutomationTasksListView's convention,
    /// adapted to routine `state` values (defined/running/completed/failed,
    /// routine-engine.ts:12,64-74).
    static func statusColor(_ state: String) -> Color {
        switch state {
        case "running": return Theme.statusInfo
        case "completed": return Theme.statusSuccess
        case "failed": return .red
        default: return Color("TextSecondary")
        }
    }

    private static func statusBadge(_ state: String) -> some View {
        Text(state.capitalized)
            .font(.caption2)
            .fontWeight(.semibold)
            .foregroundColor(statusColor(state))
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(statusColor(state).opacity(0.15))
            .clipShape(Capsule())
    }

    private static func subtitleText(_ routine: Routine) -> String {
        var parts: [String] = []
        let stepCount = routine.steps.count
        parts.append(stepCount == 1 ? "1 step" : "\(stepCount) steps")
        if let trigger = routine.trigger, !trigger.isEmpty {
            parts.append(trigger)
        } else if let schedule = routine.schedule, !schedule.isEmpty {
            parts.append(schedule)
        }
        return parts.joined(separator: " · ")
    }

    /// `time_created`/`time_updated` are ms-epoch numbers (automations.ts:
    /// 147-148), unlike cron's ISO-8601 strings.
    static func relativeText(_ msEpoch: Double?) -> String? {
        guard let msEpoch else { return nil }
        let date = Date(timeIntervalSince1970: msEpoch / 1000)
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: date, relativeTo: Date())
    }

    private func errorStyle(_ error: String) -> FriendlyStateView.Style {
        let lowered = error.lowercased()
        if lowered.contains("could not connect")
            || lowered.contains("failed to connect")
            || lowered.contains("internet connection")
            || lowered.contains("offline")
            || lowered.contains("network") {
            return .offline
        }
        return .error
    }
}
