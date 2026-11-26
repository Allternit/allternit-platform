import SwiftUI

/// Lists the organization's webhook subscriptions so Bot Home can surface
/// how many integrations are wired to this bot.
struct BotWebhooksSheet: View {
    @Environment(\.dismiss) private var dismiss
    private let client = WebhookClient.shared
    @State private var subscriptions: [WebhookSubscription] = []
    @State private var isLoading = false
    @State private var loadError: String? = nil

    var body: some View {
        NavigationStack {
            Group {
                if isLoading && subscriptions.isEmpty {
                    ProgressView()
                } else if let loadError, subscriptions.isEmpty {
                    VStack(spacing: 12) {
                        Text("Couldn't load webhooks")
                            .font(.subheadline)
                            .foregroundColor(Color("TextPrimary"))
                        Text(loadError)
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                            .multilineTextAlignment(.center)
                        Button("Retry") {
                            Task { await load() }
                        }
                        .font(.subheadline)
                        .foregroundColor(Color("AccentPrimary"))
                    }
                    .padding(.horizontal, 20)
                } else if subscriptions.isEmpty {
                    FriendlyStateView(
                        title: "No organization subscriptions yet",
                        message: "Webhooks are organization-wide. They let outside services push events into Allternit.",
                        art: .sheets,
                        size: .panel
                    )
                    .padding(.top, 40)
                } else {
                    List {
                        ForEach(subscriptions) { subscription in
                            VStack(alignment: .leading, spacing: 6) {
                                HStack(spacing: 8) {
                                    Text(subscription.url)
                                        .font(.system(.subheadline, design: .monospaced))
                                        .foregroundColor(Color("TextPrimary"))
                                        .lineLimit(1)
                                    Spacer()
                                    Circle()
                                        .fill(subscription.active ? Theme.statusSuccess : Theme.statusWarning)
                                        .frame(width: 8, height: 8)
                                }

                                Text(subscription.events.joined(separator: " · "))
                                    .font(.caption)
                                    .foregroundColor(Color("TextSecondary"))
                                    .lineLimit(1)
                            }
                            .padding(.vertical, 4)
                        }
                    }
                    .listStyle(.insetGrouped)
                    .scrollContentBackground(.hidden)
                }
            }
            .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
            .navigationTitle("Org Webhooks")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Done") { dismiss() }
                }
            }
            .task {
                await load()
            }
        }
    }

    private func load() async {
        isLoading = true
        loadError = nil
        do {
            subscriptions = try await client.listSubscriptions()
        } catch is CancellationError {
            // no-op
        } catch {
            loadError = error.localizedDescription
        }
        isLoading = false
    }
}
