import SwiftUI

/// A thread the pad can list and open (a chat or a bot thread).
struct PadThread: Identifiable {
    let id: String
    let title: String
    let at: Date?
    let open: (() -> Void)?
}

/// The project pad: tiles in a grid you can reorder and resize from the edit
/// sheet. Mirrors the web's `ProjectPad` + `PadTiles`. The eight Style tiles
/// reuse today's dashboard tile bodies (one tile at a time, through
/// `ProjectDashboardView`); the five pad tiles (quick actions, links, notes,
/// checklist, threads) keep their data in `pad.data`.
///
/// Phones draw 2 columns (S one column; M, L and W the full width). Regular
/// width (iPad) draws 4 columns (S one, M and L two, W four).
struct ProjectPadView: View {
    @ObservedObject var store: ProjectPadStore
    let data: ProjectBoardData
    let threads: [PadThread]
    let title: String
    var openStatus: (() -> Void)? = nil

    @Environment(\.horizontalSizeClass) private var sizeClass

    private var columns: Int { sizeClass == .regular ? 4 : 2 }
    private var canEdit: Bool { store.status != .signedOut }

    var body: some View {
        let pad = store.pad
        if pad.tiles.isEmpty {
            EmptyStatePadNote(text: canEdit ? "No tiles yet. Choose Edit to add some." : "No tiles yet.", art: .sheets)
        } else {
            PadFlowLayout(columns: columns, spacing: 12) {
                ForEach(Array(pad.tiles.enumerated()), id: \.offset) { _, tile in
                    tileView(tile, pad: pad)
                        .padFlowSpan(span(tile.size))
                }
            }
        }
    }

    private func span(_ size: PadSize) -> Int {
        switch size {
        case .S: return 1
        case .M, .L: return min(2, columns)
        case .W: return columns
        }
    }

    @ViewBuilder
    private func tileView(_ tile: PadTile, pad: ProjectPad) -> some View {
        if let style = tile.id.projectTile {
            ProjectDashboardView(layout: .bento, look: Self.singleTileLook(style, colors: pad.colors),
                                 data: data, title: title, openStatus: openStatus)
                .accessibilityElement(children: .contain)
        } else {
            PadCard(title: tile.id.name, size: tile.size) {
                newTile(tile, pad: pad)
            }
        }
    }

    @ViewBuilder
    private func newTile(_ tile: PadTile, pad: ProjectPad) -> some View {
        switch tile.id {
        case .notes:
            PadNotesTile(notes: pad.data.notes, canEdit: canEdit, save: saveData)
        case .links:
            PadLinksTile(links: pad.data.links, canEdit: canEdit, save: saveData)
        case .checklist:
            PadChecklistTile(items: pad.data.checklist, canEdit: canEdit, save: saveData)
        case .actions:
            PadActionsTile(actions: pad.data.actions, threads: threads, canEdit: canEdit, save: saveData)
        default:
            PadThreadsTile(threads: threads, limit: tile.size == .L ? 8 : 4)
        }
    }

    private func saveData(_ change: @escaping (inout PadData) -> Void) {
        store.save { change(&$0.data) }
    }

    /// A Bento look with every Style tile hidden but one: the dashboard then
    /// draws just that tile's body.
    static func singleTileLook(_ tile: ProjectTile, colors: String) -> ProjectLook {
        ProjectLook(layout: .bento, colors: colors, hidden: ProjectTile.allCases.filter { $0 != tile })
    }
}

// MARK: - Grid layout

private struct PadSpanKey: LayoutValueKey { static let defaultValue = 1 }

private extension View {
    func padFlowSpan(_ span: Int) -> some View { layoutValue(key: PadSpanKey.self, value: span) }
}

/// Packs tiles left to right into rows of `columns`; a tile takes `span`
/// columns. LazyVGrid has no column spans, so the pad lays itself out.
private struct PadFlowLayout: Layout {
    let columns: Int
    let spacing: CGFloat

