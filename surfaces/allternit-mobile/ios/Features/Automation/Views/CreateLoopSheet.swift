import SwiftUI

/// "New loop" sheet — structural pattern from `CreateRoutineSheet`: labeled
/// field "bubbles" with a Create button disabled until the required fields
/// are filled. Unlike Routine, the one required field is `command` (a
/// single shell command run on repeat, monospaced like Routine's step
/// fields), `exit_condition` is optional free text, and `max_iterations` is
/// a number — a `Stepper`, not a text field, since the backend field is
/// numeric (`LoopCreateSchema.max_iterations`, automations.ts:38).
struct CreateLoopSheet: View {
    /// Called with the trimmed command, the trimmed exit condition (nil when
    /// left blank — the server defaults to "exit code zero" when absent,
    /// loop-engine.ts:67-69), and the iteration cap.
    let onCreate: (String, String?, Int, LoopMetric?) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var command = ""
    @State private var exitCondition = ""
    @State private var maxIterations = 10
    // Loop Card: a blank instrument keeps this a plain loop.
    @State private var instrument = ""
    @State private var lowerIsBetter = true
    @State private var keepBarPercent = 5
    @State private var minPairs = 5
    @FocusState private var focusedField: Field?

    private enum Field {
        case command, exitCondition
    }

    private var canCreate: Bool {
        !command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("New loop")
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
                    VStack(alignment: .leading, spacing: 6) {
                        Text("Command")
                            .font(.caption)
                            .fontWeight(.semibold)
                            .foregroundColor(Color("TextSecondary"))
                        TextField("Command to run each iteration", text: $command)
                            .font(.subheadline.monospaced())
                            .foregroundColor(Color("TextPrimary"))
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .submitLabel(.next)
                            .focused($focusedField, equals: .command)
                            .onSubmit { focusedField = .exitCondition }
                            .padding(.horizontal, 14)
                            .padding(.vertical, 12)
                            .background(Color("BgPanel"))
                            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                            .overlay(
                                RoundedRectangle(cornerRadius: Theme.radiusMD)
                                    .stroke(focusedField == .command ? Color("AccentPrimary").opacity(0.5) : Theme.borderWarmDefault, lineWidth: 1)
                            )
                    }

                    VStack(alignment: .leading, spacing: 6) {
                        Text("Exit condition (optional)")
                            .font(.caption)
                            .fontWeight(.semibold)
                            .foregroundColor(Color("TextSecondary"))
                        TextField("e.g. exit_code_zero, or text to match in output", text: $exitCondition)
                            .font(.subheadline)
                            .foregroundColor(Color("TextPrimary"))
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .submitLabel(.done)
                            .focused($focusedField, equals: .exitCondition)
                            .onSubmit { if canCreate { create() } }
                            .padding(.horizontal, 14)
                            .padding(.vertical, 12)
                            .background(Color("BgPanel"))
                            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                            .overlay(
                                RoundedRectangle(cornerRadius: Theme.radiusMD)
                                    .stroke(focusedField == .exitCondition ? Color("AccentPrimary").opacity(0.5) : Theme.borderWarmDefault, lineWidth: 1)
                            )
                        Text("Left blank, the loop stops the first time the command exits 0.")
                            .font(.caption2)
                            .foregroundColor(Color("TextSecondary"))
                    }

                    VStack(alignment: .leading, spacing: 6) {
                        Text("Max iterations")
                            .font(.caption)
                            .fontWeight(.semibold)
                            .foregroundColor(Color("TextSecondary"))
                        Stepper(value: $maxIterations, in: 1...100) {
                            Text("\(maxIterations)")
                                .font(.subheadline)
                                .foregroundColor(Color("TextPrimary"))
                        }
                        .padding(.horizontal, 14)
                        .padding(.vertical, 12)
                        .background(Color("BgPanel"))
                        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                        .overlay(
                            RoundedRectangle(cornerRadius: Theme.radiusMD)
                                .stroke(Theme.borderWarmDefault, lineWidth: 1)
                        )
                    }

                    loopCardSection

                    Text("A loop starts running the moment it's created — there's no separate Run step.")
                        .font(.caption2)
                        .foregroundColor(Color("TextSecondary"))

                    Button(action: create) {
                        Text("Create loop")
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
        .onAppear { focusedField = .command }
    }

    // MARK: - Loop Card

    private var loopCardSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Loop Card (optional)")
                .font(.caption)
                .fontWeight(.semibold)
                .foregroundColor(Color("TextSecondary"))
            Text("Name the one number to move. Each attempt is measured, kept or reverted, and written to the ledger.")
                .font(.caption2)
                .foregroundColor(Color("TextSecondary"))
            TextField("Instrument command that prints the number", text: $instrument)
                .font(.subheadline.monospaced())
                .foregroundColor(Color("TextPrimary"))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .padding(.horizontal, 14)
                .padding(.vertical, 12)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
            Picker("Better is", selection: $lowerIsBetter) {
                Text("Lower").tag(true)
                Text("Higher").tag(false)
            }
            .pickerStyle(.segmented)
            Stepper("Keep bar: \(keepBarPercent)%", value: $keepBarPercent, in: 0...100)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
            Stepper("Paired runs: \(minPairs)", value: $minPairs, in: 2...50)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
        }
        .padding(14)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
    }

    // MARK: - Create

    private func create() {
        let trimmedCommand = command.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedCommand.isEmpty else { return }
        let trimmedExitCondition = exitCondition.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedInstrument = instrument.trimmingCharacters(in: .whitespacesAndNewlines)
        let metric: LoopMetric? = trimmedInstrument.isEmpty ? nil : LoopMetric(
            command: trimmedInstrument,
            direction: lowerIsBetter ? "lower" : "higher",
            keepBar: Double(keepBarPercent) / 100,
            noiseRuns: 5,
            minPairs: minPairs
        )
        let generator = UIImpactFeedbackGenerator(style: .light)
        generator.impactOccurred()
        onCreate(
            trimmedCommand,
            trimmedExitCondition.isEmpty ? nil : trimmedExitCondition,
            maxIterations,
            metric
        )
        dismiss()
    }
}
