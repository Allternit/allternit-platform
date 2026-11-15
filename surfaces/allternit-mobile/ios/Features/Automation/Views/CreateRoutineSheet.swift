import SwiftUI

/// "New routine" sheet — structural pattern from `CreateAutomationTaskSheet`:
/// labeled field "bubbles" with a Create button disabled until the required
/// fields are filled. Unlike cron, `trigger`/`schedule` are both optional (a
/// routine can be run manually) and `steps` is a free-form list of command
/// strings rather than a single prompt.
struct CreateRoutineSheet: View {
    /// Called with the trimmed name, the step list (each starting at status
    /// "pending", per `RoutineEngine.startRoutine`'s state machine), and the
    /// trimmed trigger/schedule (nil when left blank — sent as `nil` in the
    /// request body, which `RoutineCreateSchema` treats as absent).
    let onCreate: (String, [RoutineStep], String?, String?) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var stepCommands: [String] = [""]
    @State private var trigger = ""
    @State private var schedule = ""
    @FocusState private var focusedField: Field?

    private enum Field: Hashable {
        case name, step(Int), trigger, schedule
    }

    private var trimmedStepCommands: [String] {
        stepCommands
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
    }

    private var canCreate: Bool {
        !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("New routine")
                    .font(.system(.title3, design: .serif))
                    .fontWeight(.medium)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
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

            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    fieldBubble(
                        label: "Name",
                        placeholder: "Routine name",
                        text: $name,
                        field: .name,
                        submitLabel: .next
                    )
                    .textInputAutocapitalization(.words)
                    .onSubmit { focusedField = .step(0) }

                    stepsSection

                    VStack(alignment: .leading, spacing: 6) {
                        fieldBubble(
                            label: "Trigger (optional)",
                            placeholder: "e.g. manual, webhook",
                            text: $trigger,
                            field: .trigger,
                            submitLabel: .next
                        )
                        .onSubmit { focusedField = .schedule }
                    }

                    VStack(alignment: .leading, spacing: 6) {
                        fieldBubble(
                            label: "Schedule (optional)",
                            placeholder: "0 9 * * *",
                            text: $schedule,
                            field: .schedule,
                            submitLabel: .done
                        )
                        .onSubmit { if canCreate { create() } }
                        Text("A routine can also be run manually — trigger and schedule are both optional.")
                            .font(.caption2)
                            .foregroundColor(Color("TextSecondary"))
                    }

                    // Day-of-week toggles supplementing the cron expression —
                    // same pairing as the web (AutomationTasksView.tsx): the
                    // schedule string stays the single source of truth, read
                    // via `CronDays.parseCronDays` and rewritten via
                    // `CronDays.applyCronDays` on change.
                    VStack(alignment: .leading, spacing: 6) {
                        Text("Days of week")
                            .font(.caption)
                            .fontWeight(.semibold)
                            .foregroundColor(Color("TextSecondary"))
                        DayOfWeekSelector(
                            selectedDays: CronDays.parseCronDays(schedule),
                            onChange: { days in
                                schedule = CronDays.applyCronDays(schedule, days: days)
                            }
                        )
                    }

                    Button(action: create) {
                        Text("Create routine")
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundColor(canCreate ? Color("BgPrimary") : Color("TextSecondary"))
                            .frame(maxWidth: .infinity)
                            .frame(height: 48)
                            .background(canCreate ? Color("AccentPrimary") : Color("BgSecondary"))
                            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                    }
                    .buttonStyle(.plain)
                    .disabled(!canCreate)
                    .padding(.top, 8)
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 20)
            }
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .onAppear { focusedField = .name }
    }

    // MARK: - Steps

    private var stepsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Steps (optional)")
                .font(.caption)
                .fontWeight(.semibold)
                .foregroundColor(Color("TextSecondary"))

            ForEach(stepCommands.indices, id: \.self) { index in
                HStack(spacing: 8) {
                    TextField("Command", text: Binding(
                        get: { stepCommands[index] },
                        set: { stepCommands[index] = $0 }
                    ))
                    .font(.subheadline.monospaced())
                    .foregroundColor(Color("TextPrimary"))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.next)
                    .focused($focusedField, equals: .step(index))
                    .onSubmit { addStepOrAdvance(from: index) }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 12)
                    .background(Color("BgPanel"))
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                    .overlay(
                        RoundedRectangle(cornerRadius: Theme.radiusMD)
                            .stroke(focusedField == .step(index) ? Color("AccentPrimary").opacity(0.5) : Theme.borderWarmDefault, lineWidth: 1)
                    )

                    if stepCommands.count > 1 {
                        Button(action: { removeStep(at: index) }) {
                            Image(systemName: "minus.circle.fill")
                                .font(.system(size: 18))
                                .foregroundColor(Color("TextSecondary"))
                        }
                    }
                }
            }

            Button(action: addStep) {
                HStack(spacing: 6) {
                    Image(systemName: "plus.circle.fill")
                    Text("Add step")
                }
                .font(.caption)
                .fontWeight(.medium)
                .foregroundColor(Color("AccentPrimary"))
            }
            .padding(.top, 2)
        }
    }

    private func addStep() {
        stepCommands.append("")
        focusedField = .step(stepCommands.count - 1)
    }

    private func addStepOrAdvance(from index: Int) {
        if index == stepCommands.count - 1 {
            addStep()
        } else {
            focusedField = .step(index + 1)
        }
    }

    private func removeStep(at index: Int) {
        stepCommands.remove(at: index)
    }

    // MARK: - Field bubble

    private func fieldBubble(
        label: String,
        placeholder: String,
        text: Binding<String>,
        field: Field,
        submitLabel: SubmitLabel
    ) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(label)
                .font(.caption)
                .fontWeight(.semibold)
                .foregroundColor(Color("TextSecondary"))
            TextField(placeholder, text: text)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .submitLabel(submitLabel)
                .focused($focusedField, equals: field)
                .padding(.horizontal, 14)
                .padding(.vertical, 12)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusMD)
                        .stroke(focusedField == field ? Color("AccentPrimary").opacity(0.5) : Theme.borderWarmDefault, lineWidth: 1)
                )
        }
    }

    // MARK: - Create

    private func create() {
        let trimmedName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedName.isEmpty else { return }
        let steps = trimmedStepCommands.map { RoutineStep(command: $0, status: "pending") }
        let trimmedTrigger = trigger.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedSchedule = schedule.trimmingCharacters(in: .whitespacesAndNewlines)
        let generator = UIImpactFeedbackGenerator(style: .light)
        generator.impactOccurred()
        onCreate(
            trimmedName,
            steps,
            trimmedTrigger.isEmpty ? nil : trimmedTrigger,
            trimmedSchedule.isEmpty ? nil : trimmedSchedule
        )
        dismiss()
    }
}