    private func frames(width: CGFloat, subviews: Subviews) -> (frames: [CGRect], height: CGFloat) {
        let cols = CGFloat(max(columns, 1))
        let unit = (width - spacing * (cols - 1)) / cols
        var out: [CGRect] = []
        var x = 0, y: CGFloat = 0, rowHeight: CGFloat = 0
        var rowStart = 0
        func finishRow(upTo end: Int) {
            for i in rowStart..<end { out[i].size.height = max(out[i].size.height, rowHeight) }
        }
        for (i, sub) in subviews.enumerated() {
            let span = min(max(sub[PadSpanKey.self], 1), columns)
            if x + span > columns {
                finishRow(upTo: i)
                y += rowHeight + spacing
                x = 0; rowHeight = 0; rowStart = i
            }
            let w = unit * CGFloat(span) + spacing * CGFloat(span - 1)
            let h = sub.sizeThatFits(ProposedViewSize(width: w, height: nil)).height
            out.append(CGRect(x: CGFloat(x) * (unit + spacing), y: y, width: w, height: h))
            rowHeight = max(rowHeight, h)
            x += span
        }
        finishRow(upTo: out.count)
        return (out, out.isEmpty ? 0 : y + rowHeight)
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? 340
        return CGSize(width: width, height: frames(width: width, subviews: subviews).height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let laid = frames(width: bounds.width, subviews: subviews)
        for (sub, frame) in zip(subviews, laid.frames) {
            sub.place(at: CGPoint(x: bounds.minX + frame.minX, y: bounds.minY + frame.minY),
                      anchor: .topLeading,
                      proposal: ProposedViewSize(width: frame.width, height: frame.height))
        }
    }
}

// MARK: - Shared parts

private struct EmptyStatePadNote: View {
    let text: String
    var art: EmptyArtKind? = nil
    var body: some View {
        HStack(spacing: 8) {
            if let art { EmptyArt(kind: art, size: .inline) }
            Text(text)
                .font(.footnote)
                .foregroundColor(Color("TextTertiary"))
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

/// The frame of a pad tile: white sheet, hairline, gray label.
private struct PadCard<Content: View>: View {
    let title: String
    let size: PadSize
    @ViewBuilder var content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.footnote.weight(.semibold))
                .foregroundColor(Color("TextSecondary"))
                .accessibilityAddTraits(.isHeader)
            content()
        }
        .frame(maxWidth: .infinity, minHeight: size == .L ? 220 : 110, maxHeight: .infinity, alignment: .topLeading)
        .padding(16)
        .background(RoundedRectangle(cornerRadius: 20, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 20, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .accessibilityElement(children: .contain)
        .accessibilityLabel(title)
    }
}

private struct PadIconButton: View {
    let systemName: String
    let label: String
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            Image(systemName: systemName)
                .font(.footnote.weight(.semibold))
                .foregroundColor(Color("TextTertiary"))
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

private struct PadTextButton: View {
    let title: String
    var systemImage: String? = nil
    var prominent = false
    var disabled = false
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            HStack(spacing: 4) {
                if let systemImage { Image(systemName: systemImage).font(.footnote) }
                Text(title).font(.footnote.weight(.medium))
            }
            .foregroundColor(prominent ? Color("BgPanel") : Color("TextPrimary"))
            .padding(.horizontal, 12)
            .frame(minHeight: 44)
            .background(Capsule().fill(prominent ? Color("TextPrimary") : Color("FillNeutral")))
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.5 : 1)
    }
}

private struct PadField: ViewModifier {
    func body(content: Content) -> some View {
        content
            .font(.subheadline)
            .padding(.horizontal, 12)
            .frame(minHeight: 44)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
    }
}

private extension View {
    func padField() -> some View { modifier(PadField()) }
}

private let padSafeHostSeparators = CharacterSet(charactersIn: "/?#")

/// A typed address without a scheme ("github.com/x") gets https://.
private func padReadURL(_ raw: String) -> String? {
    let v = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !v.isEmpty else { return nil }
    if let safe = PadJSON.safeHttpURL(v) { return safe }
    // Another scheme (javascript:, data:, mailto:) is never made into a link.
    let hasScheme = v.range(of: "^[A-Za-z][A-Za-z0-9+.-]*:", options: .regularExpression) != nil
    return hasScheme ? nil : PadJSON.safeHttpURL("https://" + v)
}

private func padHost(_ url: String) -> String {
    guard let host = URL(string: url)?.host else { return "" }
    return host.hasPrefix("www.") ? String(host.dropFirst(4)) : host
}

private func padRelative(_ date: Date?) -> String {
    guard let date else { return "" }
    return RelativeDateTimeFormatter().localizedString(for: date, relativeTo: Date())
}

// MARK: - Notes

private struct PadNotesTile: View {
    let notes: String
    let canEdit: Bool
    let save: (@escaping (inout PadData) -> Void) -> Void

