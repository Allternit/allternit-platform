import SwiftUI

/// Settings → Memory (pushed from SettingsView). Reads the live
/// `/api/v1/memory/*` routes (stats + documents; `/query` exists but the UI
/// searches the already-fetched document list client-side). A failed
/// request shows a plain error state with retry.
struct MemorySettingsView: View {
    @ObservedObject private var settings = SettingsStore.shared

    @State private var documents: [MemoryDocument] = []
    @State private var stats: MemoryStats? = nil
    @State private var isLoading = false
    /// Non-nil when the memory request genuinely failed — the view renders
    /// an error state with retry.
    @State private var loadError: String? = nil
    @State private var searchText = ""
    @State private var selectedDocument: MemoryDocument? = nil
    @State private var refreshToken = 0

    private let client = MemoryClient()

    private var filtered: [MemoryDocument] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return documents }
        return documents.filter {
            $0.title.localizedCaseInsensitiveContains(query)
                || $0.sourceType.localizedCaseInsensitiveContains(query)
        }
    }

    var body: some View {
        ScrollView {
            // The account memory map (every bot plus your own memory).
            ImpactMapView(botId: nil)
                .padding(.horizontal, 20)
                .padding(.top, 16)
                .padding(.bottom, 8)
            MemoryAboutYouSection(refreshToken: refreshToken)
            MemoryItemsSection(refreshToken: refreshToken)
            sectionHeader("Drive")
            if loadError == nil {
                searchBar
                Divider().background(Color("BorderSubtle"))
            }
            content
        }
        .refreshable {
            refreshToken += 1
            await load()
        }
        .background(Color("BgPrimary"))
        .navigationTitle("Memory")
        .navigationBarTitleDisplayMode(.inline)
        .task { await load() }
        .sheet(item: $selectedDocument) { document in
            MemoryDocumentDetailView(document: document)
        }
    }

    private func sectionHeader(_ title: String) -> some View {
        Text(title)
            .font(.headline)
            .foregroundColor(Color("TextPrimary"))
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 20)
            .padding(.top, 20)
    }

    // MARK: - Search

    private var searchBar: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
            TextField("Search documents", text: $searchText)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            if !searchText.isEmpty {
                Button(action: { searchText = "" }) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(Color("BgSecondary"))
        .cornerRadius(10)
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        if isLoading && documents.isEmpty && loadError == nil {
            HStack {
                Spacer()
                ProgressView().padding(.top, 40)
                Spacer()
            }
        } else if let loadError, documents.isEmpty {
            FriendlyStateView(
                style: .offline,
                icon: "brain.head.profile",
                title: "Couldn't load memory",
                message: FriendlyErrorMessage.from(loadError),
                actionTitle: "Retry",
                action: { Task { await load() } }
            )
            .frame(maxWidth: .infinity)
        } else {
            VStack(alignment: .leading, spacing: 0) {
                if let loadError {
                    FriendlyInlineStateView(
                        style: .error,
                        icon: "exclamationmark.triangle",
                        title: "Refresh failed",
                        message: FriendlyErrorMessage.from(loadError),
                        actionTitle: "Retry",
                        action: { Task { await load() } }
                    )
                    .padding(.horizontal, 20)
                    .padding(.top, 12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }

                statsHeader

                // Persisted locally; the consolidation pipeline consumes this
                // flag in a later phase (no backend preference endpoint yet).
                Toggle(isOn: $settings.generateMemoryFromHistory) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Generate memory from chat history")
                            .font(.subheadline)
                            .foregroundColor(Color("TextPrimary"))
                        Text("Let Allternit learn from your chats to personalize replies.")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
                .tint(Color("AccentPrimary"))
                .padding(.horizontal, 20)
                .padding(.vertical, 12)

                Divider().background(Color("BorderSubtle")).padding(.leading, 20)

                if filtered.isEmpty {
                    FriendlyStateView(
                        style: .empty,
                        icon: searchText.isEmpty ? "brain" : "magnifyingglass",
                        title: searchText.isEmpty ? "No memories yet" : "No matches",
                        message: searchText.isEmpty
                            ? "Memories Allternit learns from your chats will appear here."
                            : "No memories match your search."
                    )
                    .frame(maxWidth: .infinity)
                } else {
                    LazyVStack(spacing: 0) {
                        ForEach(filtered) { document in
                            documentRow(document)
                            Divider().background(Color("BorderSubtle")).padding(.leading, 68)
                        }
                    }
                }
            }
        }
    }

    private var statsHeader: some View {
        HStack(spacing: 16) {
            statPill(title: "Memories", value: stats?.memoryTotal)
            statPill(title: "Documents", value: stats == nil ? nil : documents.count)
            Spacer()
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    private func statPill(title: String, value: Int?) -> some View {
        VStack(spacing: 2) {
            Text(value.map(String.init) ?? "—")
                .font(.headline)
                .foregroundColor(Color("TextPrimary"))
            Text(title)
                .font(.caption2)
                .foregroundColor(Color("TextSecondary"))
        }
        .frame(width: 88)
        .padding(.vertical, 10)
        .background(Color("BgSecondary"))
        .cornerRadius(10)
    }

    private func documentRow(_ document: MemoryDocument) -> some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            selectedDocument = document
        }) {
            HStack(spacing: 12) {
                ZStack {
                    Circle().fill(Color("AccentPrimary").opacity(0.12))
                    Image(systemName: "doc.text")
                        .font(.subheadline)
                        .foregroundColor(Color("AccentPrimary"))
                }
                .frame(width: 40, height: 40)

                VStack(alignment: .leading, spacing: 2) {
                    Text(document.title)
                        .font(.subheadline)
                        .fontWeight(.semibold)
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text("\(document.sourceType) · \(document.chunkCount) chunk\(document.chunkCount == 1 ? "" : "s")")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }

                Spacer(minLength: 8)

                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - Loading

    @MainActor
    private func load() async {
        if documents.isEmpty { isLoading = true }
        loadError = nil
        do {
            async let statsResult = client.stats()
            async let documentsResult = client.listDocuments()
            stats = try await statsResult
            documents = try await documentsResult
        } catch is CancellationError {
            // View disappeared mid-flight — keep current state.
        } catch {
            loadError = error.localizedDescription
        }
        isLoading = false
    }
}

/// Read-only detail sheet for a memory document. The document list API
/// returns metadata only (no content field), so this shows every stored
/// field verbatim.
private struct MemoryDocumentDetailView: View {
    let document: MemoryDocument
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    detailRow("Title", value: document.title)
                    detailRow("Source type", value: document.sourceType)
                    if let sourceURL = document.sourceUrl, !sourceURL.isEmpty {
                        detailRow("Source URL", value: sourceURL)
                    }
                    detailRow("Chunks", value: String(document.chunkCount))
                    detailRow("Indexed", value: document.isIndexed ? "Yes" : "No")
                    if let agentId = document.agentId, !agentId.isEmpty {
                        detailRow("Agent", value: agentId)
                    }
                    detailRow("Created", value: document.createdAt)
                    detailRow("Updated", value: document.updatedAt)
                }
                .padding(20)
            }
            .background(Color("BgPrimary"))
            .navigationTitle("Memory")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Close") { dismiss() }
                }
            }
        }
    }

    private func detailRow(_ label: String, value: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
            Text(value)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .textSelection(.enabled)
        }
    }
}

