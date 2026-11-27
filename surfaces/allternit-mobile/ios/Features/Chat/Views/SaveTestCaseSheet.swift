import SwiftUI

/// What the Save as test case sheet is showing (CONTRACT section 9 copy).
enum SaveTestCaseState: Equatable {
    case idle
    case saving
    case saved
    case needsCheck
    case needsComputer
    case unreachable
    case failed(String)
}

/// The five quick-check reasons, raw values as stored in `quick_check`.
enum QuickCheckReason: String, CaseIterable, Identifiable {
    case wrongAnswer = "wrong_answer"
    case brokeFile = "broke_file"
    case ignoredInstructions = "ignored_instructions"
    case tooSlow = "too_slow"
    case other

    var id: String { rawValue }

    var label: String {
        switch self {
        case .wrongAnswer: return "Wrong answer"
        case .brokeFile: return "Broke a file"
        case .ignoredInstructions: return "Ignored instructions"
        case .tooSlow: return "Too slow or costly"
        case .other: return "Other"
        }
    }
}

/// Pure mapping from a save outcome to the state shown, so it can be tested.
func saveTestCaseState(result: Result<EvalSaveResult, AgentEvalError>) -> SaveTestCaseState {
    switch result {
    case .success(let saved): return saved.needsCheck ? .needsCheck : .saved
    case .failure(.noComputer): return .needsComputer
    case .failure(.unreachable): return .unreachable
    case .failure(let error): return .failed(error.message)
    }
}

/// Chat ⋯ › Save as test case. A medium sheet: Went well / Went wrong, the
/// reasons (Went wrong only), a note and Save (mockups.md panel G).
struct SaveTestCaseSheet: View {
    let sessionId: String
    var client = AgentEvalClient()

    @Environment(\.dismiss) private var dismiss
    @State private var wentWrong = true
    @State private var reasons: Set<QuickCheckReason> = []
    @State private var note = ""
    @State private var state: SaveTestCaseState = .idle

    private var busy: Bool { state == .saving }
    private var done: Bool { state == .saved || state == .needsCheck }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Save as test case").font(.system(size: 17, weight: .semibold))
                Spacer()
                Button { dismiss() } label: {
                    Image(systemName: "xmark").font(.system(size: 12, weight: .semibold))
                        .frame(width: 28, height: 28)
                        .background(Color(.tertiarySystemFill), in: Circle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Close")
            }

            Picker("Result", selection: $wentWrong) {
                Text("Went well").tag(false)
                Text("Went wrong").tag(true)
            }
            .pickerStyle(.segmented)
            .disabled(busy || done)

            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    if wentWrong {
                        Text("WHAT WENT WRONG?")
                            .font(.system(size: 13)).foregroundColor(Color("TextTertiary"))
                        VStack(spacing: 0) {
                            ForEach(Array(QuickCheckReason.allCases.enumerated()), id: \.element) { index, reason in
                                if index > 0 { Divider() }
                                Button { toggle(reason) } label: {
                                    HStack {
                                        Text(reason.label).font(.system(size: 15))
                                            .foregroundColor(Color("TextPrimary"))
                                        Spacer()
                                        if reasons.contains(reason) { Image(systemName: "checkmark") .foregroundColor(Color("TextPrimary")) }
                                    }
                                    .padding(.horizontal, 12).frame(minHeight: 44)
                                    .contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                                .accessibilityAddTraits(reasons.contains(reason) ? .isSelected : [])
                            }
                        }
                        .background(Color(.quaternarySystemFill), in: RoundedRectangle(cornerRadius: 12))
                    }
                    TextField("What should have happened? (optional)", text: $note, axis: .vertical)
                        .font(.system(size: 15)).lineLimit(1...4)
                        .padding(12)
                        .background(Color(.quaternarySystemFill), in: RoundedRectangle(cornerRadius: 12))
                    statusLine
                }
            }

            Button(action: save) {
                HStack(spacing: 8) {
                    if busy { ProgressView().tint(.white) }
                    Text(buttonTitle).font(.system(size: 17, weight: .semibold))
                }
                .frame(maxWidth: .infinity).frame(height: 50)
                .foregroundColor(.white)
                .background(Color(red: 0.12, green: 0.118, blue: 0.113), in: RoundedRectangle(cornerRadius: 14))
                .opacity(state == .needsComputer ? 0.4 : 1)
            }
            .buttonStyle(.plain)
            .disabled(busy || state == .needsComputer)
        }
        .padding(.horizontal, 16).padding(.top, 20).padding(.bottom, 12)
        .background(Color.white.ignoresSafeArea())
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }

    private var buttonTitle: String {
        switch state {
        case .saving: return "Saving…"
        case .saved, .needsCheck: return "Done"
        default: return "Save"
        }
    }

    @ViewBuilder private var statusLine: some View {
        switch state {
        case .idle, .saving: EmptyView()
        case .saved: line("Saved", "checkmark.circle", nil)
        case .needsCheck:
            line("Saved. Needs a check", "exclamationmark.circle",
                 "The replay did not match your choice. Open Settings › Test cases to check it.")
        case .needsComputer: line("Needs a computer", "desktopcomputer", "Test cases are saved on your computer.")
        case .unreachable: line("Couldn't reach your computer, try again", "wifi.exclamationmark", nil)
        case .failed(let text): line(text, "exclamationmark.triangle", nil)
        }
    }

    private func line(_ title: String, _ symbol: String, _ detail: String?) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Label(title, systemImage: symbol).font(.system(size: 15, weight: .medium))
            if let detail {
                Text(detail).font(.system(size: 13)).foregroundColor(Color("TextTertiary"))
            }
        }
        .foregroundColor(Color("TextPrimary"))
        .accessibilityElement(children: .combine)
    }

    private func toggle(_ reason: QuickCheckReason) {
        Haptics.selection()
        if reasons.contains(reason) { reasons.remove(reason) } else { reasons.insert(reason) }
    }

    private func save() {
        if done { dismiss(); return }
        state = .saving
        let label = wentWrong ? "bad" : "good"
        let picked = wentWrong ? QuickCheckReason.allCases.filter { reasons.contains($0) }.map(\.rawValue) : []
        Task {
            let result: Result<EvalSaveResult, AgentEvalError>
            do {
                result = .success(try await client.saveCase(sessionId: sessionId, label: label,
                                                            note: note, quickCheck: picked))
            } catch {
                result = .failure(AgentEvalError.map(error, conflict: .caseLimit))
            }
            await MainActor.run {
                state = saveTestCaseState(result: result)
                if done { Haptics.success() }
            }
        }
    }
}
