import SwiftUI

/// Settings → Bots & agents → Subagents (the web's SubagentsPanel): the
/// agents a brain hands work to. Built-ins can be edited (an override row)
/// or reset; your own types can be made, edited and deleted. A look belongs
/// to the type, so the face picked here is the face every run of that type
/// shows in every chat (SubagentCatalog updates as soon as you save).
///
/// Phone subset: the face picker offers the office-professionals faces
/// (every one has the blink/working frames subagents need); other avatar
/// packs are picked on Desktop / the web. List layout only (no grid/deck switch).
struct SubagentsSettingsView: View {
    @EnvironmentObject private var authManager: AuthManager

    @State private var types: [SubagentTypeEntry] = SubagentTypeEntry.builtins
    @State private var loaded = false
    @State private var isLoading = false
    @State private var failure: SettingsLoadFailure? = nil
    @State private var editing: EditTarget? = nil

    struct EditTarget: Identifiable {
        let type: SubagentTypeEntry?
        let id = UUID()
    }

    private var signedIn: Bool { authManager.canCallAccountAPIs }
    private var builtins: [SubagentTypeEntry] { types.filter(\.builtin) }
    private var custom: [SubagentTypeEntry] { types.filter { !$0.builtin } }

    var body: some View {
        List {
            Section {
                Text("Your agents hand work to these when it can run in parallel, needs a fresh context, or needs other tools. Planning, debugging, docs and summaries are skills your agent runs itself.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                if !signedIn {
                    SettingsSignedOutView(message: "Sign in to edit subagents and add your own. The built-in ones work either way.")
                } else if let failure {
                    if failure == .signedOut {
                        SettingsSignedOutView(message: "Sign in to edit subagents and add your own. The built-in ones work either way.")
                    } else {
                        FriendlyInlineStateView(
                            style: failure == .offline || isComputerOffline(failure) ? .offline : .error,
                            icon: isComputerOffline(failure) ? "desktopcomputer.trianglebadge.exclamationmark" : "exclamationmark.triangle",
                            title: "Showing the built-in ones",
                            message: "Couldn't load your subagents. \(failure.message)",
                            actionTitle: isLoading ? nil : "Try again",
                            action: isLoading ? nil : { Task { await load() } }
                        )
                    }
                }
            }

            Section("Built in") {
                if isLoading && !loaded {
                    ProgressView().frame(maxWidth: .infinity)
                }
                ForEach(builtins) { row($0) }
            }

            if signedIn {
                Section {
                    if loaded && custom.isEmpty {
                        Text("No custom subagents yet. Make one for work you hand off often, like reading CI logs or writing release notes.")
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                    }
                    ForEach(custom) { row($0) }
                    Button { editing = EditTarget(type: nil) } label: {
                        Label("New subagent", systemImage: "plus")
                    }
                    .disabled(!loaded || failure != nil)
                } header: {
                    Text("Yours")
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Subagents")
        .navigationBarTitleDisplayMode(.inline)
        .task { if !loaded { await load() } }
        .refreshable { await load() }
        .onChange(of: authManager.isSignedIn) { _, _ in
            types = SubagentTypeEntry.builtins
            loaded = false
            Task { await load() }
        }
        .sheet(item: $editing) { target in
            SubagentEditorView(
                type: target.type,
                readOnly: !signedIn,
                takenSlugs: types.map(\.slug),
                onSaved: { row in apply(row) },
                onRemoved: { slug in removeRow(slug) }
            )
        }
    }

    private func isComputerOffline(_ failure: SettingsLoadFailure) -> Bool {
        if case .computerOffline = failure { return true }
        return false
    }

    private func row(_ type: SubagentTypeEntry) -> some View {
        Button { editing = EditTarget(type: type) } label: {
            HStack(spacing: 12) {
                SubagentLookImage(look: type.look, name: type.name, size: 40)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(type.name)
                            .font(.subheadline.weight(.semibold))
                            .foregroundColor(Color("TextPrimary"))
                        chip(SubagentTypeEntry.label(forClass: type.modelClass))
                        if !type.builtin { chip("Custom") } else if type.overridden { chip("Edited") }
                    }
                    Text(type.description.isEmpty ? "No description yet." : type.description)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(2)
                }
                Spacer(minLength: 0)
                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
            }
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(type.name). \(SubagentTypeEntry.label(forClass: type.modelClass)) model. \(type.description)")
    }

    private func chip(_ text: String) -> some View {
        Text(text)
            .font(.caption2.weight(.semibold))
            .foregroundColor(Color("TextSecondary"))
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Capsule().fill(Color("BgSecondary")))
    }

    @MainActor
    private func load() async {
        guard signedIn else {
            loaded = true
            return
        }
        isLoading = true
        defer { isLoading = false }
        do {
            let rows = try await SubagentTypesClient.shared.list()
            rowsCache = rows
            types = SubagentTypeEntry.merge(rows)
            failure = nil
            var looks: [String: SubagentLook] = [:]
            for row in rows { if let look = row.look { looks[row.slug] = look } }
            SubagentCatalog.shared.replaceSavedLooks(looks)
        } catch {
            failure = SettingsLoadFailure.from(error)
        }
        loaded = true
    }

    @State private var rowsCache: [SubagentTypeRow] = []

    private func apply(_ row: SubagentTypeRow) {
        rowsCache.removeAll { $0.slug == row.slug }
        rowsCache.append(row)
        types = SubagentTypeEntry.merge(rowsCache)
        SubagentCatalog.shared.setSavedLook(row.look, for: row.slug)
    }

    private func removeRow(_ slug: String) {
        rowsCache.removeAll { $0.slug == slug }
        types = SubagentTypeEntry.merge(rowsCache)
        SubagentCatalog.shared.setSavedLook(nil, for: slug)
    }
}

/// A subagent face from a look (portrait from the web's avatar packs,
/// initials until it loads).
struct SubagentLookImage: View {
    let look: SubagentLook
    let name: String
    var size: CGFloat = 40

