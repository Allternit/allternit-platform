import SwiftUI

/// Choosing the bot-screen background (StageBackdropPicker.tsx): the ASCII /
/// Classic tile grid and its options in Settings → General, and the sheet a
/// bot, a group chat or a Code workspace opens to keep a look of its own.

/// A small segmented choice, styled like the Theme control.
private struct BackdropSegmented<T: Hashable>: View {
    let label: String
    @Binding var value: T
    let options: [(T, String)]

    var body: some View {
        HStack(spacing: 2) {
            ForEach(options, id: \.0) { option in
                let on = value == option.0
                Button { value = option.0 } label: {
                    Text(option.1)
                        .font(.system(size: 12.5))
                        .foregroundColor(on ? Color("TextPrimary") : Color("TextTertiary"))
                        .padding(.horizontal, 10)
                        .frame(height: 28)
                        .background(RoundedRectangle(cornerRadius: 6).fill(on ? Color("BgPanel") : .clear)
                            .shadow(color: .black.opacity(on ? 0.12 : 0), radius: 1, y: 1))
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .padding(2)
        .background(RoundedRectangle(cornerRadius: 8).fill(Color("FillNeutral")))
        .accessibilityElement(children: .contain)
        .accessibilityLabel(label)
    }
}

/// A look's live swatch (still unless it's the one picked).
struct BackdropSwatch: View {
    let variant: String
    var colors: [String?] = StageBackdrops.sampleColors
    var animated = false

    var body: some View {
        StageBackdropView(variant: variant, colors: colors, size: .tile, still: !animated, veil: false)
            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle")))
    }
}

/// Settings → General → Background: the set, a tile per look, then the options.
struct StageBackgroundSettingsSection: View {
    @ObservedObject private var store = StageBackdropStore.shared
    @State private var family: BackdropFamily = StageBackdrops.family(StageBackdropStore.shared.variant)

    private let columns = [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)]

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            BackdropSegmented(label: "Background set", value: $family,
                              options: BackdropFamily.allCases.map { ($0, $0.label) })
            LazyVGrid(columns: columns, spacing: 12) {
                ForEach(StageBackdrops.list(family)) { option in
                    let on = option.id == store.variant
                    Button { store.variant = option.id } label: {
                        VStack(spacing: 6) {
                            BackdropSwatch(variant: option.id, animated: on)
                                .frame(height: 64)
                                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
                                    .stroke(on ? Color("TextPrimary") : .clear, lineWidth: 2))
                            Text(option.name)
                                .font(.system(size: 12))
                                .foregroundColor(on ? Color("TextPrimary") : Color("TextSecondary"))
                                .lineLimit(1)
                        }
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(option.name)
                    .accessibilityHint(option.note)
                    .accessibilityAddTraits(on ? .isSelected : [])
                }
            }
            options
        }
    }

    @ViewBuilder
    private var options: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Show on").font(.system(size: 14, weight: .medium)).foregroundColor(Color("TextPrimary"))
            Text("Where the background plays. Off shows the plain app colour.")
                .font(.system(size: 12.5)).foregroundColor(Color("TextSecondary"))
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(BackdropSurface.allCases) { surface in
                        let on = store.isOn(surface)
                        Button { store.setOn(surface, !on) } label: {
                            Text(surface.label)
                                .font(.system(size: 12.5))
                                .foregroundColor(on ? Color("BgPrimary") : Color("TextSecondary"))
                                .padding(.horizontal, 12)
                                .frame(height: 30)
                                .background(Capsule().fill(on ? Color("TextPrimary") : .clear))
                                .overlay(Capsule().stroke(on ? Color("TextPrimary") : Color("BorderSubtle")))
                        }
                        .buttonStyle(.plain)
                        .accessibilityAddTraits(on ? .isSelected : [])
                    }
                }
            }
        }
        row("Movement", "Still draws one frame. Reduce motion always wins.") {
            BackdropSegmented(label: "Movement", value: $store.motion,
                              options: [(.still, "Still"), (.slow, "Slow"), (.normal, "Normal")])
        }
        if StageBackdrops.isAscii(store.variant) {
            row("Colours", "The look's own colours, the bot's colour, or black and white") {
                BackdropSegmented(label: "Colours", value: $store.colour,
                                  options: [(.scene, "As designed"), (.accent, "Bot's colour"), (.mono, "Mono")])
            }
            row("Characters", "Plain text, or each character on its own tile") {
                BackdropSegmented(label: "Characters", value: $store.chars,
                                  options: [(.design, "As designed"), (.text, "Text"), (.tiles, "Tiles")])
            }
            row("Character size", "Bigger characters read as a softer pattern") {
                BackdropSegmented(label: "Character size", value: $store.charSize,
                                  options: [(.small, "Small"), (.medium, "Medium"), (.large, "Large")])
            }
            row("Density", "How much is drawn") {
                Slider(value: $store.density, in: 0.4...1.6, step: 0.1).accessibilityLabel("Density")
            }
            row("Veil behind messages", "A soft fade behind the conversation so text stays easy to read") {
                Slider(value: $store.veil, in: 20...70, step: 2).accessibilityLabel("Veil behind messages")
            }
        }
    }

    private func row<Content: View>(_ title: String, _ detail: String, @ViewBuilder _ control: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.system(size: 14, weight: .medium)).foregroundColor(Color("TextPrimary"))
            Text(detail).font(.system(size: 12.5)).foregroundColor(Color("TextSecondary"))
            control()
        }
    }
}

/// One screen's look (a bot, a group chat, a Code workspace): "Same as
/// Settings" goes back to the default; any other pick applies here only.
struct StageBackdropScopeSheet: View {
    let scope: BackdropScope
    let title: String
    var colors: [String?] = StageBackdrops.sampleColors

    @ObservedObject private var store = StageBackdropStore.shared
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section {
                    row(id: nil, name: "Same as Settings", note: StageBackdrops.name(store.variant))
                }
                Section("ASCII") {
                    ForEach(StageBackdrops.ascii) { row(id: $0.id, name: $0.name, note: $0.note) }
                }
                Section("Classic") {
                    ForEach(StageBackdrops.classic) { row(id: $0.id, name: $0.name, note: $0.note) }
                }
            }
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
        .presentationDetents([.medium, .large])
    }

    private func row(id: String?, name: String, note: String) -> some View {
        let on = store.own(scope) == id
        return Button {
            store.setOwn(scope, id)
        } label: {
            HStack(spacing: 12) {
                BackdropSwatch(variant: id ?? store.variant, colors: colors)
                    .frame(width: 44, height: 32)
                VStack(alignment: .leading, spacing: 2) {
                    Text(name).font(.system(size: 15)).foregroundColor(Color("TextPrimary"))
                    Text(note).font(.system(size: 12)).foregroundColor(Color("TextSecondary")).lineLimit(1)
                }
                Spacer(minLength: 0)
                if on { Image(systemName: "checkmark").foregroundColor(Color("AccentPrimary")) }
            }
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}
