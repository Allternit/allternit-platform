import SwiftUI

/// Edits a v2 doc, sheet or deck on the phone and saves it back to the
/// shared store as a new version (`POST /api/v2/artifacts/:id/versions`),
/// so Desktop, the web and the PWA open the same edit. Bodies are written
/// in the web editors' formats (ArtifactBodyEditing); anything the phone
/// can't edit (images, dividers, layout) is kept as it is. Docs get the
/// web's tab bar: add, rename and delete tabs; Save writes every tab as one
/// version.
struct V2ArtifactEditorView: View {
    let artifact: ArtifactV2
    let initialModel: ArtifactBodyEditing.Model
    /// Called with the saved artifact; the caller swaps it in.
    let onSaved: (ArtifactV2) -> Void
    /// Called when the server has a newer version, so the caller reloads.
    let onConflict: () -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var model: ArtifactBodyEditing.Model
    @State private var isSaving = false
    @State private var saveError: String?
    @State private var conflict = false
    @State private var confirmDiscard = false

    init(artifact: ArtifactV2, model: ArtifactBodyEditing.Model,
         onSaved: @escaping (ArtifactV2) -> Void, onConflict: @escaping () -> Void) {
        self.artifact = artifact
        self.initialModel = model
        self.onSaved = onSaved
        self.onConflict = onConflict
        _model = State(initialValue: model)
    }

    var body: some View {
        NavigationStack {
            ZStack {
                Color("BgPrimary").ignoresSafeArea()
                editor
            }
            .navigationTitle(artifact.title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Cancel") {
                        if model.hasChanges { confirmDiscard = true } else { dismiss() }
                    }
                    .disabled(isSaving)
                }
                ToolbarItem(placement: .topBarTrailing) {
                    if isSaving {
                        ProgressView()
                    } else {
                        Button("Save") { Task { await save() } }
                            .fontWeight(.semibold)
                            .disabled(!model.hasChanges)
                    }
                }
            }
            .interactiveDismissDisabled(model.hasChanges || isSaving)
            .confirmationDialog("Discard your changes?", isPresented: $confirmDiscard, titleVisibility: .visible) {
                Button("Discard changes", role: .destructive) { dismiss() }
                Button("Keep editing", role: .cancel) {}
            }
            .alert("Couldn't save", isPresented: Binding(get: { saveError != nil }, set: { if !$0 { saveError = nil } })) {
                Button("OK", role: .cancel) {}
            } message: {
                Text(saveError ?? "")
            }
            .alert("Someone saved a newer version", isPresented: $conflict) {
                Button("Get the latest") {
                    onConflict()
                    dismiss()
                }
                Button("Keep editing", role: .cancel) {}
            } message: {
                Text("Your changes aren't saved. Copy anything you need, then get the latest version and edit again.")
            }
        }
    }

    // MARK: Editors

    @ViewBuilder
    private var editor: some View {
        switch model {
        case .markdown, .docMarkdownField, .docBlocks:
            if artifact.kind == "doc" {
                VStack(spacing: 0) {
                    DocTabBar {
                        AddTabButton(label: "Add tab") {
                            if let tabbed = ArtifactBodyEditing.addingFirstTab(to: model, originalBody: artifact.version.body) {
                                model = tabbed
                            }
                        }
                    }
                    DocModelEditor(model: $model)
                }
            } else {
                DocModelEditor(model: $model)
            }
        case .docTabs:
            DocTabsEditor(tabs: docTabsBinding)
        case .sheet:
            SheetEditor(grid: sheetBinding)
        case .blockSlides:
            BlockSlidesEditor(slides: blockSlidesBinding)
        case .simpleSlides:
            SimpleSlidesEditor(slides: simpleSlidesBinding)
        }
    }

    private var docTabsBinding: Binding<[ArtifactBodyEditing.DocTab]> {
        Binding(
            get: { if case .docTabs(let t, _) = model { return t }; return [] },
            set: { next in if case .docTabs(_, let ids) = model { model = .docTabs(next, originalIds: ids) } }
        )
    }

    private var sheetBinding: Binding<ArtifactBodyEditing.SheetGrid> {
        Binding(
            get: { if case .sheet(let g) = model { return g }; return ArtifactBodyEditing.SheetGrid(rows: [], original: [], isWorkbook: false) },
            set: { model = .sheet($0) }
        )
    }

    private var blockSlidesBinding: Binding<[ArtifactBodyEditing.BlockSlide]> {
        Binding(get: { if case .blockSlides(let s) = model { return s }; return [] }, set: { model = .blockSlides($0) })
    }

    private var simpleSlidesBinding: Binding<[ArtifactBodyEditing.SimpleSlide]> {
        Binding(get: { if case .simpleSlides(let s) = model { return s }; return [] }, set: { model = .simpleSlides($0) })
    }

    // MARK: Save

    private func save() async {
        isSaving = true
        defer { isSaving = false }
        do {
            let next = try ArtifactBodyEditing.serialize(
                model,
                kind: artifact.kind,
                originalBody: artifact.version.body,
                originalFormat: artifact.version.bodyFormat,
                title: artifact.title
            )
            let saved = try await ArtifactsV2Client.shared.appendVersion(
                id: artifact.id,
                baseVersion: artifact.version.version,
                body: next.body,
                bodyFormat: next.format,
                meta: artifact.version.meta
            )
            UINotificationFeedbackGenerator().notificationOccurred(.success)
            onSaved(saved)
            dismiss()
        } catch let error as APIError where error.httpStatus == 409 {
            conflict = true
        } catch {
            saveError = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}

// MARK: - Doc

/// One doc body: Markdown in a text editor, block docs block by block.
private struct DocModelEditor: View {
    @Binding var model: ArtifactBodyEditing.Model

    var body: some View {
        switch model {
        case .docBlocks:
            DocBlocksEditor(blocks: Binding(
                get: { if case .docBlocks(let b) = model { return b }; return [] },
                set: { model = .docBlocks($0) }
            ))
        default:
            TextEditor(text: markdownBinding)
                .font(.system(.body, design: .monospaced))
                .scrollContentBackground(.hidden)
                .padding(.horizontal, 12)
                .accessibilityLabel("Document text")
        }
    }

    private var markdownBinding: Binding<String> {
        Binding(
            get: {
                switch model {
                case .markdown(let text, _), .docMarkdownField(let text, _): return text
                default: return ""
                }
            },
            set: { next in
                switch model {
                case .markdown(_, let original): model = .markdown(next, original: original)
                case .docMarkdownField(_, let original): model = .docMarkdownField(next, original: original)
                default: break
                }
            }
        )
    }
}

/// The strip above a doc (editors.tsx DocTabsEditor tablist).
private struct DocTabBar<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) { content }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
        }
        .background(Color("BgSecondary"))
        .overlay(alignment: .bottom) { Rectangle().fill(Color("BorderSubtle")).frame(height: 0.5) }
    }
}

