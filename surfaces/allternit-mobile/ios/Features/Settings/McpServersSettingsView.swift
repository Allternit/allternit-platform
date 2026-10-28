import SwiftUI

/// Settings → Bots & agents → MCP servers (and Customize → MCP servers): the
/// MCP servers on your computer (the web's McpSettingsPanel plus the
/// capabilities manager's MCP tab). Each row shows whether the runtime has it
/// connected; tap one for its tools, connect/disconnect, edit and remove.
struct McpServersSettingsView: View {
    @EnvironmentObject private var authManager: AuthManager
    @State private var servers: [McpServer] = []
    @State private var status: [String: McpLiveStatus] = [:]
    @State private var tools: [String: [McpToolDescriptor]] = [:]
    /// Whether the runtime answered; nil before the first load.
    @State private var liveReachable: Bool? = nil
    @State private var loaded = false
    @State private var isLoading = false
    @State private var failure: SettingsLoadFailure? = nil
    @State private var actionError: String? = nil
    @State private var pending: Set<String> = []
    @State private var search = ""
    @State private var isAdding = false

    private var visible: [McpServer] {
        let query = search.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return servers }
        return servers.filter { $0.name.localizedCaseInsensitiveContains(query) || $0.summary.localizedCaseInsensitiveContains(query) }
    }

    var body: some View {
        List {
            Section {
                Text("MCP servers on your computer. Agents can call their tools when a server is on.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
            }

            Section {
                if !authManager.canCallAccountAPIs {
                    SettingsSignedOutView(message: "Sign in to see the MCP servers on your computer.")
                } else if isLoading && !loaded {
                    HStack(spacing: 10) {
                        ProgressView()
                        Text("Looking for MCP servers on your computer…")
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                    }
                    .padding(.vertical, 6)
                } else if let failure, servers.isEmpty {
                    SettingsFailureView(failure: failure, subject: "your MCP servers", retry: { Task { await load() } })
                } else if servers.isEmpty {
                    FriendlyInlineStateView(
                        style: .empty,
                        icon: "cpu",
                        title: "No MCP servers yet",
                        message: "Add a server to give your agents its tools.",
                        actionTitle: "Add a server",
                        action: { isAdding = true }
                    )
                } else if visible.isEmpty {
                    Text("No servers match “\(search)”.")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                } else {
                    ForEach(visible) { server in
                        row(server)
                    }
                }
            } header: {
                if loaded && !servers.isEmpty { Text("\(servers.count) \(servers.count == 1 ? "server" : "servers")") }
            } footer: {
                if let actionError {
                    Text(actionError).foregroundColor(Theme.statusError)
                } else if let failure, !servers.isEmpty {
                    Text("Couldn't refresh. \(failure.message)").foregroundColor(Theme.statusError)
                } else if liveReachable == false, !servers.isEmpty {
                    Text("Connection status isn't available: the Allternit runtime on your computer didn't answer.")
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("MCP servers")
        .navigationBarTitleDisplayMode(.inline)
        .searchable(text: $search, prompt: "Search servers")
        .toolbar {
            if authManager.canCallAccountAPIs {
                ToolbarItem(placement: .primaryAction) {
                    Button { isAdding = true } label: { Image(systemName: "plus") }
                        .accessibilityLabel("Add a server")
                        .disabled(!loaded || (failure != nil && servers.isEmpty))
                }
            }
        }
        .sheet(isPresented: $isAdding) {
            McpServerEditorView(server: nil) { Task { await load() } }
        }
        .task { if !loaded { await load() } }
        .refreshable { await load() }
        .onChange(of: authManager.isSignedIn) { _, signedIn in
            if signedIn { Task { await load() } } else { servers = []; loaded = false }
        }
    }

    private func liveStatus(for server: McpServer) -> (name: String, status: McpLiveStatus)? {
        let lowered = Dictionary(status.map { ($0.key.lowercased(), ($0.key, $0.value)) }, uniquingKeysWith: { a, _ in a })
        for name in server.runtimeNames {
            if let hit = lowered[name] { return (hit.0, hit.1) }
        }
        return nil
    }

    private func toolsFor(_ server: McpServer) -> [McpToolDescriptor] {
        if let live = liveStatus(for: server), let list = tools[live.name.lowercased()] { return list }
        for name in server.runtimeNames { if let list = tools[name] { return list } }
        return []
    }

    private func row(_ server: McpServer) -> some View {
        let live = liveStatus(for: server)
        return HStack(spacing: 12) {
            NavigationLink {
                McpServerDetailView(
                    server: server,
                    runtimeName: live?.name,
                    initialStatus: live?.status,
                    tools: toolsFor(server),
                    liveReachable: liveReachable == true,
                    onChanged: { Task { await load() } }
                )
            } label: {
                VStack(alignment: .leading, spacing: 3) {
                    Text(server.name)
                        .font(.subheadline.weight(.medium))
                        .foregroundColor(Color("TextPrimary"))
                    Text(server.summary)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                        .truncationMode(.middle)
                    McpStatusBadge(status: live?.status, enabled: server.enabled, liveReachable: liveReachable == true)
                }
            }
            if server.isEditable {
                Toggle("", isOn: Binding(
                    get: { server.enabled },
                    set: { next in Task { await setEnabled(server, next) } }
                ))
                .labelsHidden()
                .disabled(pending.contains(server.id))
                .accessibilityLabel("Turn \(server.name) \(server.enabled ? "off" : "on")")
            }
        }
    }

    @MainActor
    private func load() async {
        guard authManager.canCallAccountAPIs else { return }
        isLoading = true
        failure = nil
        defer {
            isLoading = false
            loaded = true
        }
        async let liveTask: [String: McpLiveStatus]? = try? McpServersClient.shared.liveStatus()
        async let toolsTask: [String: [McpToolDescriptor]]? = try? McpServersClient.shared.tools()
        do {
            servers = try await McpServersClient.shared.scan()
        } catch {
            failure = SettingsLoadFailure.from(error)
        }
        let live = await liveTask
        status = live ?? [:]
        tools = (await toolsTask) ?? [:]
        liveReachable = live != nil
    }

    /// Optimistic, rolled back on failure (as the web's toggle does).
    @MainActor
    private func setEnabled(_ server: McpServer, _ enabled: Bool) async {
        guard let index = servers.firstIndex(where: { $0.id == server.id }) else { return }
        actionError = nil
        pending.insert(server.id)
        servers[index].enabled = enabled
        defer { pending.remove(server.id) }
        do {
            try await McpServersClient.shared.setEnabled(server, enabled: enabled)
        } catch {
            if let current = servers.firstIndex(where: { $0.id == server.id }) { servers[current].enabled = !enabled }
            actionError = "Couldn't turn \(server.name) \(enabled ? "on" : "off"). \(SettingsLoadFailure.from(error).message)"
        }
    }
}

/// Small status line: the runtime's state, or the saved on/off.
struct McpStatusBadge: View {
    let status: McpLiveStatus?
    let enabled: Bool
    let liveReachable: Bool

    private var text: String {
        if !enabled { return "Off" }
        if let status { return status.label }
        return liveReachable ? "Not loaded by the runtime" : "On"
    }

    private var color: Color {
        if !enabled { return Color("TextSecondary") }
        if let status {
            if status.isConnected { return Theme.statusSuccess }
            if status.isProblem { return Theme.statusError }
        }
        return Color("TextSecondary")
    }

    var body: some View {
        HStack(spacing: 5) {
            Circle().fill(color).frame(width: 6, height: 6)
            Text(text).font(.caption2).foregroundColor(Color("TextSecondary"))
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Status: \(text)")
    }
}

/// One server: status, connect/disconnect, its tools, settings, edit, remove.
struct McpServerDetailView: View {
    let server: McpServer
    let runtimeName: String?
    let tools: [McpToolDescriptor]
    let liveReachable: Bool
    let onChanged: () -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var current: McpServer
    @State private var status: McpLiveStatus?
    @State private var busy = false
    @State private var error: String? = nil
    @State private var confirmRemove = false
    @State private var isEditing = false

    init(server: McpServer, runtimeName: String?, initialStatus: McpLiveStatus?, tools: [McpToolDescriptor],
         liveReachable: Bool, onChanged: @escaping () -> Void) {
        self.server = server
        self.runtimeName = runtimeName
        self.tools = tools
        self.liveReachable = liveReachable
        self.onChanged = onChanged
        _current = State(initialValue: server)
        _status = State(initialValue: initialStatus)
    }

    var body: some View {
        List {
            Section {
                VStack(alignment: .leading, spacing: 6) {
                    Text(current.name).font(.headline).foregroundColor(Color("TextPrimary"))
                    if let description = current.userDescription {
                        Text(description).font(.subheadline).foregroundColor(Color("TextSecondary")).textSelection(.enabled)
                    }
                    McpStatusBadge(status: status, enabled: current.enabled, liveReachable: liveReachable)
                }
                .padding(.vertical, 4)
                if let message = status?.error, !message.isEmpty {
                    Text(message).font(.caption).foregroundColor(Theme.statusError).textSelection(.enabled)
                }
                if current.isEditable {
                    Toggle(isOn: Binding(get: { current.enabled }, set: { next in Task { await setEnabled(next) } })) {
                        Text("Agents can use it").font(.subheadline)
                    }
                    .disabled(busy)
                }
                if let runtimeName, status != nil {
                    if status?.isConnected == true {
                        Button { Task { await setConnected(runtimeName, false) } } label: {
                            Label("Disconnect now", systemImage: "bolt.slash")
                        }
                        .disabled(busy)
                    } else {
                        Button { Task { await setConnected(runtimeName, true) } } label: {
                            Label(status?.isProblem == true ? "Try connecting again" : "Connect now", systemImage: "bolt")
                        }
                        .disabled(busy)
                    }
                }
            } footer: {
                if runtimeName != nil, status != nil {
                    Text("Connect and disconnect apply to the running Allternit runtime until it restarts.")
                }
            }

            Section {
                if !liveReachable {
                    Text("Tools show when the Allternit runtime on your computer is online.")
                        .font(.subheadline).foregroundColor(Color("TextSecondary"))
                } else if tools.isEmpty {
                    Text(status?.isConnected == true ? "This server offers no tools." : "Connect the server to see its tools.")
                        .font(.subheadline).foregroundColor(Color("TextSecondary"))
                } else {
                    ForEach(tools) { tool in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(tool.displayName).font(.subheadline).foregroundColor(Color("TextPrimary"))
                            if tool.displayName != tool.originalName {
                                Text(tool.originalName).font(.system(.caption, design: .monospaced)).foregroundColor(Color("TextSecondary"))
                            }
                            if tool.requiresConfirmation == true {
                                Text("Asks before each use").font(.caption2).foregroundColor(Color("TextSecondary"))
                            }
                        }
                    }
                }
            } header: {
                Text(tools.isEmpty ? "Tools" : "Tools (\(tools.count))")
            }

            Section {
                if let command = current.command {
                    mono("Command", ([command] + current.args).joined(separator: " "))
                }
                if let url = current.url { mono("URL", url) }
                if !current.tags.isEmpty { detailRow("Tags", current.tags.joined(separator: ", ")) }
                if let version = current.version { detailRow("Version", version) }
                if let updated = CapabilityDetailView.dateText(current.updatedAt) { detailRow("Last updated", updated) }
                mono(current.isEditable ? "File" : "Set in", current.sourcePath)
            } footer: {
                if !current.isEditable {
                    Text("This server comes from another app's settings file. Change or remove it in that file on your computer.")
                }
            }

            if current.isEditable {
                Section {
                    Button { isEditing = true } label: { Label("Edit server", systemImage: "pencil") }
                        .disabled(busy)
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
            } else if let error {
                Section { Text(error).font(.footnote).foregroundColor(Theme.statusError) }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle(current.name)
        .navigationBarTitleDisplayMode(.inline)
        .sheet(isPresented: $isEditing) {
            McpServerEditorView(server: current) {
                onChanged()
                dismiss()
            }
        }
        .confirmationDialog("Remove \(current.name)?", isPresented: $confirmRemove, titleVisibility: .visible) {
            Button("Remove server", role: .destructive) { Task { await remove() } }
            Button("Keep", role: .cancel) {}
        } message: {
            Text("This deletes \(current.sourcePath) from your computer. Agents lose its tools. This can't be undone here.")
        }
    }

    private func detailRow(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label).font(.subheadline).foregroundColor(Color("TextPrimary"))
            Spacer()
            Text(value).font(.subheadline).foregroundColor(Color("TextSecondary")).lineLimit(1)
        }
    }

    private func mono(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label).font(.subheadline).foregroundColor(Color("TextPrimary"))
            Text(value)
                .font(.system(.caption, design: .monospaced))
                .foregroundColor(Color("TextSecondary"))
                .textSelection(.enabled)
        }
    }

    @MainActor
    private func setEnabled(_ enabled: Bool) async {
        busy = true
        error = nil
        let before = current
        current.enabled = enabled
        defer { busy = false }
        do {
            try await McpServersClient.shared.setEnabled(before, enabled: enabled)
            onChanged()
        } catch {
            current = before
            self.error = "Couldn't turn it \(enabled ? "on" : "off"). \(SettingsLoadFailure.from(error).message)"
        }
    }

    @MainActor
    private func setConnected(_ name: String, _ connect: Bool) async {
        busy = true
        error = nil
        defer { busy = false }
        do {
            if connect { try await McpServersClient.shared.connect(name) } else { try await McpServersClient.shared.disconnect(name) }
            let live = try? await McpServersClient.shared.liveStatus()
            status = live?[name] ?? status
            onChanged()
        } catch {
            self.error = "Couldn't \(connect ? "connect" : "disconnect"). \(SettingsLoadFailure.from(error).message)"
        }
    }

    @MainActor
    private func remove() async {
        busy = true
        error = nil
        defer { busy = false }
        do {
            try await McpServersClient.shared.remove(current)
            onChanged()
            dismiss()
        } catch {
            self.error = "Couldn't remove it. \(SettingsLoadFailure.from(error).message)"
        }
    }
}

/// Add or edit a server file: the web form's fields (name, description,
/// command, arguments, tags).
struct McpServerEditorView: View {
    /// nil adds a new server.
    let server: McpServer?
    let onSaved: () -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var draft: McpServersClient.Draft
    @State private var saving = false
    @State private var error: String? = nil
    @State private var confirmDiscard = false

    init(server: McpServer?, onSaved: @escaping () -> Void) {
        self.server = server
        self.onSaved = onSaved
        _draft = State(initialValue: server.map { McpServersClient.Draft($0) } ?? McpServersClient.Draft())
    }

    private var original: McpServersClient.Draft { server.map { McpServersClient.Draft($0) } ?? McpServersClient.Draft() }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Name", text: $draft.name)
                        .autocorrectionDisabled()
                    TextField("What it does (optional)", text: $draft.description, axis: .vertical)
                        .lineLimit(1...4)
                } footer: {
                    if server == nil, !draft.name.trimmingCharacters(in: .whitespaces).isEmpty {
                        Text("Saved as \(McpServersClient.newServerFolder)/\(McpServersClient.slugify(draft.name)).json on your computer.")
                    }
                }
                Section {
                    TextField("npx", text: $draft.command)
                        .font(.system(.body, design: .monospaced))
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                } header: {
                    Text("Command")
                } footer: {
                    Text("The program that starts the server on your computer.")
                }
                Section {
                    TextField("-y\n@modelcontextprotocol/server-filesystem", text: $draft.argsText, axis: .vertical)
                        .font(.system(.body, design: .monospaced))
                        .lineLimit(2...8)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                } header: {
                    Text("Arguments")
                } footer: {
                    Text("One per line.")
                }
                Section {
                    TextField("files, local", text: $draft.tagsText)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                } header: {
                    Text("Tags")
                } footer: {
                    if let error { Text(error).foregroundColor(Theme.statusError) } else { Text("Comma separated, optional.") }
                }
            }
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary"))
            .navigationTitle(server == nil ? "Add MCP server" : "Edit server")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { if draft != original { confirmDiscard = true } else { dismiss() } }
                        .disabled(saving)
                }
                ToolbarItem(placement: .confirmationAction) {
                    if saving { ProgressView() } else {
                        Button(server == nil ? "Add" : "Save") { Task { await save() } }
                            .disabled(!draft.isValid || draft == original)
                    }
                }
            }
            .confirmationDialog("Discard your changes?", isPresented: $confirmDiscard, titleVisibility: .visible) {
                Button("Discard", role: .destructive) { dismiss() }
                Button("Keep editing", role: .cancel) {}
            }
        }
        .interactiveDismissDisabled(saving || draft != original)
    }

    @MainActor
    private func save() async {
        saving = true
        error = nil
        defer { saving = false }
        do {
            if let server {
                try await McpServersClient.shared.update(server, with: draft)
            } else {
                _ = try await McpServersClient.shared.create(draft)
            }
            onSaved()
            dismiss()
        } catch {
            self.error = SettingsLoadFailure.from(error).message
        }
    }
}
