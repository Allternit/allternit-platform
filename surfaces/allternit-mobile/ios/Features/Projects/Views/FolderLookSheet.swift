import PhotosUI
import SwiftUI
import UIKit

/// Cover presets, same ids and values as `COVER_PRESETS` in
/// `allternit-ai/src/lib/appearance/model.ts`.
struct FolderCoverPreset: Identifiable {
    let id: String
    let name: String
    let color: String?
    let art: Cover.Art
    let font: Cover.Font
    let place: Cover.Place
    let pattern: Cover.Pattern
    let frame: Cover.Frame
    let edge: Cover.Edge

    static let all: [FolderCoverPreset] = [
        .init(id: "classic", name: "Classic", color: nil, art: .title, font: .mono, place: .center, pattern: .grain, frame: .dotted, edge: .ink),
        .init(id: "blueprint", name: "Blueprint", color: "#1F4FA8", art: .title, font: .mono, place: .top, pattern: .blueprint, frame: .none, edge: .ink),
        .init(id: "notebook", name: "Notebook", color: "#F4F1EA", art: .title, font: .typewriter, place: .top, pattern: .grid, frame: .none, edge: .soft),
        .init(id: "editorial", name: "Editorial", color: "#D2453A", art: .title, font: .serif, place: .bottom, pattern: .grain, frame: .double, edge: .ink),
        .init(id: "circuit", name: "Circuit", color: "#2FA7A0", art: .title, font: .mono, place: .center, pattern: .circuit, frame: .dotted, edge: .ink),
        .init(id: "badge", name: "Badge", color: "#9B6BD6", art: .symbol, font: .sans, place: .center, pattern: .dots, frame: .none, edge: .shadow),
    ]
}

/// Pure edits on an appearance record. Every edit of a cover field goes through
/// `Appearance.withCoverSync()` so the old `overlay` / `documents` fields never
/// undo it (web `patchCover`), and an edit of the old fields is mirrored into
/// the cover first (web `patchAppearance`).
enum FolderLookEdit {
    /// Edit cover fields, starting from the cover the folder shows now. The
    /// preset name drops once a field is changed by hand.
    static func patchCover(_ current: Appearance, _ change: (inout Cover) -> Void) -> Appearance {
        var cover = Appearance.resolvedCover(current)
        cover.preset = nil
        change(&cover)
        cover.tabLabel = String(cover.tabLabel.prefix(Cover.tabLabelMax))
        cover.sheets = min(3, max(0, cover.sheets))
        var next = current
        next.cover = cover
        return next.withCoverSync()
    }

    static func applyPreset(_ current: Appearance, _ preset: FolderCoverPreset) -> Appearance {
        var cover = Appearance.resolvedCover(current)
        cover.preset = preset.id
        cover.art = preset.art; cover.font = preset.font; cover.place = preset.place
        cover.pattern = preset.pattern; cover.frame = preset.frame; cover.edge = preset.edge
        var next = current
        if let color = preset.color { next.color = color }
        next.cover = cover
        return next.withCoverSync()
    }

    /// Scale to at most 256px on the long side and encode as a PNG data URL
    /// under the 128000-character cap the record allows.
    static func imageDataURL(from data: Data) -> String? {
        guard let source = UIImage(data: data), source.size.width > 0, source.size.height > 0 else { return nil }
        for side in [256.0, 192.0, 128.0] as [CGFloat] {
            let ratio = min(1, side / max(source.size.width, source.size.height))
            let size = CGSize(width: max(1, (source.size.width * ratio).rounded()), height: max(1, (source.size.height * ratio).rounded()))
            let format = UIGraphicsImageRendererFormat.default()
            format.scale = 1
            let scaled = UIGraphicsImageRenderer(size: size, format: format).image { _ in
                source.draw(in: CGRect(origin: .zero, size: size))
            }
            guard let png = scaled.pngData() else { continue }
            let url = "data:image/png;base64," + png.base64EncodedString()
            if url.count <= 128_000 { return url }
        }
        return nil
    }
}

private extension Color {
    /// Failable: a malformed saved colour shows nothing (Color(hex:) in Color+Theme never fails).
    init?(parsingHex hex: String) {
        var s = hex.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("#") { s.removeFirst() }
        guard s.count == 6, let v = UInt32(s, radix: 16) else { return nil }
        self.init(red: Double((v >> 16) & 0xFF) / 255, green: Double((v >> 8) & 0xFF) / 255, blue: Double(v & 0xFF) / 255)
    }

    var hexString: String? {
        var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
        guard UIColor(self).getRed(&r, green: &g, blue: &b, alpha: &a) else { return nil }
        return String(format: "#%02X%02X%02X", Int((r * 255).rounded()), Int((g * 255).rounded()), Int((b * 255).rounded()))
    }
}