    var body: some View {
        AsyncImage(url: look.url(working: false)) { phase in
            if let image = phase.image {
                image.resizable().scaledToFill()
            } else {
                ZStack {
                    Circle().fill(Color("AccentPrimary").opacity(0.14))
                    Text(String(name.prefix(1)).uppercased())
                        .font(.system(size: size * 0.42, weight: .bold))
                        .foregroundColor(Color("TextPrimary"))
                }
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .accessibilityHidden(true)
    }
}

/// Make, edit, reset or delete one subagent type (the web's SubagentEditor), as a sheet.
struct SubagentEditorView: View {
    let type: SubagentTypeEntry?
    let readOnly: Bool
    let takenSlugs: [String]
    let onSaved: (SubagentTypeRow) -> Void
    let onRemoved: (String) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var name: String
    @State private var description: String
    @State private var modelClass: String
    @State private var maxSteps: Int
    @State private var tools: [String]
    @State private var prompt: String
    @State private var look: SubagentLook
    @State private var busy: Busy? = nil
    @State private var error: String? = nil
    @State private var confirmDelete = false
    @State private var confirmReset = false

    enum Busy { case save, reset, delete }

    init(type: SubagentTypeEntry?, readOnly: Bool, takenSlugs: [String],
         onSaved: @escaping (SubagentTypeRow) -> Void, onRemoved: @escaping (String) -> Void) {
        self.type = type
        self.readOnly = readOnly
        self.takenSlugs = takenSlugs
        self.onSaved = onSaved
        self.onRemoved = onRemoved
        _name = State(initialValue: type?.name ?? "")
        _description = State(initialValue: type?.description ?? "")
        _modelClass = State(initialValue: type?.modelClass ?? "small")
        _maxSteps = State(initialValue: type?.maxSteps ?? 40)
        _tools = State(initialValue: type?.tools ?? ["read", "grep", "glob"])
        _prompt = State(initialValue: type?.prompt ?? "")
        _look = State(initialValue: type?.look ?? SubagentLook(pack: SubagentCatalog.pack, face: "kest"))
    }

    private var creating: Bool { type == nil }
    private var builtin: Bool { type?.builtin ?? false }
    private var slug: String { creating ? SubagentTypeEntry.deriveSlug(name) : (type?.slug ?? "") }
    private var slugProblem: String? {
        guard creating, !name.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        return SubagentTypeEntry.slugProblem(slug, taken: takenSlugs)
    }
    private var toolOptions: [String] {
        SubagentTypeEntry.toolOptions + (type?.tools ?? []).filter { !SubagentTypeEntry.toolOptions.contains($0) }
    }
    private var canSave: Bool {
        !readOnly && busy == nil && !name.trimmingCharacters(in: .whitespaces).isEmpty && slugProblem == nil && (!creating || !slug.isEmpty)
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    HStack(spacing: 12) {
                        SubagentLookImage(look: look, name: name.isEmpty ? "?" : name, size: 48)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(creating ? "New subagent" : (name.isEmpty ? (type?.name ?? "") : name))
                                .font(.headline)
                                .foregroundColor(Color("TextPrimary"))
                            Text(slug.isEmpty ? "Brains call it by a name made from yours." : "Brains call it “\(slug)”.")
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
                        }
                    }
                    if readOnly {
                        Text("Sign in to change subagents. You can still look at the built-in ones.")
                            .font(.footnote)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }

                Section {
                    TextField("Name", text: Binding(get: { name }, set: { name = String($0.prefix(60)) }))
                    if let slugProblem {
                        Text(slugProblem).font(.caption).foregroundColor(Theme.statusError)
                    }
                } header: { Text("Name") }

                Section {
                    TextField("What a brain should hand to it, e.g. “Reads failing CI logs and finds the cause.”",
                              text: Binding(get: { description }, set: { description = String($0.prefix(300)) }), axis: .vertical)
                        .lineLimit(2...5)
                } header: { Text("When to use") }

                Section {
                    Picker("Model", selection: $modelClass) {
                        ForEach(SubagentTypeEntry.modelClasses) { Text($0.label).tag($0.id) }
                    }
                    .pickerStyle(.segmented)
                    Stepper(value: $maxSteps, in: 1...500, step: maxSteps >= 20 ? 10 : 1) {
                        Text("Step cap: \(maxSteps)").font(.subheadline)
                    }
                } header: {
                    Text("Model")
                } footer: {
                    Text("\(SubagentTypeEntry.modelClasses.first { $0.id == modelClass }?.hint ?? "") It stops and reports back after \(maxSteps) steps.")
                }

                Section("Tools") {
                    ForEach(toolOptions, id: \.self) { tool in
                        Toggle(tool, isOn: Binding(
                            get: { tools.contains(tool) },
                            set: { on in if on { if !tools.contains(tool) { tools.append(tool) } } else { tools.removeAll { $0 == tool } } }
                        ))
                        .font(.subheadline)
                    }
                }

                Section {
                    if builtin {
                        Text("Built in — managed by Allternit.").font(.subheadline).foregroundColor(Color("TextSecondary"))
                    } else {
                        TextField("You are a … Do … Stop when … Report …", text: $prompt, axis: .vertical)
                            .lineLimit(4...12)
                            .font(.system(.subheadline, design: .monospaced))
                    }
                } header: { Text("Instructions") }

                Section {
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 52), spacing: 10)], spacing: 10) {
                        ForEach(SubagentCatalog.seedFaces, id: \.self) { face in
                            let option = SubagentLook(pack: SubagentCatalog.pack, face: face)
                            let selected = look.pack == option.pack && look.face == option.face
                            Button { look = SubagentLook(pack: option.pack, face: option.face, accent: look.accent) } label: {
                                SubagentLookImage(look: option, name: face, size: 48)
                                    .overlay(Circle().stroke(selected ? Color("AccentPrimary") : Color.clear, lineWidth: 3))
                                    .padding(2)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Face \(face)")
                            .accessibilityAddTraits(selected ? .isSelected : [])
                        }
                    }
                    .padding(.vertical, 4)
                } header: {
                    Text("Look")
                } footer: {
                    Text("Every run of this subagent shows this face, in every chat and on every device.")
                }

