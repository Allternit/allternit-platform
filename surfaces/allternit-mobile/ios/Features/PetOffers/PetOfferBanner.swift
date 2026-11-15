import SwiftUI
import UIKit

/// The pet-offers banner: the bot says a run failed and offers help. It sits
/// over the top of every screen (WorkspaceShell), leaves after 8 s, and never
/// sends anything. "Fix it" opens that bot's chat with the composer filled;
/// the owner reads it and presses send.
struct PetOfferBannerLayer: View {
    @ObservedObject private var center = PetOfferCenter.shared
    @ObservedObject private var hub = AgentHubStore.shared
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// Leaves after 8 s; VoiceOver users get longer to hear it.
    private var lifetime: Duration { UIAccessibility.isVoiceOverRunning ? .seconds(20) : .seconds(8) }

    var body: some View {
        VStack {
            if let offer = center.current {
                PetOfferBanner(
                    offer: offer,
                    bot: bot(for: offer),
                    onFix: { fix(offer) },
                    onNotNow: { center.dismiss(offer.id) },
                    onMute: { mute(offer) }
                )
                .padding(.horizontal, 12)
                .padding(.top, 6)
                .transition(reduceMotion ? .opacity : .move(edge: .top).combined(with: .opacity))
                .task(id: offer.id) {
                    try? await Task.sleep(for: lifetime)
                    guard !Task.isCancelled else { return }
                    center.dismiss(offer.id)
                }
            }
            Spacer(minLength: 0)
        }
        .animation(reduceMotion ? .linear(duration: 0.12) : .spring(response: 0.38, dampingFraction: 1), value: center.current?.id)
        .allowsHitTesting(center.current != nil)
    }

    private func bot(for offer: PetOffer) -> AgentRecord? {
        if let id = offer.botId, let bot = hub.agent(withId: id) { return bot }
        return hub.agents.first(where: \.isMainBot)
    }

    private func fix(_ offer: PetOffer) {
        let prompt = PetOfferCenter.prefill(for: offer)
        center.dismiss(offer.id)
        if let id = bot(for: offer)?.id ?? offer.botId {
            DeepLinkRouter.shared.enqueue(.botChat(botId: id, prompt: prompt))
        } else {
            DeepLinkRouter.shared.setComposerPrefill(prompt)
            DeepLinkRouter.shared.enqueue(.callMain)
        }
    }

    private func mute(_ offer: PetOffer) {
        if let id = bot(for: offer)?.id ?? offer.botId { center.muteBot(id) } else { center.dismiss(offer.id) }
    }
}

struct PetOfferBanner: View {
    let offer: PetOffer
    let bot: AgentRecord?
    let onFix: () -> Void
    let onNotNow: () -> Void
    let onMute: () -> Void

    private var botName: String { bot?.botDisplayName ?? "your bot" }
    private var copy: String { PetOfferCenter.copy(for: offer.kind) }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Group {
                    if let bot { BotAvatarView(bot: bot, size: 36, live: true) } else { Image(systemName: "exclamationmark.triangle").font(.title3) }
                }
                .frame(width: 36, height: 36)
                .accessibilityHidden(true)
                Text(copy)
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(Color("TextPrimary"))
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            HStack(spacing: 8) {
                Button(PetOfferContract.fixItLabel, action: onFix)
                    .font(.subheadline.weight(.semibold))
                    .padding(.horizontal, 14).padding(.vertical, 8)
                    .background(Color("TextPrimary"), in: Capsule())
                    .foregroundStyle(Color("BgPrimary"))
                Button("Not now", action: onNotNow)
                    .font(.subheadline)
                    .padding(.horizontal, 12).padding(.vertical, 8)
                    .background(Color("FillNeutral"), in: Capsule())
                    .foregroundStyle(Color("TextPrimary"))
                Spacer(minLength: 0)
                Button("Mute \(botName)", action: onMute)
                    .font(.footnote)
                    .foregroundStyle(Color("TextSecondary"))
                    .padding(.vertical, 8)
                    .lineLimit(1)
            }
            .buttonStyle(.plain)
        }
        .padding(12)
        .background(Color("BgPanel"), in: RoundedRectangle(cornerRadius: 18, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .shadow(color: .black.opacity(0.12), radius: 12, y: 4)
        // Long-press is a shortcut for Mute; the Mute button is the main way.
        .onLongPressGesture(minimumDuration: 0.6) { onMute() }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(botName): \(copy)")
        .accessibilityAction(named: PetOfferContract.fixItLabel, onFix)
        .accessibilityAction(named: "Not now", onNotNow)
        .accessibilityAction(named: "Mute \(botName)", onMute)
    }
}
