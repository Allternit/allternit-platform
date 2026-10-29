import SwiftUI

/// A place skills and plugins come from (web PersonalMarketplaceSource). The
/// web keeps these per device (localStorage
/// `allternit:plugin-manager:personal-marketplaces:v1`); so does the app.
/// The phone adds GitHub repos and URLs; uploads and local folders are added
/// from a computer.
struct PersonalSource: Codable, Identifiable, Equatable {
    var id: String
    var type: String
    var value: String
    var label: String?
    var createdAt: String
    var isDevMode: Bool?
}

enum PersonalSources {
    private static let key = "allternit:plugin-manager:personal-marketplaces:v1"
    private static let types: Set<String> = ["github", "url", "upload", "local"]

    static func load() -> [PersonalSource] {
        guard let data = UserDefaults.standard.data(forKey: key),
              let list = try? JSONDecoder().decode([PersonalSource].self, from: data) else { return [] }
        return list.filter { types.contains($0.type) }
    }

    private static func save(_ list: [PersonalSource]) {
        if let data = try? JSONEncoder().encode(list) { UserDefaults.standard.set(data, forKey: key) }
    }

    /// handleAddPersonalMarketplaceSource: trimmed, no duplicates.
    static func add(type: String, value: String, label: String) {
        let value = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else { return }
        var list = load()
        guard !list.contains(where: { $0.type == type && $0.value == value }) else { return }
        let label = label.trimmingCharacters(in: .whitespacesAndNewlines)
        let suffix = String(UUID().uuidString.prefix(5)).lowercased()
        list.append(PersonalSource(
            id: "personal-\(type)-\(Int(Date().timeIntervalSince1970 * 1000))-\(suffix)",
            type: type, value: value, label: label.isEmpty ? nil : label,
            createdAt: ISO8601DateFormatter().string(from: Date())
        ))
        save(list)
    }

    static func remove(id: String) {
        save(load().filter { $0.id != id })
    }
}

/// Customize → Sources (web SourcesSection).
struct CustomizeSourcesView: View {
    @State private var sources = PersonalSources.load()
    @State private var isAdding = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                DirectoryPageHeader(title: "Sources",
                                    subtitle: "Where skills and plugins come from: marketplaces, GitHub repos, URLs, and local folders you've added.")
                VStack(spacing: 8) {
                    ForEach(sources) { source in
                        HStack(spacing: 12) {
                            Image(systemName: "books.vertical")
                                .font(.system(size: 16))
                                .foregroundColor(Color("TextSecondary"))
                                .frame(width: 36, height: 36)
                                .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderSubtle"), lineWidth: 1))
                            VStack(alignment: .leading, spacing: 2) {
                                Text(source.label ?? source.value)
                                    .font(.system(size: 14, weight: .medium))
                                    .foregroundColor(Color("TextPrimary"))
                                    .lineLimit(1)
                                Text("\(source.type) · \(source.value)")
                                    .font(.system(size: 12))
                                    .foregroundColor(Color("TextTertiary"))
                                    .lineLimit(1)
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                            Button {
                                PersonalSources.remove(id: source.id)
                                sources = PersonalSources.load()
                            } label: {
                                Image(systemName: "xmark")
                                    .font(.system(size: 12, weight: .semibold))
                                    .foregroundColor(Color("TextTertiary"))
                                    .frame(width: 32, height: 32)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Remove \(source.label ?? source.value)")
                        }
                        .padding(14)
                        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                    }
                    if sources.isEmpty {
                        Text("No sources added yet.")
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextSecondary"))
                            .frame(maxWidth: .infinity)
                            .padding(.horizontal, 24)
                            .padding(.vertical, 40)
                            .overlay(
                                RoundedRectangle(cornerRadius: 12, style: .continuous)
                                    .strokeBorder(Color("BorderSubtle"), style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
                            )
                    }
                }
                Button { isAdding = true } label: {
                    Label("Add source", systemImage: "plus")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 12)
                        .padding(.vertical, 7)
                        .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderSubtle"), lineWidth: 1))
                }
                .buttonStyle(.plain)
                .padding(.top, 16)
            }
            .padding(.horizontal, 24)
            .padding(.bottom, 48)
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .navigationTitle("Customize")
        .navigationBarTitleDisplayMode(.inline)
        .sheet(isPresented: $isAdding, onDismiss: { sources = PersonalSources.load() }) {
            AddSourceSheet()
        }
    }
}

