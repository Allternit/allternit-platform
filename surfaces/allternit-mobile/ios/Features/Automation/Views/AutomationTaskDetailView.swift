import SwiftUI

/// Automation task detail — structural pattern from `ProjectDetailView`:
/// header (name/description/status/schedule/type), a Pause-or-Resume + Run
/// Now + Delete action row, and a run-history list below (`GET v1/cron/
/// jobs/:id/runs`) with each run's status/timing/duration and an
/// expandable output/error.
struct AutomationTaskDetailView: View {
    let job: CronJob

    @StateObject private var jobStore = CronJobStore.shared
    @StateObject private var automationStore = AutomationStore()
    @State private var editDraft: AutomationTaskDraft? = nil
    @Environment(\.dismiss) private var dismiss

    @State private var runs: [CronRun] = []
    @State private var isLoadingRuns = false
    @State private var runsError: String? = nil
    @State private var expandedRunIds: Set<String> = []

    @State private var actionError: String? = nil
    @State private var isMutating = false
    @State private var isDeleteConfirmPresented = false

    /// The live store record (pause/resume/delete land here); falls back to
    /// the value captured on push (ProjectDetailView's `liveProject`).
    private var liveJob: CronJob {
        jobStore.job(withId: job.id) ?? job
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                if let actionError {
                    Text(actionError)
                        .font(.caption)
                        .foregroundColor(Theme.statusWarning)
                }

                headerSection
                actionRow
                runsSection
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 20)
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .navigationTitle(liveJob.name)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("Edit") { editDraft = AutomationTaskDraft(job: liveJob) }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button(role: .destructive, action: { isDeleteConfirmPresented = true }) {
                    Image(systemName: "trash")
                        .font(.system(size: 15, weight: .medium))
                        .foregroundColor(Color("TextSecondary"))
                }
                .accessibilityLabel("Delete task")
            }
        }
        .sheet(item: $editDraft) { draft in
            AutomationWizardSheet(store: automationStore, draft: draft)
        }
        .alert("Delete this automation task?", isPresented: $isDeleteConfirmPresented) {
            Button("Delete", role: .destructive) { deleteJob() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Its run history is deleted with it. This can't be undone.")
        }
        .task {
            await loadRuns()
        }
    }

    // MARK: - Header

    private var headerSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let description = liveJob.description, !description.isEmpty {
                Text(description)
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
            }

            infoRow(label: "Status", value: liveJob.status.capitalized, valueColor: AutomationTasksListView.statusColor(liveJob.status))
            infoRow(label: "Type", value: liveJob.type)
            infoRow(label: "Schedule", value: liveJob.schedule.displayText)
            if let lastRunText = AutomationTasksListView.relativeText(liveJob.lastRunAt) {
                infoRow(label: "Last run", value: lastRunText)
            }
            if let nextRunText = AutomationTasksListView.relativeText(liveJob.nextRunAt) {
                infoRow(label: "Next run", value: nextRunText)
            }
            infoRow(label: "Runs", value: "\(liveJob.runCount) succeeded, \(liveJob.failCount) failed")
            if let prompt = liveJob.agentConfig?.prompt, !prompt.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Prompt")
                        .font(.caption)
                        .fontWeight(.semibold)
                        .foregroundColor(Color("TextSecondary"))
                    Text(prompt)
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                        .padding(10)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(Color("BgPanel"))
                        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                        .overlay(
                            RoundedRectangle(cornerRadius: Theme.radiusMD)
                                .stroke(Theme.borderWarmDefault, lineWidth: 1)
                        )
                }
                .padding(.top, 4)
            }
        }
    }

    private func infoRow(label: String, value: String, valueColor: Color = Color("TextPrimary")) -> some View {
        HStack {
            Text(label)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
            Spacer()
            Text(value)
                .font(.caption)
                .fontWeight(.medium)
                .foregroundColor(valueColor)
        }
    }

    // MARK: - Actions

    private var actionRow: some View {
        HStack(spacing: 12) {
            if liveJob.status == "paused" {
                actionButton(icon: "play.fill", label: "Resume", action: resumeJob)
            } else {
                actionButton(icon: "pause.fill", label: "Pause", action: pauseJob)
            }
            actionButton(icon: "bolt.fill", label: "Run Now", action: runNow)
        }
        .disabled(isMutating)
    }

    private func actionButton(icon: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            action()
        }) {
            VStack(spacing: 8) {
                Image(systemName: icon)
                    .font(.system(size: 18, weight: .medium))
                Text(label)
                    .font(.system(size: 12, weight: .medium))
            }
            .foregroundColor(Color("TextPrimary"))
            .frame(maxWidth: .infinity)
            .frame(height: 68)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusLG)
                    .stroke(Theme.borderWarmDefault, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
    }

    // MARK: - Runs

    private var runsSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Run history")
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextSecondary"))

            if isLoadingRuns && runs.isEmpty {
                HStack {
                    Spacer()
                    ProgressView()
                    Spacer()
                }
                .padding(.vertical, 12)
            } else if let runsError {
                FriendlyInlineStateView(
                    style: .error,
                    icon: "exclamationmark.triangle",
                    title: "Couldn't load runs",
                    message: FriendlyErrorMessage.from(runsError),
                    actionTitle: "Retry",
                    action: { Task { await loadRuns() } }
                )
            } else if runs.isEmpty {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "play.circle",
                    title: "No runs yet",
                    message: "Runs for this task will appear here."
                )
            } else {
                VStack(spacing: 8) {
                    ForEach(runs) { run in
                        runRow(run)
                    }
                }
            }
        }
    }

    private func runRow(_ run: CronRun) -> some View {
        let isExpanded = expandedRunIds.contains(run.id)
        let detail = run.error ?? run.output
        return VStack(alignment: .leading, spacing: 6) {
            Button(action: {
                guard detail != nil else { return }
                if isExpanded {
                    expandedRunIds.remove(run.id)
                } else {
                    expandedRunIds.insert(run.id)
                }
            }) {
                HStack(spacing: 12) {
                    Circle()
                        .fill(Self.runStatusColor(run.status))
                        .frame(width: 8, height: 8)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(run.status.capitalized)
                            .font(.system(size: 13, weight: .medium))
                            .foregroundColor(Color("TextPrimary"))
                        Text(Self.timingText(run))
                            .font(.caption2)
                            .foregroundColor(Color("TextSecondary"))
                    }
                    Spacer()
                    if let durationMs = run.durationMs {
                        Text(Self.durationText(durationMs))
                            .font(.caption2)
                            .foregroundColor(Color("TextSecondary"))
                    }
                    if detail != nil {
                        Image(systemName: isExpanded ? "chevron.up" : "chevron.down")
                            .font(.system(size: 10, weight: .bold))
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
            }
            .buttonStyle(.plain)

            if isExpanded, let detail {
                Text(detail)
                    .font(.caption.monospaced())
                    .foregroundColor(run.error != nil ? Theme.statusWarning : Color("TextSecondary"))
                    .padding(8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color("BgSecondary"))
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM))
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusMD)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    private static func runStatusColor(_ status: String) -> Color {
        switch status {
        case "success": return Theme.statusSuccess
        case "failed", "timeout": return Theme.statusError
        case "cancelled": return Color("TextSecondary")
        case "running", "pending": return Theme.statusInfo
        default: return Color("TextSecondary")
        }
    }

    private static func timingText(_ run: CronRun) -> String {
        if let startedAt = AutomationTasksListView.relativeText(run.startedAt) {
            return "Started \(startedAt)"
        }
        if let scheduledAt = AutomationTasksListView.relativeText(run.scheduledAt) {
            return "Scheduled \(scheduledAt)"
        }
        return run.scheduledAt
    }

    private static func durationText(_ durationMs: Int) -> String {
        if durationMs < 1000 { return "\(durationMs)ms" }
        return String(format: "%.1fs", Double(durationMs) / 1000)
    }

    // MARK: - Actions

    private func pauseJob() {
        isMutating = true
        Task {
            do {
                try await jobStore.pause(id: job.id)
            } catch {
                actionError = "Couldn't pause: \(error.localizedDescription)"
            }
            isMutating = false
        }
    }

    private func resumeJob() {
        isMutating = true
        Task {
            do {
                try await jobStore.resume(id: job.id)
            } catch {
                actionError = "Couldn't resume: \(error.localizedDescription)"
            }
            isMutating = false
        }
    }

    private func runNow() {
        isMutating = true
        Task {
            do {
                try await jobStore.runNow(id: job.id)
                await loadRuns()
            } catch {
                actionError = "Couldn't trigger a run: \(error.localizedDescription)"
            }
            isMutating = false
        }
    }

    private func deleteJob() {
        Task {
            do {
                try await jobStore.deleteJob(id: job.id)
                dismiss()
            } catch {
                actionError = "Couldn't delete: \(error.localizedDescription)"
            }
        }
    }

    private func loadRuns() async {
        if runs.isEmpty {
            isLoadingRuns = true
        }
        runsError = nil
        do {
            runs = try await jobStore.listRuns(jobId: job.id)
        } catch is CancellationError {
            // View went away mid-flight — keep current state.
        } catch {
            runsError = error.localizedDescription
        }
        isLoadingRuns = false
    }
}
