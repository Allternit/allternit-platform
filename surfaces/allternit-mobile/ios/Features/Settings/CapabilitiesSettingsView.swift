import SwiftUI

/// Settings → Bots & agents → Skills / Plugins: the skills and plugins
/// installed on your computer, with an on/off switch each (the web's
/// SkillsSettingsPanel / PluginsSettingsPanel). Tap a row for its details
/// and to remove it from the computer.
///
/// Skills can be created here (+, the web's Create skill). Uploading a
/// skill or plugin package and the plugin marketplace stay on Desktop and
/// the web: they write a whole package tree from the desktop file picker.
struct CapabilitiesSettingsView: View {
    let kind: CapabilitiesClient.Kind

    @EnvironmentObject private var authManager: AuthManager
    @State private var items: [InstalledCapability] = []
    @State private var loaded = false
    @State private var isLoading = false
    @State private var failure: SettingsLoadFailure? = nil
    @State private var actionError: String? = nil
    @State private var pending: Set<String> = []
    @State private var search = ""
    @State private var isCreating = false

    private var noun: String { kind == .skill ? "skill" : "plugin" }
    private var title: String { kind == .skill ? "Skills" : "Plugins" }
    private var icon: String { kind == .skill ? "sparkles" : "puzzlepiece.extension" }

