import SwiftUI

/// Settings → Bots & agents → Your twin (the web's TwinPanel / TwinView):
/// one voice and one memory for all your bots. Set how they speak for you
/// and what they may say, see what they know and where they learned it, and
/// review facts a bot learned in a conversation (nothing reaches a bot until
/// you accept it).
struct TwinSettingsView: View {
    @EnvironmentObject private var authManager: AuthManager

    private enum Phase: Equatable { case loading, unavailable, failed(SettingsLoadFailure), ready }

    @State private var phase: Phase = .loading
    @State private var profile = TwinProfile(displayName: "", speakingStyle: "", signature: "", ownerDisclosure: "", updatedAt: nil)
    @State private var savedProfile: TwinProfile? = nil
    @State private var items: [TwinMemory] = []
    @State private var savingProfile = false
    @State private var profileNote: String? = nil
    @State private var profileError: String? = nil
    @State private var busyId: String? = nil
    @State private var listError: String? = nil
    @State private var editing: TwinMemory? = nil
    @State private var isAdding = false
    @State private var confirmDelete: TwinMemory? = nil

    private var proposed: [TwinMemory] { items.filter(\.isProposed) }
    private var active: [TwinMemory] { items.filter { $0.status == "active" } }

    var body: some View {
        List {
            Section {
                Text("One voice and one memory for all your bots. Set how they speak for you and what they may say, and see what they know and where they learned it.")
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
            }
            content
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Your twin")
        .navigationBarTitleDisplayMode(.inline)
        .task { await load() }
        .refreshable { await load() }
        .onChange(of: authManager.isSignedIn) { _, _ in Task { await load() } }
        .sheet(isPresented: $isAdding) {
            TwinMemoryEditor(initial: nil) { input in
                let memory = try await TwinClient.shared.add(input)
                items.insert(memory, at: 0)
            }
        }
        .sheet(item: $editing) { memory in
            TwinMemoryEditor(initial: memory) { input in
                let updated = try await TwinClient.shared.update(id: memory.id, input)
                replace(updated)
            }
        }
        .confirmationDialog(
            confirmDelete?.isProposed == true ? "Dismiss this suggestion?" : "Delete this memory?",
            isPresented: Binding(get: { confirmDelete != nil }, set: { if !$0 { confirmDelete = nil } }),
            titleVisibility: .visible,
            presenting: confirmDelete
        ) { memory in
            Button(memory.isProposed ? "Dismiss" : "Delete", role: .destructive) {
                Task { await remove(memory) }
            }
            Button("Keep", role: .cancel) {}
        } message: { memory in
            Text(memory.isProposed ? "Your bots won't use it." : "Every bot forgets “\(memory.content)” from its next turn.")
        }
    }

    @ViewBuilder
    private var content: some View {
        if !authManager.canCallAccountAPIs {
            Section { SettingsSignedOutView(message: "Sign in to set up your twin.") }
        } else {
            switch phase {
            case .loading:
                Section {
                    HStack(spacing: 10) {
                        ProgressView()
                        Text("Loading your twin…").font(.subheadline).foregroundColor(Color("TextSecondary"))
                    }
                }
            case .unavailable:
                Section {
                    FriendlyInlineStateView(
                        style: .empty,
                        icon: "person.2.wave.2",
                        title: "Not on this computer yet",
                        message: "Your twin isn't available on this computer yet. Update Allternit Desktop, then try again.",
                        actionTitle: "Try again",
                        action: { Task { await load() } }
                    )
                }
            case .failed(let failure):
                Section {
                    SettingsFailureView(failure: failure, subject: "your twin", retry: { Task { await load() } })
                }
            case .ready:
                voiceSection
                if !proposed.isEmpty {
                    Section {
                        ForEach(proposed) { memoryRow($0) }
                    } header: {
                        Text("Suggested by your bots (\(proposed.count))")
                    } footer: {
                        Text("Your bots learned these in conversations. None is used until you accept it.")
                    }
                }
                Section {
                    if active.isEmpty {
                        Text("Nothing saved yet.").font(.subheadline).foregroundColor(Color("TextSecondary"))
                    } else {
                        ForEach(active) { memoryRow($0) }
                    }
                    Button { isAdding = true } label: {
                        Label("Add a memory", systemImage: "plus")
                    }
                } header: {
                    Text("What your bots know")
                } footer: {
                    if let listError {
                        Text(listError).foregroundColor(Theme.statusError)
                    } else {
                        Text("Shared by all your bots and their vendor bots, unless you limit a fact to one bot or to yourself.")
                    }
                }
            }
        }
    }

    private var voiceSection: some View {
        Section {
            field("Name your bots speak for", text: $profile.displayName, prompt: "Your name", axis: false)
            field("How it speaks", text: $profile.speakingStyle, prompt: "Plain and direct. Short sentences. No exclamation marks.", axis: true)
            field("Signature", text: $profile.signature, prompt: "Your name, via Allternit", axis: false)
            field("What bots may say about you", text: $profile.ownerDisclosure, prompt: "Share my availability and company name. Never my address or finances.", axis: true)
            Button {
                Task { await saveProfile() }
            } label: {
                HStack {
                    if savingProfile { ProgressView() }
                    Text(savingProfile ? "Saving…" : "Save")
                }
            }
            .disabled(savingProfile || profile == savedProfile)
        } header: {
            Text("Voice")
        } footer: {
            if let profileError {
                Text(profileError).foregroundColor(Theme.statusError)
            } else if let profileNote {
                Text(profileNote)
            } else {
                Text("Left empty, bots share nothing about you beyond what the task needs.")
            }
        }
    }

    private func field(_ label: String, text: Binding<String>, prompt: String, axis: Bool) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label).font(.caption.weight(.medium)).foregroundColor(Color("TextSecondary"))
            if axis {
                TextField(prompt, text: limited(text, 2000), axis: .vertical)
                    .lineLimit(2...6)
                    .font(.subheadline)
            } else {
                TextField(prompt, text: limited(text, label == "Signature" ? 2000 : 200))
                    .font(.subheadline)
            }
        }
        .padding(.vertical, 2)
        .onChange(of: text.wrappedValue) { _, _ in profileNote = nil }
    }

    private func limited(_ binding: Binding<String>, _ max: Int) -> Binding<String> {
        Binding(get: { binding.wrappedValue }, set: { binding.wrappedValue = String($0.prefix(max)) })
    }

    private func memoryRow(_ memory: TwinMemory) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Group {
                if memory.subject.isEmpty {
                    Text(memory.content)
                } else {
                    Text(memory.subject).bold() + Text(": ") + Text(memory.content)
                }
            }
            .font(.subheadline)
            .foregroundColor(Color("TextPrimary"))
            Text("\(memory.kind.label) · \(Self.visibilityText(memory)) · \(Self.provenanceText(memory))")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
            HStack(spacing: 8) {
                if memory.isProposed {
                    pill("Accept") { Task { await accept(memory) } }
                        .accessibilityLabel("Accept: \(memory.content)")
                }
                pill("Edit") { editing = memory }
                    .accessibilityLabel("Edit: \(memory.content)")
                pill(memory.isProposed ? "Dismiss" : "Delete") { confirmDelete = memory }
                    .accessibilityLabel("\(memory.isProposed ? "Dismiss" : "Delete"): \(memory.content)")
                if busyId == memory.id { ProgressView().controlSize(.small) }
            }
            .disabled(busyId == memory.id)
        }
        .padding(.vertical, 4)
    }

    private func pill(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.caption.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 12)
                .frame(minHeight: 32)
                .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
    }

    // MARK: - Text (twin TwinView.tsx provenanceText / visibilityText)

    static func visibilityText(_ memory: TwinMemory) -> String {
        if memory.visibility == .bot, let bot = memory.botId { return "Only \(bot)" }
        return memory.visibility.label
    }

    static func provenanceText(_ memory: TwinMemory) -> String {
        let p = memory.provenance
        let date = CapabilityDetailView.dateText(p.learnedAt)
        if p.source == "owner" { return "Added by you" + (date.map { ", \($0)" } ?? "") }
        var text = "Learned by \(p.botId ?? "a bot")"
        if let channel = p.channel, !channel.isEmpty { text += " on \(channel)" }
        if let date { text += ", \(date)" }
        if let confidence = p.confidence { text += " (\(Int((confidence * 100).rounded()))% sure)" }
        return text
    }

    // MARK: - Actions

    @MainActor
    private func load() async {
        guard authManager.canCallAccountAPIs else { return }
        if phase != .ready { phase = .loading }
        do {
            guard let p = try await TwinClient.shared.profile() else {
                phase = .unavailable
                return
            }
            if savedProfile == nil || profile == savedProfile { profile = p }
            savedProfile = p
            items = try await TwinClient.shared.memory()
            phase = .ready
        } catch {
            if phase != .ready { phase = .failed(SettingsLoadFailure.from(error)) }
            else { listError = "Couldn't refresh. \(SettingsLoadFailure.from(error).message)" }
        }
    }

    @MainActor
    private func saveProfile() async {
        savingProfile = true
        profileError = nil
        profileNote = nil
        defer { savingProfile = false }
        do {
            let saved = try await TwinClient.shared.saveProfile(profile)
            profile = saved
            savedProfile = saved
            profileNote = "Saved. Every bot uses this from its next turn."
        } catch {
            profileError = "Couldn't save your twin. \(Self.errorText(error))"
        }
    }

    private func replace(_ memory: TwinMemory) {
        if let index = items.firstIndex(where: { $0.id == memory.id }) { items[index] = memory }
    }

    @MainActor
    private func accept(_ memory: TwinMemory) async {
        await act(memory, fallback: "Couldn't accept that.") {
            replace(try await TwinClient.shared.accept(id: memory.id))
        }
    }

    @MainActor
    private func remove(_ memory: TwinMemory) async {
        await act(memory, fallback: "Couldn't delete that.") {
            try await TwinClient.shared.remove(id: memory.id)
            items.removeAll { $0.id == memory.id }
        }
    }

    @MainActor
    private func act(_ memory: TwinMemory, fallback: String, _ run: () async throws -> Void) async {
        listError = nil
        busyId = memory.id
        defer { busyId = nil }
        do {
            try await run()
        } catch let APIError.httpError(status, _) where status == 404 {
            // Already gone elsewhere: the list was stale.
            items.removeAll { $0.id == memory.id }
        } catch {
            listError = "\(fallback) \(Self.errorText(error))"
        }
    }

    /// twin-api.ts `twinErrorText`.
    static func errorText(_ error: Error) -> String {
        if case let APIError.httpError(status, _) = error {
            switch status {
            case 400: return "That isn't valid. Check the fields and try again."
            case 401, 403: return "Your Allternit session expired. Sign in again to continue."
            case 429: return "Too many suggestions are waiting. Review them first."
            default: break
            }
        }
        return SettingsLoadFailure.from(error).message
    }
}

