import SwiftUI

/// The pad's edit mode: a native sheet (the plan's design decision 9) with the
/// tiles to reorder, resize and delete, an Add tile menu, the template picker
/// (built-ins, yours, Save as template) and the store's status line.
struct PadEditSheet: View {
    @ObservedObject var store: ProjectPadStore
    @ObservedObject var templates: PadTemplatesStore
    /// The project still draws an old Style: its tiles can't be edited, but
    /// picking a template turns it into a pad.
    let isStyle: Bool

    @Environment(\.dismiss) private var dismiss
    @State private var pendingTemplate: PadTemplate?
    @State private var isNaming = false
    @State private var templateName = ""
    @State private var templateMessage: String?
    @State private var isSavingTemplate = false

    private static let sizeNames: [PadSize: String] = [.S: "Small", .M: "Medium", .L: "Large", .W: "Wide"]

    private var tiles: [PadTile] { store.pad.tiles }
    private var addable: [PadTileID] {
        let present = Set(tiles.map(\.id))
        return PadTileID.allCases.filter { !present.contains($0) }
    }

    var body: some View {
        NavigationStack {
            List {
                if !isStyle { tilesSection }
                colorsSection
                templatesSection
                if let line = store.statusMessage ?? store.error {
                    Section { Text(line).font(.footnote).foregroundColor(Color("TextTertiary")) }
                }
            }
            .environment(\.editMode, .constant(isStyle ? .inactive : .active))
            .navigationTitle(isStyle ? "Choose a pad" : "Edit pad")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                        .frame(minHeight: 44)
                        .accessibilityLabel("Done editing")
                }
            }
            .confirmationDialog(pendingTemplate.map { "Use \($0.name)?" } ?? "",
                                isPresented: Binding(get: { pendingTemplate != nil }, set: { if !$0 { pendingTemplate = nil } }),
                                titleVisibility: .visible, presenting: pendingTemplate) { template in
                Button("Replace tiles") {
                    store.save { $0 = ProjectPad.fromTemplate(template, base: $0) }
                }
                Button("Cancel", role: .cancel) {}
            } message: { _ in
                Text("This replaces the tiles. Your notes, links and checklist stay.")
            }
            .alert("Save as template", isPresented: $isNaming) {
                TextField("Template name", text: $templateName)
                Button("Save") { saveTemplate() }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Saves the tiles and quick actions to your account.")
            }
        }
        .task { if !templates.loaded && !templates.loading && templates.error == nil { templates.load() } }
    }

    // MARK: Tiles

    private var tilesSection: some View {
        Section {
            if tiles.isEmpty {
                FriendlyInlineStateView(title: "No tiles yet.", art: .sheets)
            }
            ForEach(Array(tiles.enumerated()), id: \.offset) { index, tile in
                HStack {
                    Text(tile.id.name).font(.body)
                    Spacer(minLength: 8)
                    Picker("Size of \(tile.id.name)", selection: Binding(
                        get: { tile.size },
                        set: { size in store.save { if $0.tiles.indices.contains(index) { $0.tiles[index].size = size } } }
                    )) {
                        ForEach(PadSize.allCases, id: \.self) { Text(Self.sizeNames[$0] ?? $0.rawValue).tag($0) }
                    }
                    .pickerStyle(.menu)
                    .labelsHidden()
                    .accessibilityLabel("Size of \(tile.id.name)")
                }
                .frame(minHeight: 44)
                .accessibilityElement(children: .combine)
                .accessibilityLabel("\(tile.id.name), \(Self.sizeNames[tile.size] ?? tile.size.rawValue)")
                .accessibilityHint("Drag to reorder. Swipe up or down for actions.")
                .accessibilityAction(named: "Move up") { move(index, by: -1) }
                .accessibilityAction(named: "Move down") { move(index, by: 1) }
                .accessibilityAction(named: "Remove") { remove(IndexSet(integer: index)) }
            }
            .onMove { from, to in store.save { $0.tiles.move(fromOffsets: from, toOffset: to) } }
            .onDelete(perform: remove)

            Menu {
                ForEach(addable) { id in
                    Button(id.name) { store.save { $0.tiles.append(PadTile(id: id, size: id.defaultSize)) } }
                }
            } label: {
                Label("Add tile", systemImage: "plus")
                    .frame(minHeight: 44, alignment: .leading)
            }
            .disabled(addable.isEmpty || tiles.count >= PadLimits.tiles)
            .accessibilityLabel("Add tile")
        } header: {
            Text("Tiles")
        } footer: {
            if tiles.count >= PadLimits.tiles { Text("Up to \(PadLimits.tiles) tiles.") }
        }
    }

    private func move(_ index: Int, by offset: Int) {
        let target = index + offset
        guard tiles.indices.contains(target) else { return }
        store.save { $0.tiles.move(fromOffsets: IndexSet(integer: index), toOffset: offset > 0 ? target + 1 : target) }
    }

    private func remove(_ offsets: IndexSet) {
        store.save { $0.tiles.remove(atOffsets: offsets) }
    }

    // MARK: Colors

    private var ownColors: CustomPadColors? { CustomPadColors.parse(store.pad.colors) }

    private var colorsSection: some View {
        Section("Colors") {
            ForEach(ProjectColorSet.all) { set in
                let on = ownColors == nil && ProjectColorSet.named(store.pad.colors).id == set.id
                Button { store.save { $0.colors = set.id } } label: {
                    HStack {
                        Circle().fill(set.accent).frame(width: 18, height: 18)
                        Text(set.name).foregroundColor(Color("TextPrimary"))
                        Spacer()
                        if on { Image(systemName: "checkmark").foregroundColor(Color("TextPrimary")) }
                    }
                    .frame(minHeight: 44)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(set.name)
                .accessibilityAddTraits(on ? .isSelected : [])
            }
            Button {
                if ownColors == nil {
                    let base = Self.hex(ProjectColorSet.named(store.pad.colors).accent) ?? "#c8451f"
                    store.save { $0.colors = "\(base),\(base)" }
                }
            } label: {
                HStack {
                    Text("Your colors").foregroundColor(Color("TextPrimary"))
                    Spacer()
                    if ownColors != nil { Image(systemName: "checkmark").foregroundColor(Color("TextPrimary")) }
                }
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Your colors")
            .accessibilityAddTraits(ownColors != nil ? .isSelected : [])
            if ownColors != nil {
                ColorPicker("Main", selection: ownBinding(\.main), supportsOpacity: false).frame(minHeight: 44)
                ColorPicker("Accent", selection: ownBinding(\.accent), supportsOpacity: false).frame(minHeight: 44)
            }
        }
    }

    /// `#rrggbb`, lowercase.
    private static func hex(_ color: Color) -> String? {
        var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
        guard UIColor(color).getRed(&r, green: &g, blue: &b, alpha: &a) else { return nil }
        return String(format: "#%02x%02x%02x", Int((r * 255).rounded()), Int((g * 255).rounded()), Int((b * 255).rounded()))
    }

    private func ownBinding(_ path: WritableKeyPath<CustomPadColors, UInt32>) -> Binding<Color> {
        Binding(
            get: {
                let v: UInt32 = ownColors?[keyPath: path] ?? 0
                let r = Double((v >> 16) & 0xFF) / 255
                let g = Double((v >> 8) & 0xFF) / 255
                let b = Double(v & 0xFF) / 255
                return Color(red: r, green: g, blue: b)
            },
            set: { color in
                guard let hex = Self.hex(color), let v = UInt32(hex.dropFirst(), radix: 16) else { return }
                store.save {
                    var own = CustomPadColors.parse($0.colors) ?? CustomPadColors(main: v, accent: v)
                    own[keyPath: path] = v
                    $0.colors = own.serialized
                }
            }
        )
    }

    // MARK: Templates

    private var templatesSection: some View {
        Group {
            Section {
                ForEach(templates.builtInTemplates) { templateRow($0) }
            } header: {
                Text("Starter templates")
            }
            Section {
                if templates.loading {
                    HStack(spacing: 8) { ProgressView(); Text("Loading your templates").font(.subheadline) }
                        .frame(minHeight: 44)
                } else if let error = templates.error {
                    Text(error).font(.footnote).foregroundColor(Color("TextSecondary"))
                    Button("Retry") { templates.retry() }
                        .frame(minHeight: 44)
                        .accessibilityLabel("Retry loading your templates")
                } else if templates.userTemplates.isEmpty {
                    Text("Templates you save show here.").font(.subheadline).foregroundColor(Color("TextTertiary"))
                } else {
                    ForEach(templates.userTemplates) { templateRow($0) }
                }
                if !isStyle {
                    Button {
                        templateName = ""
                        templateMessage = nil
                        isNaming = true
                    } label: {
                        Label("Save as template", systemImage: "square.and.arrow.down").frame(minHeight: 44, alignment: .leading)
                    }
                    .disabled(!templates.loaded || isSavingTemplate || tiles.isEmpty)
                    .accessibilityLabel("Save as template")
                }
                if let templateMessage {
                    Text(templateMessage).font(.footnote).foregroundColor(Color("TextSecondary"))
                }
            } header: {
                Text("Your templates")
            }
        }
    }

    private func templateRow(_ template: PadTemplate) -> some View {
        Button { pendingTemplate = template } label: {
            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(template.name).font(.body).foregroundColor(Color("TextPrimary"))
                    if let blurb = template.blurb, !blurb.isEmpty {
                        Text(blurb).font(.caption).foregroundColor(Color("TextTertiary"))
                    }
                }
                Spacer(minLength: 4)
                if !isStyle && store.pad.template == template.id {
                    Image(systemName: "checkmark").foregroundColor(Color("TextPrimary")).accessibilityHidden(true)
                }
            }
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(template.name)
        .accessibilityValue(!isStyle && store.pad.template == template.id ? "In use" : "")
        .accessibilityHint("Use this template")
    }

    private func saveTemplate() {
        let name = templateName
        let pad = store.pad
        isSavingTemplate = true
        templateMessage = nil
        Task {
            do {
                let saved = try await templates.saveTemplate(name: name, pad: pad)
                templateMessage = "Saved \(saved.name)."
            } catch {
                templateMessage = (error as? PadTemplatesClient.Failure)?.message ?? "Could not save the template. Try again."
            }
            isSavingTemplate = false
        }
    }
}