// MARK: - About you

/// Section 1: the twin profile bots speak from, plus what bots are told
/// about you right now. The full voice and bot-scope controls stay in
/// "Your twin".
private struct MemoryAboutYouSection: View {
    let refreshToken: Int
    @EnvironmentObject private var authManager: AuthManager

    private enum Phase: Equatable { case loading, unavailable, failed(String), ready }

    @State private var phase: Phase = .loading
    @State private var profile = TwinProfile(displayName: "", speakingStyle: "", signature: "", ownerDisclosure: "", updatedAt: nil)
    @State private var saved: TwinProfile? = nil
    @State private var context: TwinContext? = nil
    @State private var saving = false
    @State private var note: String? = nil
    @State private var saveError: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("About you")
                .font(.headline)
                .foregroundColor(Color("TextPrimary"))
            if !authManager.canCallAccountAPIs {
                SettingsSignedOutView(message: "Sign in to see what Allternit knows about you.")
            } else {
                switch phase {
                case .loading:
                    HStack(spacing: 10) {
                        ProgressView()
                        Text("Loading…").font(.subheadline).foregroundColor(Color("TextSecondary"))
                    }
                case .unavailable:
                    Text("Your twin isn't available on this computer yet. Update Allternit Desktop, then pull to refresh.")
                        .font(.footnote).foregroundColor(Color("TextSecondary"))
                case .failed(let message):
                    FriendlyInlineStateView(
                        style: .error, icon: "exclamationmark.triangle",
                        title: "Couldn't load your profile", message: message,
                        actionTitle: "Retry", action: { Task { await load() } }
                    )
                case .ready:
                    field("Name your bots speak for", text: $profile.displayName, prompt: "Your name", axis: false, limit: 200)
                    field("How it speaks", text: $profile.speakingStyle, prompt: "Plain and direct. Short sentences.", axis: true, limit: 2000)
                    field("Signature", text: $profile.signature, prompt: "Your name, via Allternit", axis: false, limit: 2000)
                    field("What bots may say about you", text: $profile.ownerDisclosure, prompt: "Share my availability. Never my address or finances.", axis: true, limit: 2000)
                    HStack {
                        Button {
                            Task { await save() }
                        } label: {
                            HStack(spacing: 6) {
                                if saving { ProgressView() }
                                Text(saving ? "Saving…" : "Save")
                            }
                        }
                        .disabled(saving || profile == saved)
                        Spacer()
                        NavigationLink {
                            TwinSettingsView()
                        } label: {
                            Text("Open Your twin")
                        }
                    }
                    .font(.subheadline.weight(.medium))
                    if let saveError {
                        Text(saveError).font(.caption).foregroundColor(Theme.statusError)
                    } else if let note {
                        Text(note).font(.caption).foregroundColor(Color("TextSecondary"))
                    }
                    contextBlock
                }
            }
        }
        .padding(.horizontal, 20)
        .padding(.top, 20)
        .frame(maxWidth: .infinity, alignment: .leading)
        .task(id: refreshToken) { await load() }
        .onChange(of: authManager.isSignedIn) { _, _ in Task { await load() } }
    }

    @ViewBuilder
    private var contextBlock: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("What bots are told about you")
                .font(.caption.weight(.medium))
                .foregroundColor(Color("TextSecondary"))
            if let context, context.configured, !context.context.isEmpty {
                Text(context.context)
                    .font(.footnote)
                    .foregroundColor(Color("TextPrimary"))
                    .textSelection(.enabled)
            } else {
                Text("Nothing yet. Fill in the fields above and bots will use them from their next turn.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgSecondary"))
        .cornerRadius(10)
    }

    private func field(_ label: String, text: Binding<String>, prompt: String, axis: Bool, limit: Int) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label).font(.caption.weight(.medium)).foregroundColor(Color("TextSecondary"))
            Group {
                if axis {
                    TextField(prompt, text: text, axis: .vertical).lineLimit(2...6)
                } else {
                    TextField(prompt, text: text)
                }
            }
            .font(.subheadline)
            .foregroundColor(Color("TextPrimary"))
            .padding(10)
            .background(Color("BgSecondary"))
            .cornerRadius(10)
            .onChange(of: text.wrappedValue) { _, new in
                if new.count > limit { text.wrappedValue = String(new.prefix(limit)) }
                note = nil
            }
        }
    }

    @MainActor
    private func load() async {
        guard authManager.canCallAccountAPIs else { return }
        if phase != .ready { phase = .loading }
        do {
            guard let p = try await TwinClient.shared.profile() else {
                phase = .unavailable
                return
            }
            if saved == nil || profile == saved { profile = p }
            saved = p
            context = try? await TwinClient.shared.context()
            phase = .ready
        } catch is CancellationError {
        } catch {
            if phase != .ready { phase = .failed(SettingsLoadFailure.from(error).message) }
        }
    }

    @MainActor
    private func save() async {
        saving = true
        saveError = nil
        note = nil
        defer { saving = false }
        do {
            let result = try await TwinClient.shared.saveProfile(profile)
            profile = result
            saved = result
            context = try? await TwinClient.shared.context()
            note = "Saved. Every bot uses this from its next turn."
        } catch {
            saveError = "Couldn't save. \(TwinSettingsView.errorText(error))"
        }
    }
}

