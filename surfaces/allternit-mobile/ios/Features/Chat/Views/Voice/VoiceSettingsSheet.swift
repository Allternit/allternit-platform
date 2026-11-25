import AVFoundation
import SwiftUI

/// Voice settings sheet (Phase 7b, Claude parity): voice picker carousel
/// (cards like Claude's "Buttery/Airy"), speech language, playback speed,
/// and the voice-mode interaction style (Hands free / Push to talk).
///
/// Voice source: on-device AVSpeechSynthesisVoice options — SpeechSpeaker
/// (the actual TTS engine) applies the selected identifier to its
/// utterances.
struct VoiceSettingsSheet: View {
    /// One selectable voice card from the on-device voice list.
    private struct VoiceOption: Identifiable, Equatable {
        let id: String
        let name: String
        /// Caption under the name (a locale plus a quality tag).
        let detail: String
    }

    /// Opened from a bot's live mode: its own look comes first.
    var bot: AgentRecord? = nil

    @StateObject private var settings = SettingsStore.shared
    @State private var voices: [VoiceOption] = []
    @State private var centeredID: String?
    @State private var sampler = AVSpeechSynthesizer()
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    if let bot { LiveModeLookSection(bot: bot) }
                    LiveModeLookSection()
                    voiceCarousel
                    languageRow
                    speedRow
                    modeRow
                }
                .padding(.vertical, 20)
            }
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .navigationTitle("Voice settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                        .foregroundColor(Color("AccentPrimary"))
                }
            }
        }
        .presentationDetents([.medium, .large])
        .task { await loadVoices() }
    }

    // MARK: - Voice carousel

    private var voiceCarousel: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Voice")
                .font(.subheadline)
                .fontWeight(.semibold)
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 20)

            if voices.isEmpty {
                HStack(spacing: 8) {
                    ProgressView().scaleEffect(0.7)
                    Text("Loading voices…")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
                .padding(.horizontal, 20)
            } else {
                carouselCards
            }
        }
    }

    /// Snapping card row: the centered card is large, neighbors are small and
    /// dim, page dots sit under it, and a glow sits under the selected card.
    private var carouselCards: some View {
        let cardSize: CGFloat = 132
        return VStack(spacing: 12) {
            GeometryReader { geo in
                let side = max((geo.size.width - cardSize) / 2, 0)
                ScrollView(.horizontal, showsIndicators: false) {
                    LazyHStack(spacing: 10) {
                        ForEach(voices) { voice in
                            voiceCard(voice, cardSize: cardSize)
                                .scrollTransition(.interactive, axis: .horizontal) { content, phase in
                                    content
                                        .scaleEffect(reduceMotion ? 1 : (phase.isIdentity ? 1 : 0.82))
                                        .opacity(reduceMotion ? 1 : (phase.isIdentity ? 1 : 0.6))
                                }
                                .id(voice.id)
                        }
                    }
                    .scrollTargetLayout()
                    .padding(.horizontal, side)
                }
                .scrollTargetBehavior(.viewAligned)
                .scrollPosition(id: $centeredID, anchor: .center)
            }
            .frame(height: cardSize + 20)

            HStack(spacing: 6) {
                ForEach(voices) { voice in
                    Circle()
                        .fill(voice.id == centeredID ? Color("AccentPrimary") : Color("TextSecondary").opacity(0.3))
                        .frame(width: 6, height: 6)
                }
            }
            .accessibilityHidden(true)
        }
        .onAppear { syncCenterToSelection() }
        .onChange(of: voices) { _, _ in syncCenterToSelection() }
        .onChange(of: centeredID) { _, newID in
            // Swiping a card to the center picks it and plays the sample.
            guard let newID, newID != settings.voiceIdentifier,
                  let voice = voices.first(where: { $0.id == newID }) else { return }
            select(voice)
        }
    }

    private func syncCenterToSelection() {
        guard !voices.isEmpty else { return }
        centeredID = settings.voiceIdentifier ?? voices.first?.id
    }

    private func select(_ voice: VoiceOption) {
        Haptics.selection()
        settings.voiceIdentifier = voice.id
        playSample(voice)
    }

    private func playSample(_ voice: VoiceOption) {
        sampler.stopSpeaking(at: .immediate)
        let utterance = AVSpeechUtterance(string: "Hi, this is \(voice.name).")
        utterance.voice = AVSpeechSynthesisVoice(identifier: voice.id)
        utterance.rate = AVSpeechUtteranceDefaultSpeechRate * Float(settings.speechSpeed)
        sampler.speak(utterance)
    }

    private func voiceCard(_ voice: VoiceOption, cardSize: CGFloat) -> some View {
        let isSelected = (settings.voiceIdentifier ?? "") == voice.id
            || (settings.voiceIdentifier == nil && voice.id == voices.first?.id)
        return Button(action: {
            withAnimation(reduceMotion ? nil : .spring(response: 0.5, dampingFraction: 0.82)) {
                centeredID = voice.id
            }
            select(voice)
        }) {
            VStack(alignment: .leading, spacing: 8) {
                Image(systemName: "waveform")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundColor(isSelected ? Color("AccentPrimary") : Color("TextSecondary"))

                Spacer()

                Text(voice.name)
                    .font(.system(.subheadline, design: .serif))
                    .fontWeight(.semibold)
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)

                Text(voice.detail)
                    .font(.caption2)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
            }
            .padding(12)
            .frame(width: cardSize, height: cardSize, alignment: .topLeading)
            .background(Color("BgSecondary"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusMD)
                    .stroke(isSelected ? Color("AccentPrimary") : Theme.borderWarmDefault,
                            lineWidth: isSelected ? 2 : 1)
            )
            .background(alignment: .bottom) {
                // Soft glow under the selected card.
                Ellipse()
                    .fill(Color("AccentPrimary").opacity(isSelected ? 0.35 : 0))
                    .frame(width: cardSize * 0.7, height: 36)
                    .blur(radius: 18)
                    .offset(y: 18)
                    .animation(reduceMotion ? nil : .easeOut(duration: 0.3), value: isSelected)
            }
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }

    // MARK: - Language / speed / mode rows

    private var languageRow: some View {
        settingsRow(title: "Language", detail: "Speech recognition and playback language") {
            Picker(selection: Binding(
                get: { settings.speechLanguage },
                set: { settings.speechLanguage = $0 }
            )) {
                Text("System default").tag(SpeechLanguage?.none)
                ForEach(SpeechLanguage.allCases, id: \.self) { language in
                    Text(language.label).tag(SpeechLanguage?.some(language))
                }
            } label: {
                EmptyView()
            }
            .pickerStyle(.menu)
            .tint(Color("TextSecondary"))
        }
    }

    private var speedRow: some View {
        settingsRow(title: "Speed", detail: "Voice playback speed") {
            Picker(selection: $settings.speechSpeed) {
                ForEach(SettingsStore.speechSpeeds, id: \.self) { speed in
                    Text(speed == 1.0 ? "1×" : String(format: "%g×", speed)).tag(speed)
                }
            } label: {
                EmptyView()
            }
            .pickerStyle(.menu)
        }
    }

    private var modeRow: some View {
        settingsRow(title: "Mode", detail: "Hands free listens after each reply; push to talk records while you hold the mic") {
            Picker(selection: $settings.voiceInteractionMode) {
                ForEach(VoiceInteractionMode.allCases, id: \.self) { mode in
                    Text(mode.label).tag(mode)
                }
            } label: {
                EmptyView()
            }
            .pickerStyle(.menu)
        }
    }

    private func settingsRow<Control: View>(title: String, detail: String, @ViewBuilder control: () -> Control) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.subheadline)
                    .fontWeight(.semibold)
                    .foregroundColor(Color("TextPrimary"))
                Text(detail)
                    .font(.caption2)
                    .foregroundColor(Color("TextSecondary"))
            }
            control()
        }
        .padding(.horizontal, 20)
    }

    // MARK: - Voice loading

    /// Loads the carousel from the on-device AVSpeechSynthesisVoice list —
    /// the voices SpeechSpeaker can actually speak with.
    private func loadVoices() async {
        voices = Self.onDeviceVoices(preferredLanguage: settings.speechLanguage)
    }

    /// On-device voice list: enhanced/premium voices first, preferring
    /// the speech-language setting's locale, capped so the carousel stays
    /// scannable.
    nonisolated private static func onDeviceVoices(preferredLanguage: SpeechLanguage?) -> [VoiceOption] {
        let all = AVSpeechSynthesisVoice.speechVoices()
        let preferredPrefix = preferredLanguage?.rawValue
            ?? Locale.current.language.languageCode?.identifier ?? "en"

        func rank(_ voice: AVSpeechSynthesisVoice) -> Int {
            var score = 0
            if voice.language.hasPrefix(preferredPrefix) { score += 10 }
            switch voice.quality {
            case .enhanced: score += 2
            case .premium: score += 3
            default: break
            }
            return score
        }

        return all
            .sorted { rank($0) > rank($1) }
            .prefix(12)
            .map { voice in
                VoiceOption(
                    id: voice.identifier,
                    name: voice.name,
                    detail: [
                        voice.language,
                        voice.quality == .premium ? "Premium" : (voice.quality == .enhanced ? "Enhanced" : nil)
                    ].compactMap { $0 }.joined(separator: " · ")
                )
            }
    }
}