    private var visible: [InstalledCapability] {
        let query = search.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return items }
        return items.filter {
            $0.name.localizedCaseInsensitiveContains(query) || $0.description.localizedCaseInsensitiveContains(query)
        }
    }

    var body: some View {
        List {
            Section {
                Text(kind == .skill
                     ? "Skills your agents can use on this computer. Turn one off to keep agents from using it."
                     : "Plugin packages installed on this computer. Turn one off to keep agents from loading it.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
            }

            Section {
                if !authManager.canCallAccountAPIs {
                    SettingsSignedOutView(message: "Sign in to see the \(noun)s on your computer.")
                } else if isLoading && !loaded {
                    HStack(spacing: 10) {
                        ProgressView()
                        Text("Looking for \(noun)s on your computer…")
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                    }
                    .padding(.vertical, 6)
                } else if let failure, items.isEmpty {
                    SettingsFailureView(failure: failure, subject: "your \(noun)s", retry: { Task { await load() } })
                } else if items.isEmpty {
                    FriendlyInlineStateView(
                        style: .empty,
                        icon: icon,
                        title: "No \(noun)s installed yet",
                        message: kind == .skill
                            ? "Tap + to write one, or add skills from Settings → Skills in Allternit Desktop or on ai.allternit.com."
                            : "Add plugins from Settings → Plugins in Allternit Desktop or on ai.allternit.com."
                    )
                } else if visible.isEmpty {
                    Text("No \(noun)s match “\(search)”.")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                } else {
                    ForEach(visible) { item in
                        row(item)
                    }
                }
            } header: {
                if loaded && !items.isEmpty { Text("\(items.count) installed") }
            } footer: {
                if let actionError {
                    Text(actionError).foregroundColor(Theme.statusError)
                } else if let failure, !items.isEmpty {
                    Text("Couldn't refresh. \(failure.message)").foregroundColor(Theme.statusError)
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .searchable(text: $search, prompt: "Search \(noun)s")
        .toolbar {
            if kind == .skill && authManager.canCallAccountAPIs {
                ToolbarItem(placement: .primaryAction) {
                    Button { isCreating = true } label: { Image(systemName: "plus") }
                        .accessibilityLabel("Create skill")
                }
            }
        }
        .sheet(isPresented: $isCreating) {
            CreateSkillSheet { created in
                items.removeAll { $0.id == created.id }
                items.append(created)
                items.sort { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
            }
        }
        .task { if !loaded { await load() } }
        .refreshable { await load() }
        .onChange(of: authManager.isSignedIn) { _, signedIn in
            if signedIn { Task { await load() } } else { items = []; loaded = false }
        }
    }

    private func row(_ item: InstalledCapability) -> some View {
        HStack(spacing: 12) {
            NavigationLink {
                CapabilityDetailView(item: item, onChanged: { updated in
                    if let updated {
                        if let index = items.firstIndex(where: { $0.id == updated.id }) { items[index] = updated }
                    } else {
                        items.removeAll { $0.id == item.id }
                    }
                })
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text(item.name)
                        .font(.subheadline.weight(.medium))
                        .foregroundColor(Color("TextPrimary"))
                    if !item.description.isEmpty {
                        Text(item.description)
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(2)
                    }
                    Text(item.author)
                        .font(.caption2)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            Toggle("", isOn: Binding(
                get: { item.enabled },
                set: { next in Task { await setEnabled(item, next) } }
            ))
            .labelsHidden()
            .disabled(pending.contains(item.id))
            .accessibilityLabel("Turn \(item.name) \(item.enabled ? "off" : "on")")
        }
    }

    @MainActor
    private func load() async {
        guard authManager.canCallAccountAPIs else { return }
        isLoading = true
        failure = nil
        defer { isLoading = false }
        do {
            items = kind == .skill
                ? try await CapabilitiesClient.shared.scanSkills()
                : try await CapabilitiesClient.shared.scanPlugins()
            loaded = true
        } catch {
            failure = SettingsLoadFailure.from(error)
            loaded = true
        }
    }

    /// Optimistic, rolled back on failure (as the web's toggle does).
    @MainActor
    private func setEnabled(_ item: InstalledCapability, _ enabled: Bool) async {
        guard let index = items.firstIndex(where: { $0.id == item.id }) else { return }
        actionError = nil
        pending.insert(item.id)
        items[index].enabled = enabled
        defer { pending.remove(item.id) }
        do {
            try await CapabilitiesClient.shared.setEnabled(item, enabled: enabled)
        } catch {
            if let current = items.firstIndex(where: { $0.id == item.id }) { items[current].enabled = !enabled }
            actionError = "Couldn't turn \(item.name) \(enabled ? "on" : "off"). \(SettingsLoadFailure.from(error).message)"
        }
    }
}

/// Create skill (CustomizeView's Create skill): a name, when to use it and
/// the instructions, written to `~/.allternit/skills/<name>/` on the computer.
private struct CreateSkillSheet: View {
    let onCreated: (InstalledCapability) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var description = ""
    @State private var instructions = ""
    @State private var saving = false
    @State private var error: String?

    private var canSave: Bool {
        !name.trimmingCharacters(in: .whitespaces).isEmpty
            && !description.trimmingCharacters(in: .whitespaces).isEmpty && !saving
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Name", text: $name)
                        .textInputAutocapitalization(.words)
                    TextField("When to use it", text: $description, axis: .vertical)
                        .lineLimit(2...4)
                } footer: {
                    Text("Agents read “when to use it” to decide when to load the skill.")
                }
                Section {
                    TextField("Steps, rules and examples the agent should follow", text: $instructions, axis: .vertical)
                        .lineLimit(6...16)
                        .font(.system(.subheadline, design: .monospaced))
                } header: { Text("Instructions") } footer: {
                    Text("Saved as SKILL.md on your computer. Edit it later on Desktop or the web.")
                }
                if let error {
                    Section { Text(error).font(.subheadline).foregroundColor(Theme.statusError) }
                }
            }
            .navigationTitle("New skill")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(saving) }
                ToolbarItem(placement: .confirmationAction) {
                    if saving { ProgressView() } else {
                        Button("Create") { Task { await create() } }.disabled(!canSave)
                    }
                }
            }
            .interactiveDismissDisabled(saving)
        }
    }

    @MainActor
    private func create() async {
        saving = true
        error = nil
        defer { saving = false }
        do {
            let created = try await CapabilitiesClient.shared.createSkill(
                name: name.trimmingCharacters(in: .whitespaces),
                description: description.trimmingCharacters(in: .whitespacesAndNewlines),
                instructions: instructions
            )
            onCreated(created)
            dismiss()
        } catch let failure as CapabilitiesClient.CreateError {
            error = failure.errorDescription
        } catch {
            self.error = "Couldn't save the skill. \(SettingsLoadFailure.from(error).message)"
        }
    }
}