// MARK: - Memories

/// Section 2: active twin memories and learned v2 facts in one searchable
/// list. Twin items edit through the shared `TwinMemoryEditor`; facts edit
/// through `PATCH memory/v2/facts/:id`.
private struct MemoryItemsSection: View {
    let refreshToken: Int
    @EnvironmentObject private var authManager: AuthManager

    private struct Row: Identifiable {
        enum Source { case twin(TwinMemory), learned(MemoryFact) }
        let source: Source
        var id: String {
            switch source {
            case .twin(let m): return "twin-\(m.id)"
            case .learned(let f): return "fact-\(f.id)"
            }
        }
        var text: String {
            switch source {
            case .twin(let m): return m.content
            case .learned(let f): return f.fact
            }
        }
        var detail: String? {
            switch source {
            case .twin(let m): return m.subject.isEmpty ? m.kind.label : "\(m.kind.label) · \(m.subject)"
            case .learned(let f): return f.memoryType
            }
        }
        var chip: String {
            switch source {
            case .twin: return "Twin"
            case .learned: return "Learned"
            }
        }
    }

    @State private var twinItems: [TwinMemory] = []
    @State private var facts: [MemoryFact] = []
    @State private var loading = true
    @State private var twinError: String? = nil
    @State private var factsError: String? = nil
    @State private var search = ""
    @State private var editingTwin: TwinMemory? = nil
    @State private var editingFact: MemoryFact? = nil
    @State private var confirmDelete: Row? = nil
    @State private var actionError: String? = nil

