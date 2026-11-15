import SwiftUI

/// Automation Tasks tab surface — the Cron sub-view (Phase 1). Goals under
/// `/v1/automations` is a later phase; Routines (Phase 2) and Loops
/// (Phase 3) are its siblings, `RoutinesListView`/`LoopsListView`, reachable
/// via the Cron/Routines/Loops segmented control below the header — see
/// `modeStore.automationKind` and ChatView.swift's `.automation` case.
/// Structural pattern from `ProjectsListView`: search + status segmented
/// control, row → detail push, toolbar "+" opens a creation sheet.
///
/// Data: `CronJobStore.shared` over `GET v1/cron/jobs` on gizzi-code's own
/// server (CronClient — same host as PtyClient/PermissionClient, not the
/// `allternit-api` relay ProjectsClient uses).
struct AutomationTasksListView: View {
    @Binding var isSidebarOpen: Bool

    @EnvironmentObject private var modeStore: AppModeStore
    @StateObject private var jobStore = CronJobStore.shared

    @State private var searchText = ""
    @State private var statusFilter: StatusFilter = .all
    /// Pushed detail (nil = list).
    @State private var detailJob: CronJob? = nil
    @State private var isCreateSheetPresented = false
    @State private var actionError: String? = nil

    private enum StatusFilter: String, CaseIterable {
        case all = "All"
        case active = "Active"
        case paused = "Paused"
    }

    private var visibleJobs: [CronJob] {
        var jobs = jobStore.jobs
        switch statusFilter {
        case .all: break
        case .active: jobs = jobs.filter { $0.status == "active" }
        case .paused: jobs = jobs.filter { $0.status == "paused" }
        }
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return jobs }
        return jobs.filter { $0.name.localizedCaseInsensitiveContains(query) }
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
        .accessibilityLabel("Filter tasks")
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                // Header (AgentHubView's standalone-tab chrome: sidebar
                // toggle + title + "+" — this surface has no parent sheet to
                // dismiss, unlike ProjectsListView's sheet-hosted variant).
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
                    .accessibilityLabel("New automation task")
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
            .navigationDestination(item: $detailJob) { job in
                AutomationTaskDetailView(job: job)
            }
        }
        .sheet(isPresented: $isCreateSheetPresented) {
            CreateAutomationTaskSheet { name, prompt, schedule in
                createJob(name: name, prompt: prompt, schedule: schedule)
            }
        }
        .task {
            jobStore.fetchJobsIfNeeded()
        }
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        VStack(spacing: 0) {
            if jobStore.isLoading && jobStore.jobs.isEmpty {
                Spacer()
                ProgressView()
                Spacer()
            } else if let loadError = jobStore.loadError, jobStore.jobs.isEmpty {
                Spacer()
                FriendlyStateView(
                    style: errorStyle(loadError),
                    icon: "wifi.slash",
                    title: "Couldn't load automation tasks",
                    message: FriendlyErrorMessage.from(loadError),
                    actionTitle: "Retry",
                    action: { jobStore.fetchJobsIfNeeded(force: true) }
                )
                Spacer()
            } else if jobStore.jobs.isEmpty {
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
                TextField("Search automation tasks", text: $searchText)
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
                    ForEach(visibleJobs) { job in
                        jobRow(job)
                    }
                    if visibleJobs.isEmpty {
                        FriendlyInlineStateView(
                            style: .empty,
                            icon: "magnifyingglass",
                            title: "No matches",
                            message: "No automation tasks match."
                        )
                        .padding(.top, 24)
                    }
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 20)
            }
            .scrollDismissesKeyboard(.interactively)
            .refreshable {
                await jobStore.refresh()
            }
        }
    }

    private func jobRow(_ job: CronJob) -> some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            detailJob = job
        }) {
            HStack(spacing: 12) {
                Image(systemName: "clock.arrow.circlepath")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Color("AccentPrimary"))
                    .frame(width: 28)
                VStack(alignment: .leading, spacing: 2) {
                    Text(job.name)
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text(job.schedule.displayText)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                    if let nextRunText = Self.relativeText(job.nextRunAt) {
                        Text("Next run \(nextRunText)")
                            .font(.caption2)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
                Spacer()
                statusBadge(job.status)
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
            icon: "clock.arrow.circlepath",
            title: "No automation tasks",
            message: "Schedule a prompt to run automatically — daily digests, recurring reminders, anything on a timer.",
            actionTitle: "Create automation task",
            action: {
                let generator = UIImpactFeedbackGenerator(style: .light)
                generator.impactOccurred()
                isCreateSheetPresented = true
            }
        )
    }

    // MARK: - Actions

    private func createJob(name: String, prompt: String, schedule: String) {
        Task {
            do {
                try await jobStore.createAgentJob(name: name, prompt: prompt, schedule: schedule)
            } catch {
                actionError = "Couldn't create the automation task: \(error.localizedDescription)"
            }
        }
    }

    // MARK: - Formatting (shared with AutomationTaskDetailView)

    /// Status badge colors mirroring CodeModeView's session-status
    /// convention (Theme.status* + `.red` for hard failures).
    static func statusColor(_ status: String) -> Color {
        switch status {
        case "active": return Theme.statusSuccess
        case "paused": return Theme.statusWarning
        case "error": return Theme.statusError
        default: return Color("TextSecondary")
        }
    }

    private func statusBadge(_ status: String) -> some View {
        Text(status.capitalized)
            .font(.caption2)
            .fontWeight(.semibold)
            .foregroundColor(Self.statusColor(status))
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(Self.statusColor(status).opacity(0.15))
            .clipShape(Capsule())
    }

    /// Backend timestamps are ISO-8601 (`Date.toISOString()`), with
    /// fractional seconds. Mirrored from CodeModeView's private copy.
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