/// Add or edit one twin memory (the web's MemoryForm), as a sheet.
struct TwinMemoryEditor: View {
    let initial: TwinMemory?
    let onSubmit: (TwinMemoryInput) async throws -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var kind: TwinKind
    @State private var subject: String
    @State private var content: String
    @State private var visibility: TwinVisibility
    @State private var busy = false
    @State private var error: String? = nil

    init(initial: TwinMemory?, onSubmit: @escaping (TwinMemoryInput) async throws -> Void) {
        self.initial = initial
        self.onSubmit = onSubmit
        _kind = State(initialValue: initial?.kind ?? .fact)
        _subject = State(initialValue: initial?.subject ?? "")
        _content = State(initialValue: initial?.content ?? "")
        _visibility = State(initialValue: initial?.visibility ?? .all)
    }

    private var botId: String? { initial?.botId }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Picker("Type", selection: $kind) {
                        ForEach(TwinKind.allCases, id: \.self) { Text($0.label).tag($0) }
                    }
                    Picker("Who can see it", selection: $visibility) {
                        Text(TwinVisibility.all.label).tag(TwinVisibility.all)
                        if let botId { Text("Only \(botId)").tag(TwinVisibility.bot) }
                        Text(TwinVisibility.owner.label).tag(TwinVisibility.owner)
                    }
                }
                Section("About (optional)") {
                    TextField("Acme, Dana, Fridays…", text: Binding(get: { subject }, set: { subject = String($0.prefix(200)) }))
                }
                Section {
                    TextField("I don't take calls before 10am.", text: Binding(get: { content }, set: { content = String($0.prefix(1000)) }), axis: .vertical)
                        .lineLimit(3...8)
                } header: {
                    Text("What to remember")
                } footer: {
                    if let error { Text(error).foregroundColor(Theme.statusError) }
                }
            }
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary"))
            .navigationTitle(initial == nil ? "New memory" : "Edit memory")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(busy) }
                ToolbarItem(placement: .confirmationAction) {
                    if busy { ProgressView() } else {
                        Button(initial == nil ? "Add" : "Save") { Task { await submit() } }
                    }
                }
            }
        }
        .presentationDetents([.medium, .large])
    }

    @MainActor
    private func submit() async {
        let text = content.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else {
            error = "Write what your bots should know."
            return
        }
        busy = true
        error = nil
        defer { busy = false }
        do {
            try await onSubmit(TwinMemoryInput(
                kind: kind,
                subject: subject.trimmingCharacters(in: .whitespacesAndNewlines),
                content: text,
                visibility: visibility,
                botId: visibility == .bot ? botId : nil
            ))
            dismiss()
        } catch {
            self.error = "Couldn't save that. \(TwinSettingsView.errorText(error))"
        }
    }
}
