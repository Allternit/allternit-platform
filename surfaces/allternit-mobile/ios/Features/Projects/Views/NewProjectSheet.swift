import SwiftUI

/// Slide-up "New project" sheet (replaces the old single-field alert):
/// project name + optional description in rounded field "bubbles", with a
/// Create button that stays disabled until the name is non-empty.
struct NewProjectSheet: View {
    /// Called with the trimmed title and the (nil-when-empty) description.
    let onCreate: (String, String?) -> Void
    /// "Cover and pad": creates the project and opens its cover and pad
    /// editor. Nil hides the row (the sheet from the composer).
    var onCreateAndStyle: ((String, String?) -> Void)? = nil
    /// The project type; nil hides the picker (chat project).
    var kind: Binding<LibraryProject.Mode>? = nil

    @ReducedMotion private var reduced
    @Environment(\.dismiss) private var dismiss
    @State private var title = ""
    @State private var descriptionText = ""
    @FocusState private var focusedField: Field?

    private enum Field {
        case title, description
    }

    /// Bot: the description is the bot's brief.
    private var isBot: Bool { kind?.wrappedValue == .bot }

    private var canCreate: Bool {
        !title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        VStack(spacing: 0) {
            // Header: ✕ left, ✓ right (phone calm step 27).
            HStack {
                Button(action: { dismiss() }) {
                    Image(systemName: "xmark")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 44, height: 44)
                        .background(Color("FillNeutral"))
                        .clipShape(Circle())
                }
                .accessibilityLabel("Close")
                .accessibilityIdentifier("newProjectClose")
                Spacer()
                Text("New project")
                    .accessibilityIdentifier("newProjectTitle")
                    .font(.system(.title3, design: .serif))
                    .fontWeight(.medium)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Button(action: create) {
                    Image(systemName: "checkmark")
                        .font(.system(size: 14, weight: .bold))
                        .foregroundColor(canCreate ? Color("BgPrimary") : Color("TextTertiary"))
                        .frame(width: 44, height: 44)
                        .background(Circle().fill(canCreate ? Color("TextPrimary") : Color("FillNeutral")))
                }
                .disabled(!canCreate)
                .accessibilityLabel("Create project")
                .accessibilityIdentifier("newProjectCreate")
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 14)

            Divider().background(Color("BorderSubtle"))

            ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                // The folder cover, live with the typed name.
                FolderCoverView(title: title.isEmpty ? "New project" : title,
                                mode: kind?.wrappedValue.rawValue ?? "chat",
                                appearance: nil, width: 120, flat: true)
                    .frame(maxWidth: .infinity)
                    .padding(.top, 4)
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel("Folder cover for \(title.isEmpty ? "the new project" : title)")

                if let kind {
                    Picker("Type", selection: kind) {
                        Text("Chat").tag(LibraryProject.Mode.chat)
                        Text("Code").tag(LibraryProject.Mode.code)
                        Text("Design").tag(LibraryProject.Mode.design)
                        Text("Bot").tag(LibraryProject.Mode.bot)
                    }
                    .pickerStyle(.segmented)
                }

                fieldBubble(
                    label: "Name",
                    placeholder: "Project name",
                    text: $title,
                    field: .title,
                    submitLabel: .next
                )

                fieldBubble(
                    label: isBot ? "Brief" : "Description",
                    placeholder: isBot ? "What should this bot do? (optional)" : "What is this project about? (optional)",
                    text: $descriptionText,
                    field: .description,
                    submitLabel: .done
                )

                if let onCreateAndStyle, (kind?.wrappedValue ?? .chat) == .chat {
                    Button {
                        let t = title.trimmingCharacters(in: .whitespacesAndNewlines)
                        let d = descriptionText.trimmingCharacters(in: .whitespacesAndNewlines)
                        Haptics.light()
                        onCreateAndStyle(t, d.isEmpty ? nil : d)
                        dismiss()
                    } label: {
                        HStack {
                            Text("Cover and pad")
                                .font(.subheadline.weight(.medium))
                            Spacer()
                            Image(systemName: "chevron.right").font(.system(size: 12, weight: .semibold))
                        }
                        .foregroundColor(canCreate ? Color("TextPrimary") : Color("TextTertiary"))
                        .padding(.horizontal, 14)
                        .frame(minHeight: 48)
                        .background(Color("BgPanel"))
                        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                    }
                    .buttonStyle(.plain)
                    .disabled(!canCreate)
                    .accessibilityHint("Creates the project, then opens its cover and pad editor")
                }

                Button(action: create) {
                    Text(isBot ? "Create bot" : "Create project")
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
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .onAppear {
            Haptics.light()
            focusedField = .title
        }
        .animation(reduced ? Motion.fade : nil, value: kind?.wrappedValue)
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
                .accessibilityIdentifier(field == .title ? "newProjectName" : "")
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
        Haptics.light()
        onCreate(trimmedTitle, trimmedDescription.isEmpty ? nil : trimmedDescription)
        dismiss()
    }
}