    private var rows: [Row] {
        let all = twinItems.filter { $0.status == "active" }.map { Row(source: .twin($0)) }
            + facts.map { Row(source: .learned($0)) }
        let q = search.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !q.isEmpty else { return all }
        return all.filter {
            $0.text.localizedCaseInsensitiveContains(q) || ($0.detail ?? "").localizedCaseInsensitiveContains(q)
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Memories")
                .font(.headline)
                .foregroundColor(Color("TextPrimary"))
            if !authManager.canCallAccountAPIs {
                SettingsSignedOutView(message: "Sign in to see your memories.")
            } else {
                searchField
                list
                if let actionError {
                    Text(actionError).font(.caption).foregroundColor(Theme.statusError)
                }
            }
        }
        .padding(.horizontal, 20)
        .padding(.top, 20)
        .frame(maxWidth: .infinity, alignment: .leading)
        .task(id: refreshToken) { await load() }
        .onChange(of: authManager.isSignedIn) { _, _ in Task { await load() } }
        .sheet(item: $editingTwin) { memory in
            TwinMemoryEditor(initial: memory) { input in
                let updated = try await TwinClient.shared.update(id: memory.id, input)
                if let i = twinItems.firstIndex(where: { $0.id == updated.id }) { twinItems[i] = updated }
            }
        }
        .sheet(item: $editingFact) { fact in
            MemoryFactEditor(fact: fact) { text in
                let result = try await MemoryFactsClient.shared.edit(id: fact.id, fact: text)
                if let i = facts.firstIndex(where: { $0.id == fact.id }) {
                    facts[i] = MemoryFact(id: result.id, userId: fact.userId, agentId: fact.agentId, fact: text,
                                          confidence: fact.confidence, validFrom: fact.validFrom, validUntil: nil,
                                          sourceObservationId: fact.sourceObservationId,
                                          memoryType: result.memoryType ?? fact.memoryType)
                }
            }
        }
        .confirmationDialog(
            "Delete this memory?",
            isPresented: Binding(get: { confirmDelete != nil }, set: { if !$0 { confirmDelete = nil } }),
            titleVisibility: .visible,
            presenting: confirmDelete
        ) { row in
            Button("Delete", role: .destructive) { Task { await remove(row) } }
            Button("Keep", role: .cancel) {}
        } message: { row in
            Text("Allternit forgets “\(row.text)”.")
        }
    }

