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
    @State private var loadError: String? = nil
    @State private var signedOut = false
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
        var note = ""
        var analyzing = false
        var error: String? = nil
    }

    enum Sheet: Identifiable {
        case replay(TeachSkill), schedule(TeachSkill)
        var id: String {
            switch self {
            case .replay(let skill): return "replay-\(skill.id)"
            case .schedule(let skill): return "schedule-\(skill.id)"
            }
        }
    }

    private var unanalyzed: [TeachContract] {
        contracts.filter { contract in !skills.contains { ($0.raw["contract_id"]) == .string(contract.id) } }
    }

    private var selected: TeachSkill? { skills.first { $0.id == selectedId } }
    private var showList: Bool { !skills.isEmpty || !unanalyzed.isEmpty || (loading && !loaded) }

    var body: some View {
        VStack(spacing: 0) {
            header
            if signedOut {
                TeachBanner(text: "\(TeachClient.signInToSave) Recording and review still work here.", tone: .info)
            } else if let loadError {
                TeachBanner(text: loadError, tone: .error, onRetry: { Task { await load() } })
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
            }
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
                .font(.system(size: 17, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
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
                            Circle().fill(Theme.statusError).frame(width: 8, height: 8)
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
            .padding(.top, 8)
            ScrollView {
                LazyVStack(spacing: 2) {
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
                        if skill.skillPath != nil { TeachBadge(icon: "checkmark.circle.fill", text: "Skill saved") }
                        if skill.routineId != nil { TeachBadge(icon: "calendar", text: "Scheduled") }
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
        } else if loading && !loaded {
            Text("Loading…")
                .font(.system(size: 13))
                .foregroundColor(Color("TextTertiary"))
        } else {
            emptyTeach
        }
    }

    /// EmptyTeach, the phone's variant (nothing records here: import leads).
    private var emptyTeach: some View {
        VStack(spacing: 0) {
            Button { isImporting = true } label: {
                Image(systemName: "square.and.arrow.up")
                    .font(.system(size: 26))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 72, height: 72)
                    .background(Circle().fill(Color("FillNeutral")))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Import a recording")
            Text("Teach the agent a site")
                .font(.system(size: 20, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .padding(.top, 20)
                .padding(.bottom, 4)
            Text("Recording runs in Allternit Desktop or the Allternit browser extension: click through the flow once and it's saved as a skill the agent can replay. Here, you can import a recording (.har).")
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
                .multilineTextAlignment(.center)
                .lineSpacing(3)
                .frame(maxWidth: 340)
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
                        Text("The recording of \(contract.domain): \(contract.endpoints.count) API call\(contract.endpoints.count == 1 ? "" : "s").")
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
            onTeachAgain: { isImporting = true },
            onDelete: { isDeleteConfirming = true }
        )
        .id(skill.id)
    }

    private func load() async {
        loading = true
        defer { loading = false; loaded = true }
        do {
            async let skillList = TeachClient.shared.listSkills()
            async let contractList = TeachClient.shared.listContracts()
            skills = try await skillList
            contracts = (try? await contractList) ?? []
            loadError = nil
            signedOut = false
            if selectedId == nil || !skills.contains(where: { $0.id == selectedId }) {
                selectedId = skills.first?.id
            }
        } catch {
            if case APIError.httpError(let status, _) = error, status == 401 {
                signedOut = true
                loadError = nil
            } else {
                loadError = "Couldn’t load your skills. \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
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
                let skill = try await TeachClient.shared.analyze(har: current.har, contractId: current.contract?.id, note: current.note)
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
         onTeachAgain: @escaping () -> Void,
         onDelete: @escaping () -> Void) {
        self.skill = skill
        self.blocked = blocked
        self.onSave = onSave
        self.onReplay = onReplay
        self.onSchedule = onSchedule
        self.onTeachAgain = onTeachAgain
        self.onDelete = onDelete
        let start = TeachDraft(skill)
        _draft = State(initialValue: start)
        _baseline = State(initialValue: start)
        _base = State(initialValue: skill)
    }

    private var dirty: Bool { draft != baseline }
    private var problem: String? { draft.blockingProblem }
    private var current: TeachSkill { draft.apply(to: base) ?? base }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                if base.workflowStepCount > 0 {
                    Text("\(base.workflowStepCount) recorded step\(base.workflowStepCount == 1 ? "" : "s"). Run plays them back exactly in Allternit Desktop; here the agent replays the skill.")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 10)
                        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
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
                        .padding(10)
                        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Color("BorderSubtle")))
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
        if let domain = base.domain { parts.append(domain) }
        parts.append("\(draft.steps.count) step\(draft.steps.count == 1 ? "" : "s")")
        parts.append(base.source == "model" ? "Written by a model" : "Built from the recording")
        if base.redactions > 0 { parts.append("\(base.redactions) redacted") }
        return VStack(alignment: .leading, spacing: 6) {
            Text(parts.joined(separator: " · "))
                .font(.system(size: 12))
                .foregroundColor(Color("TextTertiary"))
            if base.skillPath != nil || base.routineId != nil {
                HStack(spacing: 4) {
                    if base.skillPath != nil { TeachBadge(icon: "checkmark.circle.fill", text: "Skill saved") }
                    if base.routineId != nil { TeachBadge(icon: "calendar", text: "Scheduled") }
                }
            }
        }
    }

    private var actions: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                Button { onReplay(current) } label: {
                    Label("Replay", systemImage: "play.fill")
                }
                .buttonStyle(TeachPrimaryButton())
                .disabled(problem != nil)
                Button {
                    Task {
                        // withSaved: Schedule saves pending edits first.
                        if dirty { if let saved = await saveDraft() { onSchedule(saved) } } else { onSchedule(base) }
                    }
                } label: {
                    Label(base.routineId == nil ? "Run on a schedule" : "Schedule again", systemImage: "calendar")
                }
                .buttonStyle(TeachSecondaryButton())
                .disabled(saving || blocked || problem != nil)
                Button(action: onTeachAgain) {
                    Label("Teach again", systemImage: "record.circle")
                }
                .buttonStyle(TeachSecondaryButton())
                Button(action: onDelete) {
                    Image(systemName: "trash")
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 40, height: 40)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Delete skill")
            }
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
            Label("Add step", systemImage: "plus")
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
                        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderStrong")))
                        .accessibilityLabel("Step \(index + 1)")
                    HStack(spacing: 8) {
                        Menu {
                            ForEach(TeachDraft.stepKinds, id: \.self) { kind in
                                Button(TeachDraft.stepKindLabel(kind)) { binding.wrappedValue.kind = kind }
                            }
                        } label: {
                            HStack(spacing: 4) {
                                Text(TeachDraft.stepKindLabel(step.kind))
                                Image(systemName: "chevron.up.chevron.down").font(.system(size: 9))
                            }
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextPrimary"))
                            .padding(.horizontal, 10)
                            .frame(height: 36)
                            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderStrong")))
                        }
                        .accessibilityLabel("Step type")
                        if step.kind == "api" {
                            TextField("GET host/path", text: binding.target)
                                .font(.system(size: 12, design: .monospaced))
                                .textInputAutocapitalization(.never)
                                .autocorrectionDisabled()
                                .padding(.horizontal, 8)
                                .frame(height: 36)
                                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderStrong")))
                                .accessibilityLabel("API call")
                        }
                        Spacer(minLength: 0)
                        Button("Done") { editingStep = nil }
                            .buttonStyle(TeachSecondaryButton())
                    }
                } else {
                    Button { editingStep = step.id } label: {
                        VStack(alignment: .leading, spacing: 2) {
                            (Text(TeachDraft.stepKindLabel(step.kind).uppercased() + "  ")
                                .font(.system(size: 10, weight: .bold))
                                .foregroundColor(Color("TextTertiary"))
                             + Text(step.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? "Empty step. Write it here." : step.text)
                                .font(.system(size: 13))
                                .foregroundColor(step.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? Color("TextTertiary") : Color("TextPrimary")))
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
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: 0) {
                iconButton("arrow.up", label: "Move up", disabled: index == 0) { draft.moveStep(at: index, by: -1) }
                iconButton("arrow.down", label: "Move down", disabled: index == draft.steps.count - 1) { draft.moveStep(at: index, by: 1) }
                iconButton("trash", label: "Delete step") {
                    draft.steps.removeAll { $0.id == step.id }
                    if editingStep == step.id { editingStep = nil }
                }
            }
        }
        .padding(8)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Color("BorderSubtle")))
    }

    private func iconButton(_ systemName: String, label: String, disabled: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: systemName)
                .font(.system(size: 13))
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
                            .font(.system(size: 13, design: .monospaced))
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .accessibilityLabel("Input name")
                        Menu {
                            ForEach(TeachDraft.inputTypes, id: \.type) { option in
                                Button(option.label) { input.type = option.type }
                            }
                        } label: {
                            HStack(spacing: 4) {
                                Text(TeachDraft.inputTypeLabel(input.type))
                                Image(systemName: "chevron.up.chevron.down").font(.system(size: 9))
                            }
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextSecondary"))
                        }
                        .accessibilityLabel("Input type")
                        iconButton("trash", label: "Delete input") {
                            draft.inputs.removeAll { $0.id == input.id }
                        }
                    }
                    TextField("Label", text: $input.label)
                        .font(.system(size: 13))
                        .accessibilityLabel("Input label")
                    if input.isSecret {
                        Text("No example for secrets")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextTertiary"))
                    } else {
                        TextField("Example", text: $input.example)
                            .font(.system(size: 13))
                            .accessibilityLabel("Input example")
                    }
                }
                .padding(10)
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
            }
        }
        Button { draft.addInput() } label: {
            Label("Add input", systemImage: "plus")
        }
        .buttonStyle(TeachSecondaryButton())
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
        HStack(spacing: 3) {
            Image(systemName: icon).font(.system(size: 9))
            Text(text)
        }
        .font(.system(size: 10))
        .foregroundColor(Color("TextSecondary"))
        .padding(.horizontal, 6)
        .padding(.vertical, 2)
        .background(Capsule().fill(Color("BgPanel")))
        .overlay(Capsule().stroke(Color("BorderSubtle")))
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
            if tone == .error { Image(systemName: "exclamationmark.triangle").font(.system(size: 12)) }
            Text(text).font(.system(size: 12)).frame(maxWidth: .infinity, alignment: .leading)
            if let onRetry {
                Button("Try again", action: onRetry).font(.system(size: 12, weight: .medium)).underline()
            }
            if let onDismiss {
                Button(action: onDismiss) { Image(systemName: "xmark").font(.system(size: 11)) }
                    .frame(width: 32, height: 32)
                    .accessibilityLabel("Dismiss")
            }
        }
        .foregroundColor(tone == .error ? Theme.statusError : Color("TextSecondary"))
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous)
            .fill(tone == .error ? Theme.statusError.opacity(0.08) : Color("FillNeutral")))
        .padding(.horizontal, 14)
        .padding(.top, 8)
    }
}

private struct TeachPrimaryButton: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: .semibold))
            .foregroundColor(Color("BgPrimary"))
            .padding(.horizontal, 14)
            .frame(minHeight: 40)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("TextPrimary")))
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
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
            .opacity(configuration.isPressed ? 0.7 : 1)
    }
}
