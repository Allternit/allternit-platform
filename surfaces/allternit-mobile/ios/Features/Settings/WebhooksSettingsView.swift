import SwiftUI

/// Settings → Webhooks (pushed from SettingsView's Agent section and from
/// the bot detail's Webhooks card). iOS port of the web
/// `WebhooksSettingsPanel`
/// (surfaces/ai.allternit.com/src/views/settings/WebhooksSettingsPanel.tsx):
/// searchable trigger list, create/edit sheet (name + target-bot picker),
/// active toggle, copyable inbound URL, delete with confirmation, and an
/// expandable per-row "Recent deliveries" log.
///
/// All data flows through `WebhookTriggersStore` over
/// `/api/v1/webhook-triggers`
/// (cmd/allternit-api/src/webhook_trigger_routes.rs). The signing secret is
/// generated server-side and never returned, so the inbound URL is the only
/// thing this screen can hand out.
struct WebhooksSettingsView: View {
    @StateObject private var store = WebhookTriggersStore.shared
    @StateObject private var hubStore = AgentHubStore.shared

    @State private var searchText = ""
    /// Sheet state: nil = hidden, .some(nil) = create, .some(trigger) = edit.
    @State private var editorTrigger: WebhookTrigger? = nil
    @State private var isEditorPresented = false
    /// Per-row "Recent deliveries" expansion (one at a time, like the web).
    @State private var expandedTriggerId: String? = nil
    /// Drives the transient checkmark on the copy button (1.5s, web parity).
    @State private var copiedTriggerId: String? = nil
    /// Set by the swipe action; drives the delete confirmation dialog.
    @State private var triggerPendingDeletion: WebhookTrigger? = nil
    /// Mutation failures (toggle/save/delete) surface as an alert; list
    /// failures render inline via the store's `loadError`.
    @State private var actionError: String? = nil

    /// Bots only — a trigger's target must be a packaged bot
    /// (`AgentRecord.isBot`), matching the picker on the web.
    private var bots: [AgentRecord] {
        hubStore.agents.filter(\.isBot)
    }

