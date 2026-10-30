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

    // MARK: - Skill (TeachRunBar + TeachEditor, read on the phone)

    private func skillDetail(_ skill: TeachSkill) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                if skill.workflowStepCount > 0 {
                    HStack(alignment: .center, spacing: 10) {
                        Text("\(skill.workflowStepCount) recorded step\(skill.workflowStepCount == 1 ? "" : "s"). Run plays them back exactly in Allternit Desktop; here the agent replays the skill.")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextSecondary"))
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
                    .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
                    .padding(.bottom, 14)
                }

                Text(skill.title)
                    .font(.system(size: 18, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                meta(skill)
                    .padding(.top, 4)
                actions(skill)
                    .padding(.top, 12)

                Rectangle().fill(Color("BorderSubtle")).frame(height: 1).padding(.vertical, 16)

                section("Goal") {
                    Text(skill.intent.isEmpty ? "No goal written yet." : skill.intent)
                        .font(.system(size: 14))
                        .foregroundColor(skill.intent.isEmpty ? Color("TextTertiary") : Color("TextPrimary"))
                }
                section("Steps") {
                    if skill.steps.isEmpty {
                        Text("No steps yet.")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextTertiary"))
                    } else {
                        VStack(alignment: .leading, spacing: 8) {
                            ForEach(Array(skill.steps.enumerated()), id: \.element.id) { index, step in
                                stepRow(step, number: index + 1)
                            }
                        }
                    }
                }
                section("Inputs") {
                    if skill.inputs.isEmpty {
                        Text("This skill doesn't ask for anything.")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextTertiary"))
                    } else {
                        VStack(alignment: .leading, spacing: 8) {
                            ForEach(skill.inputs) { input in inputRow(input) }
                        }
                    }
                }
            }
            .padding(16)
            .padding(.bottom, 24)
        }
    }

    private func meta(_ skill: TeachSkill) -> some View {
        var parts: [String] = []
        if let domain = skill.domain { parts.append(domain) }
        parts.append("\(skill.steps.count) step\(skill.steps.count == 1 ? "" : "s")")
        parts.append(skill.source == "model" ? "Written by a model" : "Built from the recording")
        if skill.redactions > 0 { parts.append("\(skill.redactions) redacted") }
        return VStack(alignment: .leading, spacing: 6) {
            Text(parts.joined(separator: " · "))
                .font(.system(size: 12))
                .foregroundColor(Color("TextTertiary"))
            if skill.skillPath != nil || skill.routineId != nil {
                HStack(spacing: 4) {
                    if skill.skillPath != nil { TeachBadge(icon: "checkmark.circle.fill", text: "Skill saved") }
                    if skill.routineId != nil { TeachBadge(icon: "calendar", text: "Scheduled") }
                }
            }
        }
    }

    private func actions(_ skill: TeachSkill) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                Button {
                    if skill.inputs.isEmpty { replay(skill, values: [:]) } else { sheet = .replay(skill) }
                } label: {
                    Label("Replay", systemImage: "play.fill")
                }
                .buttonStyle(TeachPrimaryButton())
                Button { sheet = .schedule(skill) } label: {
                    Label(skill.routineId == nil ? "Run on a schedule" : "Schedule again", systemImage: "calendar")
                }
                .buttonStyle(TeachSecondaryButton())
                Button { isImporting = true } label: {
                    Label("Teach again", systemImage: "record.circle")
                }
                .buttonStyle(TeachSecondaryButton())
                Button { isDeleteConfirming = true } label: {
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

    private func stepRow(_ step: TeachStep, number: Int) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text("\(number)")
                .font(.system(size: 12, weight: .semibold).monospacedDigit())
                .foregroundColor(Color("TextTertiary"))
                .frame(width: 18, alignment: .trailing)
            VStack(alignment: .leading, spacing: 3) {
                Text(step.text.isEmpty ? "Empty step." : step.text)
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextPrimary"))
                    .fixedSize(horizontal: false, vertical: true)
                HStack(spacing: 6) {
                    Text(Self.kindLabel(step.kind))
                        .font(.system(size: 10.5, weight: .semibold))
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(Capsule().fill(Color("FillNeutral")))
                    if let target = step.target, !target.isEmpty {
                        Text(target)
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundColor(Color("TextTertiary"))
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                }
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Color("BorderSubtle")))
    }

    private func inputRow(_ input: TeachInput) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 6) {
                Text(input.label.isEmpty ? input.name : input.label)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                Text("{{\(input.name)}}")
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundColor(Color("TextTertiary"))
                Spacer(minLength: 0)
                Text((input.type ?? "text").capitalized)
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextTertiary"))
            }
            Text(input.isSecret ? "No example for secrets" : (input.example.isEmpty ? "No example" : input.example))
                .font(.system(size: 12))
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(2)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
    }

    static func kindLabel(_ kind: String) -> String {
        switch kind {
        case "api": return "API call"
        case "ui": return "On the page"
        case "shell": return "Command"
        default: return "Note"
        }
    }

    // MARK: - Actions

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

    private func save(_ skill: TeachSkill) async {
        if let index = skills.firstIndex(where: { $0.id == skill.id }) { skills[index] = skill }
        do {
            let saved = try await TeachClient.shared.saveSkill(skill)
            if let index = skills.firstIndex(where: { $0.id == saved.id }) { skills[index] = saved }
        } catch {
            if case APIError.httpError(let status, _) = error, status == 401 {
                notice = TeachClient.signInToSave
            } else {
                notice = "Couldn’t save the skill. \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
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