    @State private var draft = ""
    @State private var dirty = false
    @State private var pending: Task<Void, Never>?
    @FocusState private var focused: Bool

    var body: some View {
        if !canEdit {
            if notes.isEmpty {
                EmptyStatePadNote(text: "No notes yet.", art: .sheets)
            } else {
                Text(notes)
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
            }
        } else {
            VStack(alignment: .trailing, spacing: 4) {
                TextEditor(text: $draft)
                    .font(.subheadline)
                    .scrollContentBackground(.hidden)
                    .frame(minHeight: 88)
                    .padding(6)
                    .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
                    .overlay(alignment: .topLeading) {
                        if draft.isEmpty {
                            Text("No notes yet. Write anything this project should remember.")
                                .font(.subheadline)
                                .foregroundColor(Color("TextTertiary"))
                                .padding(.horizontal, 12).padding(.vertical, 14)
                                .allowsHitTesting(false)
                                .accessibilityHidden(true)
                        }
                    }
                    .focused($focused)
                    .accessibilityLabel("Notes")
                    .onChange(of: draft) { _, value in
                        if value.unicodeScalars.count > PadLimits.notes {
                            draft = PadJSON.clip(value, PadLimits.notes)
                            return
                        }
                        guard focused || dirty else { return }
                        dirty = true
                        schedule()
                    }
                    .onChange(of: focused) { _, isFocused in if !isFocused { flush() } }
                let left = PadLimits.notes - draft.unicodeScalars.count
                if left <= 400 {
                    Text("\(left) characters left").font(.caption2).foregroundColor(Color("TextTertiary"))
                }
            }
            .onAppear { draft = notes }
            .onChange(of: notes) { _, value in if !dirty { draft = value } }
            .onDisappear { flush() }
        }
    }

    private func schedule() {
        pending?.cancel()
        pending = Task {
            try? await Task.sleep(nanoseconds: 500_000_000)
            guard !Task.isCancelled else { return }
            flush()
        }
    }

    private func flush() {
        pending?.cancel()
        pending = nil
        guard dirty else { return }
        dirty = false
        let text = draft
        save { $0.notes = text }
    }
}

// MARK: - Links

private struct PadLinksTile: View {
    let links: [PadLink]
    let canEdit: Bool
    let save: (@escaping (inout PadData) -> Void) -> Void

    @State private var adding = false
    @State private var name = ""
    @State private var address = ""
    @State private var error: String?

    var body: some View {
        let shown = links.enumerated().compactMap { i, l -> (Int, PadLink, URL)? in
            guard let safe = PadJSON.safeHttpURL(l.url), let url = URL(string: safe) else { return nil }
            return (i, l, url)
        }
        VStack(alignment: .leading, spacing: 4) {
            if shown.isEmpty {
                EmptyStatePadNote(text: canEdit ? "No links yet. Add the sites this project uses." : "No links yet.", art: .sheets)
            }
            ForEach(shown, id: \.0) { index, link, url in
                HStack(spacing: 4) {
                    Link(destination: url) {
                        HStack(spacing: 6) {
                            Image(systemName: "arrow.up.right.square").font(.footnote).foregroundColor(Color("TextTertiary"))
                            Text(link.title).font(.subheadline).lineLimit(1).foregroundColor(Color("TextPrimary"))
                            Spacer(minLength: 4)
                            Text(padHost(link.url)).font(.caption).lineLimit(1).foregroundColor(Color("TextTertiary"))
                        }
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                    }
                    .accessibilityLabel("\(link.title), \(padHost(link.url))")
                    .accessibilityHint("Opens in Safari")
                    if canEdit {
                        PadIconButton(systemName: "xmark", label: "Remove link \(link.title)") {
                            save { $0.links.remove(at: min(index, max($0.links.count - 1, 0))) }
                        }
                    }
                }
            }
            if canEdit {
                if links.count >= PadLimits.links {
                    Text("Up to \(PadLimits.links) links.").font(.caption).foregroundColor(Color("TextTertiary"))
                } else if !adding {
                    PadTextButton(title: "Add link", systemImage: "plus") { adding = true }
                }
                if adding { form }
            }
        }
    }

