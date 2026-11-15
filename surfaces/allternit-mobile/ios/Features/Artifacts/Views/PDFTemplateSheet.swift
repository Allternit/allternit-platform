import SwiftUI

/// "PDF from template": pick a template, theme and header style, then make a
/// real PDF on the server and hand it to the share sheet. Uses the artifact's
/// own JSON when it has some; otherwise the template's sample, and says so.
struct PDFTemplateSheet: View {
    let artifactTitle: String
    let artifactBody: String
    let onFile: (URL) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var template: PDFTemplate = .invoice
    @State private var theme: PDFTheme = .professional
    @State private var variant: String?
    /// Letter by default in the US region, A4 elsewhere; the same two sizes as web.
    @State private var pageSize: String = Locale.current.region?.identifier == "US" ? "letter" : "a4"
    @State private var isWorking = false
    @State private var errorText: String?

    private var data: [String: Any]? { PDFTemplateClient.jsonObject(from: artifactBody) }

    var body: some View {
        NavigationStack {
            Form {
                Section("Template") {
                    Picker("Template", selection: $template) {
                        ForEach(PDFTemplate.allCases) { Text($0.label).tag($0) }
                    }
                    .pickerStyle(.inline)
                    .labelsHidden()
                }
                Section("Look") {
                    Picker("Theme", selection: $theme) {
                        ForEach(PDFTheme.allCases) { Text($0.label).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    if template.variants.count > 1 {
                        Picker("Header", selection: Binding(get: { variant ?? template.variants[0] }, set: { variant = $0 })) {
                            ForEach(template.variants, id: \.self) { Text($0.capitalized).tag($0) }
                        }
                        .pickerStyle(.segmented)
                    }
                    Picker("Paper", selection: $pageSize) {
                        Text("Letter").tag("letter")
                        Text("A4").tag("a4")
                    }
                    .pickerStyle(.segmented)
                }
                Section {
                    Text(data == nil
                         ? "This artifact has no JSON data, so the template's sample is used. Edit the data on the web or Desktop."
                         : "Uses the data in this artifact.")
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                }
                if let errorText {
                    Section { Text(errorText).font(.footnote).foregroundColor(.red) }
                }
            }
            .navigationTitle("PDF from template")
            .navigationBarTitleDisplayMode(.inline)
            .onChange(of: template) { _ in variant = nil }
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Close") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(isWorking ? "Making…" : "Make PDF") { Task { await make() } }
                        .disabled(isWorking)
                }
            }
        }
    }

    private func make() async {
        isWorking = true
        errorText = nil
        defer { isWorking = false }
        do {
            let pdf = try await PDFTemplateClient.render(template: template, theme: theme, variant: variant, pageSize: pageSize, data: data ?? template.sample)
            let base = artifactTitle.trimmingCharacters(in: .whitespacesAndNewlines)
            let url = try ArtifactFileShare.write(pdf, suggestedName: "\(base.isEmpty ? template.label : base).pdf", mimeType: "application/pdf")
            dismiss()
            onFile(url)
        } catch {
            errorText = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}
