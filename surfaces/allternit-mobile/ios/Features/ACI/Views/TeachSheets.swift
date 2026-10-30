import SwiftUI

// MARK: - Recording timeline (TeachTimeline)

/// One thing that happened during a recording (`TeachEvent`), read for the
/// timeline: what kind, when (ms from the start) and its words.
struct TeachEvent: Identifiable, Equatable {
    let id: Int
    let kind: String
    let t: Double
    let label: String

    /// TeachTimeline eventLabel, from the event's raw fields.
    init(index: Int, raw: [String: JSONValue]) {
        func text(_ key: String) -> String? {
            if case .string(let s)? = raw[key], !s.isEmpty { return s }
            return nil
        }
        id = index
        kind = text("kind") ?? "click"
        if case .number(let n)? = raw["t"] { t = n } else { t = 0 }
        let label = text("label") ?? ""
        switch kind {
        case "navigate":
            let url = text("url") ?? ""
            self.label = text("title") ?? URL(string: url).map { ($0.host ?? "") + ($0.path == "/" ? "" : $0.path) } ?? url
        case "input": self.label = text("valuePreview").map { "\(label): \($0)" } ?? label
        case "app": self.label = text("window").map { "\(text("app") ?? ""): \($0)" } ?? (text("app") ?? "")
        case "narration": self.label = text("text") ?? ""
        case "snapshot": self.label = "Screen"
        case "key": self.label = text("label").map { "\(text("key") ?? "") in \($0)" } ?? (text("key") ?? "")
        case "select": self.label = text("value").map { "\(label): \($0)" } ?? label
        case "upload": self.label = "\(label): \(text("fileName") ?? "")"
        case "download": self.label = text("fileName") ?? ""
        case "copy": self.label = "Copied “\(text("textPreview") ?? "")”"
        case "paste": self.label = "Pasted into \(label)"
        default: self.label = label
        }
    }

    var icon: String {
        switch kind {
        case "navigate": return "globe"
        case "click": return "cursorarrow.click"
        case "input": return "textformat"
        case "app": return "macwindow"
        case "narration": return "text.bubble"
        case "snapshot": return "photo"
        case "key": return "keyboard"
        case "select": return "checklist"
        case "upload": return "square.and.arrow.up"
        case "download": return "square.and.arrow.down"
        case "copy": return "doc.on.doc"
        case "paste": return "doc.on.clipboard"
        default: return "circle"
        }
    }

    /// 0:07, 1:12.
    var time: String {
        let total = max(0, Int((t / 1000).rounded()))
        return "\(total / 60):\(String(format: "%02d", total % 60))"
    }

    static func list(_ values: [JSONValue]) -> [TeachEvent] {
        values.enumerated().compactMap { index, value in
            guard case .object(let raw) = value else { return nil }
            return TeachEvent(index: index, raw: raw)
        }
    }
}

/// The recording as chips that wrap, each with its time. In Review a chip
/// can be struck out so it isn't sent.
struct TeachTimeline: View {
    let events: [TeachEvent]
    var struck: Set<Int> = []
    var onToggleStrike: ((Int) -> Void)? = nil

    var body: some View {
        if events.isEmpty {
            Text("No actions recorded, only network traffic.")
                .font(.system(size: 12))
                .foregroundColor(Color("TextTertiary"))
        } else {
            ScrollView {
                TeachChipFlow(spacing: 6) {
                    ForEach(events) { event in
                        chip(event)
                    }
                }
            }
            .frame(maxHeight: 240)
            .accessibilityLabel("Recording timeline")
        }
    }

    private func chip(_ event: TeachEvent) -> some View {
        let isStruck = struck.contains(event.id)
        return HStack(spacing: 5) {
            Image(systemName: event.icon)
                .font(.system(size: 10))
                .foregroundColor(Color("TextTertiary"))
            Text(event.label)
                .font(.system(size: 11))
                .foregroundColor(Color("TextSecondary"))
                .strikethrough(isStruck)
                .lineLimit(1)
                .frame(maxWidth: 160, alignment: .leading)
            Text(event.time)
                .font(.system(size: 10, design: .monospaced))
                .foregroundColor(Color("TextTertiary"))
            if let onToggleStrike, event.kind != "snapshot" {
                Button { onToggleStrike(event.id) } label: {
                    Image(systemName: isStruck ? "arrow.uturn.backward" : "xmark")
                        .font(.system(size: 10))
                        .foregroundColor(Color("TextTertiary"))
                        .frame(width: 28, height: 28)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(isStruck ? "Send this action" : "Don’t send this action")
            }
        }
        .padding(.leading, 7)
        .padding(.trailing, onToggleStrike == nil ? 7 : 0)
        .frame(minHeight: 28)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle")))
        .opacity(isStruck ? 0.45 : 1)
    }
}

/// Lays chips out left to right, wrapping onto new rows.
struct TeachChipFlow: Layout {
    var spacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        var x: CGFloat = 0, y: CGFloat = 0, row: CGFloat = 0, widest: CGFloat = 0
        for view in subviews {
            let size = view.sizeThatFits(.unspecified)
            if x > 0 && x + size.width > width { y += row + spacing; x = 0; row = 0 }
            x += size.width + spacing
            row = max(row, size.height)
            widest = max(widest, x - spacing)
        }
        return CGSize(width: width.isFinite ? width : widest, height: y + row)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX, y = bounds.minY, row: CGFloat = 0
        for view in subviews {
            let size = view.sizeThatFits(.unspecified)
            if x > bounds.minX && x + size.width > bounds.maxX { y += row + spacing; x = bounds.minX; row = 0 }
            view.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
            x += size.width + spacing
            row = max(row, size.height)
        }
    }
}