    private var form: some View {
        VStack(alignment: .leading, spacing: 8) {
            TextField("Name (optional)", text: $name)
                .padField()
                .accessibilityLabel("Link name")
            TextField("https://", text: $address)
                .padField()
                .keyboardType(.URL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .accessibilityLabel("Web address")
                .onChange(of: address) { _, _ in error = nil }
            if let error {
                Text(error).font(.caption).foregroundColor(Theme.statusWarning)
                    .accessibilityAddTraits(.updatesFrequently)
            }
            HStack(spacing: 8) {
                PadTextButton(title: "Add", prominent: true, action: submit)
                PadTextButton(title: "Cancel") { adding = false; error = nil }
            }
        }
        .padding(.top, 4)
    }

    private func submit() {
        guard let safe = padReadURL(address) else {
            error = "Enter a full web address, like https://example.com."
            return
        }
        let title = PadJSON.clip(name.trimmingCharacters(in: .whitespacesAndNewlines), PadLimits.linkTitle)
        let link = PadLink(title: title.isEmpty ? padHost(safe) : title, url: safe)
        save { $0.links.append(link) }
        name = ""; address = ""; error = nil; adding = false
    }
}

// MARK: - Checklist

private struct PadChecklistTile: View {
    let items: [PadCheckItem]
    let canEdit: Bool
    let save: (@escaping (inout PadData) -> Void) -> Void

    @State private var text = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            if items.isEmpty {
                EmptyStatePadNote(text: "Nothing on the list yet.", art: .sheets)
            } else {
                Text("\(items.filter(\.done).count) of \(items.count) done")
                    .font(.caption).foregroundColor(Color("TextTertiary"))
                ForEach(items) { item in
                    HStack(spacing: 4) {
                        Button {
                            save { data in
                                if let i = data.checklist.firstIndex(where: { $0.id == item.id }) { data.checklist[i].done.toggle() }
                            }
                        } label: {
                            HStack(spacing: 10) {
                                Image(systemName: item.done ? "checkmark.circle.fill" : "circle")
                                    .foregroundColor(item.done ? Color("TextPrimary") : Color("TextTertiary"))
                                Text(item.text)
                                    .font(.subheadline)
                                    .strikethrough(item.done)
                                    .foregroundColor(item.done ? Color("TextTertiary") : Color("TextPrimary"))
                                    .multilineTextAlignment(.leading)
                                    .fixedSize(horizontal: false, vertical: true)
                                Spacer(minLength: 0)
                            }
                            .frame(minHeight: 44)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .disabled(!canEdit)
                        .accessibilityLabel(item.text)
                        .accessibilityValue(item.done ? "Done" : "Not done")
                        .accessibilityHint(canEdit ? "Double tap to toggle" : "")
                        if canEdit {
                            PadIconButton(systemName: "xmark", label: "Remove \(item.text)") {
                                save { $0.checklist.removeAll { $0.id == item.id } }
                            }
                        }
                    }
                }
            }
            if canEdit {
                if items.count >= PadLimits.checklist {
                    Text("Up to \(PadLimits.checklist) items.").font(.caption).foregroundColor(Color("TextTertiary"))
                } else {
                    HStack(spacing: 8) {
                        TextField("Add an item", text: $text)
                            .padField()
                            .submitLabel(.done)
                            .onSubmit(add)
                            .accessibilityLabel("New checklist item")
                        PadTextButton(title: "Add", disabled: text.trimmingCharacters(in: .whitespaces).isEmpty, action: add)
                    }
                    .padding(.top, 4)
                }
            }
        }
    }

    private func add() {
        let clean = PadJSON.clip(text.trimmingCharacters(in: .whitespacesAndNewlines), PadLimits.checkText)
        guard !clean.isEmpty, items.count < PadLimits.checklist else { return }
        save { $0.checklist.append(PadCheckItem(id: PadJSON.itemID("c"), text: clean, done: false)) }
        text = ""
    }
}

// MARK: - Quick actions

private struct PadActionsTile: View {
    let actions: [PadAction]
    let threads: [PadThread]
    let canEdit: Bool
    let save: (@escaping (inout PadData) -> Void) -> Void