private struct AddTabButton: View {
    let label: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Label(label, systemImage: "plus")
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("docAddTab")
    }
}

/// Docs with tabs (editors.tsx DocTabsEditor): every tab's edits are kept
/// while switching. Tap the open tab to rename it (the web's double-click),
/// × deletes it (the last tab stays), + adds one and opens its rename.
/// Save writes all tabs as one version.
private struct DocTabsEditor: View {
    @Binding var tabs: [ArtifactBodyEditing.DocTab]
    @State private var activeId: String?
    @State private var renamingId: String?
    @State private var renameText = ""

    private var activeIndex: Int {
        tabs.firstIndex { $0.id == activeId } ?? 0
    }

    var body: some View {
        VStack(spacing: 0) {
            DocTabBar {
                if tabs.count > 1 {
                    ForEach(Array(tabs.enumerated()), id: \.element.id) { index, tab in
                        HStack(spacing: 0) {
                            tabButton(tab, selected: index == activeIndex)
                            if index == activeIndex {
                                Button { remove(tab.id) } label: {
                                    Image(systemName: "xmark")
                                        .font(.caption2.weight(.semibold))
                                        .foregroundColor(Color("TextSecondary"))
                                        .frame(width: 28, height: 28)
                                }
                                .buttonStyle(.plain)
                                .accessibilityLabel("Delete tab \(tab.title)")
                            }
                        }
                    }
                }
                AddTabButton(label: tabs.count > 1 ? "Tab" : "Add tab") { add() }
            }
            if tabs.indices.contains(activeIndex) {
                if tabs[activeIndex].model != nil {
                    DocModelEditor(model: Binding(
                        get: { tabs[safeTab: activeIndex]?.model ?? .markdown("", original: "") },
                        set: { next in if tabs.indices.contains(activeIndex) { tabs[activeIndex].model = next } }
                    ))
                    .id(tabs[activeIndex].id)
                } else {
                    FriendlyStateView(
                        style: .empty,
                        icon: "lock",
                        title: "This tab can't be edited here",
                        message: "Edit \u{201C}\(tabs[activeIndex].title)\u{201D} on the web or Desktop. Your edits to the other tabs still save."
                    )
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
        }
        .alert("Tab name", isPresented: Binding(get: { renamingId != nil }, set: { if !$0 { renamingId = nil } })) {
            TextField("Tab name", text: $renameText)
            Button("Rename") { commitRename() }
            Button("Cancel", role: .cancel) { renamingId = nil }
        }
    }

    private func add() {
        let tab = ArtifactBodyEditing.DocTab.added(after: tabs)
        tabs.append(tab)
        activeId = tab.id
        startRename(tab)
    }

    private func remove(_ id: String) {
        guard tabs.count > 1, let index = tabs.firstIndex(where: { $0.id == id }) else { return }
        tabs.remove(at: index)
        if activeId == id || activeId == nil { activeId = tabs[min(index, tabs.count - 1)].id }
    }

    private func startRename(_ tab: ArtifactBodyEditing.DocTab) {
        renameText = tab.title
        renamingId = tab.id
    }

    private func commitRename() {
        let title = renameText.trimmingCharacters(in: .whitespacesAndNewlines)
        if let id = renamingId, !title.isEmpty, let index = tabs.firstIndex(where: { $0.id == id }) {
            tabs[index].title = title
        }
        renamingId = nil
    }

    private func tabButton(_ tab: ArtifactBodyEditing.DocTab, selected: Bool) -> some View {
        Button {
            if selected { startRename(tab) } else { activeId = tab.id }
        } label: {
            HStack(spacing: 5) {
                if tab.model == nil {
                    Image(systemName: "lock").font(.caption2)
                }
                Text(tab.title)
                    .font(.subheadline.weight(selected ? .semibold : .regular))
                    .lineLimit(1)
                if tab.hasChanges {
                    Circle().fill(Color("AccentPrimary")).frame(width: 6, height: 6)
                        .accessibilityHidden(true)
                }
            }
            .foregroundColor(selected ? Color("TextPrimary") : Color("TextSecondary"))
            .padding(.horizontal, 12)
            .padding(.vertical, 7)
            .background(
                Capsule().fill(selected ? Color("BgPrimary") : Color.clear)
            )
            .overlay(Capsule().stroke(selected ? Color("BorderSubtle") : Color.clear, lineWidth: 1))
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(tab.title) tab\(tab.hasChanges ? ", edited" : "")")
        .accessibilityHint(selected ? "Rename this tab" : "")
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

private extension Array where Element == ArtifactBodyEditing.DocTab {
    subscript(safeTab index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}

private struct DocBlocksEditor: View {
    @Binding var blocks: [ArtifactBodyEditing.DocBlock]

    /// Consecutive cells of one table render as a grid; everything else is a row.
    private enum Segment: Identifiable {
        case single(Int)
        case table(id: String, rows: [[Int]])

        var id: String {
            switch self {
            case .single(let i): return "b\(i)"
            case .table(let id, _): return "t\(id)"
            }
        }
    }

    private var segments: [Segment] {
        var out: [Segment] = []
        var i = 0
        while i < blocks.count {
            guard case .tableCell = blocks[i].role, let table = blocks[i].path.first else {
                out.append(.single(i)); i += 1; continue
            }
            var rows: [[Int]] = []
            var lastRow = -1
            while i < blocks.count, case .tableCell(let row, _, _) = blocks[i].role, blocks[i].path.first == table {
                if row != lastRow { rows.append([]); lastRow = row }
                rows[rows.count - 1].append(i)
                i += 1
            }
            out.append(.table(id: "\(table)", rows: rows))
        }
        return out
    }

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 10) {
                ForEach(segments) { segment in
                    switch segment {
                    case .single(let i):
                        row($blocks[i])
                    case .table(_, let rows):
                        tableGrid(rows)
                    }
                }
            }
            .padding(16)
        }
        .scrollDismissesKeyboard(.interactively)
    }

    private func tableGrid(_ rows: [[Int]]) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            Grid(alignment: .topLeading, horizontalSpacing: 0, verticalSpacing: 0) {
                ForEach(Array(rows.enumerated()), id: \.offset) { r, cells in
                    GridRow {
                        ForEach(cells, id: \.self) { i in
                            tableCell($blocks[i], header: r == 0)
                        }
                    }
                }
            }
            .overlay(Rectangle().stroke(Color("BorderSubtle"), lineWidth: 1))
        }
    }

    @ViewBuilder
    private func tableCell(_ block: Binding<ArtifactBodyEditing.DocBlock>, header: Bool) -> some View {
        let editable: Bool = {
            if case .tableCell(_, _, let e) = block.wrappedValue.role { return e }
            return false
        }()
        let row: Int = { if case .tableCell(let r, _, _) = block.wrappedValue.role { return r }; return 0 }()
        let column: Int = { if case .tableCell(_, let c, _) = block.wrappedValue.role { return c }; return 0 }()
        Group {
            if editable {
                TextField("", text: block.text, axis: .vertical)
                    .foregroundColor(Color("TextPrimary"))
            } else {
                HStack(alignment: .firstTextBaseline, spacing: 4) {
                    Image(systemName: "lock").font(.caption2)
                    Text(block.wrappedValue.text.isEmpty ? "Locked" : block.wrappedValue.text)
                }
                .foregroundColor(Color("TextSecondary"))
            }
        }
        .font(.footnote.weight(header ? .semibold : .regular))
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .frame(width: 140, alignment: .topLeading)
        .background(header ? Color("BgSecondary") : Color.clear)
        .overlay(Rectangle().stroke(Color("BorderSubtle"), lineWidth: 0.5))
        .accessibilityLabel("Table row \(row + 1), column \(column + 1)\(editable ? "" : ", edit on the web or Desktop")")
    }

    @ViewBuilder
    private func row(_ block: Binding<ArtifactBodyEditing.DocBlock>) -> some View {
        switch block.wrappedValue.role {
        case .image(let src):
            VStack(alignment: .leading, spacing: 6) {
                DocImagePreview(src: src)
                TextField("Describe the image", text: block.text, axis: .vertical)
                    .font(.footnote)
                    .foregroundColor(Color("TextPrimary"))
                    .accessibilityLabel("Image description")
            }
            .padding(.vertical, 4)
        case .fixed(let label):
            Label("\(label) — edit on the web or Desktop", systemImage: "lock")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .padding(.vertical, 6)
        case .tableCell:
            tableCell(block, header: false)
        case .heading(let level):
            TextField("Heading", text: block.text, axis: .vertical)
                .font(level <= 1 ? .title2.weight(.bold) : level == 2 ? .title3.weight(.semibold) : .headline)
                .foregroundColor(Color("TextPrimary"))
        case .listItem(let numbered, let number):
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(numbered ? "\(number)." : "•").foregroundColor(Color("TextSecondary"))
                TextField("List item", text: block.text, axis: .vertical)
                    .foregroundColor(Color("TextPrimary"))
            }
            .padding(.leading, 8)
        case .paragraph:
            TextField("Text", text: block.text, axis: .vertical)
                .font(.body)
                .foregroundColor(Color("TextPrimary"))
        }
    }
}