    private var searchField: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").font(.subheadline).foregroundColor(Color("TextSecondary"))
            TextField("Search your memories", text: $search)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            if !search.isEmpty {
                Button { search = "" } label: {
                    Image(systemName: "xmark.circle.fill").font(.subheadline).foregroundColor(Color("TextSecondary"))
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(Color("BgSecondary"))
        .cornerRadius(10)
    }

    @ViewBuilder
    private var list: some View {
        if loading && twinItems.isEmpty && facts.isEmpty {
            HStack { Spacer(); ProgressView().padding(.vertical, 20); Spacer() }
        } else if let message = twinError ?? factsError, twinItems.isEmpty && facts.isEmpty {
            FriendlyInlineStateView(
                style: .error, icon: "exclamationmark.triangle",
                title: "Couldn't load memories", message: FriendlyErrorMessage.from(message),
                actionTitle: "Retry", action: { Task { await load() } }
            )
        } else {
            if let message = twinError ?? factsError {
                Text("Some memories didn't load. \(FriendlyErrorMessage.from(message))")
                    .font(.caption).foregroundColor(Theme.statusError)
            }
            if rows.isEmpty {
                Text(search.isEmpty ? "Nothing remembered yet. Memories from your chats and your twin appear here."
                                    : "No memories match your search.")
                    .font(.footnote).foregroundColor(Color("TextSecondary"))
                    .padding(.vertical, 8)
            } else {
                LazyVStack(spacing: 0) {
                    ForEach(rows) { row in
                        rowView(row)
                        Divider().background(Color("BorderSubtle"))
                    }
                }
            }
        }
    }

    private func rowView(_ row: Row) -> some View {
        HStack(alignment: .top, spacing: 10) {
            VStack(alignment: .leading, spacing: 4) {
                Text(row.text)
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                    .frame(maxWidth: .infinity, alignment: .leading)
                HStack(spacing: 6) {
                    Text(row.chip)
                        .font(.caption2.weight(.semibold))
                        .padding(.horizontal, 7).padding(.vertical, 2)
                        .background(Color("AccentPrimary").opacity(0.12))
                        .foregroundColor(Color("AccentPrimary"))
                        .clipShape(Capsule())
                    if let detail = row.detail, !detail.isEmpty {
                        Text(detail).font(.caption).foregroundColor(Color("TextSecondary"))
                    }
                }
            }
            Button { startEdit(row) } label: {
                Image(systemName: "pencil").font(.subheadline).frame(width: 32, height: 32)
            }
            .accessibilityLabel("Edit memory")
            Button { confirmDelete = row } label: {
                Image(systemName: "trash").font(.subheadline).frame(width: 32, height: 32)
            }
            .foregroundColor(Theme.statusError)
            .accessibilityLabel("Delete memory")
        }
        .buttonStyle(.plain)
        .padding(.vertical, 10)
    }

    private func startEdit(_ row: Row) {
        switch row.source {
        case .twin(let m): editingTwin = m
        case .learned(let f): editingFact = f
        }
    }

    @MainActor
    private func load() async {
        guard authManager.canCallAccountAPIs else { return }
        loading = true
        defer { loading = false }
        async let twin = Result { try await TwinClient.shared.memory() }
        async let learned = Result { try await MemoryFactsClient.shared.list() }
        switch await twin {
        case .success(let items): twinItems = items; twinError = nil
        case .failure(let e):
            // 404 = no twin routes on this computer yet; not an error to show.
            if case APIError.httpError(let status, _) = e, status == 404 { twinItems = []; twinError = nil }
            else { twinError = e.localizedDescription }
        }
        switch await learned {
        case .success(let items): facts = items; factsError = nil
        case .failure(let e):
            if case APIError.httpError(let status, _) = e, status == 404 { facts = []; factsError = nil }
            else { factsError = e.localizedDescription }
        }
    }

    @MainActor
    private func remove(_ row: Row) async {
        actionError = nil
        do {
            switch row.source {
            case .twin(let m):
                try await TwinClient.shared.remove(id: m.id)
                twinItems.removeAll { $0.id == m.id }
            case .learned(let f):
                try await MemoryFactsClient.shared.remove(id: f.id)
                facts.removeAll { $0.id == f.id }
            }
        } catch {
            actionError = "Couldn't delete that. \(TwinSettingsView.errorText(error))"
        }
    }
}

/// Edit the text of a learned fact. The server stores the edit as a new
/// version and refuses text that looks like a secret.
private struct MemoryFactEditor: View {
    let fact: MemoryFact
    let onSubmit: (String) async throws -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var text: String
    @State private var busy = false
    @State private var error: String? = nil

    init(fact: MemoryFact, onSubmit: @escaping (String) async throws -> Void) {
        self.fact = fact
        self.onSubmit = onSubmit
        _text = State(initialValue: fact.fact)
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("What to remember", text: $text, axis: .vertical).lineLimit(3...8)
                } footer: {
                    if let error { Text(error).foregroundColor(Theme.statusError) }
                }
            }
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary"))
            .navigationTitle("Edit memory")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(busy) }
                ToolbarItem(placement: .confirmationAction) {
                    if busy { ProgressView() } else { Button("Save") { Task { await submit() } } }
                }
            }
        }
        .presentationDetents([.medium, .large])
    }

    @MainActor
    private func submit() async {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { error = "Write what should be remembered."; return }
        busy = true
        error = nil
        defer { busy = false }
        do {
            try await onSubmit(trimmed)
            dismiss()
        } catch {
            if case APIError.httpError(let status, _) = error, status == 400 {
                self.error = "That text looks like a password or key, so it can't be saved."
            } else if case APIError.httpError(let status, _) = error, status == 404 {
                self.error = "That memory is gone. Pull to refresh."
            } else {
                self.error = "Couldn't save that. \(TwinSettingsView.errorText(error))"
            }
        }
    }
}