/// Customize → Publish (web PublishSection).
struct CustomizePublishView: View {
    /// "Start publishing": the web opens the plugin manager on Plugins.
    let onStart: () -> Void
    /// "Published artifacts": the Artifacts library.
    let onPublished: () -> Void

    private let steps: [(String, String)] = [
        ("Package", "Bundle skills, commands, and connectors into a plugin."),
        ("Validate", "Check the manifest, permissions, and file layout."),
        ("Submit", "Publish to the Allternit marketplace or share a link."),
    ]

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                DirectoryPageHeader(title: "Publish",
                                    subtitle: "Share what you've built. Published artifacts from sessions live in Artifacts.")
                VStack(spacing: 8) {
                    ForEach(Array(steps.enumerated()), id: \.offset) { index, step in
                        HStack(spacing: 12) {
                            Text("\(index + 1)")
                                .font(.system(size: 12))
                                .foregroundColor(Color("TextPrimary"))
                                .frame(width: 24, height: 24)
                                .overlay(Circle().stroke(Color("BorderSubtle"), lineWidth: 1))
                            VStack(alignment: .leading, spacing: 2) {
                                Text(step.0)
                                    .font(.system(size: 14, weight: .medium))
                                    .foregroundColor(Color("TextPrimary"))
                                Text(step.1)
                                    .font(.system(size: 12))
                                    .foregroundColor(Color("TextSecondary"))
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .padding(14)
                        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                    }
                }
                HStack(spacing: 8) {
                    Button(action: onStart) {
                        Text("Start publishing")
                            .font(.system(size: 13, weight: .medium))
                            .foregroundColor(Color("OnAccent"))
                            .padding(.horizontal, 12)
                            .padding(.vertical, 7)
                            .background(RoundedRectangle(cornerRadius: 8).fill(Color("TextPrimary")))
                    }
                    Button(action: onPublished) {
                        Text("Published artifacts")
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextPrimary"))
                            .padding(.horizontal, 12)
                            .padding(.vertical, 7)
                            .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderSubtle"), lineWidth: 1))
                    }
                }
                .buttonStyle(.plain)
                .padding(.top, 16)
            }
            .padding(.horizontal, 24)
            .padding(.bottom, 48)
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .navigationTitle("Customize")
        .navigationBarTitleDisplayMode(.inline)
    }
}

/// Add a marketplace source: a GitHub repo or a URL, with an optional name.
struct AddSourceSheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var type = "github"
    @State private var value = ""
    @State private var label = ""

    private var placeholder: String { type == "github" ? "owner/repo" : "https://example.com/marketplace.json" }

    var body: some View {
        NavigationStack {
            Form {
                Picker("Type", selection: $type) {
                    Text("GitHub repo").tag("github")
                    Text("URL").tag("url")
                }
                .pickerStyle(.segmented)
                Section {
                    TextField(placeholder, text: $value)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(type == "url" ? .URL : .default)
                    TextField("Name (optional)", text: $label)
                } footer: {
                    Text("Uploads and local folders are added from Allternit on your computer.")
                }
            }
            .navigationTitle("Add source")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add") {
                        PersonalSources.add(type: type, value: value, label: label)
                        dismiss()
                    }
                    .disabled(value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
        }
        .presentationDetents([.medium])
    }
}

/// ViewHeader on a Customize sub-page: serif title and a secondary line.
private struct DirectoryPageHeader: View {
    let title: String
    let subtitle: String

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.system(size: 28, design: .serif))
                .foregroundColor(Color("TextPrimary"))
            Text(subtitle)
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.top, 8)
        .padding(.bottom, 20)
    }
}