/// An image block in the doc editor: the picture, small.
private struct DocImagePreview: View {
    let src: String
    @State private var image: UIImage?
    @State private var failed = false

    var body: some View {
        Group {
            if let image {
                Image(uiImage: image).resizable().scaledToFit()
            } else {
                ZStack {
                    RoundedRectangle(cornerRadius: Theme.radiusSM).fill(Color("BgSecondary"))
                    if failed {
                        Label("Image", systemImage: "photo").font(.caption).foregroundColor(Color("TextSecondary"))
                    } else {
                        ProgressView()
                    }
                }
                .frame(height: 120)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: 220, alignment: .leading)
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM))
        .accessibilityHidden(true)
        .task(id: src) {
            guard !src.isEmpty, let data = try? await ArtifactMediaLoader.data(for: src), let decoded = UIImage(data: data) else {
                failed = true
                return
            }
            image = decoded
        }
    }
}

// MARK: - Sheet

private struct SheetEditor: View {
    @Binding var grid: ArtifactBodyEditing.SheetGrid

    var body: some View {
        VStack(spacing: 0) {
            ScrollView([.horizontal, .vertical]) {
                Grid(alignment: .leading, horizontalSpacing: 0, verticalSpacing: 0) {
                    ForEach(grid.rows.indices, id: \.self) { r in
                        GridRow {
                            Text("\(r + 1)")
                                .font(.caption2.monospacedDigit())
                                .foregroundColor(Color("TextSecondary"))
                                .frame(width: 32)
                            ForEach(0..<grid.columnCount, id: \.self) { c in
                                TextField("", text: cellBinding(r, c))
                                    .font(.system(.footnote, design: .monospaced))
                                    .foregroundColor(Color("TextPrimary"))
                                    .textInputAutocapitalization(.never)
                                    .autocorrectionDisabled()
                                    .padding(.horizontal, 8)
                                    .padding(.vertical, 6)
                                    .frame(width: 112, alignment: .leading)
                                    .background(r == 0 ? Color("BgSecondary") : Color.clear)
                                    .overlay(Rectangle().stroke(Color("BorderSubtle"), lineWidth: 0.5))
                                    .accessibilityLabel("Row \(r + 1), column \(c + 1)")
                            }
                        }
                    }
                }
                .padding(16)
            }
            .scrollDismissesKeyboard(.interactively)
            HStack(spacing: 12) {
                Button { addRow() } label: { Label("Add row", systemImage: "plus") }
                Button { addColumn() } label: { Label("Add column", systemImage: "plus") }
                Spacer()
            }
            .font(.subheadline.weight(.semibold))
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .background(Color("BgSecondary"))
        }
    }

