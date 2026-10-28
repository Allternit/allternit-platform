import SwiftUI
import UIKit

/// The drawer's Phones button (allternit-ai src/components/bot-phone/
/// PhonesRailButton.tsx, "phone-icon" variant): the bot orb with the total
/// unread across the bots' phones.
struct PhonesDrawerButton: View {
    @ObservedObject private var phones = BotPhonesStore.shared
    let action: () -> Void

    var body: some View {
        Button {
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
            action()
        } label: {
            Image(systemName: "iphone")
                .font(.system(size: 17, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .frame(width: 40, height: 40)
                .background(Circle().fill(Color("BgPanel")))
                .overlay(Circle().stroke(Color("BorderSubtle"), lineWidth: 1))
                .overlay(alignment: .topTrailing) {
                    if phones.totalUnread > 0 {
                        Text(phones.totalUnread > 99 ? "99+" : "\(phones.totalUnread)")
                            .font(.system(size: 10, weight: .bold))
                            .foregroundColor(.white)
                            .padding(.horizontal, 5)
                            .frame(minWidth: 17, minHeight: 17)
                            .background(Capsule().fill(Theme.statusError))
                            .offset(x: 3, y: -2)
                    }
                }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(phones.totalUnread > 0 ? "Phones, \(phones.totalUnread) unread" : "Phones")
        .accessibilityIdentifier("drawerPhones")
    }
}

/// "Your bots' phones": one row per bot that has a channel, number or email,
/// with its unread count. Choosing one opens that bot's phone in Bots mode.
/// "Give a bot a phone" opens the connect flow (web Connectors settings,
/// in-app — iOS has no native connect wizard).
struct PhonesSheet: View {
    var onOpenPhone: (String) -> Void

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var phones = BotPhonesStore.shared
    @ObservedObject private var botStore = BotModeStore.shared
    @ObservedObject private var hub = AgentHubStore.shared
    @ObservedObject private var auth = AuthManager.shared
    @State private var safariURL: IdentifiableURL? = nil

    var body: some View {
        NavigationStack {
            List {
                content
                Section {
                    Button {
                        safariURL = IdentifiableURL(url: BotPhoneLinks.givePhone)
                    } label: {
                        Label("Give a bot a phone", systemImage: "plus")
                    }
                    .disabled(!auth.isSignedIn && !auth.isSkippingAuth)
                }
            }
            .navigationTitle("Your bots' phones")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
            .refreshable { await phones.refresh() }
            .task {
                await phones.refresh()
                while !Task.isCancelled {
                    try? await Task.sleep(nanoseconds: 60_000_000_000)
                    if Task.isCancelled { break }
                    await phones.refresh()
                }
            }
            .sheet(item: $safariURL) { SafariView(url: $0.url) }
        }
        .presentationDetents([.medium, .large])
    }

    @ViewBuilder
    private var content: some View {
        if !auth.isSignedIn && !auth.isSkippingAuth {
            FriendlyStateView(style: .empty, icon: "person.crop.circle", title: "Sign in to see your bots' phones",
                              message: "Your bots' numbers, email addresses and channels live in your account.",
                              actionTitle: "Sign in", action: {
                                  dismiss()
                                  auth.isPresentingAuth = true
                              })
                .listRowBackground(Color.clear)
        } else if (phones.isLoading || hub.isLoading) && !phones.hasLoaded {
            HStack(spacing: 10) {
                ProgressView()
                Text("Checking your bots…").foregroundColor(Color("TextSecondary"))
            }
        } else if let error = phones.loadError, phones.withPhone.isEmpty {
            FriendlyStateView(style: .offline, icon: "wifi.slash", title: "Couldn't load your bots' phones",
                              message: error, actionTitle: "Retry", action: { Task { await phones.refresh() } })
                .listRowBackground(Color.clear)
        } else if phones.withPhone.isEmpty {
            Text("No bot has a phone yet. Give one a number, an email address or a messaging channel.")
                .font(.footnote)
                .foregroundColor(Color("TextSecondary"))
        } else {
            Section {
                ForEach(phones.withPhone) { entry in
                    if let bot = botStore.bot(entry.botId) {
                        Button {
                            UIImpactFeedbackGenerator(style: .light).impactOccurred()
                            dismiss()
                            onOpenPhone(entry.botId)
                        } label: {
                            HStack(spacing: 12) {
                                BotAvatarView(bot: bot, size: 30)
                                Text(bot.botDisplayName)
                                    .foregroundColor(Color("TextPrimary"))
                                    .lineLimit(1)
                                Spacer()
                                if entry.unread > 0 {
                                    Text(entry.unread > 99 ? "99+" : "\(entry.unread)")
                                        .font(.caption2.weight(.bold))
                                        .foregroundColor(.white)
                                        .padding(.horizontal, 6)
                                        .frame(minHeight: 18)
                                        .background(Capsule().fill(Theme.statusError))
                                }
                                Image(systemName: "chevron.right")
                                    .font(.caption)
                                    .foregroundColor(Color("TextSecondary"))
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel(entry.unread > 0 ? "\(bot.botDisplayName)'s phone, \(entry.unread) unread" : "\(bot.botDisplayName)'s phone")
                    }
                }
            }
        }
    }
}
