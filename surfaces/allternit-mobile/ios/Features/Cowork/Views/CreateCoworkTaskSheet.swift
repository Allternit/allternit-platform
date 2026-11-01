import SwiftUI

/// Slide-up "New task" sheet (`NewProjectSheet`'s field-bubble pattern):
/// title + optional multiline description, an assignee-type segmented
/// control (Human / Agent, defaulting to whichever list segment was active
/// when the sheet opened), Create disabled until title is non-empty.
struct CreateCoworkTaskSheet: View {
    let defaultAssigneeType: CoworkAssigneeType
    /// Called with the trimmed title, the (nil-when-empty) description, and
    /// the chosen assignee type.
    let onCreate: (String, String?, CoworkAssigneeType) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var title = ""
    @State private var descriptionText = ""
    @State private var assigneeType: CoworkAssigneeType
    @FocusState private var focusedField: Field?

    private enum Field {
        case title, description
    }

    init(defaultAssigneeType: CoworkAssigneeType, onCreate: @escaping (String, String?, CoworkAssigneeType) -> Void) {
        self.defaultAssigneeType = defaultAssigneeType
        self.onCreate = onCreate
        _assigneeType = State(initialValue: defaultAssigneeType)
    }

    private var canCreate: Bool {
        !title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        VStack(spacing: 0) {
            // Header (matches ComposerPlusSheet / NewProjectSheet chrome).
            HStack {
                Text("New task")
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
                    label: "Title",
                    placeholder: "Task title",
                    text: $title,
                    field: .title,
                    submitLabel: .next
                )

                fieldBubble(
                    label: "Description",
                    placeholder: "What needs to happen? (optional)",
                    text: $descriptionText,
                    field: .description,
                    submitLabel: .done
                )

                VStack(alignment: .leading, spacing: 6) {
                    Text("Assignee")
                        .font(.caption)
                        .fontWeight(.semibold)
                        .foregroundColor(Color("TextSecondary"))
                    Menu {
                        Button(action: { assigneeType = .human }) {
                            HStack {
                                if assigneeType == .human { Image(systemName: "checkmark") }
                                Text("Human")
                            }
                        }
                        Button(action: { assigneeType = .agent }) {
                            HStack {
                                if assigneeType == .agent { Image(systemName: "checkmark") }
                                Text("Agent")
                            }
                        }
                    } label: {
                        HStack {
                            Text(assigneeType == .human ? "Human" : "Agent")
                                .font(.subheadline)
                                .foregroundColor(Color("TextPrimary"))
                            Spacer()
                            Image(systemName: "chevron.up.chevron.down")
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
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

            Spacer(minLength: 0)
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .presentationDetents([.medium])
        .presentationDragIndicator(.visible)
        .onAppear { focusedField = .title }
    }

    /// Rounded labeled input "bubble" (`NewProjectSheet`'s field chrome).
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
                .textInputAutocapitalization(.sentences)
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
        onCreate(trimmedTitle, trimmedDescription.isEmpty ? nil : trimmedDescription, assigneeType)
        dismiss()
    }
}