    @State private var adding = false
    @State private var kind: PadAction.Kind = .link
    @State private var label = ""
    @State private var value = ""
    @State private var threadId = ""
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if actions.isEmpty {
                EmptyStatePadNote(text: canEdit ? "No quick actions yet. Add a button that opens a thread or a link." : "No quick actions yet.", art: .envelope)
            }
            ForEach(actions) { action in
                HStack(spacing: 4) {
                    actionButton(action)
                    if canEdit {
                        PadIconButton(systemName: "xmark", label: "Remove \(action.label)") {
                            save { $0.actions.removeAll { $0.id == action.id } }
                        }
                    }
                    Spacer(minLength: 0)
                }
            }
            if canEdit {
                if actions.count >= PadLimits.actions {
                    Text("Up to \(PadLimits.actions) actions.").font(.caption).foregroundColor(Color("TextTertiary"))
                } else if !adding {
                    PadTextButton(title: "Add action", systemImage: "plus") {
                        kind = threads.isEmpty ? .link : .thread
                        adding = true
                    }
                }
                if adding { form }
            }
        }
    }

    @ViewBuilder
    private func actionButton(_ action: PadAction) -> some View {
        switch action.kind {
        case .link:
            if let safe = PadJSON.safeHttpURL(action.value), let url = URL(string: safe) {
                Link(destination: url) { chip(action.label, "arrow.up.right.square") }
                    .accessibilityLabel(action.label)
                    .accessibilityHint("Opens \(padHost(safe)) in Safari")
            } else {
                chip(action.label, "arrow.up.right.square").opacity(0.5)
                    .accessibilityLabel("\(action.label), unavailable")
            }
        case .thread:
            let thread = threads.first { $0.id == action.value }
            Button { thread?.open?() } label: { chip(action.label, "bubble.left") }
                .buttonStyle(.plain)
                .disabled(thread?.open == nil)
                .opacity(thread?.open == nil ? 0.5 : 1)
                .accessibilityLabel(action.label)
                .accessibilityHint(thread.map { "Opens \($0.title)" } ?? "This thread is no longer in the project")
        }
    }

    private func chip(_ text: String, _ icon: String) -> some View {
        HStack(spacing: 6) {
            Image(systemName: icon).font(.footnote)
            Text(text).font(.footnote.weight(.medium)).lineLimit(1)
        }
        .foregroundColor(Color("TextPrimary"))
        .padding(.horizontal, 12)
        .frame(minHeight: 44)
        .background(Capsule().fill(Color("FillNeutral")))
        .contentShape(Capsule())
    }

    private var form: some View {
        VStack(alignment: .leading, spacing: 8) {
            Picker("The button opens", selection: $kind) {
                if !threads.isEmpty { Text("A thread").tag(PadAction.Kind.thread) }
                Text("A link").tag(PadAction.Kind.link)
            }
            .pickerStyle(.segmented)
            .accessibilityLabel("The button opens")
            TextField("Button name", text: $label)
                .padField()
                .accessibilityLabel("Button name")
            if kind == .link {
                TextField("https://", text: $value)
                    .padField()
                    .keyboardType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .accessibilityLabel("Web address")
            } else {
                Picker("Thread", selection: $threadId) {
                    Text("Choose a thread").tag("")
                    ForEach(threads) { Text($0.title).tag($0.id) }
                }
                .frame(minHeight: 44)
                .accessibilityLabel("Thread")
            }
            if let error { Text(error).font(.caption).foregroundColor(Theme.statusWarning) }
            HStack(spacing: 8) {
                PadTextButton(title: "Add", prominent: true, action: submit)
                PadTextButton(title: "Cancel") { adding = false; error = nil }
            }
        }
        .padding(.top, 4)
    }

    private func submit() {
        let name = PadJSON.clip(label.trimmingCharacters(in: .whitespacesAndNewlines), PadLimits.actionLabel)
        guard !name.isEmpty else { error = "Give the button a name."; return }
        let target: String
        if kind == .link {
            guard let safe = padReadURL(value) else { error = "Enter a full web address, like https://example.com."; return }
            target = safe
        } else {
            guard threads.contains(where: { $0.id == threadId }) else { error = "Choose a thread."; return }
            target = threadId
        }
        let action = PadAction(id: PadJSON.itemID("a"), kind: kind, label: name, value: target)
        save { $0.actions.append(action) }
        label = ""; value = ""; threadId = ""; error = nil; adding = false
    }
}

