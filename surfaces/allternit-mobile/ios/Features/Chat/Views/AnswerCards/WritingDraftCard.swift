import SwiftUI
import UIKit

/// A `writing` draft from a reply: the draft, up to 2 alternatives, and
/// Copy, Edit and Open in Mail or Messages. Nothing here sends: Mail and
/// Messages open with the draft filled in and the user taps Send there.
struct WritingDraftCard: View {
    let title: String
    let draft: AnswerWritingDraft

    /// The user's edits (nil = the draft as written). Kept per card while it's on screen.
    @State private var editedSubject: String?
    @State private var editedBody: String?
    /// Which version is in use: nil = the main draft, else an alternative's index.
    @State private var chosen: Int?
    @State private var isEditing = false
    @State private var copied = false
    @Environment(\.openURL) private var openURL

    private var subject: String { editedSubject ?? draft.subject ?? "" }
    private var body_: String {
        if let editedBody { return editedBody }
        if let chosen, let alternatives = draft.alternatives, alternatives.indices.contains(chosen) {
            return alternatives[chosen].body
        }
        return draft.body
    }

    var body: some View {
        AnswerCardChrome {
            HStack(spacing: 8) {
                Text(draft.variantLabel)
                    .font(.caption.weight(.semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background(Capsule().fill(Color("FillNeutral")))
                if !title.isEmpty {
                    Text(title)
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(2)
                }
            }
            .accessibilityElement(children: .combine)
            .accessibilityAddTraits(.isHeader)

            if let to = draft.to, !to.isEmpty {
                field("To", to)
            }
            if !subject.isEmpty {
                field("Subject", subject)
            }

            Text(body_)
                .font(.body)
                .foregroundColor(Color("TextPrimary"))
                .frame(maxWidth: .infinity, alignment: .leading)
                .textSelection(.enabled)
                .padding(10)
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
                .accessibilityLabel("Draft: \(body_)")

            if let alternatives = draft.alternatives, !alternatives.isEmpty, editedBody == nil {
                versionPicker(alternatives)
            } else if editedBody != nil {
                Button("Undo edits") {
                    editedBody = nil
                    editedSubject = nil
                }
                .font(.caption.weight(.semibold))
                .foregroundColor(Color("AccentPrimary"))
            }

            actions
        }
        .sheet(isPresented: $isEditing) {
            WritingDraftEditSheet(
                showsSubject: draft.variant == "email",
                subject: subject,
                body: body_,
                onSave: { newSubject, newBody in
                    editedSubject = newSubject
                    editedBody = newBody
                }
            )
        }
    }

    private func field(_ label: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(label)
                .font(.caption.weight(.semibold))
                .foregroundColor(Color("TextSecondary"))
            Text(value)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .textSelection(.enabled)
        }
        .accessibilityElement(children: .combine)
    }

    /// Main draft plus its labelled alternatives; picking one swaps the draft shown and used.
    private func versionPicker(_ alternatives: [AnswerWritingDraft.Alternative]) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                versionChip("Draft", selected: chosen == nil) { chosen = nil }
                ForEach(Array(alternatives.enumerated()), id: \.offset) { index, alternative in
                    versionChip(alternative.label, selected: chosen == index) { chosen = index }
                }
            }
        }
    }

    private func versionChip(_ label: String, selected: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .font(.caption.weight(.medium))
                .foregroundColor(selected ? Color("TextPrimary") : Color("TextSecondary"))
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
                .background(Capsule().fill(selected ? Color("FillNeutral") : Color.clear))
                .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Version: \(label)")
        .accessibilityAddTraits(selected ? AccessibilityTraits.isSelected : [])
    }

    private var actions: some View {
        HStack(spacing: 8) {
            actionButton(copied ? "Copied" : "Copy", symbol: copied ? "checkmark" : "doc.on.doc", main: !hasHandoff) {
                UIPasteboard.general.string = subject.isEmpty || draft.variant != "email" ? body_ : "\(subject)\n\n\(body_)"
                copied = true
                Task {
                    try? await Task.sleep(for: .seconds(1.5))
                    copied = false
                }
            }
            actionButton("Edit", symbol: "pencil") { isEditing = true }
            Spacer(minLength: 0)
            if draft.variant == "email" {
                actionButton("Open in Mail", symbol: "envelope", main: true) {
                    if let url = WritingDraftLinks.mailto(to: draft.to, subject: subject, body: body_) { openURL(url) }
                }
                .accessibilityHint("Opens Mail with this draft. You send it from there.")
            } else if draft.variant == "text_message" {
                actionButton("Open in Messages", symbol: "message", main: true) {
                    if let url = MessageComposer.smsURL(to: WritingDraftLinks.phoneNumber(draft.to) ?? "", text: body_) { openURL(url) }
                }
                .accessibilityHint("Opens Messages with this draft. You send it from there.")
            }
        }
    }

    /// Mail and Messages hand-off drafts have one main button; others lead with Copy.
    private var hasHandoff: Bool { draft.variant == "email" || draft.variant == "text_message" }

    /// `main` is the card's one filled button; the rest stay quiet neutral pills.
    private func actionButton(_ label: String, symbol: String, main: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(label, systemImage: symbol)
                .font(.caption.weight(.semibold))
                .foregroundColor(main ? Color("BgPrimary") : Color("TextPrimary"))
                .padding(.horizontal, main ? 12 : 10)
                .padding(.vertical, 6)
                .background(Capsule().fill(main ? Color("TextPrimary") : Color("FillNeutral")))
        }
        .buttonStyle(.plain)
    }
}

