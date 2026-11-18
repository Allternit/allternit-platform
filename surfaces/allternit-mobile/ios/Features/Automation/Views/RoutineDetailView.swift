import SwiftUI

/// Routine detail — structural pattern from `AutomationTaskDetailView`:
/// header (name/state/trigger/schedule) + a Run Now + Delete action row.
/// Unlike cron there's no run-history endpoint, so the step list itself
/// (with each step's own `status`) stands in for run history — refetched via
/// `RoutineStore.refresh()` (whole-list, since there's no `GET
/// /routines/:id`), which is also how the view picks up state changes after
/// `run` (`RoutineEngine.startRoutine` mutates step statuses as it goes,
/// routine-engine.ts:26-46). There's no dedicated pause/resume endpoint for
/// routines, so no pause/resume button here.
struct RoutineDetailView: View {
    let routine: Routine

    @StateObject private var routineStore = RoutineStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var actionError: String? = nil
    @State private var isMutating = false
    @State private var isDeleteConfirmPresented = false

    /// The live store record (run/delete land here); falls back to the value
    /// captured on push (AutomationTaskDetailView's `liveJob`).
    private var liveRoutine: Routine {
        routineStore.routine(withId: routine.id) ?? routine
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                if let actionError {
                    FriendlyInlineStateView(
                        style: .error,
                        icon: "exclamationmark.triangle",
                        title: "Action failed",
                        message: actionError
                    )
                }

                headerSection
                actionRow
                stepsSection
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 20)
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .navigationTitle(liveRoutine.name)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(role: .destructive, action: { isDeleteConfirmPresented = true }) {
                    Image(systemName: "trash")
                        .font(.system(size: 15, weight: .medium))
                        .foregroundColor(Color("TextSecondary"))
                }
            }
        }
        .alert("Delete this routine?", isPresented: $isDeleteConfirmPresented) {
            Button("Delete", role: .destructive) { deleteRoutine() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This can't be undone.")
        }
    }

    // MARK: - Header

    private var headerSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            infoRow(label: "State", value: liveRoutine.state.capitalized, valueColor: RoutinesListView.statusColor(liveRoutine.state))
            if let trigger = liveRoutine.trigger, !trigger.isEmpty {
                infoRow(label: "Trigger", value: trigger)
            }
            if let schedule = liveRoutine.schedule, !schedule.isEmpty {
                infoRow(label: "Schedule", value: schedule)
            }
            if let updatedText = RoutinesListView.relativeText(liveRoutine.timeUpdated) {
                infoRow(label: "Updated", value: updatedText)
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
            actionButton(icon: "bolt.fill", label: "Run Now", action: runNow)
        }
        .disabled(isMutating)
    }

    private func actionButton(icon: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: {
            Haptics.light()
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

    // MARK: - Steps

    private var stepsSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Steps")
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextSecondary"))

            if liveRoutine.steps.isEmpty {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "list.bullet",
                    title: "No steps",
                    message: "This routine doesn't have any steps yet."
                )
            } else {
                VStack(spacing: 8) {
                    ForEach(Array(liveRoutine.steps.enumerated()), id: \.offset) { index, step in
                        stepRow(index: index, step: step)
                    }
                }
            }
        }
    }

    private func stepRow(index: Int, step: RoutineStep) -> some View {
        HStack(spacing: 12) {
            Circle()
                .fill(Self.stepStatusColor(step.status))
                .frame(width: 8, height: 8)
            VStack(alignment: .leading, spacing: 2) {
                Text(step.command)
                    .font(.system(size: 13, weight: .medium, design: .monospaced))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(2)
                Text("Step \(index + 1) · \(step.status.capitalized)")
                    .font(.caption2)
                    .foregroundColor(Color("TextSecondary"))
            }
            Spacer()
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

    /// Step status colors (routine-engine.ts:26-46: pending -> running ->
    /// done|failed).
    private static func stepStatusColor(_ status: String) -> Color {
        switch status {
        case "done": return Theme.statusSuccess
        case "failed": return .red
        case "running": return Theme.statusInfo
        default: return Color("TextSecondary")
        }
    }

    // MARK: - Actions

    private func runNow() {
        isMutating = true
        Task {
            do {
                try await routineStore.runRoutine(id: routine.id)
            } catch {
                actionError = "Couldn't trigger a run: \(error.localizedDescription)"
            }
            isMutating = false
        }
    }

    private func deleteRoutine() {
        Task {
            do {
                try await routineStore.deleteRoutine(id: routine.id)
                dismiss()
            } catch {
                actionError = "Couldn't delete: \(error.localizedDescription)"
            }
        }
    }
}