// MARK: - Threads

private struct PadThreadsTile: View {
    let threads: [PadThread]
    let limit: Int

    var body: some View {
        if threads.isEmpty {
            EmptyStatePadNote(text: "No threads yet.", art: .envelope)
        } else {
            let sorted = threads.sorted { ($0.at ?? .distantPast) > ($1.at ?? .distantPast) }.prefix(limit)
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(sorted)) { thread in
                    Button { thread.open?() } label: {
                        HStack(spacing: 8) {
                            Text(thread.title).font(.subheadline).lineLimit(1).foregroundColor(Color("TextPrimary"))
                            Spacer(minLength: 4)
                            Text(padRelative(thread.at)).font(.caption).foregroundColor(Color("TextTertiary"))
                        }
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(thread.open == nil)
                    .accessibilityLabel(thread.title)
                    .accessibilityHint("Opens the thread")
                }
            }
        }
    }
}

// MARK: - List view

/// The project's "List" view: plain grouped rows (Needs you, Threads, Files).
struct ProjectPadListView: View {
    let data: ProjectBoardData
    let threads: [PadThread]
    var openStatus: (() -> Void)? = nil

    private struct Row: Identifiable {
        let id: String
        let name: String
        let meta: String
        let open: (() -> Void)?
    }

    private var needs: [Row] {
        var rows = data.needsYou.map { w in
            Row(id: "need-\(w.id)", name: w.title, meta: padRelative(w.at), open: w.open ?? openStatus)
        }
        if rows.isEmpty, data.statusNeedsYou {
            let text = data.nextAction.isEmpty ? (data.blockers.isEmpty ? "Check the status page" : data.blockers) : data.nextAction
            rows.append(Row(id: "status", name: text, meta: data.stateLabel ?? "Needs you", open: openStatus))
        }
        return rows
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            group("Needs you", icon: "hand.raised", rows: needs, empty: "Nothing needs you", art: .bell)
            group("Threads", icon: "bubble.left",
                  rows: threads.sorted { ($0.at ?? .distantPast) > ($1.at ?? .distantPast) }
                    .map { Row(id: "thread-\($0.id)", name: $0.title, meta: padRelative($0.at), open: $0.open) },
                  empty: "No threads yet", art: .envelope)
            group("Files", icon: "doc",
                  rows: data.made.map { Row(id: "file-\($0.id)", name: $0.name, meta: $0.kind, open: $0.open) },
                  empty: "No files yet", art: .sheets)
        }
    }

    private func group(_ title: String, icon: String, rows: [Row], empty: String, art: EmptyArtKind) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .font(.footnote.weight(.semibold))
                .foregroundColor(Color("TextSecondary"))
                .accessibilityAddTraits(.isHeader)
            if rows.isEmpty {
                EmptyStatePadNote(text: empty, art: art).padding(.vertical, 8)
            } else {
                ForEach(rows) { row in
                    Button { row.open?() } label: {
                        HStack(spacing: 10) {
                            Image(systemName: icon).font(.footnote).foregroundColor(Color("TextSecondary"))
                            Text(row.name).font(.subheadline).lineLimit(2).foregroundColor(Color("TextPrimary"))
                                .multilineTextAlignment(.leading)
                            Spacer(minLength: 4)
                            if !row.meta.isEmpty {
                                Text(row.meta).font(.caption).lineLimit(1).foregroundColor(Color("TextTertiary"))
                            }
                            if row.open != nil {
                                Image(systemName: "chevron.right").font(.caption2).foregroundColor(Color("TextTertiary"))
                            }
                        }
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(row.open == nil)
                    .accessibilityLabel([row.name, row.meta].filter { !$0.isEmpty }.joined(separator: ", "))
                }
            }
        }
    }
}

// MARK: - Tab card

/// The opened project's card: a tab in the folder color with the title and
/// mode, a slim "Needs you" bar, the Pad | List picker and (signed in) Edit.
struct ProjectTabCard<Content: View>: View {
    let title: String
    let modeLabel: String
    /// The folder color as `#rrggbb`.
    let colorHex: String
    let view: PadView
    let onViewChange: (PadView) -> Void
    /// Nil when signed out (the pad is read-only).
    let onEdit: (() -> Void)?
    let needsCount: Int
    let needsText: String
    let onNeeds: () -> Void
    let statusLine: String?
    @ViewBuilder var content: () -> Content