// MARK: - Run (TeachRunSheet)

/// Run plays the recorded steps back exactly, without the model. The runner
/// drives Allternit Desktop's own browser pane, which a phone can't reach,
/// so here the sheet says so (as the web phone does), lists the steps Run
/// would play, and offers Replay with the agent.
struct TeachRunSheet: View {
    let skill: TeachSkill
    let onReplay: () -> Void

    @Environment(\.dismiss) private var dismiss

    private var steps: [TeachRecordedStep] { TeachRecordedStep.list(skill) }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    Text("Run needs the Desktop app; use Replay. Here the agent replays “\(skill.title)” instead and asks before anything that submits, pays, sends or deletes.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .fixedSize(horizontal: false, vertical: true)
                    if !steps.isEmpty {
                        VStack(alignment: .leading, spacing: 0) {
                            Text("Recorded steps (used by Run)")
                                .font(.system(size: 12, weight: .medium))
                                .foregroundColor(Color("TextSecondary"))
                                .padding(.horizontal, 12)
                                .padding(.vertical, 8)
                            ForEach(steps) { step in
                                HStack(alignment: .top, spacing: 8) {
                                    Image(systemName: "circle")
                                        .font(.system(size: 13))
                                        .foregroundColor(Color("TextTertiary"))
                                        .padding(.top, 1)
                                    Text(step.label)
                                        .font(.system(size: 12))
                                        .foregroundColor(Color("TextPrimary"))
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                    if step.unsafe {
                                        Text("Asks first")
                                            .font(.system(size: 10, weight: .semibold))
                                            .foregroundColor(Theme.statusWarning)
                                    }
                                }
                                .padding(.leading, 12 + CGFloat(step.depth) * 16)
                                .padding(.trailing, 12)
                                .frame(minHeight: 36)
                            }
                        }
                        .padding(.bottom, 6)
                        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
                        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderSubtle")))
                    }
                }
                .padding(16)
            }
            .background(Color("BgPrimary"))
            .navigationTitle("Run")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) { Button("Replay", action: onReplay) }
            }
        }
        .presentationDetents([.medium, .large])
    }
}

// MARK: - Save skill (SkillPreview)

/// The exact SKILL.md for this skill. Allternit Desktop writes it into
/// `~/.allternit/skills/`; a phone can't write skill files, so here you
/// copy it or share it (Files, AirDrop, a message) instead.
struct TeachSkillFileSheet: View {
    let skill: TeachSkill

    @Environment(\.dismiss) private var dismiss
    @State private var copied = false

    private var markdown: String { TeachSkillFile.render(skill) }

    /// The file the share sheet hands over, named SKILL.md as the web's download.
    private var fileURL: URL? {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(TeachSkillFile.slug(skill.title), isDirectory: true)
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let url = folder.appendingPathComponent("SKILL.md")
        return (try? markdown.write(to: url, atomically: true, encoding: .utf8)) == nil ? nil : url
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    Text("The SKILL.md that will be written for this skill. Allternit Desktop saves it to ~/\(TeachSkillFile.relPath(skill.title)), where agents on that computer use it as a skill. This device can’t write skill files: copy or share the SKILL.md instead.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .fixedSize(horizontal: false, vertical: true)
                    Text(markdown)
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(12)
                        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
                        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Color("BorderSubtle")))
                    HStack(spacing: 8) {
                        Button {
                            UIPasteboard.general.string = markdown
                            copied = true
                            Task {
                                try? await Task.sleep(nanoseconds: 1_600_000_000)
                                copied = false
                            }
                        } label: {
                            Label(copied ? "Copied" : "Copy SKILL.md", systemImage: "doc.on.doc")
                        }
                        .buttonStyle(TeachSheetButton())
                        if let fileURL {
                            ShareLink(item: fileURL) {
                                Label("Share", systemImage: "square.and.arrow.up")
                            }
                            .buttonStyle(TeachSheetButton())
                        }
                    }
                }
                .padding(16)
            }
            .background(Color("BgPrimary"))
            .navigationTitle(skill.skillPath == nil ? "Save skill" : "Update skill file")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
    }
}

