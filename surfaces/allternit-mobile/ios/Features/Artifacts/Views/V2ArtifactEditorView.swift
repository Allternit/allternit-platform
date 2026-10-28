import SwiftUI

/// Edits a v2 doc, sheet or deck on the phone and saves it back to the
/// shared store as a new version (`POST /api/v2/artifacts/:id/versions`),
/// so Desktop, the web and the PWA open the same edit. Bodies are written
/// in the web editors' formats (ArtifactBodyEditing); anything the phone
/// can't edit (tables, images, layout) is kept as it is.
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
        case .markdown, .docMarkdownField:
            TextEditor(text: markdownBinding)
                .font(.system(.body, design: .monospaced))
                .scrollContentBackground(.hidden)
                .padding(.horizontal, 12)
                .accessibilityLabel("Document text")
        case .docBlocks:
            DocBlocksEditor(blocks: docBlocksBinding)
        case .sheet:
            SheetEditor(grid: sheetBinding)
        case .blockSlides:
            BlockSlidesEditor(slides: blockSlidesBinding)
        case .simpleSlides:
            SimpleSlidesEditor(slides: simpleSlidesBinding)
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

    private var docBlocksBinding: Binding<[ArtifactBodyEditing.DocBlock]> {
        Binding(get: { if case .docBlocks(let b) = model { return b }; return [] }, set: { model = .docBlocks($0) })
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

private struct DocBlocksEditor: View {
    @Binding var blocks: [ArtifactBodyEditing.DocBlock]

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 10) {
                ForEach($blocks) { $block in
                    row($block)
                }
            }
            .padding(16)
        }
        .scrollDismissesKeyboard(.interactively)
    }

    @ViewBuilder
    private func row(_ block: Binding<ArtifactBodyEditing.DocBlock>) -> some View {
        switch block.wrappedValue.role {
        case .fixed(let label):
            Label("\(label) — edit on the web or Desktop", systemImage: "lock")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
                .padding(.vertical, 6)
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
