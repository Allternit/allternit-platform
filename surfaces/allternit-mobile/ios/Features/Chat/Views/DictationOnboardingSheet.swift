import SwiftUI

/// First-run dictation onboarding (Claude iOS parity): shown ONCE before the
/// first dictation, ahead of the mic permission priming sheet — mic
/// illustration, headline, three benefit bullets, a Speech language row with
/// an inline locale picker, and a big Continue.
///
/// Continue marks the sheet shown and hands off to the caller, which then
/// runs the existing flow (permission priming if system prompts are pending,
/// else dictation starts). Swipe-dismissing does NOT mark it shown — the
/// next mic tap offers it again, matching "once before the first dictation".
///
/// The Speech language row writes the same SettingsStore key as
/// Settings → Voice; the recognizer reads it at session start.
struct DictationOnboardingSheet: View {
    /// Called after Continue; the caller then primes permissions / starts
    /// dictation.
    let onContinue: () -> Void

    @Environment(\.dismiss) private var dismiss
    @StateObject private var settings = SettingsStore.shared
    /// The Speech language row expands into the locale list in place.
    @State private var isLanguageListExpanded = false

    // MARK: Shown-once flag

    private static let shownKey = "allternit-dictation-onboarding-shown"

    static var hasShown: Bool { UserDefaults.standard.bool(forKey: shownKey) }
    static func markShown() { UserDefaults.standard.set(true, forKey: shownKey) }
    /// `-reset-onboarding` (DEBUG): show the sheet again on next mic tap.
    static func resetShown() { UserDefaults.standard.removeObject(forKey: shownKey) }

    private struct Bullet: Identifiable {
        let id: String
        let icon: String
        let text: String
    }

    private static let bullets: [Bullet] = [
        Bullet(id: "language", icon: "globe", text: "Choose a language to speak in"),
        Bullet(id: "duration", icon: "clock", text: "Dictate for up to 10 minutes"),
        Bullet(id: "natural", icon: "text.bubble", text: "Chat more quickly and naturally"),
    ]

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(spacing: 20) {
                    // ── Mic illustration (accent circle, mirrors the priming
                    // sheet's icon treatment) ──
                    Image(systemName: "mic.fill")
                        .font(.system(size: 28, weight: .medium))
                        .foregroundColor(Color("AccentPrimary"))
                        .frame(width: 72, height: 72)
                        .background(Color("AccentPrimary").opacity(0.14))
                        .clipShape(Circle())
                        .overlay(
                            Circle()
                                .stroke(Color("AccentPrimary").opacity(0.28), lineWidth: 1)
                        )
                        .padding(.top, 24)

                    Text("Send messages to Allternit using your voice.")
                        .font(.system(.title3, design: .serif))
                        .fontWeight(.medium)
                        .foregroundColor(Color("TextPrimary"))
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 24)

                    // ── Benefit bullets ──
                    VStack(alignment: .leading, spacing: 14) {
                        ForEach(Self.bullets) { bullet in
                            HStack(spacing: 12) {
                                Image(systemName: bullet.icon)
                                    .font(.system(size: 14, weight: .medium))
                                    .foregroundColor(Color("AccentPrimary"))
                                    .frame(width: 24)
                                Text(bullet.text)
                                    .font(.subheadline)
                                    .foregroundColor(Color("TextPrimary"))
                            }
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 40)

                    // ── Speech language row → inline locale picker ──
                    VStack(spacing: 0) {
                        Button(action: {
                            Haptics.light()
                            withAnimation { isLanguageListExpanded.toggle() }
                        }) {
                            HStack {
                                Text("Speech language")
                                    .font(.subheadline)
                                    .foregroundColor(Color("TextPrimary"))
                                Spacer()
                                Text(settings.speechLanguage?.label ?? "System default")
                                    .font(.subheadline)
                                    .foregroundColor(Color("TextSecondary"))
                                Image(systemName: "chevron.right")
                                    .font(.system(size: 11, weight: .bold))
                                    .foregroundColor(Color("TextSecondary"))
                                    .rotationEffect(.degrees(isLanguageListExpanded ? 90 : 0))
                            }
                            .padding(.horizontal, 16)
                            .frame(height: 48)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)

                        if isLanguageListExpanded {
                            Divider().background(Color("BorderSubtle"))
                            languageOption(label: "System default", value: nil)
                            ForEach(SpeechLanguage.allCases, id: \.self) { language in
                                languageOption(label: language.label, value: language)
                            }
                        }
                    }
                    .background(Color("BgSecondary"))
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                    .overlay(
                        RoundedRectangle(cornerRadius: Theme.radiusMD)
                            .stroke(Theme.borderWarmDefault, lineWidth: 1)
                    )
                    .padding(.horizontal, 20)
                }
            }

            // ── Continue ──
            Button(action: {
                Haptics.light()
                Self.markShown()
                dismiss()
                onContinue()
            }) {
                Text("Continue")
                    .font(.system(.body, weight: .semibold))
                    .foregroundColor(.black)
                    .frame(maxWidth: .infinity)
                    .frame(height: 48)
                    .background(Color("AccentPrimary"))
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
            }
            .padding(.horizontal, 20)
            .padding(.top, 12)
            .padding(.bottom, 12)
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        // .medium leaves room for the collapsed sheet; the expanded locale
        // list scrolls inside it.
        .presentationDetents([.medium, .large])
    }

    private func languageOption(label: String, value: SpeechLanguage?) -> some View {
        Button(action: {
            Haptics.light()
            settings.speechLanguage = value
            withAnimation { isLanguageListExpanded = false }
        }) {
            HStack {
                Text(label)
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                if settings.speechLanguage == value {
                    Image(systemName: "checkmark")
                        .font(.system(size: 12, weight: .bold))
                        .foregroundColor(Color("AccentPrimary"))
                }
            }
            .padding(.horizontal, 16)
            .frame(height: 40)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}