// MARK: - Business profile (TeachProfileSheet)

/// Values set once and used in any skill as `{{profile.key}}` (an ERP
/// address, a reports folder), so a skill doesn't need an input for them.
struct TeachProfileSheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var profile = TeachProfile()
    @State private var loading = true
    @State private var saving = false
    @State private var error: String? = nil

    private var problems: [Int: String] { profile.problems }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    Text("Set things like your ERP address or reports folder once. A skill uses them as {{profile.key}}, so it doesn’t need an input for them each run.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .fixedSize(horizontal: false, vertical: true)
                    if loading {
                        Text("Loading…")
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextTertiary"))
                    } else if profile.rows.isEmpty {
                        Text("Nothing here yet. Add the first value.")
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextTertiary"))
                    }
                    ForEach(Array(profile.rows.enumerated()), id: \.element.id) { index, row in
                        rowEditor(index: index, row: row)
                    }
                    Button {
                        profile.rows.append(TeachProfile.Row(id: UUID(), key: "", label: "", value: "", type: "text"))
                    } label: {
                        Label("Add value", systemImage: "plus")
                    }
                    .buttonStyle(TeachSheetButton())
                    if let error {
                        Text(error)
                            .font(.system(size: 13))
                            .foregroundColor(Theme.statusError)
                    }
                }
                .padding(16)
            }
            .scrollDismissesKeyboard(.interactively)
            .background(Color("BgPrimary"))
            .navigationTitle("Business profile")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(saving ? "Saving…" : "Save") { save() }
                        .disabled(saving || loading || !problems.isEmpty)
                }
            }
            .task { await load() }
        }
    }

    private func rowEditor(index: Int, row: TeachProfile.Row) -> some View {
        let binding = Binding<TeachProfile.Row>(
            get: { profile.rows.first { $0.id == row.id } ?? row },
            set: { next in if let i = profile.rows.firstIndex(where: { $0.id == row.id }) { profile.rows[i] = next } }
        )
        let key = row.key.trimmingCharacters(in: .whitespacesAndNewlines)
        return VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                TextField("erp_url", text: Binding(
                    get: { binding.wrappedValue.key },
                    set: { binding.wrappedValue.key = $0.replacingOccurrences(of: "[^a-zA-Z0-9_-]", with: "_", options: .regularExpression) }
                ))
                .font(.system(size: 13, design: .monospaced))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .accessibilityLabel("Key")
                Menu {
                    ForEach(TeachProfile.types, id: \.type) { option in
                        Button(option.label) { binding.wrappedValue.type = option.type }
                    }
                } label: {
                    HStack(spacing: 4) {
                        Text(TeachProfile.types.first { $0.type == row.type }?.label ?? "Text")
                        Image(systemName: "chevron.up.chevron.down").font(.system(size: 9))
                    }
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
                }
                .accessibilityLabel("Type")
                Button {
                    profile.rows.removeAll { $0.id == row.id }
                } label: {
                    Image(systemName: "trash")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextTertiary"))
                        .frame(width: 36, height: 36)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Remove \(key.isEmpty ? "row" : key)")
            }
            TextField("ERP address", text: binding.label)
                .font(.system(size: 13))
                .accessibilityLabel("Label")
            TextField("Value", text: binding.value)
                .font(.system(size: 13))
                .keyboardType(row.type == "url" ? .URL : row.type == "email" ? .emailAddress : row.type == "number" ? .decimalPad : .default)
                .textInputAutocapitalization(row.type == "text" ? .sentences : .never)
                .accessibilityLabel("Value")
            if let problem = problems[index] {
                Text(problem).font(.system(size: 12)).foregroundColor(Theme.statusError)
            } else if !key.isEmpty {
                Text("Use as {{profile.\(key)}}").font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
            }
        }
        .padding(10)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous)
            .stroke(problems[index] == nil ? Color.clear : Theme.statusError.opacity(0.4)))
    }

    private func load() async {
        defer { loading = false }
        do {
            profile = try await TeachClient.shared.profile()
        } catch {
            self.error = "Couldn’t load the profile. \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    private func save() {
        guard problems.isEmpty else { return }
        saving = true
        error = nil
        Task {
            do {
                try await TeachClient.shared.saveProfile(profile)
                dismiss()
            } catch {
                if case APIError.httpError(let status, _) = error, status == 401 {
                    self.error = TeachClient.signInToSave
                } else {
                    self.error = "Couldn’t save the profile. \(FriendlyErrorMessage.from(error.localizedDescription))"
                }
            }
            saving = false
        }
    }
}

/// The sheets' secondary button: outlined, 40pt tall.
struct TeachSheetButton: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: .medium))
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 12)
            .frame(minHeight: 40)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Color("BorderStrong")))
            .opacity(configuration.isPressed ? 0.6 : 1)
    }
}