    private func cellBinding(_ r: Int, _ c: Int) -> Binding<String> {
        Binding(
            get: { grid.rows.indices.contains(r) && grid.rows[r].indices.contains(c) ? grid.rows[r][c] : "" },
            set: { value in
                guard grid.rows.indices.contains(r) else { return }
                while grid.rows[r].count <= c { grid.rows[r].append("") }
                grid.rows[r][c] = value
            }
        )
    }

    private func addRow() {
        grid.rows.append(Array(repeating: "", count: max(grid.columnCount, 1)))
    }

    private func addColumn() {
        let width = grid.columnCount
        grid.rows = grid.rows.map { $0 + Array(repeating: "", count: width + 1 - $0.count) }
        if grid.rows.isEmpty { grid.rows = [[""]] }
    }
}

// MARK: - Slides

private struct SlideCard<Content: View>: View {
    let number: Int
    let total: Int
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("\(number) / \(total)")
                .font(.caption2.monospacedDigit())
                .foregroundColor(Color("TextSecondary"))
            content
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .padding(18)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
    }
}

private struct BlockSlidesEditor: View {
    @Binding var slides: [ArtifactBodyEditing.BlockSlide]

    var body: some View {
        ScrollView {
            LazyVStack(spacing: 14) {
                ForEach($slides) { $slide in
                    SlideCard(number: slide.index + 1, total: slides.count) {
                        if slide.texts.isEmpty {
                            Text("No text on this slide").font(.subheadline).foregroundColor(Color("TextSecondary"))
                        }
                        ForEach($slide.texts) { $text in
                            TextField("Text", text: $text.text, axis: .vertical)
                                .font(text.id == slide.texts.first?.id ? .title3.weight(.semibold) : .subheadline)
                                .foregroundColor(Color("TextPrimary"))
                        }
                    }
                }
            }
            .padding(16)
        }
        .scrollDismissesKeyboard(.interactively)
    }
}

private struct SimpleSlidesEditor: View {
    @Binding var slides: [ArtifactBodyEditing.SimpleSlide]

    var body: some View {
        ScrollView {
            LazyVStack(spacing: 14) {
                ForEach($slides) { $slide in
                    SlideCard(number: slide.index + 1, total: slides.count) {
                        TextField("Title", text: $slide.title, axis: .vertical)
                            .font(.title3.weight(.semibold))
                        TextField("Bullets, one per line", text: $slide.bullets, axis: .vertical)
                            .font(.subheadline)
                        TextField("Notes or body", text: $slide.body, axis: .vertical)
                            .font(.subheadline)
                    }
                    .foregroundColor(Color("TextPrimary"))
                }
            }
            .padding(16)
        }
        .scrollDismissesKeyboard(.interactively)
    }
}
