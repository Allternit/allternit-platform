import SwiftUI
import UniformTypeIdentifiers

/// Teach (PhoneTeachOverlay → the desktop ACI rail's ApiCaptureView, in its
/// phone layout): the skills the agent was taught, each one's steps and
/// inputs, and Replay / Run on a schedule. Recording runs in Allternit
/// Desktop or the browser extension; here you import a recording (.har)
/// and Analyze turns it into a skill.
struct TeachView: View {
    /// Replay: the agent runs this prompt in ACI's browser.
    var onRun: (String) -> Void = { _ in }

    @Environment(\.dismiss) private var dismiss

    @State private var skills: [TeachSkill] = []
    @State private var contracts: [TeachContract] = []
    @State private var loaded = false
    @State private var loading = false
    /// Why the list didn't load, nil when it did. Skills live on the person's
    /// computer, so "no computer" is its own screen (`computerOffline`).
    @State private var failure: SettingsLoadFailure? = nil
    @State private var signedOut = false
    @State private var isConnectOpen = false
    @ObservedObject private var relay = RuntimeRelayStore.shared
    @ObservedObject private var auth = AuthManager.shared
    @State private var notice: String? = nil
    @State private var selectedId: String? = nil
    @State private var review: Review? = nil
    @State private var isImporting = false
    @State private var sheet: Sheet? = nil
    @State private var isDeleteConfirming = false

    /// What Analyze will send: an imported recording, or a recording
    /// already on the server that has no skill yet.
    struct Review: Equatable {
        var har: String?
        var fileName: String?
        var contract: TeachContract?
        /// The recording's actions (a Desktop recording); a .har or a
        /// server recording has none, only network traffic.
        var events: [JSONValue] = []
        /// Actions struck out in Review: not sent.
        var struck: Set<Int> = []
        var note = ""
        var analyzing = false
        var error: String? = nil
    }

    enum Sheet: Identifiable {
        case replay(TeachSkill), schedule(TeachSkill), run(TeachSkill), skillFile(TeachSkill), profile
        var id: String {
            switch self {
            case .replay(let skill): return "replay-\(skill.id)"
            case .schedule(let skill): return "schedule-\(skill.id)"
            case .run(let skill): return "run-\(skill.id)"
            case .skillFile(let skill): return "file-\(skill.id)"
            case .profile: return "profile"
            }
        }
    }

    private var unanalyzed: [TeachContract] {
        contracts.filter { contract in !skills.contains { ($0.raw["contract_id"]) == .string(contract.id) } }
    }

    private var selected: TeachSkill? { skills.first { $0.id == selectedId } }
    private var showList: Bool { !skills.isEmpty || !unanalyzed.isEmpty || (loading && !loaded) }
    private var isComputerOffline: Bool {
        if case .computerOffline = failure { return true }
        return false
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            if signedOut {
                TeachBanner(text: "\(TeachClient.signInToSave) Recording and review still work here.", tone: .info)
            } else if let failure, !isComputerOffline {
                TeachBanner(text: failure.message, tone: .error, onRetry: { Task { await load() } })
            }
            if let notice {
                TeachBanner(text: notice, tone: .info, onDismiss: { self.notice = nil })
            }
            if showList {
                skillList
                Rectangle().fill(Color("BorderSubtle")).frame(height: 1)
            }
            detail
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .task { await load() }
        .fileImporter(isPresented: $isImporting,
                      allowedContentTypes: [UTType(filenameExtension: "har") ?? .json, .json],
                      allowsMultipleSelection: false) { result in
            importRecording(result)
        }
        .sheet(item: $sheet) { sheet in
            switch sheet {
            case .replay(let skill):
                TeachReplaySheet(skill: skill) { values in
                    self.sheet = nil
                    replay(skill, values: values)
                }
            case .schedule(let skill):
                TeachScheduleSheet(skill: skill) { routineId in
                    self.sheet = nil
                    notice = "“\(skill.title)” is scheduled. It’s in Automation Tasks."
                    if let next = skill.with(["routine_id": .string(routineId)]) { Task { await save(next) } }
                }
            case .run(let skill):
                TeachRunSheet(skill: skill) {
                    self.sheet = nil
                    if skill.inputs.isEmpty {
                        replay(skill, values: [:])
                    } else {
                        // The sheet closes first; the inputs sheet follows.
                        Task {
                            try? await Task.sleep(nanoseconds: 350_000_000)
                            self.sheet = .replay(skill)
                        }
                    }
                }
            case .skillFile(let skill):
                TeachSkillFileSheet(skill: skill)
            case .profile:
                TeachProfileSheet()
            }
        }
        .sheet(isPresented: $isConnectOpen, onDismiss: { Task { await load() } }) {
            ConnectComputerSheet()
        }
        .confirmationDialog("Delete this skill?", isPresented: $isDeleteConfirming, titleVisibility: .visible) {
            Button("Delete skill", role: .destructive) { deleteSelected() }
        } message: {
            Text("The agent won't be able to replay it any more.")
        }
    }

    // MARK: - Header

