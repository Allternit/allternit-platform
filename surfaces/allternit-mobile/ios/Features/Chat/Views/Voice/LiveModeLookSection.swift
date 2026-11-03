import SwiftUI

/// Live mode's look in settings (allternit-ai SpeakerLookPicker.tsx): the
/// speaker style tiles, the ring colours and the caption size. With a bot it
/// edits that bot's own look, with "Same as Settings" first.
struct LiveModeLookSection: View {
    /// nil edits the defaults (Settings → Voice).
    var bot: AgentRecord? = nil
    /// Side padding (the settings page pads its own content).
    var inset: CGFloat = 20

    @ObservedObject private var store = SpeakerLookStore.shared
    @ObservedObject private var botColors = BotColorStore.shared
    @State private var showMotion = true

    private var botColor: Color? { bot.flatMap { botColors.hex(for: $0) }.map { Color(hex: $0) } }
    private var ownStyle: SpeakerStyle? { bot.flatMap { store.ownStyle(for: $0.id) } }
    private var ownPalette: RingPalette? { bot.flatMap { store.ownPalette(for: $0.id) } }
    private var currentStyle: SpeakerStyle? { bot == nil ? store.style : ownStyle }
    private var currentPalette: RingPalette? { bot == nil ? store.palette : ownPalette }

    var body: some View {
        let _ = store.botRevision
        VStack(alignment: .leading, spacing: 14) {
            Text(bot.map { "\($0.botDisplayName)'s live mode look" } ?? "Live mode")
                .font(.subheadline.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
            Text("What moves around a bot's avatar while you talk by voice.")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))

            Picker("Style set", selection: $showMotion) {
                Text("Motion").tag(true)
                Text("Allternit").tag(false)
            }
            .pickerStyle(.segmented)
            .frame(maxWidth: 260)
            styleTiles

            Text("Colour").font(.footnote.weight(.semibold)).foregroundColor(Color("TextPrimary"))
            colourChips
            Text("Bot's colour follows whichever bot you talk with. Muted and stopped calls turn grey.")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))

            if bot == nil {
                Text("Caption size").font(.footnote.weight(.semibold)).foregroundColor(Color("TextPrimary"))
                Picker("Caption size", selection: $store.captionSize) {
                    ForEach(CaptionSize.allCases) { Text($0.name).tag($0) }
                }
                .pickerStyle(.segmented)
            }
        }
        .padding(.horizontal, inset)
        .onAppear { showMotion = (currentStyle ?? store.style).isMotion }
    }

    private var styleTiles: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 10) {
                if let bot {
                    tile(title: "Same as Settings", subtitle: store.style.name, selected: ownStyle == nil) {
                        store.setOwnStyle(nil, for: bot.id)
                    } preview: {
                        Text("Settings").font(.caption2).foregroundColor(Color("TextSecondary"))
                    }
                }
                ForEach(SpeakerStyle.allCases.filter { $0.isMotion == showMotion }) { style in
                    tile(title: style.name, subtitle: nil, selected: currentStyle == style) {
                        if let bot { store.setOwnStyle(style, for: bot.id) } else { store.style = style }
                    } preview: {
                        VoiceSpeakerView(phase: .speaking, muted: false, style: style, palette: currentPalette ?? store.palette,
                                         botColor: botColor, size: 30, compact: true, still: currentStyle != style) {
                            previewFace
                        }
                    }
                    .accessibilityHint(style.note)
                }
            }
            .padding(.vertical, 2)
        }
    }

    @ViewBuilder
    private var previewFace: some View {
        if let bot {
            BotAvatarView(bot: bot, size: 30)
        } else {
            Circle().fill(Color("BgTertiary")).frame(width: 30, height: 30)
        }
    }

    private func tile<P: View>(title: String, subtitle: String?, selected: Bool, action: @escaping () -> Void, @ViewBuilder preview: () -> P) -> some View {
        Button(action: {
            UISelectionFeedbackGenerator().selectionChanged()
            action()
        }) {
            VStack(alignment: .leading, spacing: 6) {
                ZStack { preview() }
                    .frame(width: 96, height: 60)
                    .background(Color("BgPanel"))
                    .clipShape(RoundedRectangle(cornerRadius: 10))
                    .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(selected ? Color("TextPrimary") : Color("BorderSubtle"), lineWidth: selected ? 2 : 1))
                Text(title).font(.caption.weight(selected ? .semibold : .regular)).foregroundColor(Color("TextPrimary")).lineLimit(1)
                if let subtitle { Text(subtitle).font(.caption2).foregroundColor(Color("TextSecondary")).lineLimit(1) }
            }
            .frame(width: 96, alignment: .leading)
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private var colourChips: some View {
        let columns = [GridItem(.adaptive(minimum: 116), spacing: 8, alignment: .leading)]
        return LazyVGrid(columns: columns, alignment: .leading, spacing: 8) {
            if let bot {
                chip(name: "Same as Settings", colors: nil, neutral: true, selected: ownPalette == nil) { store.setOwnPalette(nil, for: bot.id) }
            }
            ForEach(RingPalette.allCases) { p in
                chip(name: p.name, colors: p.colors(bot: botColor) ?? RingPalette.spectrumColors, neutral: false, selected: currentPalette == p) {
                    if let bot { store.setOwnPalette(p, for: bot.id) } else { store.palette = p }
                }
            }
        }
    }

    private func chip(name: String, colors: [Color]?, neutral: Bool, selected: Bool, action: @escaping () -> Void) -> some View {
        Button(action: {
            UISelectionFeedbackGenerator().selectionChanged()
            action()
        }) {
            HStack(spacing: 6) {
                Circle()
                    .strokeBorder(neutral ? AnyShapeStyle(Color("BgTertiary")) : AnyShapeStyle(AngularGradient(colors: colors ?? [], center: .center)), lineWidth: 5)
                    .frame(width: 20, height: 20)
                Text(name).font(.caption).foregroundColor(Color("TextPrimary")).lineLimit(1)
            }
            .padding(.leading, 4)
            .padding(.trailing, 10)
            .frame(height: 30)
            .background(Capsule().fill(Color("BgPanel")))
            .overlay(Capsule().strokeBorder(selected ? Color("TextPrimary") : Color("BorderSubtle"), lineWidth: selected ? 1.5 : 1))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}