/// Edits a draft in place on the card. Saving changes only what the card shows and copies.
private struct WritingDraftEditSheet: View {
    let showsSubject: Bool
    @State var subject: String
    @State var body_: String
    let onSave: (String?, String) -> Void
    @Environment(\.dismiss) private var dismiss

    init(showsSubject: Bool, subject: String, body: String, onSave: @escaping (String?, String) -> Void) {
        self.showsSubject = showsSubject
        _subject = State(initialValue: subject)
        _body_ = State(initialValue: body)
        self.onSave = onSave
    }

    var body: some View {
        NavigationStack {
            Form {
                if showsSubject {
                    Section("Subject") {
                        TextField("Subject", text: $subject)
                    }
                }
                Section("Draft") {
                    TextEditor(text: $body_)
                        .frame(minHeight: 220)
                        .accessibilityLabel("Draft")
                }
            }
            .navigationTitle("Edit draft")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") {
                        onSave(showsSubject ? subject : nil, body_)
                        dismiss()
                    }
                    .disabled(body_.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
        }
        .presentationDetents([.medium, .large])
    }
}

enum WritingDraftLinks {
    /// Characters left unescaped in a mailto query value (same rule as MessageComposer.smsURL).
    private static let allowed: CharacterSet = {
        var set = CharacterSet.urlQueryAllowed
        set.remove(charactersIn: "&+=#?%/;:@$,")
        return set
    }()

    /// `mailto:<address>?subject=…&body=…`. `to` goes in only when it is an
    /// address; a name ("Sam") leaves the recipient for the user to fill in.
    static func mailto(to: String?, subject: String, body: String) -> URL? {
        let address = to.flatMap { emailAddress($0) } ?? ""
        var query: [String] = []
        if !subject.isEmpty, let s = subject.addingPercentEncoding(withAllowedCharacters: allowed) { query.append("subject=\(s)") }
        if let b = body.addingPercentEncoding(withAllowedCharacters: allowed) { query.append("body=\(b)") }
        let encodedAddress = address.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? ""
        return URL(string: "mailto:\(encodedAddress)" + (query.isEmpty ? "" : "?" + query.joined(separator: "&")))
    }

    static func emailAddress(_ raw: String) -> String? {
        let trimmed = raw.trimmingCharacters(in: .whitespaces)
        return trimmed.range(of: #"^[^\s@<>]+@[^\s@<>]+\.[^\s@<>]+$"#, options: .regularExpression) != nil ? trimmed : nil
    }

    /// A phone number when `to` is one; a name leaves the recipient empty.
    static func phoneNumber(_ raw: String?) -> String? {
        guard let raw else { return nil }
        let digits = raw.filter { $0.isNumber }
        let looksLikeNumber = raw.allSatisfy { $0.isNumber || " +-().".contains($0) }
        return looksLikeNumber && digits.count >= 3 ? raw : nil
    }
}