/// The Folder look sheet: live preview, presets and every cover field. Edits
/// the cover fields (not the old ones) and saves through `AppearanceClient`.
///
/// Save needs the runtime (appearance has no on-device store, unlike the pad).
/// When no computer is connected (503) or the runtime is too old (404) the
/// sheet stays open with the edits intact and says so, instead of dropping them.
struct FolderLookSheet<Extras: View>: View {
    let title: String
    let mode: String
    let key: [String]
    let initial: Appearance?
    var client: AppearanceClient = AppearanceClient(client: .shared)
    let onSaved: (Appearance) -> Void
    @ViewBuilder var extras: () -> Extras

    @Environment(\.dismiss) private var dismiss
    @State private var draft: Appearance
    @State private var photoItem: PhotosPickerItem?
    @State private var saving = false
    @State private var message: String?

    private static var symbolChoices: [String] {
        ["sparkle", "code", "briefcase", "robot", "star", "heart", "lightning", "leaf", "globe", "music", "camera", "book",
         "game", "palette", "terminal", "diamond", "rocket", "shield", "target", "chart", "gear", "lightbulb", "cube", "folder"]
    }

    init(title: String, mode: String, key: [String], initial: Appearance?,
         client: AppearanceClient = AppearanceClient(client: .shared),
         onSaved: @escaping (Appearance) -> Void,
         @ViewBuilder extras: @escaping () -> Extras) {
        self.title = title; self.mode = mode; self.key = key; self.initial = initial
        self.client = client; self.onSaved = onSaved; self.extras = extras
        _draft = State(initialValue: initial ?? Appearance())
    }

    private var cover: Cover { Appearance.resolvedCover(draft) }
    private var hasImage: Bool { draft.image?.isEmpty == false }
    private var saveBlocked: Bool { saving || (cover.art == .image && !hasImage) }

    private func edit(_ change: (inout Cover) -> Void) {
        draft = FolderLookEdit.patchCover(draft, change)
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    HStack {
                        Spacer()
                        FolderCoverView(title: title, mode: mode, appearance: draft, width: 150, flat: true)
                            .accessibilityElement(children: .ignore)
                            .accessibilityLabel("Folder preview for \(title)")
                        Spacer()
                    }
                    .padding(.vertical, 8)
                    .listRowBackground(Color.clear)
                }