/// One skill or plugin: what it is, where it lives, on/off, and remove.
struct CapabilityDetailView: View {
    let item: InstalledCapability
    /// The updated item, or nil when it was removed.
    let onChanged: (InstalledCapability?) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var current: InstalledCapability
    @State private var busy = false
    @State private var error: String? = nil
    @State private var confirmRemove = false

    init(item: InstalledCapability, onChanged: @escaping (InstalledCapability?) -> Void) {
        self.item = item
        self.onChanged = onChanged
        _current = State(initialValue: item)
    }

    private var noun: String { item.kind == .skill ? "skill" : "plugin" }

    var body: some View {
        List {
            Section {
                VStack(alignment: .leading, spacing: 6) {
                    Text(current.name)
                        .font(.headline)
                        .foregroundColor(Color("TextPrimary"))
                    Text(current.description)
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                        .textSelection(.enabled)
                }
                .padding(.vertical, 4)
                Toggle(isOn: Binding(get: { current.enabled }, set: { next in Task { await setEnabled(next) } })) {
                    Text("Agents can use it").font(.subheadline)
                }
                .disabled(busy)
            }

            Section {
                detailRow("Author", current.author)
                if let version = current.version { detailRow("Version", version) }
                if let updated = Self.dateText(current.updatedAt) { detailRow("Last updated", updated) }
                VStack(alignment: .leading, spacing: 2) {
                    Text("Folder").font(.subheadline).foregroundColor(Color("TextPrimary"))
                    Text(current.dirPath)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundColor(Color("TextSecondary"))
                        .textSelection(.enabled)
                }
            }

            Section {
                Button(role: .destructive) { confirmRemove = true } label: {
                    HStack {
                        if busy { ProgressView() }
                        Text("Remove from this computer")
                    }
                }
                .disabled(busy)
            } footer: {
                if let error { Text(error).foregroundColor(Theme.statusError) }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle(current.name)
        .navigationBarTitleDisplayMode(.inline)
        .confirmationDialog("Remove \(current.name)?", isPresented: $confirmRemove, titleVisibility: .visible) {
            Button("Remove \(noun)", role: .destructive) { Task { await remove() } }
            Button("Keep", role: .cancel) {}
        } message: {
            Text("This deletes the folder \(current.dirPath) from your computer. Agents stop using it right away. This can't be undone here.")
        }
    }

    private func detailRow(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label).font(.subheadline).foregroundColor(Color("TextPrimary"))
            Spacer()
            Text(value).font(.subheadline).foregroundColor(Color("TextSecondary")).lineLimit(1)
        }
    }

    static func dateText(_ iso: String?) -> String? {
        guard let iso else { return nil }
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let date = formatter.date(from: iso) ?? ISO8601DateFormatter().date(from: iso)
        return date.map { $0.formatted(date: .abbreviated, time: .omitted) }
    }

    @MainActor
    private func setEnabled(_ enabled: Bool) async {
        busy = true
        error = nil
        let before = current
        current.enabled = enabled
        defer { busy = false }
        do {
            try await CapabilitiesClient.shared.setEnabled(before, enabled: enabled)
            onChanged(current)
        } catch {
            current = before
            self.error = "Couldn't turn it \(enabled ? "on" : "off"). \(SettingsLoadFailure.from(error).message)"
        }
    }

    @MainActor
    private func remove() async {
        busy = true
        error = nil
        defer { busy = false }
        do {
            try await CapabilitiesClient.shared.remove(current)
            onChanged(nil)
            dismiss()
        } catch {
            self.error = "Couldn't remove it. \(SettingsLoadFailure.from(error).message)"
        }
    }
}