                if !creating && !readOnly && (!builtin || type?.overridden == true) {
                    Section {
                        if builtin {
                            Button { confirmReset = true } label: {
                                Label(busy == .reset ? "Resetting…" : "Reset to default", systemImage: "arrow.counterclockwise")
                            }
                        } else {
                            Button(role: .destructive) { confirmDelete = true } label: {
                                Label(busy == .delete ? "Deleting…" : "Delete", systemImage: "trash")
                            }
                        }
                    }
                    .disabled(busy != nil)
                }

                if let error {
                    Section { Text(error).font(.subheadline).foregroundColor(Theme.statusError) }
                }
            }
            .disabled(readOnly)
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary"))
            .navigationTitle(creating ? "New subagent" : (type?.name ?? "Subagent"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(readOnly ? "Close" : "Cancel") { dismiss() }.disabled(busy != nil)
                }
                if !readOnly {
                    ToolbarItem(placement: .confirmationAction) {
                        if busy == .save { ProgressView() } else {
                            Button(creating ? "Create" : "Save") { Task { await save() } }.disabled(!canSave)
                        }
                    }
                }
            }
            .confirmationDialog("Delete \(type?.name ?? "this subagent")?", isPresented: $confirmDelete, titleVisibility: .visible) {
                Button("Delete", role: .destructive) { Task { await remove(reset: false) } }
                Button("Keep", role: .cancel) {}
            } message: {
                Text("Brains can't hand work to it anymore. Past runs keep their face.")
            }
            .confirmationDialog("Reset \(type?.name ?? "this subagent")?", isPresented: $confirmReset, titleVisibility: .visible) {
                Button("Reset to default", role: .destructive) { Task { await remove(reset: true) } }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Your changes to this built-in are removed.")
            }
        }
        .interactiveDismissDisabled(busy != nil)
    }

    @MainActor
    private func save() async {
        guard canSave else { return }
        busy = .save
        error = nil
        defer { busy = nil }
        let cleanedPrompt = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        var fields = SubagentTypeFields(
            name: name.trimmingCharacters(in: .whitespaces),
            description: description.trimmingCharacters(in: .whitespacesAndNewlines),
            modelClass: modelClass,
            tools: tools,
            maxSteps: min(500, max(1, maxSteps)),
            look: look
        )
        if !builtin { fields.prompt = cleanedPrompt }
        do {
            let row: SubagentTypeRow
            if creating {
                fields.slug = slug
                row = try await SubagentTypesClient.shared.create(fields)
            } else {
                row = try await SubagentTypesClient.shared.save(slug: slug, fields)
            }
            onSaved(row)
            dismiss()
        } catch {
            self.error = "Could not save. \(Self.errorText(error))"
        }
    }

    @MainActor
    private func remove(reset: Bool) async {
        guard let type else { return }
        busy = reset ? .reset : .delete
        error = nil
        defer { busy = nil }
        do {
            try await SubagentTypesClient.shared.remove(slug: type.slug)
            onRemoved(type.slug)
            dismiss()
        } catch {
            self.error = "\(reset ? "Could not reset" : "Could not delete"). \(Self.errorText(error))"
        }
    }

    private static func errorText(_ error: Error) -> String {
        if case let APIError.httpError(status, message) = error, status == 400 || status == 409, let message {
            return message
        }
        return SettingsLoadFailure.from(error).message
    }
}