                Section("Presets") {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 8) {
                            ForEach(FolderCoverPreset.all) { preset in
                                let on = cover.preset == preset.id
                                Button { draft = FolderLookEdit.applyPreset(draft, preset) } label: {
                                    Text(preset.name)
                                        .font(.subheadline.weight(on ? .semibold : .regular))
                                        .padding(.horizontal, 14)
                                        .frame(minHeight: 44)
                                        .background(Capsule().fill(on ? Color("BgPanel") : Color.clear))
                                        .overlay(Capsule().stroke(on ? Color("TextPrimary") : Color.secondary.opacity(0.4), lineWidth: 1))
                                }
                                .buttonStyle(.plain)
                                .accessibilityLabel("\(preset.name) preset")
                                .accessibilityAddTraits(on ? .isSelected : [])
                            }
                        }
                    }
                }

                Section("Color") {
                    ColorPicker("Folder color", selection: hexBinding(\.color, fallback: Appearance.defaultColor), supportsOpacity: false)
                        .frame(minHeight: 44)
                    ColorPicker("Paper color", selection: hexBinding(\.paperColor, fallback: "#FFFFFF"), supportsOpacity: false)
                        .frame(minHeight: 44)
                }

                Section("Cover art") {
                    Picker("Cover art", selection: Binding(get: { cover.art }, set: { v in edit { $0.art = v } })) {
                        Text("Title").tag(Cover.Art.title)
                        Text("Symbol").tag(Cover.Art.symbol)
                        Text("Text").tag(Cover.Art.text)
                        Text("Image").tag(Cover.Art.image)
                        Text("None").tag(Cover.Art.none)
                    }
                    if cover.art == .symbol {
                        Picker("Symbol", selection: Binding(get: { draft.symbol }, set: { draft.symbol = $0 })) {
                            ForEach(Self.symbolChoices, id: \.self) { Text($0.capitalized).tag($0) }
                        }
                    }
                    if cover.art == .text {
                        TextField("Text or emoji (12 characters)", text: Binding(
                            get: { draft.text },
                            set: { draft.text = String($0.prefix(12)) }
                        ))
                        .frame(minHeight: 44)
                        .accessibilityLabel("Cover text")
                    }
                    if cover.art == .image {
                        PhotosPicker(selection: $photoItem, matching: .images) {
                            Label(hasImage ? "Change image" : "Choose image", systemImage: "photo")
                                .frame(minHeight: 44)
                        }
                        if !hasImage {
                            Text("Choose an image to use it as cover art.").font(.footnote).foregroundColor(.secondary)
                        } else {
                            Button("Remove image", role: .destructive) {
                                draft.image = nil
                                edit { $0.art = .title }
                            }
                            .frame(minHeight: 44)
                        }
                    }
                }

                Section("Cover style") {
                    Picker("Font", selection: Binding(get: { cover.font }, set: { v in edit { $0.font = v } })) {
                        ForEach([Cover.Font.mono, .serif, .sans, .typewriter], id: \.self) { Text($0.rawValue.capitalized).tag($0) }
                    }
                    Picker("Position", selection: Binding(get: { cover.place }, set: { v in edit { $0.place = v } })) {
                        ForEach([Cover.Place.top, .center, .bottom], id: \.self) { Text($0.rawValue.capitalized).tag($0) }
                    }
                    Picker("Pattern", selection: Binding(get: { cover.pattern }, set: { v in edit { $0.pattern = v } })) {
                        ForEach([Cover.Pattern.plain, .grain, .linen, .grid, .dots, .stripes, .blueprint, .circuit], id: \.self) { Text($0.rawValue.capitalized).tag($0) }
                    }
                    Picker("Frame", selection: Binding(get: { cover.frame }, set: { v in edit { $0.frame = v } })) {
                        ForEach([Cover.Frame.none, .dotted, .double], id: \.self) { Text($0.rawValue.capitalized).tag($0) }
                    }
                    Picker("Edge", selection: Binding(get: { cover.edge }, set: { v in edit { $0.edge = v } })) {
                        ForEach([Cover.Edge.ink, .soft, .shadow], id: \.self) { Text($0.rawValue.capitalized).tag($0) }
                    }
                    Stepper(value: Binding(get: { cover.sheets }, set: { v in edit { $0.sheets = v } }), in: 0...3) {
                        Text("Sheets of paper: \(cover.sheets)")
                    }
                    .frame(minHeight: 44)
                    .accessibilityValue("\(cover.sheets)")
                }

                Section("Tab") {
                    ColorPicker("Tab color", selection: tabColorBinding, supportsOpacity: false).frame(minHeight: 44)
                    TextField("Tab label (up to \(Cover.tabLabelMax) characters)", text: Binding(
                        get: { cover.tabLabel },
                        set: { v in edit { $0.tabLabel = String(v.prefix(Cover.tabLabelMax)) } }
                    ))
                    .frame(minHeight: 44)
                    .accessibilityLabel("Tab label")
                    Toggle("Show mode label", isOn: Binding(get: { cover.showMode }, set: { v in edit { $0.showMode = v } }))
                        .frame(minHeight: 44)
                }

                if let message {
                    Section {
                        Text(message).font(.footnote).foregroundColor(.red)
                            .accessibilityLabel("Could not save. \(message)")
                    }
                }

                Section("Project") { extras() }
            }
            .navigationTitle("Folder look")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }.frame(minWidth: 44, minHeight: 44)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(saving ? "Saving" : "Save") { Task { await save() } }
                        .disabled(saveBlocked)
                        .frame(minWidth: 44, minHeight: 44)
                }
            }
            .onChange(of: photoItem) { _, item in
                guard let item else { return }
                Task { await loadImage(item) }
            }
        }
    }

    private func hexBinding(_ path: WritableKeyPath<Appearance, String>, fallback: String) -> Binding<Color> {
        Binding(
            get: { Color(parsingHex: draft[keyPath: path]) ?? Color(parsingHex: fallback) ?? .blue },
            set: { if let hex = $0.hexString { draft[keyPath: path] = hex } }
        )
    }

    private var tabColorBinding: Binding<Color> {
        Binding(
            get: { Color(parsingHex: cover.tabColor ?? draft.color) ?? .blue },
            set: { c in if let hex = c.hexString { edit { $0.tabColor = hex } } }
        )
    }

    private func loadImage(_ item: PhotosPickerItem) async {
        defer { photoItem = nil }
        guard let data = try? await item.loadTransferable(type: Data.self),
              let url = FolderLookEdit.imageDataURL(from: data) else {
            message = "That image could not be used. Try another one."
            return
        }
        message = nil
        draft.image = url
        edit { $0.art = .image }
    }

    private func save() async {
        saving = true
        message = nil
        defer { saving = false }
        let record = draft.withCoverSync()
        do {
            try await client.save(key: key, appearance: record)
            onSaved(record)
            dismiss()
        } catch APIError.httpError(let status, _) where status == 503 {
            message = "No computer is connected, so this look was not saved. Connect a computer to save it."
        } catch APIError.httpError(let status, _) where status == 404 {
            message = "Update your computer to save folder looks."
        } catch {
            message = "Could not save the look. Try again."
        }
    }
}