    private var filtered: [WebhookTrigger] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !query.isEmpty else { return store.triggers }
        return store.triggers.filter {
            $0.name.lowercased().contains(query)
                || $0.targetBotId.lowercased().contains(query)
        }
    }

    var body: some View {
        List {
            if let loadError = store.loadError, !store.triggers.isEmpty {
                Section {
                    HStack(spacing: 8) {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .foregroundColor(Theme.statusWarning)
                        Text("Refresh failed: \(loadError)")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(2)
                        Spacer()
                        Button("Retry") { store.fetchTriggersIfNeeded(force: true) }
                            .font(.caption)
                            .foregroundColor(Color("AccentPrimary"))
                    }
                }
            }

            if store.isLoading && store.triggers.isEmpty {
                Section {
                    HStack {
                        Spacer()
                        ProgressView()
                        Spacer()
                    }
                }
            } else if let loadError = store.loadError, store.triggers.isEmpty {
                let offline = isConnectionFailure(loadError)
                Section {
                    FriendlyInlineStateView(
                        style: offline ? .offline : .error,
                        icon: offline ? "wifi.slash" : "exclamationmark.triangle",
                        title: "Couldn't load webhook triggers",
                        message: FriendlyErrorMessage.from(loadError),
                        actionTitle: "Retry",
                        action: { store.fetchTriggersIfNeeded(force: true) }
                    )
                }
            } else if store.triggers.isEmpty {
                Section {
                    FriendlyStateView(
                        style: .empty,
                        icon: "link.badge.plus",
                        title: "No webhook triggers yet",
                        message: "A trigger gives external systems a signed URL that wakes one of your bots.",
                        actionTitle: "Create trigger",
                        action: { startCreate() },
                        art: .bell
                    )
                    .padding(.vertical, 12)
                }
            } else if filtered.isEmpty {
                Section {
                    FriendlyInlineStateView(
                        style: .empty,
                        icon: "magnifyingglass",
                        title: "No matches",
                        message: "No triggers match “\(searchText)”.",
                        art: .search
                    )
                }
            } else {
                ForEach(filtered) { trigger in
                    Section {
                        triggerRow(trigger)
                        inboundURLRow(trigger)
                        deliveriesDisclosureRow(trigger)
                        if expandedTriggerId == trigger.id {
                            deliveriesRows(for: trigger)
                        }
                    }
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Webhooks")
        .navigationBarTitleDisplayMode(.inline)
        .searchable(text: $searchText, prompt: "Search triggers")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(action: startCreate) {
                    Image(systemName: "plus")
                }
                .disabled(bots.isEmpty)
            }
        }
        .refreshable { await store.refresh() }
        .sheet(isPresented: $isEditorPresented) {
            WebhookTriggerEditorSheet(trigger: editorTrigger, bots: bots)
        }
        .confirmationDialog(
            "Delete this webhook trigger?",
            isPresented: Binding(
                get: { triggerPendingDeletion != nil },
                set: { if !$0 { triggerPendingDeletion = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Delete Trigger", role: .destructive) {
                if let trigger = triggerPendingDeletion {
                    Task { await delete(trigger) }
                }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("External systems posting to its inbound URL will stop waking the bot. This can't be undone.")
        }
        .alert("Webhook action failed", isPresented: Binding(
            get: { actionError != nil },
            set: { if !$0 { actionError = nil } }
        )) {
            Button("OK") { actionError = nil }
        } message: {
            Text(actionError ?? "")
        }
        .onAppear {
            store.fetchTriggersIfNeeded()
            hubStore.fetchAgentsIfNeeded()
        }
    }

    // MARK: - Trigger rows

    /// Name + target bot + active toggle. Tapping opens the edit sheet
    /// (the web row's pencil button).
    private func triggerRow(_ trigger: WebhookTrigger) -> some View {
        HStack(spacing: 12) {
            Button(action: { startEdit(trigger) }) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(trigger.name)
                        .font(.subheadline)
                        .fontWeight(.semibold)
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Label(botName(for: trigger.targetBotId), systemImage: "cpu")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            Spacer(minLength: 8)

            Toggle("", isOn: activeBinding(for: trigger))
                .labelsHidden()
                .tint(Color("AccentPrimary"))
        }
    }

    /// Copyable public inbound URL — `<apiOrigin>/webhooks/inbound/:id`
    /// (display only; the secret is server-side only).
    private func inboundURLRow(_ trigger: WebhookTrigger) -> some View {
        HStack(spacing: 8) {
            Text(WebhookTriggersClient.inboundURL(forTriggerId: trigger.id).absoluteString)
                .font(.system(.caption, design: .monospaced))
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 8)
            Button(action: { copyInboundURL(for: trigger) }) {
                Image(systemName: copiedTriggerId == trigger.id ? "checkmark" : "doc.on.doc")
                    .font(.subheadline)
                    .foregroundColor(copiedTriggerId == trigger.id
                                     ? Theme.statusSuccess
                                     : Color("TextSecondary"))
            }
            .buttonStyle(.plain)
        }
    }

    private func deliveriesDisclosureRow(_ trigger: WebhookTrigger) -> some View {
        Button(action: { toggleDeliveries(for: trigger) }) {
            HStack {
                Text("Recent deliveries")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Image(systemName: "chevron.down")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
                    .rotationEffect(.degrees(expandedTriggerId == trigger.id ? 180 : 0))
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    /// The expanded log: event · status badge · relative timestamp · error ·
    /// HTTP status (WebhooksSettingsPanel.tsx:374-409 parity).
    @ViewBuilder
    private func deliveriesRows(for trigger: WebhookTrigger) -> some View {
        let deliveries = store.deliveries[trigger.id] ?? []
        if store.loadingDeliveries.contains(trigger.id) && deliveries.isEmpty {
            HStack {
                Spacer()
                ProgressView()
                Spacer()
            }
        } else if deliveries.isEmpty {
            FriendlyInlineStateView(
                style: .empty,
                icon: "tray",
                title: "No deliveries yet",
                message: "Webhook events will appear here after they're received.",
                art: .log
            )
        } else {
            ForEach(deliveries) { delivery in
                HStack(alignment: .top, spacing: 8) {
                    VStack(alignment: .leading, spacing: 2) {
                        HStack(spacing: 6) {
                            Text(delivery.event ?? "webhook.received")
                                .font(.caption)
                                .foregroundColor(Color("TextPrimary"))
                                .lineLimit(1)
                            statusBadge(delivery.status)
                        }
                        if let date = Self.parseTimestamp(delivery.createdAt) {
                            Text(date, style: .relative)
                                .font(.caption2)
                                .foregroundColor(Color("TextSecondary"))
                        }
                        if let error = delivery.error, !error.isEmpty {
                            Text(error)
                                .font(.caption2)
                                .foregroundColor(Theme.statusError)
                                .lineLimit(2)
                        }
                    }
                    Spacer(minLength: 8)
                    if let responseStatus = delivery.responseStatus {
                        Text("HTTP \(responseStatus)")
                            .font(.caption2)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
            }
        }
    }

    private func statusBadge(_ status: String) -> some View {
        Text(status.capitalized)
            .font(.caption2)
            .fontWeight(.semibold)
            .foregroundColor(Self.statusColor(status))
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Self.statusColor(status).opacity(0.15))
            .clipShape(Capsule())
    }

    /// delivered → success, failed → error (web parity); anything else
    /// (pending) → warning.
    private static func statusColor(_ status: String) -> Color {
        switch status {
        case "delivered": return Theme.statusSuccess
        case "failed": return Theme.statusError
        default: return Theme.statusWarning
        }
    }

    // MARK: - Actions

    private func startCreate() {
        editorTrigger = nil
        isEditorPresented = true
    }

    private func startEdit(_ trigger: WebhookTrigger) {
        editorTrigger = trigger
        isEditorPresented = true
    }

    private func delete(_ trigger: WebhookTrigger) async {
        do {
            try await store.deleteTrigger(id: trigger.id)
            if expandedTriggerId == trigger.id { expandedTriggerId = nil }
        } catch {
            actionError = error.localizedDescription
        }
    }

    /// Toggle writes PATCH `{ active }` immediately; on failure the store
    /// resyncs so the switch lands back on the server's value.
    private func activeBinding(for trigger: WebhookTrigger) -> Binding<Bool> {
        Binding(
            get: { store.trigger(withId: trigger.id)?.active ?? trigger.active },
            set: { newValue in
                Task {
                    do {
                        try await store.setActive(id: trigger.id, active: newValue)
                    } catch {
                        actionError = error.localizedDescription
                        await store.refresh()
                    }
                }
            }
        )
    }

    private func copyInboundURL(for trigger: WebhookTrigger) {
        UIPasteboard.general.string = WebhookTriggersClient
            .inboundURL(forTriggerId: trigger.id)
            .absoluteString
        copiedTriggerId = trigger.id
        Task {
            try? await Task.sleep(nanoseconds: 1_500_000_000)
            if copiedTriggerId == trigger.id { copiedTriggerId = nil }
        }
    }

    private func toggleDeliveries(for trigger: WebhookTrigger) {
        if expandedTriggerId == trigger.id {
            expandedTriggerId = nil
            return
        }
        expandedTriggerId = trigger.id
        Task { await store.refreshDeliveries(forTriggerId: trigger.id) }
    }

    private func botName(for botId: String) -> String {
        hubStore.agent(withId: botId)?.botDisplayName ?? botId
    }

    private func isConnectionFailure(_ error: String) -> Bool {
        let lowered = error.lowercased()
        return lowered.contains("could not connect")
            || lowered.contains("failed to connect")
            || lowered.contains("offline")
            || lowered.contains("no network")
            || lowered.contains("network connection was lost")
    }

    /// Backend timestamps are SQLite `CURRENT_TIMESTAMP`
    /// ("yyyy-MM-dd HH:mm:ss", UTC — webhook_trigger_routes.rs); ISO-8601
    /// is accepted as a fallback (AutomationTasksListView's convention).
    private static let sqliteTimestampFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(identifier: "UTC")
        formatter.dateFormat = "yyyy-MM-dd HH:mm:ss"
        return formatter
    }()

    private static func parseTimestamp(_ value: String) -> Date? {
        if let date = try? Date(value, strategy: Date.ISO8601FormatStyle(includingFractionalSeconds: true)) {
            return date
        }
        if let date = try? Date(value, strategy: Date.ISO8601FormatStyle()) {
            return date
        }
        return sqliteTimestampFormatter.date(from: value)
    }
}

// MARK: - Create/edit sheet

/// Create/edit form (web panel's inline card, parity with
/// WebhooksSettingsPanel.tsx:200-264): name field + target-bot picker.
/// Save POSTs a new trigger or PATCHes `{ name, target_bot_id }` on the
/// existing one.
private struct WebhookTriggerEditorSheet: View {
    /// nil when creating.
    let trigger: WebhookTrigger?
    /// Bots only (`isBot`), resolved by the parent from AgentHubStore.
    let bots: [AgentRecord]

    @StateObject private var store = WebhookTriggersStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var name: String
    @State private var selectedBotId: String
    @State private var isSaving = false
    @State private var saveError: String? = nil

    init(trigger: WebhookTrigger?, bots: [AgentRecord]) {
        self.trigger = trigger
        self.bots = bots
        _name = State(initialValue: trigger?.name ?? "")
        _selectedBotId = State(initialValue: trigger?.targetBotId ?? bots.first?.id ?? "")
    }

    private var canSave: Bool {
        !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !selectedBotId.isEmpty
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("e.g. GitHub issue opened", text: $name)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                } header: {
                    Text("Name")
                }

                Section {
                    if bots.isEmpty {
                        FriendlyInlineStateView(
                            style: .empty,
                            icon: "cpu",
                            title: "No bots available yet",
                            message: "Create a bot first, then link it to this trigger.",
                            art: .gizzi
                        )
                    } else {
                        Picker("Target bot", selection: $selectedBotId) {
                            ForEach(bots) { bot in
                                Text(bot.botDisplayName).tag(bot.id)
                            }
                        }
                    }
                } footer: {
                    if trigger == nil {
                        Text("The signing secret is generated on the server and never returned by the API — callers sign requests with the secret managed outside this app.")
                    }
                }

                if let saveError {
                    Text(saveError)
                        .font(.caption)
                        .foregroundColor(Theme.statusWarning)
                }
            }
            .navigationTitle(trigger == nil ? "Create Trigger" : "Edit Trigger")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button(action: save) {
                        if isSaving {
                            ProgressView()
                        } else {
                            Text(trigger == nil ? "Create" : "Save")
                                .fontWeight(.semibold)
                        }
                    }
                    .disabled(isSaving || !canSave)
                }
            }
        }
    }

    private func save() {
        isSaving = true
        saveError = nil
        let trimmedName = name.trimmingCharacters(in: .whitespacesAndNewlines)
        Task {
            do {
                if let trigger {
                    try await store.updateTrigger(
                        id: trigger.id, name: trimmedName, targetBotId: selectedBotId
                    )
                } else {
                    try await store.createTrigger(name: trimmedName, targetBotId: selectedBotId)
                }
                dismiss()
            } catch {
                saveError = error.localizedDescription
            }
            isSaving = false
        }
    }
}
