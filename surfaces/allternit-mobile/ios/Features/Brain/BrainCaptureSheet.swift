import SwiftUI

/// "Capture to brain" sheet (Track D, phase D3-R2), presented from the
/// composer "+" sheet. The note is appended to the local brain repo as a
/// frontmatter page under `ideas/` (type: idea|pain, status: new), committed
/// immediately — capture succeeds offline — and pushed in the background;
/// a failed push leaves BrainStore's pending flag set for the foreground
/// retry. Chrome follows ComposerPlusSheet (header + xmark circle, detents).
struct BrainCaptureSheet: View {
    @StateObject private var brainStore = BrainStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var text = ""
    @State private var captureType = BrainCaptureType.idea
    @State private var isSaving = false
    @State private var saveError: String? = nil

    private var trimmedText: String {
        text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    var body: some View {
        VStack(spacing: 0) {
            // Header (ComposerPlusSheet chrome).
            HStack {
                Text("Capture to brain")
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

            VStack(alignment: .leading, spacing: 16) {
                Menu {
                    ForEach(BrainCaptureType.allCases, id: \.self) { type in
                        Button(action: { captureType = type }) {
                            Label(type.label, systemImage: captureType == type ? "checkmark" : "")
                        }
                    }
                } label: {
                    HStack(spacing: 8) {
                        Text(captureType.label)
                            .font(.subheadline)
                            .foregroundColor(Color("TextPrimary"))
                        Spacer()
                        Image(systemName: "chevron.down")
                            .font(.system(size: 10, weight: .bold))
                            .foregroundColor(Color("TextSecondary"))
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
                    .background(Color("BgPanel"))
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                    .overlay(
                        RoundedRectangle(cornerRadius: Theme.radiusMD)
                            .stroke(Theme.borderWarmDefault, lineWidth: 1)
                    )
                }
                .buttonStyle(.plain)

                // TextEditor has no placeholder — overlay one when empty.
                ZStack(alignment: .topLeading) {
                    TextEditor(text: $text)
                        .font(.body)
                        .foregroundColor(Color("TextPrimary"))
                        .scrollContentBackground(.hidden)
                        .padding(10)
                    if text.isEmpty {
                        Text("An idea, a pain point, a note to future you…")
                            .font(.body)
                            .foregroundColor(Color("TextSecondary"))
                            .padding(.horizontal, 15)
                            .padding(.vertical, 18)
                            .allowsHitTesting(false)
                    }
                }
                .frame(minHeight: 140)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusMD)
                        .stroke(Theme.borderWarmDefault, lineWidth: 1)
                )

                if let saveError {
                    Text(saveError)
                        .font(.caption)
                        .foregroundColor(Theme.statusWarning)
                }

                saveButton

                Spacer()
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 20)
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .presentationDetents([.medium, .large])
    }

    private var saveButton: some View {
        Button(action: save) {
            Group {
                if isSaving {
                    ProgressView()
                        .tint(.white)
                } else {
                    Text("Save to brain")
                }
            }
            .font(.headline)
            .foregroundColor(Color("OnAccent"))
            .frame(maxWidth: .infinity)
            .padding(.vertical, 16)
            .background(Color("AccentPrimary"))
            .cornerRadius(Theme.radiusMD)
        }
        .disabled(trimmedText.isEmpty || isSaving)
        .opacity(trimmedText.isEmpty ? 0.5 : 1)
    }

    /// Commit-first capture: BrainStore.capture throws only when the local
    /// commit failed — push problems queue silently in the store.
    private func save() {
        guard !trimmedText.isEmpty, !isSaving else { return }
        let generator = UIImpactFeedbackGenerator(style: .light)
        generator.impactOccurred()
        isSaving = true
        saveError = nil
        Task {
            do {
                try await brainStore.capture(text: trimmedText, type: captureType)
                dismiss()
            } catch {
                saveError = error.localizedDescription
                isSaving = false
            }
        }
    }
}