    @Environment(\.colorScheme) private var scheme

    private var color: Color { Self.color(colorHex) ?? Color("TextPrimary") }

    static func color(_ hex: String) -> Color? {
        var s = hex
        if s.hasPrefix("#") { s.removeFirst() }
        guard s.count == 6, let n = UInt32(s, radix: 16) else { return nil }
        return Color(red: Double((n >> 16) & 255) / 255, green: Double((n >> 8) & 255) / 255, blue: Double(n & 255) / 255)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Text(title).font(.footnote.weight(.medium)).lineLimit(1).foregroundColor(Color("TextPrimary"))
                Text(modeLabel).font(.caption).lineLimit(1).foregroundColor(Color("TextTertiary"))
            }
            .padding(.horizontal, 14)
            .frame(minHeight: 34)
            .background(
                UnevenRoundedRectangle(topLeadingRadius: 12, bottomLeadingRadius: 0, bottomTrailingRadius: 0, topTrailingRadius: 12, style: .continuous)
                    .fill(color.opacity(scheme == .dark ? 0.28 : 0.22))
            )
            .overlay(alignment: .top) { Rectangle().fill(color).frame(height: 3).clipShape(UnevenRoundedRectangle(topLeadingRadius: 12, topTrailingRadius: 12)) }
            .accessibilityElement(children: .combine)
            .accessibilityLabel("\(title), \(modeLabel)")
            .accessibilityAddTraits(.isHeader)

            VStack(alignment: .leading, spacing: 12) {
                if needsCount > 0 {
                    Button(action: onNeeds) {
                        HStack(spacing: 8) {
                            Text("Needs you · \(needsCount)").font(.caption.weight(.medium)).foregroundColor(Color("TextPrimary"))
                            Text(needsText).font(.footnote).lineLimit(1).foregroundColor(Color("TextSecondary"))
                            Spacer(minLength: 0)
                            Image(systemName: "chevron.right").font(.caption2).foregroundColor(Color("TextTertiary"))
                        }
                        .padding(.horizontal, 12)
                        .frame(minHeight: 44)
                        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Needs you, \(needsCount). \(needsText)")
                    .accessibilityHint("Opens what is waiting")
                }
                HStack(spacing: 8) {
                    Picker("Project view", selection: Binding(get: { view }, set: onViewChange)) {
                        Text("Pad").tag(PadView.pad)
                        Text("List").tag(PadView.list)
                    }
                    .pickerStyle(.segmented)
                    .frame(maxWidth: 220, minHeight: 44)
                    .accessibilityLabel("Project view")
                    Spacer(minLength: 0)
                    if let onEdit {
                        Button(action: onEdit) {
                            Text("Edit").font(.subheadline.weight(.medium)).foregroundColor(Color("TextPrimary"))
                                .padding(.horizontal, 14)
                                .frame(minHeight: 44)
                                .background(Capsule().fill(Color("FillNeutral")))
                                .contentShape(Capsule())
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Edit pad")
                        .accessibilityHint("Reorder, resize and add tiles")
                    }
                }
                content()
                if let statusLine {
                    Text(statusLine)
                        .font(.caption)
                        .foregroundColor(Color("TextTertiary"))
                        .lineLimit(2)
                        .accessibilityLabel(statusLine)
                }
            }
            .padding(12)
            .background(
                UnevenRoundedRectangle(topLeadingRadius: 0, bottomLeadingRadius: 16, bottomTrailingRadius: 16, topTrailingRadius: 16, style: .continuous)
                    .fill(Color("BgPanel"))
                    .overlay(UnevenRoundedRectangle(topLeadingRadius: 0, bottomLeadingRadius: 16, bottomTrailingRadius: 16, topTrailingRadius: 16, style: .continuous)
                        .fill(color.opacity(0.05)))
            )
            .overlay(
                UnevenRoundedRectangle(topLeadingRadius: 0, bottomLeadingRadius: 16, bottomTrailingRadius: 16, topTrailingRadius: 16, style: .continuous)
                    .stroke(Color("BorderSubtle"), lineWidth: 1)
            )
        }
    }
}
