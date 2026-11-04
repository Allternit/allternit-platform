import SwiftUI

/// Settings › Voice › Reply voice: the on-device Kokoro voice bots answer
/// in (iOS 18+ devices). Download/remove the ~190 MB pack and pick the voice
/// used when a bot has no voice of its own.
struct ReplyVoiceSection: View {
    @ObservedObject private var pack = VoicePackManager.kokoro
    @AppStorage(KokoroVoices.settingKey) private var voiceId = KokoroVoices.defaultId

    private static let megabytes = Int((VoicePack.kokoro.totalBytes + 999_999) / 1_000_000)
    private let columns = [GridItem(.adaptive(minimum: 104), spacing: 8)]

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            SettingsSectionHeading(text: "Reply voice")
            if KokoroVoices.deviceSupported {
                SettingsDescription(text: "Bots answer in Kokoro, a natural voice that runs on this iPhone. It downloads once (\(Self.megabytes) MB, on Wi-Fi unless you tap Download). Until then, and if this iPhone runs low on memory, the built-in voice reads replies.")
                packRow
                SettingsDescription(text: "Voice for bots that don't have their own:")
                LazyVGrid(columns: columns, alignment: .leading, spacing: 8) {
                    ForEach(KokoroVoices.all, id: \.self) { id in
                        voiceChip(id)
                    }
                }
            } else {
                SettingsDescription(text: "Replies use the iPhone's built-in voice. The on-device Kokoro voice needs iOS 18.")
            }
        }
    }

    @ViewBuilder private var packRow: some View {
        SettingsLabeledRow(title: "Kokoro voice") {
            switch pack.state {
            case .ready:
                Button("Remove") { pack.remove() }
                    .buttonStyle(SettingsQuietButtonStyle())
            case .downloading(let progress):
                HStack(spacing: 8) {
                    ProgressView(value: progress).frame(width: 80)
                    Text("\(Int(progress * 100))%")
                        .font(.system(size: 13, design: .monospaced))
                        .foregroundColor(Color("TextSecondary"))
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel("Downloading, \(Int(progress * 100)) percent")
            case .missing, .failed:
                Button("Download") { Task { await pack.install(cellular: true) } }
                    .buttonStyle(SettingsPrimaryButtonStyle())
            }
        }
        if case .failed(let message) = pack.state {
            SettingsErrorBanner(message: message)
        }
    }

    private func voiceChip(_ id: String) -> some View {
        let selected = voiceId == id
        return Button { voiceId = id } label: {
            Text(KokoroVoices.label(id))
                .font(.system(size: 13, weight: selected ? .semibold : .regular))
                .foregroundColor(selected ? Color("AccentPrimary") : Color("TextPrimary"))
                .lineLimit(1)
                .minimumScaleFactor(0.8)
                .frame(maxWidth: .infinity, minHeight: 36)
                .overlay(
                    RoundedRectangle(cornerRadius: 10, style: .continuous)
                        .stroke(selected ? Color("AccentPrimary") : Color("BorderSubtle"), lineWidth: selected ? 2 : 1)
                )
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}
