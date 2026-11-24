import SwiftUI

/// "While you were away" over the Workspace floor: what the bots answered,
/// what waits on the person, what finished, and open suggestions, from
/// `GET /floor/digest`. A row opens that bot's thread sheet (OfficeThreadSheet).
/// The floor shows it once, when the person was away 30 minutes or more and
/// there is at least one item, then marks the floor seen (`POST /floor/seen`),
/// so no device shows it again for the same stretch.
struct OfficeDigestView: View {
    let digest: FloorDigest
    let mainBotName: String
    let onOpenBot: (String) -> Void
    let onClose: () -> Void

    @ObservedObject private var store = BotModeStore.shared
    @ObservedObject private var colors = BotColorStore.shared

    /// Rows shown per section; the rest are counted.
    private static let perSection = 4

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("While you were away")
                        .font(.headline)
                        .foregroundColor(Color("TextPrimary"))
                        .accessibilityAddTraits(.isHeader)
                    Text(sinceLine)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
                Spacer(minLength: 0)
                Button(action: onClose) {
                    Image(systemName: "xmark")
                        .font(.system(size: 13, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 30, height: 30)
                        .background(Circle().fill(Color("FillNeutral")))
                        .frame(width: 44, height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Close")
            }
            .padding(.leading, 14)
            .padding(.trailing, 4)
            .padding(.top, 6)

            // Short digests sit at their own height; long ones scroll inside the card.
            ViewThatFits(in: .vertical) {
                sections
                ScrollView { sections }
            }
        }
        .frame(maxWidth: 520, maxHeight: 420)
        .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .shadow(color: .black.opacity(0.12), radius: 14, y: 4)
        .accessibilityElement(children: .contain)
    }

    private var sections: some View {
        VStack(alignment: .leading, spacing: 12) {
            section("Answered by \(mainBotName) on autopilot", items: digest.answered)
            section("Waiting on you", items: digest.waiting)
            section("Finished", items: digest.finished)
            section("Suggestions", items: digest.suggestions)
        }
        .padding(.horizontal, 14)
        .padding(.bottom, 14)
        .padding(.top, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var sinceLine: String {
        let since = Date(timeIntervalSince1970: digest.since / 1000)
        let relative = RelativeDateTimeFormatter()
        relative.unitsStyle = .full
        return "Since \(relative.localizedString(for: since, relativeTo: Date()))"
    }

    @ViewBuilder
    private func section(_ title: String, items: [FloorDigest.Item]) -> some View {
        if !items.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.caption.weight(.semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .accessibilityAddTraits(.isHeader)
                ForEach(items.prefix(Self.perSection)) { item in row(item) }
                if items.count > Self.perSection {
                    Text("\(items.count - Self.perSection) more")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
        }
    }

    private func row(_ item: FloorDigest.Item) -> some View {
        let bot = store.bot(item.botId)
        return Button {
            if bot != nil { onOpenBot(item.botId) }
        } label: {
            HStack(alignment: .top, spacing: 10) {
                if let bot {
                    BotAvatarView(bot: bot, size: 26)
                } else {
                    Circle().fill(Color("FillNeutral")).frame(width: 26, height: 26)
                }
                VStack(alignment: .leading, spacing: 1) {
                    if let bot {
                        Text(bot.botDisplayName)
                            .font(.caption.weight(.semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                    }
                    Text(item.text)
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)
                }
                Spacer(minLength: 0)
            }
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(bot == nil)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(bot?.botDisplayName ?? "A bot"): \(item.text)")
        .accessibilityHint(bot == nil ? "" : "Opens \(bot!.botDisplayName)'s thread")
    }
}