    /// PhoneTeachOverlay: "‹ ACI" back, "Teach" title.
    private var header: some View {
        ZStack {
            Text("Teach")
                .font(.system(size: 22, weight: .medium, design: .serif))
                .tracking(-0.2)
                .foregroundColor(Color("TextPrimary"))
                .accessibilityAddTraits(.isHeader)
            HStack {
                Button { dismiss() } label: {
                    HStack(spacing: 3) {
                        Image(systemName: "chevron.left").font(.system(size: 15, weight: .bold))
                        Text("ACI").font(.system(size: 15))
                    }
                    .foregroundColor(Color("TextPrimary"))
                    .frame(minHeight: 44)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Back to ACI")
                Spacer()
            }
        }
        .padding(.horizontal, 14)
        .frame(height: 52)
        .overlay(alignment: .bottom) { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
    }

    // MARK: - Skill list (TeachSkillList)

    private var skillList: some View {
        VStack(spacing: 0) {
            HStack {
                Text("SKILLS")
                    .font(.system(size: 11, weight: .semibold))
                    .tracking(1.3)
                    .foregroundColor(Color("TextTertiary"))
                Spacer()
                if review == nil {
                    Button { isImporting = true } label: {
                        HStack(spacing: 4) {
                            PhIcon("record-fill", size: 11).foregroundColor(Theme.statusError)
                            Text("Teach").font(.system(size: 12))
                        }
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.horizontal, 8)
                        .frame(minHeight: 40)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Teach: import a recording")
                }
            }
            .padding(.horizontal, 14)
            .padding(.top, 10)
            ScrollView {
                LazyVStack(spacing: 4) {
                    if loading && skills.isEmpty {
                        Text("Loading skills…")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextTertiary"))
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(10)
                    }
                    ForEach(skills) { skill in
                        skillRow(skill)
                    }
                    ForEach(unanalyzed) { contract in
                        contractRow(contract)
                    }
                }
                .padding(.horizontal, 8)
                .padding(.bottom, 10)
            }
        }
        .frame(maxHeight: UIScreen.main.bounds.height * 0.38)
        .fixedSize(horizontal: false, vertical: skills.count + unanalyzed.count <= 3)
    }

    private func skillRow(_ skill: TeachSkill) -> some View {
        let on = review == nil && selectedId == skill.id
        return Button {
            review = nil
            selectedId = skill.id
        } label: {
            VStack(alignment: .leading, spacing: 3) {
                Text(skill.title)
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(on ? Color("TextPrimary") : Color("TextSecondary"))
                    .lineLimit(1)
                Text(skill.subtitle)
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextTertiary"))
                    .lineLimit(1)
                if skill.skillPath != nil || skill.routineId != nil {
                    HStack(spacing: 4) {
                        if skill.skillPath != nil { TeachBadge(icon: "check-circle-fill", text: "Skill saved") }
                        if skill.routineId != nil { TeachBadge(icon: "calendar-blank", text: "Scheduled") }
                    }
                    .padding(.top, 2)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(on ? Color("FillNeutral") : .clear))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    private func contractRow(_ contract: TeachContract) -> some View {
        let on = review?.contract?.id == contract.id
        return Button {
            review = Review(contract: contract)
            // The list carries no endpoints; Review counts them.
            Task {
                guard let full = try? await TeachClient.shared.contract(id: contract.id),
                      review?.contract?.id == full.id else { return }
                review?.contract = full
            }
        } label: {
            VStack(alignment: .leading, spacing: 2) {
                Text(contract.domain)
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(on ? Color("TextPrimary") : Color("TextSecondary"))
                    .lineLimit(1)
                Text("Not analyzed\(contract.endpoints.isEmpty ? "" : " · \(contract.endpoints.count)")")
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextTertiary"))
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(on ? Color("FillNeutral") : .clear))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - Detail

    @ViewBuilder
    private var detail: some View {
        if review != nil {
            reviewPane
        } else if let skill = selected {
            skillDetail(skill)
        } else if isComputerOffline {
            computerOffline
        } else if loading {
            VStack(spacing: 10) {
                ProgressView()
                Text(relay.isWaking ? "Waking your computer…" : "Loading…")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextTertiary"))
            }
        } else {
            emptyTeach
        }
    }

    /// No computer answered. Skills and recordings are stored on it, so the
    /// phone can't list them or analyze a recording without one.
    private var computerOffline: some View {
        VStack(spacing: 0) {
            PhIcon("desktop", size: 28)
                .foregroundColor(Color("TextPrimary"))
                .frame(width: 72, height: 72)
                .background(Circle().fill(Color("FillNeutral")))
            Text(relay.devices.isEmpty ? "Connect a computer to see your skills" : "Can’t reach your computer")
                .font(.system(size: 20, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .multilineTextAlignment(.center)
                .padding(.top, 20)
                .padding(.bottom, 4)
            Text("Teach skills and recordings are saved on your computer. Open Allternit Desktop or start your cloud computer, then try again.")
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
                .multilineTextAlignment(.center)
                .lineSpacing(3)
                .frame(maxWidth: 340)
            HStack(spacing: 8) {
                Button(relay.devices.isEmpty ? "Connect" : "Fix") { isConnectOpen = true }
                    .buttonStyle(TeachPrimaryButton())
                Button("Try again") { Task { await load() } }
                    .buttonStyle(TeachSecondaryButton())
            }
            .padding(.top, 18)
        }
        .padding(.horizontal, 16)
    }

    /// EmptyTeach: Desktop's record mark and words. Nothing records on the
    /// phone, so its action is the phone's own: import a recording.
    private var emptyTeach: some View {
        VStack(spacing: 0) {
            PhIcon("record-fill", size: 28)
                .foregroundColor(.white)
                .frame(width: 72, height: 72)
                .background(Circle().fill(Theme.statusError))
            Text("Show the agent this site")
                .font(.system(size: 20, weight: .semibold))
                .tracking(-0.2)
                .foregroundColor(Color("TextPrimary"))
                .padding(.top, 20)
                .padding(.bottom, 4)
            Text("Click through the flow once in Allternit Desktop or the Allternit browser extension. We record the path and save it as a skill the agent can replay.")
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
                .multilineTextAlignment(.center)
                .lineSpacing(3)
                .frame(maxWidth: 340)
            Button { isImporting = true } label: {
                HStack(spacing: 6) {
                    PhIcon("upload-simple", size: 14)
                    Text("Import a recording")
                }
            }
            .buttonStyle(TeachSecondaryButton())
            .padding(.top, 18)
            .accessibilityLabel("Import a recording (.har)")
        }
        .padding(.horizontal, 16)
    }

    // MARK: - Review (TeachReview)

    private var reviewPane: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Review before Analyze")
                        .font(.system(size: 18, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text("Check what will be sent, then Analyze turns it into a skill.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                }
                VStack(alignment: .leading, spacing: 6) {
                    Text("What will be sent")
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                    if let contract = review?.contract {
                        Text(contract.endpoints.isEmpty
                             ? "The recording of \(contract.domain)."
                             : "The recording of \(contract.domain): \(contract.endpoints.count) API call\(contract.endpoints.count == 1 ? "" : "s").")
                    } else {
                        Text("Network traffic from \(review?.fileName ?? "the recording"). No actions or narration: those come from Allternit Desktop.")
                    }
                }
                .font(.system(size: 12))
                .foregroundColor(Color("TextSecondary"))
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(14)
                .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderSubtle")))

                // TeachReview "Actions": the recording's timeline; × leaves one out.
                VStack(alignment: .leading, spacing: 6) {
                    Text("Actions")
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                    if !(review?.events.isEmpty ?? true) {
                        Text("Use × to leave an action out.")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextTertiary"))
                    }
                    TeachTimeline(
                        events: TeachEvent.list(review?.events ?? []),
                        struck: review?.struck ?? [],
                        onToggleStrike: { index in
                            if review?.struck.contains(index) == true { review?.struck.remove(index) } else { review?.struck.insert(index) }
                        }
                    )
                }

                VStack(alignment: .leading, spacing: 6) {
                    Text("Your note")
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                    TextField("What should the agent know? e.g. the order number changes every time.",
                              text: Binding(get: { review?.note ?? "" }, set: { review?.note = $0 }),
                              axis: .vertical)
                        .lineLimit(2...5)
                        .font(.system(size: 14))
                        .padding(10)
                        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
                }
                if let error = review?.error {
                    Text(error)
                        .font(.system(size: 12))
                        .foregroundColor(Theme.statusError)
                }
                HStack(spacing: 10) {
                    Button("Discard") { review = nil }
                        .buttonStyle(TeachSecondaryButton())
                        .disabled(review?.analyzing == true)
                    Spacer()
                    Button { analyze() } label: {
                        HStack(spacing: 6) {
                            if review?.analyzing == true { ProgressView().controlSize(.small).tint(Color("BgPrimary")) }
                            Text(review?.analyzing == true ? "Analyzing…" : "Analyze")
                        }
                    }
                    .buttonStyle(TeachPrimaryButton())
                    .disabled(review?.analyzing == true)
                }
            }
            .padding(16)
        }
    }

    // MARK: - Skill (TeachRunBar + TeachEditor)

    private func skillDetail(_ skill: TeachSkill) -> some View {
        TeachSkillEditor(
            skill: skill,
            blocked: signedOut,
            onSave: { draft in await save(draft) },
            onReplay: { draft in
                if draft.inputs.isEmpty { replay(draft, values: [:]) } else { sheet = .replay(draft) }
            },
            onSchedule: { saved in sheet = .schedule(saved) },
            onRun: { draft in sheet = .run(draft) },
            onSaveSkill: { draft in sheet = .skillFile(draft) },
            onOpenProfile: { sheet = .profile },
            onTeachAgain: { isImporting = true },
            onDelete: { isDeleteConfirming = true }
        )
        .id(skill.id)
    }

    private func load() async {
        loading = true
        failure = nil
        signedOut = false
        defer { loading = false; loaded = true }
        guard auth.canCallAccountAPIs else {
            signedOut = true
            return
        }
        do {
            skills = try await TeachClient.shared.listSkills()
            if selectedId == nil || !skills.contains(where: { $0.id == selectedId }) {
                selectedId = skills.first?.id
            }
        } catch is CancellationError {
            return
        } catch {
            if case APIError.httpError(let status, _) = error, status == 401 {
                signedOut = true
            } else if case APIError.httpError(let status, _) = error, status == 404 {
                // A computer on an older Allternit has no Teach routes.
                failure = .error("This computer doesn’t have Teach yet. Update Allternit Desktop, then try again.")
            } else {
                failure = SettingsLoadFailure.from(error)
                if isComputerOffline { await relay.refresh() }
            }
            return
        }
        do {
            contracts = try await TeachClient.shared.listContracts()
        } catch is CancellationError {
        } catch {
            notice = "Couldn’t list your recordings. \(SettingsLoadFailure.from(error).message)"
        }
    }

    private func importRecording(_ result: Result<[URL], Error>) {
        guard case .success(let urls) = result, let url = urls.first else { return }
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        guard let data = try? Data(contentsOf: url), let text = String(data: data, encoding: .utf8) else {
            notice = "Couldn’t read that file."
            return
        }
        guard url.pathExtension.lowercased() == "har" || url.pathExtension.lowercased() == "json" else {
            notice = "Pick a .har file."
            return
        }
        review = Review(har: text, fileName: url.lastPathComponent)
    }

    private func analyze() {
        guard let current = review else { return }
        review?.analyzing = true
        review?.error = nil
        Task {
            do {
                let events = current.events.enumerated().filter { !current.struck.contains($0.offset) }.map(\.element)
                let skill = try await TeachClient.shared.analyze(har: current.har, contractId: current.contract?.id,
                                                                 events: events, note: current.note)
                skills.insert(skill, at: 0)
                selectedId = skill.id
                review = nil
                await save(skill)
                contracts = (try? await TeachClient.shared.listContracts()) ?? contracts
            } catch {
                review?.analyzing = false
                review?.error = "Couldn’t analyze the recording. \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }

    /// Saves a skill (PUT); the saved row, or nil with the reason in `notice`.
    @discardableResult
    private func save(_ skill: TeachSkill) async -> TeachSkill? {
        do {
            let saved = try await TeachClient.shared.saveSkill(skill)
            if let index = skills.firstIndex(where: { $0.id == saved.id }) { skills[index] = saved }
            return saved
        } catch {
            if case APIError.httpError(let status, _) = error, status == 401 {
                notice = TeachClient.signInToSave
            } else {
                notice = "Couldn’t save the skill. \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
            return nil
        }
    }

    private func deleteSelected() {
        guard let skill = selected else { return }
        Task {
            do {
                try await TeachClient.shared.deleteSkill(id: skill.id)
                skills.removeAll { $0.id == skill.id }
                selectedId = skills.first?.id
            } catch {
                notice = "Couldn’t delete the skill. \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }

    /// Replay: the agent runs the skill in ACI's browser, starting on its
    /// page (teach-replay.ts renderReplayPrompt).
    private func replay(_ skill: TeachSkill, values: [String: String]) {
        let start = TeachPrompt.startURL(skill)
        let where_ = start.map { "Start at \($0.absoluteString)." } ?? "Open the right page first."
        let prompt = TeachPrompt.render(skill, values: values) + "\n\n" + where_ + " Do every step in the browser, so the person watching sees each page."
        dismiss()
        onRun(prompt)
    }
}

// MARK: - Editor (TeachEditor on the phone)

/// A skill's page: run bar, name, meta and actions, then Goal, Steps and
/// Inputs, all editable. Edits stay local until "Save changes"; Schedule
/// saves first, Replay runs the draft as it stands.
private struct TeachSkillEditor: View {
    let skill: TeachSkill
    /// Signed out: nothing saves (SIGN_IN_TO_SAVE).
    let blocked: Bool
    let onSave: (TeachSkill) async -> TeachSkill?
    let onReplay: (TeachSkill) -> Void
    let onSchedule: (TeachSkill) -> Void
    /// Run (the recorded steps, model-free): its sheet.
    let onRun: (TeachSkill) -> Void
    /// Save skill / Update skill file: the SKILL.md sheet.
    let onSaveSkill: (TeachSkill) -> Void
    let onOpenProfile: () -> Void
    let onTeachAgain: () -> Void
    let onDelete: () -> Void

    @State private var draft: TeachDraft
    @State private var baseline: TeachDraft
    @State private var base: TeachSkill
    @State private var editingStep: String? = nil
    @State private var saving = false
    @State private var saveFailed = false

    init(skill: TeachSkill, blocked: Bool,
         onSave: @escaping (TeachSkill) async -> TeachSkill?,
         onReplay: @escaping (TeachSkill) -> Void,
         onSchedule: @escaping (TeachSkill) -> Void,
         onRun: @escaping (TeachSkill) -> Void,
         onSaveSkill: @escaping (TeachSkill) -> Void,
         onOpenProfile: @escaping () -> Void,
         onTeachAgain: @escaping () -> Void,
         onDelete: @escaping () -> Void) {
        self.skill = skill
        self.blocked = blocked
        self.onSave = onSave
        self.onReplay = onReplay
        self.onSchedule = onSchedule
        self.onRun = onRun
        self.onSaveSkill = onSaveSkill
        self.onOpenProfile = onOpenProfile
        self.onTeachAgain = onTeachAgain
        self.onDelete = onDelete
        let start = TeachDraft(skill)
        _draft = State(initialValue: start)
        _baseline = State(initialValue: start)
        _base = State(initialValue: skill)
    }

    private var dirty: Bool { draft != baseline }
    /// teach-validate's errors the phone checks; Replay and Save stop on them.
    private var problem: String? {
        if let blocking = draft.blockingProblem { return blocking }
        let unknown = draft.steps.flatMap { unknownNames(in: $0) }
        guard let first = unknown.first else { return nil }
        return unknown.count == 1
            ? "{{\(first)}} has no input."
            : "\(Set(unknown).count) placeholders have no input."
    }
    private var current: TeachSkill { draft.apply(to: base) ?? base }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                if base.workflowStepCount > 0 {
                    // TeachRunBar: Run is primary when the skill has recorded steps.
                    VStack(alignment: .leading, spacing: 10) {
                        Text("\(base.workflowStepCount) recorded step\(base.workflowStepCount == 1 ? "" : "s"). Run plays them back exactly, without the model; if a step doesn’t match, the agent takes over in the session.")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextSecondary"))
                            .fixedSize(horizontal: false, vertical: true)
                        Button { onRun(current) } label: {
                            Label("Run", systemImage: "play.fill")
                                .frame(maxWidth: .infinity)
                        }
                        .buttonStyle(TeachPrimaryButton())
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
                    .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("FillNeutral")))
                    .padding(.bottom, 14)
                }

                TextField("Skill name", text: $draft.title)
                    .font(.system(size: 18, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .accessibilityLabel("Skill name")
                meta
                    .padding(.top, 4)
                actions
                    .padding(.top, 12)

                Rectangle().fill(Color("BorderSubtle")).frame(height: 1).padding(.vertical, 16)

                section("Goal") {
                    TextField("What this skill does and when to use it.", text: $draft.intent, axis: .vertical)
                        .lineLimit(2...6)
                        .font(.system(size: 14))
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                        .teachField()
                }
                section("Steps") { steps }
                section("Inputs") { inputs }
            }
            .padding(16)
            .padding(.bottom, 24)
        }
        .scrollDismissesKeyboard(.interactively)
        .safeAreaInset(edge: .bottom, spacing: 0) { footer }
        .onChange(of: skill) { _, next in
            // A save elsewhere (schedule) changed the row: take it unless
            // there are edits in flight.
            base = next
            if !dirty {
                draft = TeachDraft(next)
                baseline = draft
            }
        }
    }

    // MARK: Header

    private var meta: some View {
        var parts: [String] = []
        parts.append("\(draft.steps.count) step\(draft.steps.count == 1 ? "" : "s")")
        parts.append(base.source == "model" ? "Written by a model" : "Built from the recording")
        if base.redactions > 0 { parts.append("\(base.redactions) redacted") }
        return VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                if let domain = base.domain {
                    PhIcon("globe", size: 12)
                    Text(domain).lineLimit(1)
                    Text("·")
                }
                Text(parts.joined(separator: " · ")).lineLimit(2)
            }
            .font(.system(size: 12))
            .foregroundColor(Color("TextTertiary"))
            if base.skillPath != nil || base.routineId != nil {
                HStack(spacing: 4) {
                    if base.skillPath != nil { TeachBadge(icon: "check-circle-fill", text: "Skill saved") }
                    if base.routineId != nil { TeachBadge(icon: "calendar-blank", text: "Scheduled") }
                }
            }
        }
    }

    /// TeachEditor's button row: wraps, Replay first and filled.
    private var actions: some View {
        TeachChipFlow(spacing: 8) {
            Button { onReplay(current) } label: { TeachAction(icon: "play-fill", title: "Replay") }
                .buttonStyle(TeachPrimaryButton())
                .disabled(problem != nil)
                .opacity(problem != nil ? 0.4 : 1)
            Button { onSaveSkill(current) } label: {
                TeachAction(icon: "floppy-disk", title: base.skillPath == nil ? "Save skill" : "Update skill file")
            }
            .buttonStyle(TeachSecondaryButton())
            .disabled(saving)
            Button {
                Task {
                    // withSaved: Schedule saves pending edits first.
                    if dirty { if let saved = await saveDraft() { onSchedule(saved) } } else { onSchedule(base) }
                }
            } label: {
                TeachAction(icon: "calendar-blank", title: base.routineId == nil ? "Run on a schedule" : "Schedule again")
            }
            .buttonStyle(TeachSecondaryButton())
            .disabled(saving || blocked || problem != nil)
            .opacity(saving || blocked || problem != nil ? 0.5 : 1)
            Button(action: onTeachAgain) { TeachAction(icon: "record-fill", title: "Teach again") }
                .buttonStyle(TeachSecondaryButton())
            Button(action: onOpenProfile) { TeachAction(icon: "identification-card", title: "Business profile") }
                .buttonStyle(TeachSecondaryButton())
            Button(action: onDelete) {
                PhIcon("trash", size: 14)
                    .foregroundColor(Color("TextTertiary"))
                    .frame(width: 40, height: 40)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Delete skill")
        }
    }

    private func section<Content: View>(_ title: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.system(size: 13, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
            content()
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.bottom, 22)
    }

    // MARK: Steps

    @ViewBuilder
    private var steps: some View {
        if draft.steps.isEmpty {
            Text("No steps yet. Add the first one.")
                .font(.system(size: 12))
                .foregroundColor(Color("TextTertiary"))
        }
        VStack(alignment: .leading, spacing: 6) {
            ForEach(Array(draft.steps.enumerated()), id: \.element.id) { index, step in
                stepRow(index: index, step: step)
            }
        }
        Button {
            editingStep = draft.addStep()
        } label: {
            TeachAction(icon: "plus", title: "Add step")
        }
        .buttonStyle(TeachSecondaryButton())
    }

    private func stepRow(index: Int, step: TeachDraft.Step) -> some View {
        let binding = Binding<TeachDraft.Step>(
            get: { draft.steps.first { $0.id == step.id } ?? step },
            set: { next in if let i = draft.steps.firstIndex(where: { $0.id == step.id }) { draft.steps[i] = next } }
        )
        return HStack(alignment: .top, spacing: 8) {
            Text("\(index + 1)")
                .font(.system(size: 10, weight: .semibold))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 22, height: 22)
                .background(Circle().fill(Color("FillNeutral")))
                .padding(.top, 6)
            VStack(alignment: .leading, spacing: 8) {
                if editingStep == step.id {
                    TextField("Step \(index + 1)", text: binding.text, axis: .vertical)
                        .lineLimit(2...6)
                        .font(.system(size: 14))
                        .padding(8)
                        .teachField()
                        .accessibilityLabel("Step \(index + 1)")
                    HStack(spacing: 8) {
                        Menu {
                            ForEach(TeachDraft.stepKinds, id: \.self) { kind in
                                Button(TeachDraft.stepKindLabel(kind)) { binding.wrappedValue.kind = kind }
                            }
                        } label: {
                            HStack(spacing: 4) {
                                Text(TeachDraft.stepKindLabel(step.kind))
                                PhIcon("caret-down", size: 11)
                            }
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextPrimary"))
                            .padding(.horizontal, 10)
                            .frame(height: 36)
                            .teachField()
                        }
                        .accessibilityLabel("Step type")
                        if step.kind == "api" {
                            TextField("GET host/path", text: binding.target)
                                .font(.system(size: 12, design: .monospaced))
                                .textInputAutocapitalization(.never)
                                .autocorrectionDisabled()
                                .padding(.horizontal, 8)
                                .frame(height: 36)
                                .teachField()
                                .accessibilityLabel("API call")
                        }
                        Spacer(minLength: 0)
                        Button("Done") { editingStep = nil }
                            .buttonStyle(TeachSecondaryButton())
                    }
                } else {
                    Button { editingStep = step.id } label: {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(stepLine(step))
                                .lineSpacing(2)
                                .multilineTextAlignment(.leading)
                                .fixedSize(horizontal: false, vertical: true)
                            if step.kind == "api", !step.target.isEmpty {
                                Text(step.target)
                                    .font(.system(size: 11, design: .monospaced))
                                    .foregroundColor(Color("TextTertiary"))
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                            }
                        }
                        .frame(maxWidth: .infinity, minHeight: 32, alignment: .leading)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Edit step \(index + 1)")
                }
                unknownInputs(in: step)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: 0) {
                iconButton("arrow-up", label: "Move up", disabled: index == 0) { draft.moveStep(at: index, by: -1) }
                iconButton("arrow-down", label: "Move down", disabled: index == draft.steps.count - 1) { draft.moveStep(at: index, by: 1) }
                iconButton("trash", label: "Delete step") {
                    draft.steps.removeAll { $0.id == step.id }
                    if editingStep == step.id { editingStep = nil }
                }
            }
        }
        .padding(8)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
            .stroke(unknownNames(in: step).isEmpty ? Color("BorderSubtle") : Theme.statusError.opacity(0.4)))
    }

    /// PlaceholderText: `{{name}}` marked in the step, amber when no input,
    /// business-profile value or earlier step provides it.
    private func stepLine(_ step: TeachDraft.Step) -> AttributedString {
        var line = AttributedString(TeachDraft.stepKindLabel(step.kind).uppercased() + "  ")
        line.font = Font.system(size: 10, weight: .bold)
        line.foregroundColor = Color("TextTertiary")
        let text = step.text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else {
            var empty = AttributedString("Empty step. Write it here.")
            empty.font = Font.system(size: 13)
            empty.foregroundColor = Color("TextTertiary")
            return line + empty
        }
        for part in TeachPlaceholders.split(step.text) {
            var run = AttributedString(part.text)
            if let name = part.name {
                let known = isKnown(name)
                run.font = Font.system(size: 12, design: .monospaced)
                run.foregroundColor = known ? Color("AccentPrimary") : Theme.statusWarning
                run.backgroundColor = (known ? Color("AccentPrimary") : Theme.statusWarning).opacity(known ? 0.12 : 0.15)
            } else {
                run.font = Font.system(size: 13)
                run.foregroundColor = Color("TextPrimary")
            }
            line += run
        }
        return line
    }

    private func isKnown(_ name: String) -> Bool {
        name.hasPrefix("profile.") || name.hasPrefix("var.") || draft.inputs.contains { $0.name == name }
    }

    private func unknownNames(in step: TeachDraft.Step) -> [String] {
        var seen = Set<String>()
        return TeachPlaceholders.names(in: "\(step.text) \(step.target)")
            .filter { !isKnown($0) && seen.insert($0).inserted }
    }

    /// ProblemList's unknown_input row, with its "Add input" fix.
    @ViewBuilder
    private func unknownInputs(in step: TeachDraft.Step) -> some View {
        ForEach(unknownNames(in: step), id: \.self) { name in
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    PhIcon("warning-circle-fill", size: 12)
                    Text("{{\(name)}} has no input. Add an input named “\(name)” or fix the name.")
                        .fixedSize(horizontal: false, vertical: true)
                }
                .font(.system(size: 12))
                .foregroundColor(Theme.statusError)
                Button { draft.addInput(named: name) } label: {
                    TeachAction(icon: "plus", title: "Add input \(name)")
                }
                .buttonStyle(TeachSecondaryButton())
            }
        }
    }

    private func iconButton(_ icon: String, label: String, disabled: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            PhIcon(icon, size: 13)
                .foregroundColor(Color("TextTertiary"))
                .frame(width: 36, height: 36)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.35 : 1)
        .accessibilityLabel(label)
    }

    // MARK: Inputs

    @ViewBuilder
    private var inputs: some View {
        if draft.inputs.isEmpty {
            Text("No inputs. Add one for a value that changes each run, then use {{name}} in a step.")
                .font(.system(size: 12))
                .foregroundColor(Color("TextTertiary"))
        }
        VStack(alignment: .leading, spacing: 8) {
            ForEach($draft.inputs) { $input in
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 8) {
                        TextField("name", text: $input.name)
                            .font(.system(size: 12, design: .monospaced))
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .padding(.horizontal, 10)
                            .frame(height: 40)
                            .teachField()
                            .accessibilityLabel("Input name")
                        Menu {
                            ForEach(TeachDraft.inputTypes, id: \.type) { option in
                                Button(option.label) { input.setType(option.type) }
                            }
                        } label: {
                            HStack(spacing: 4) {
                                Text(TeachDraft.inputTypeLabel(input.type))
                                PhIcon("caret-down", size: 11)
                            }
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextPrimary"))
                            .padding(.horizontal, 8)
                            .frame(height: 32)
                            .teachField()
                        }
                        .accessibilityLabel("Input type")
                        iconButton("trash", label: "Delete input") {
                            draft.inputs.removeAll { $0.id == input.id }
                        }
                    }
                    TextField("Label", text: $input.label)
                        .font(.system(size: 14))
                        .padding(.horizontal, 12)
                        .frame(height: 40)
                        .teachField()
                        .accessibilityLabel("Input label")
                    if input.isSecret {
                        Text("No example for secrets")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextTertiary"))
                    } else {
                        TextField("Example", text: $input.example)
                            .font(.system(size: 14))
                            .padding(.horizontal, 12)
                            .frame(height: 40)
                            .teachField()
                            .accessibilityLabel("Input example")
                    }
                    inputSettings($input)
                }
                .padding(8)
                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("BgPanel")))
                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle")))
            }
        }
        Button { draft.addInput() } label: {
            TeachAction(icon: "plus", title: "Add input")
        }
        .buttonStyle(TeachSecondaryButton())
    }

    /// TeachEditor InputRow's second line: Required, then the type's own
    /// setting (Format, Root, Saved login or the choices) and its help.
    @ViewBuilder
    private func inputSettings(_ input: Binding<TeachDraft.Input>) -> some View {
        let value = input.wrappedValue
        Toggle("Required", isOn: input.required)
            .font(.system(size: 12))
            .foregroundColor(Color("TextSecondary"))
            .tint(Color(hex: "#3B82F6"))
        if value.hasFormat {
            HStack(spacing: 8) {
                Text("Format").font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                TextField(value.type == "number" ? "en-US" : "MM/DD/YYYY", text: input.format)
                    .font(.system(size: 12, design: .monospaced))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .accessibilityLabel("Format")
                Menu {
                    ForEach(Self.formats(for: value.type), id: \.self) { format in
                        Button(format) { input.wrappedValue.format = format }
                    }
                } label: {
                    Image(systemName: "chevron.up.chevron.down")
                        .font(.system(size: 11))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 32, height: 32)
                }
                .accessibilityLabel("Pick a format")
            }
        }
        if value.hasRoot {
            HStack(spacing: 8) {
                Text("Root").font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                TextField("{{profile.reports_root}} or /Users/me/Reports", text: input.root)
                    .font(.system(size: 12, design: .monospaced))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .accessibilityLabel("Allowed root folder")
            }
        }
        if value.isSecret {
            HStack(spacing: 8) {
                Text("Saved login").font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                TextField("Credential id (optional)", text: input.secretRef)
                    .font(.system(size: 12, design: .monospaced))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .accessibilityLabel("Credential id")
            }
        }
        if value.type == "choice" {
            VStack(alignment: .leading, spacing: 4) {
                Text("Choices (one per line)").font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                TextField("", text: input.choices, axis: .vertical)
                    .lineLimit(3...8)
                    .font(.system(size: 13))
                    .padding(8)
                    .teachField()
                    .accessibilityLabel("Choices")
            }
        }
        Text(Self.typeHelp(value.type))
            .font(.system(size: 11))
            .foregroundColor(Color("TextTertiary"))
            .fixedSize(horizontal: false, vertical: true)
    }

    /// TeachEditor DATE_FORMATS / DATETIME_FORMATS / NUMBER_FORMATS.
    private static func formats(for type: String) -> [String] {
        switch type {
        case "date": return ["YYYY-MM-DD", "MM/DD/YYYY", "DD/MM/YYYY", "DD.MM.YYYY", "D MMM YYYY", "MMM D, YYYY"]
        case "datetime": return ["YYYY-MM-DDTHH:mm", "MM/DD/YYYY HH:mm", "DD/MM/YYYY HH:mm", "DD.MM.YYYY HH:mm"]
        default: return ["en-US", "en-US+group", "de-DE", "de-DE+group", "fr-FR", "en-GB"]
        }
    }

    /// TeachEditor TYPE_HELP.
    private static func typeHelp(_ type: String) -> String {
        switch type {
        case "number": return "Entered as a plain number (1234.5). The format sets how it's typed into the page, like de-DE for 1234,5."
        case "date": return "Picked from a calendar. The format sets how it's typed into the page."
        case "datetime": return "A date and a time. The format sets how it's typed into the page."
        case "email": return "An email address."
        case "url": return "A web address."
        case "choice": return "One of a fixed set of values. Write one choice per line."
        case "file": return "A file on this computer. Root limits where it may come from; it can be {{profile.key}}."
        case "folder": return "A folder on this computer. Root limits where it may come from; it can be {{profile.key}}."
        case "list": return "One item per line, or CSV with a header row. A “for each” step goes through it."
        case "secret": return "Never saved in the skill or typed into a prompt. The run stops and asks you when a step needs it."
        default: return "Any text, typed as is."
        }
    }

    // MARK: Footer

    private var status: String {
        if blocked { return TeachClient.signInToSave }
        if let problem { return problem }
        if saveFailed { return "Couldn’t save the skill." }
        if saving { return "Saving…" }
        return dirty ? "Unsaved changes" : "All changes saved"
    }

    private var footer: some View {
        HStack(spacing: 10) {
            Text(status)
                .font(.system(size: 12))
                .foregroundColor(blocked || problem != nil || saveFailed ? Theme.statusError : Color("TextTertiary"))
                .lineLimit(2)
                .frame(maxWidth: .infinity, alignment: .leading)
            if dirty {
                Button("Undo changes") {
                    draft = baseline
                    editingStep = nil
                }
                .buttonStyle(TeachSecondaryButton())
                .disabled(saving)
            }
            Button {
                Task { await saveDraft() }
            } label: {
                HStack(spacing: 6) {
                    if saving { ProgressView().controlSize(.mini).tint(Color("BgPrimary")) }
                    Text("Save changes")
                }
            }
            .buttonStyle(TeachPrimaryButton())
            .disabled(!dirty || saving || blocked || problem != nil)
            .opacity(!dirty || saving || blocked || problem != nil ? 0.4 : 1)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(Color("BgPrimary"))
        .overlay(alignment: .top) { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
    }

    @discardableResult
    private func saveDraft() async -> TeachSkill? {
        guard let next = draft.apply(to: base) else { return nil }
        saving = true
        saveFailed = false
        let saved = await onSave(next)
        saving = false
        guard let saved else {
            saveFailed = true
            return nil
        }
        base = saved
        draft = TeachDraft(saved)
        baseline = draft
        editingStep = nil
        return saved
    }
}

// MARK: - Replay inputs (TeachReplaySheet)

private struct TeachReplaySheet: View {
    let skill: TeachSkill
    let onReplay: ([String: String]) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var values: [String: String] = [:]

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Text("Fill in the values for this run. The agent asks before any step that submits, pays, sends or deletes.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                }
                Section("Inputs") {
                    ForEach(skill.inputs) { input in
                        TeachInputField(input: input, value: binding(input.name))
                    }
                }
            }
            .navigationTitle("Replay")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Replay") { onReplay(values) }
                        .disabled(skill.inputs.contains { $0.required != false && !$0.isSecret && (values[$0.name] ?? "").trimmingCharacters(in: .whitespaces).isEmpty })
                }
            }
        }
        .presentationDetents([.medium, .large])
        .onAppear {
            values = Dictionary(uniqueKeysWithValues: skill.inputs.map { ($0.name, $0.isSecret ? "" : $0.example) })
        }
    }

    private func binding(_ name: String) -> Binding<String> {
        Binding(get: { values[name] ?? "" }, set: { values[name] = $0 })
    }
}

/// One run value: a text field, or a picker for a choice input.
private struct TeachInputField: View {
    let input: TeachInput
    @Binding var value: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(input.label.isEmpty ? input.name : input.label)
                .font(.system(size: 13, weight: .medium))
            if let choices = input.choices, input.type == "choice", !choices.isEmpty {
                Picker(input.label, selection: $value) {
                    ForEach(choices, id: \.self) { Text($0).tag($0) }
                }
                .labelsHidden()
            } else if input.isSecret {
                Text("Not saved: the run stops and asks you when a step needs it.")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
            } else {
                TextField(input.example.isEmpty ? input.name : input.example, text: $value)
                    .textInputAutocapitalization(.never)
                    .keyboardType(Self.keyboard(input.type))
            }
        }
    }

    static func keyboard(_ type: String?) -> UIKeyboardType {
        switch type {
        case "number": return .decimalPad
        case "email": return .emailAddress
        case "url": return .URL
        default: return .default
        }
    }
}

