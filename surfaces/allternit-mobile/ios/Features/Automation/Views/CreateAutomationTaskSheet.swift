import SwiftUI

/// "New automation task" sheet — minimal creation form for `type: "agent"`
/// jobs only (shell/http/cowork/function creation is out of scope for Phase
/// 1). Structural pattern from `NewProjectSheet`: labeled field "bubbles"
/// with a Create button disabled until the required fields are filled.
struct CreateAutomationTaskSheet: View {
    /// Called with the trimmed name, prompt, and the raw schedule string as
    /// typed (sent to `CronClient.createAgentJob` verbatim — the server's
    /// `parseScheduleToType` does the parsing, cron/service.ts:220-222).
    let onCreate: (String, String, String) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var prompt = ""
    @State private var schedule = ""
    @FocusState private var focusedField: Field?

    private enum Field {
        case name, prompt, schedule
    }

    private var canCreate: Bool {
        !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !schedule.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("New automation task")
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
                        placeholder: "Task name",
                        text: $name,
                        field: .name,
                        multiline: false,
                        submitLabel: .next
                    )

                    fieldBubble(
                        label: "Prompt",
                        placeholder: "What should the agent do each time this runs?",
                        text: $prompt,
                        field: .prompt,
                        multiline: true,
                        submitLabel: .next
                    )

                    VStack(alignment: .leading, spacing: 6) {
                        fieldBubble(
                            label: "Schedule",
                            placeholder: "0 9 * * *",
                            text: $schedule,
                            field: .schedule,
                            multiline: false,
                            submitLabel: .done
                        )
                        Text("A cron expression (e.g. \"0 9 * * *\" for 9am daily) or plain-language schedule — the server validates it.")
                            .font(.caption2)
                            .foregroundColor(Color("TextSecondary"))
                    }

                    // Day-of-week shortcut (parity with the web cowork
                    // `DayOfWeekSelector`). Only shown for cron-shaped input —
                    // plain-language schedules have no dow field to edit.
                    if schedule.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                        || schedule.split(separator: " ").count == 5 {
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
                    }

                    Button(action: create) {
                        Text("Create task")
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

    private func fieldBubble(
        label: String,
        placeholder: String,
        text: Binding<String>,
        field: Field,
        multiline: Bool,
        submitLabel: SubmitLabel
    ) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(label)
                .font(.caption)
                .fontWeight(.semibold)
                .foregroundColor(Color("TextSecondary"))
            TextField(placeholder, text: text, axis: multiline ? .vertical : .horizontal)
                .lineLimit(multiline ? 3...6 : 1...1)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .textInputAutocapitalization(field == .name ? .words : .sentences)
                .submitLabel(submitLabel)
                .focused($focusedField, equals: field)
                .onSubmit {
                    switch field {
                    case .name: focusedField = .prompt
                    case .prompt: focusedField = .schedule
                    case .schedule: if canCreate { create() }
                    }
                }
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

    private func create() {
        let trimmedName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedPrompt = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedSchedule = schedule.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedName.isEmpty, !trimmedPrompt.isEmpty, !trimmedSchedule.isEmpty else { return }
        let generator = UIImpactFeedbackGenerator(style: .light)
        generator.impactOccurred()
        onCreate(trimmedName, trimmedPrompt, trimmedSchedule)
        dismiss()
    }
}
