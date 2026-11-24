import SwiftUI

/// Bottom-sheet agent | bot picker (mockup B2, docs/agent-hub-options.html):
/// the platform's agent | bot hub condensed to a quick-switcher — "Default
/// agent/bot" plus every registry agent valid for this surface, one tap to
/// select. Presented by `AgentSelectionMenu` (the deck pill) on chat and
/// cowork; the selection itself lives in AgentModeStore, per surface,
/// persisted.
///
/// Rows are CARDS in the Artifacts Library's language (BgPanel, warm
/// border, radiusMD on a BgSecondary bed) — the selected card gets the
/// accent border + check, like the composer's agent-on glow.
struct AgentSelectionSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore

    private var surface: AppMode { modeStore.mode }

    var body: some View {
        NavigationStack {
            ScrollView {
                LazyVStack(spacing: 8) {
                    // Section label — same caps style as the hub's.
                    HStack {
                        Text("CHOOSE AGENT | BOT")
                            .accessibilityIdentifier("agentSheetTitle")
                            .font(.system(size: 11, weight: .bold))
                            .tracking(1)
                            .foregroundColor(Color("TextSecondary"))
                        Spacer()
                    }
                    .padding(.horizontal, 4)
                    .padding(.bottom, 4)

                    agentCard(agent: nil)
                    ForEach(agentModeStore.agentsForSurface(surface)) { agent in
                        agentCard(agent: agent)
                    }

                    if agentModeStore.isLoadingAgents, agentModeStore.agents.isEmpty {
                        FriendlyInlineStateView(
                            style: .empty,
                            icon: "arrow.clockwise",
                            title: "Loading agents",
                            message: "Hang tight…"
                        )
                    } else if let error = agentModeStore.agentsError {
                        FriendlyInlineStateView(
                            style: .offline,
                            icon: "wifi.slash",
                            title: "Couldn't load agents",
                            message: FriendlyErrorMessage.from(error),
                            actionTitle: "Retry",
                            action: { agentModeStore.fetchAgentsIfNeeded(force: true) }
                        )
                    }

                    actionCard("New from template", systemImage: "plus", action: openHub)
                    actionCard("agent | bot hub", systemImage: "cpu", action: openHub)
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 16)
            }
            .background(Color("BgSecondary"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                        .accessibilityIdentifier("agentSheetDone")
                }
            }
        }
        .presentationDetents([.medium, .large])
        .onAppear { agentModeStore.fetchAgentsIfNeeded() }
    }

    // MARK: - Cards

    /// One selectable agent card (nil = the platform's default agent).
    @ViewBuilder
    private func agentCard(agent: AgentRecord?) -> some View {
        let isSelected = agentModeStore.selectedAgentId(for: surface) == agent?.id
        Button(action: {
            Haptics.light()
            agentModeStore.selectAgent(agent, for: surface)
            dismiss()
        }) {
            HStack(spacing: 12) {
                if let agent {
                    AgentAvatarView(agent: agent, size: 40)
                } else {
                    // Default-agent tile: the platform's built-in behavior,
                    // same accent tile as the registry avatars.
                    ZStack {
                        RoundedRectangle(cornerRadius: 40 * 0.325, style: .continuous)
                            .fill(Color("AccentPrimary").opacity(0.14))
                        Image(systemName: "sparkles")
                            .font(.system(size: 40 * 0.42, weight: .medium))
                            .foregroundColor(Color("AccentPrimary"))
                    }
                    .frame(width: 40, height: 40)
                }

                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(agent?.name ?? "Default agent/bot")
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                        if agent?.isPrimary == true {
                            Text("PRIMARY")
                                .font(.system(size: 8, weight: .bold))
                                .tracking(0.6)
                                .foregroundColor(Color("AccentPrimary"))
                                .padding(.horizontal, 6)
                                .padding(.vertical, 2)
                                .background(Color("AccentPrimary").opacity(0.14))
                                .clipShape(Capsule())
                        }
                    }
                    Text(agent?.description ?? "The platform's built-in agent")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }

                Spacer(minLength: 8)

                VStack(alignment: .trailing, spacing: 5) {
                    if isSelected {
                        Image(systemName: "checkmark.circle.fill")
                            .font(.system(size: 17))
                            .foregroundColor(Color("AccentPrimary"))
                    }
                    if let agent, !agent.model.isEmpty {
                        Text(agent.model)
                            .font(.system(size: 10, weight: .semibold))
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(1)
                            .padding(.horizontal, 8)
                            .padding(.vertical, 2.5)
                            .background(Color("BgSecondary"))
                            .clipShape(Capsule())
                    }
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            .background(isSelected ? Color("AccentPrimary").opacity(0.05) : Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusMD)
                    .stroke(isSelected ? Color("AccentPrimary") : Theme.borderWarmDefault,
                            lineWidth: isSelected ? 1.5 : 1)
            )
            .shadow(color: .black.opacity(0.045), radius: 5, x: 0, y: 2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    /// Non-selectable action card (hub entry points), same card chrome.
    private func actionCard(_ title: String, systemImage: String, action: @escaping () -> Void) -> some View {
        Button(action: {
            Haptics.light()
            action()
        }) {
            HStack(spacing: 12) {
                Image(systemName: systemImage)
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Color("AccentPrimary"))
                    .frame(width: 28)
                Text(title)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
            }
            .padding(.horizontal, 14)
            .frame(height: 52)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusMD)
                    .stroke(Theme.borderWarmDefault, lineWidth: 1)
            )
            .shadow(color: .black.opacity(0.045), radius: 5, x: 0, y: 2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    /// Both creation and management live on the Agent Hub tab — the sheet
    /// is only a switcher. Hop tabs after the dismissal starts so the hub
    /// is already underneath when the sheet slides away.
    private func openHub() {
        dismiss()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
            modeStore.selectBarItem(.agents)
        }
    }
}