// MARK: - Run on a schedule (TeachScheduleSheet)

private struct TeachScheduleSheet: View {
    let skill: TeachSkill
    let onScheduled: (String) -> Void

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var bots = BotModeStore.shared
    @State private var botId = ""
    @State private var preset = "weekdays"
    @State private var custom = "0 9 * * 1-5"
    @State private var values: [String: String] = [:]
    @State private var saving = false
    @State private var error: String? = nil

    private static let presets: [(id: String, label: String)] = [
        ("hourly", "Hourly"), ("daily", "Daily 9:00"), ("weekdays", "Weekdays 9:00"), ("custom", "Custom"),
    ]

    /// teachScheduleCron: preset → cron; custom must be 5 fields.
    private var cron: String? {
        switch preset {
        case "hourly": return "0 * * * *"
        case "daily": return "0 9 * * *"
        case "weekdays": return "0 9 * * 1-5"
        default:
            let fields = custom.split(whereSeparator: \.isWhitespace)
            return fields.count == 5 ? fields.joined(separator: " ") : nil
        }
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Text("Runs “\(skill.title)” as a turn in a bot’s chat, on this computer. You’ll find it in Automation Tasks.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                }
                Section("Runs as") {
                    Picker("Bot", selection: $botId) {
                        Text("Pick a bot").tag("")
                        ForEach(bots.bots) { bot in Text(bot.botDisplayName).tag(bot.id) }
                    }
                }
                Section("How often") {
                    Picker("How often", selection: $preset) {
                        ForEach(Self.presets, id: \.id) { Text($0.label).tag($0.id) }
                    }
                    .pickerStyle(.segmented)
                    if preset == "custom" {
                        TextField("0 9 * * 1-5", text: $custom)
                            .font(.system(size: 14, design: .monospaced))
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                    }
                }
                if !skill.inputs.isEmpty {
                    Section("Input values") {
                        ForEach(skill.inputs) { input in
                            TeachInputField(input: input, value: Binding(get: { values[input.name] ?? "" }, set: { values[input.name] = $0 }))
                        }
                    }
                }
                if let error {
                    Section { Text(error).foregroundColor(Theme.statusError).font(.system(size: 13)) }
                }
            }
            .navigationTitle("Run on a schedule")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(saving ? "Saving…" : "Schedule") { save() }
                        .disabled(botId.isEmpty || cron == nil || saving)
                }
            }
        }
        .onAppear {
            values = Dictionary(uniqueKeysWithValues: skill.inputs.map { ($0.name, $0.isSecret ? "" : $0.example) })
            if botId.isEmpty { botId = bots.bots.first(where: \.isMainBot)?.id ?? bots.bots.first?.id ?? "" }
        }
    }

    private func save() {
        guard let cron, !botId.isEmpty else { return }
        saving = true
        error = nil
        Task {
            defer { saving = false }
            do {
                let routine = try await AutomationClient.shared.createRoutine(CreateAutomationRoutineRequest(
                    name: skill.title,
                    description: String((skill.intent.isEmpty ? "Teach skill: \(skill.title)" : skill.intent).prefix(200)),
                    agentId: botId,
                    scheduleType: "cron",
                    scheduleExpression: cron,
                    timezone: TimeZone.current.identifier,
                    executionDomain: "local",
                    config: ["jobType": "agent", "prompt": TeachPrompt.render(skill, values: values), "teach_skill_id": skill.id],
                    metadata: ["origin": .string("teach"), "teach_skill_id": .string(skill.id)]
                ))
                onScheduled(routine.id)
            } catch {
                self.error = "Couldn’t create the schedule. \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }
}

// MARK: - Bits

private struct TeachBadge: View {
    let icon: String
    let text: String

    var body: some View {
        HStack(spacing: 4) {
            PhIcon(icon, size: 11)
            Text(text)
        }
        .font(.system(size: 11))
        .foregroundColor(Color("TextSecondary"))
        .padding(.horizontal, 8)
        .padding(.vertical, 2)
        .background(Capsule().fill(Color("FillNeutral")))
    }
}

/// An action button's icon and words.
private struct TeachAction: View {
    let icon: String
    let title: String

    var body: some View {
        HStack(spacing: 6) {
            PhIcon(icon, size: 13)
            Text(title).lineLimit(1)
        }
    }
}

/// `{{name}}` in a step (teach-prompt.ts splitPlaceholders, placeholderNames).
enum TeachPlaceholders {
    struct Part { let text: String; let name: String? }

    private static let pattern = #"\{\{\s*([a-zA-Z0-9_.-]+)\s*\}\}"#

    static func split(_ text: String) -> [Part] {
        guard let regex = try? NSRegularExpression(pattern: pattern) else { return [Part(text: text, name: nil)] }
        var parts: [Part] = []
        var cursor = text.startIndex
        for match in regex.matches(in: text, range: NSRange(text.startIndex..., in: text)) {
            guard let whole = Range(match.range, in: text), let name = Range(match.range(at: 1), in: text) else { continue }
            if cursor < whole.lowerBound { parts.append(Part(text: String(text[cursor..<whole.lowerBound]), name: nil)) }
            parts.append(Part(text: String(text[whole]), name: String(text[name])))
            cursor = whole.upperBound
        }
        if cursor < text.endIndex { parts.append(Part(text: String(text[cursor...]), name: nil)) }
        return parts
    }

    static func names(in text: String) -> [String] { split(text).compactMap(\.name) }
}

private extension View {
    /// `teachField`: a default-border field on the elevated surface, 8 pt corners.
    func teachField() -> some View {
        background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
    }
}

private struct TeachBanner: View {
    enum Tone { case info, error }
    let text: String
    let tone: Tone
    var onRetry: (() -> Void)? = nil
    var onDismiss: (() -> Void)? = nil

    var body: some View {
        HStack(spacing: 8) {
            if tone == .error { PhIcon("warning", size: 14) }
            Text(text).font(.system(size: 12)).frame(maxWidth: .infinity, alignment: .leading)
            if let onRetry {
                Button("Try again", action: onRetry).font(.system(size: 12, weight: .medium)).underline()
            }
            if let onDismiss {
                Button(action: onDismiss) { PhIcon("x", size: 12) }
                    .frame(width: 32, height: 32)
                    .accessibilityLabel("Dismiss")
            }
        }
        .foregroundColor(tone == .error ? Theme.statusError : Color("TextSecondary"))
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous)
            .fill(tone == .error ? Theme.statusError.opacity(0.08) : Color("FillNeutral")))
        .overlay {
            if tone == .error {
                RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Theme.statusError.opacity(0.25), lineWidth: 1)
            }
        }
        .padding(.horizontal, 14)
        .padding(.top, 8)
    }
}

private struct TeachPrimaryButton: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: .semibold))
            .foregroundColor(Color("BgPrimary"))
            .padding(.horizontal, 12)
            .frame(minHeight: 40)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("TextPrimary")))
            .opacity(configuration.isPressed ? 0.8 : 1)
    }
}

private struct TeachSecondaryButton: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: .medium))
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 12)
            .frame(minHeight: 40)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
            .opacity(configuration.isPressed ? 0.7 : 1)
    }
}
