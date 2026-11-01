import SwiftUI

/// Slide-up "New project" sheet (replaces the old single-field alert):
/// project name + optional description in rounded field "bubbles", with a
/// Create button that stays disabled until the name is non-empty.
struct NewProjectSheet: View {
    /// Called with the trimmed title and the (nil-when-empty) description.
    let onCreate: (String, String?) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var title = ""
    @State private var descriptionText = ""
    @FocusState private var focusedField: Field?

    private enum Field {
        case title, description
    }

    private var canCreate: Bool {
        !title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        VStack(spacing: 0) {
            // Header (matches ComposerPlusSheet / ProjectsListView chrome).
            HStack {
                Text("New project")
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
                fieldBubble(
                    label: "Name",
                    placeholder: "Project name",
                    text: $title,
                    field: .title,
                    submitLabel: .next
                )

                fieldBubble(
                    label: "Description",
                    placeholder: "What is this project about? (optional)",
                    text: $descriptionText,
                    field: .description,
                    submitLabel: .done
                )

                Button(action: create) {
                    Text("Create project")
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

            Spacer(minLength: 0)
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .presentationDetents([.medium])
        .presentationDragIndicator(.visible)
        .onAppear { focusedField = .title }
    }

    /// Rounded labeled input "bubble": caption above a soft-bg rounded field,
    /// matching the composer card's corner language.
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
            TextField(placeholder, text: text, axis: field == .description ? .vertical : .horizontal)
                .lineLimit(field == .description ? 3...5 : 1...1)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .textInputAutocapitalization(field == .title ? .words : .sentences)
                .submitLabel(submitLabel)
                .focused($focusedField, equals: field)
                .onSubmit {
                    if field == .title {
                        focusedField = .description
                    } else if canCreate {
                        create()
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
        let trimmedTitle = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedTitle.isEmpty else { return }
        let trimmedDescription = descriptionText.trimmingCharacters(in: .whitespacesAndNewlines)
        let generator = UIImpactFeedbackGenerator(style: .light)
        generator.impactOccurred()
        onCreate(trimmedTitle, trimmedDescription.isEmpty ? nil : trimmedDescription)
        dismiss()
    }
}
