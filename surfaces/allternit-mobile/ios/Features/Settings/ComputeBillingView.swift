import SwiftUI

/// Settings > Plans & Compute / Compute Billing parity.
///
/// Displays the current plan, plan usage, credits, and upgrade/buy-credits
/// actions (placeholders until the real purchase flow is wired).
struct ComputeBillingView: View {
    @ObservedObject private var usageStore = UsageStore.shared
    @State private var safariURL: IdentifiableURL? = nil

    var body: some View {
        List {
            planSection
            usageSection
            creditsSection
            actionsSection
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Plans & Compute")
        .navigationBarTitleDisplayMode(.inline)
        .sheet(item: $safariURL) { item in
            SafariView(url: item.url)
        }
        .task {
            UsageStore.shared.fetchUsageIfNeeded(force: true)
        }
        .refreshable {
            UsageStore.shared.fetchUsageIfNeeded(force: true)
        }
    }

    // MARK: - Plan

    private var planSection: some View {
        Section {
            HStack {
                Text("Current plan")
                    .font(.subheadline)
                Spacer()
                Text(usageStore.snapshot?.plan ?? usageStore.planLabel ?? "Free")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 10)
                    .padding(.vertical, 4)
                    .background(Color("BgSecondary"))
                    .clipShape(Capsule())
            }
        } header: {
            Text("Plan")
        }
    }

    // MARK: - Usage

    @ViewBuilder
    private var usageSection: some View {
        Section {
            if usageStore.isLoading {
                ProgressView()
                    .frame(maxWidth: .infinity, alignment: .center)
            } else if let snapshot = usageStore.snapshot, usageStore.percentUsed != nil {
                VStack(alignment: .leading, spacing: 8) {
                    HStack {
                        Text(usageStore.periodLabel)
                            .font(.subheadline)
                        Spacer()
                        if let percentText = usageStore.percentText {
                            Text("\(percentText) used")
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
                        }
                    }

                    let percent = (usageStore.percentUsed ?? 0) / 100
                    ProgressView(value: min(max(percent, 0), 1))
                        .tint(percent >= 1
                              ? Theme.statusError
                              : (percent >= 0.8 ? Theme.statusWarning : Color("AccentPrimary")))

                    if let resetsLabel = usageStore.resetsLabel {
                        Text("Resets \(resetsLabel)")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
                .padding(.vertical, 4)
            } else if case .unavailable(let message) = usageStore.availability {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "gauge.with.dots.needle.67percent",
                    title: "Usage unavailable",
                    message: message
                )
            } else {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "gauge.with.dots.needle.67percent",
                    title: "Usage metering unavailable",
                    message: "Usage metering isn't available on this backend."
                )
            }
        } header: {
            Text("Usage")
        }
    }

    // MARK: - Credits

    @ViewBuilder
    private var creditsSection: some View {
        Section {
            if let credits = usageStore.snapshot?.credits {
                HStack {
                    Text("Credits")
                        .font(.subheadline)
                    Spacer()
                    Text(String(format: "%g", credits))
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                }
            } else {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "creditcard",
                    title: "No credits balance",
                    message: "No credits balance returned by this backend."
                )
            }
        } header: {
            Text("Credits")
        }
    }

    // MARK: - Actions

    private var actionsSection: some View {
        Section {
            Button(action: {
                safariURL = IdentifiableURL(url: URL(string: "https://allternit.com/upgrade")!)
            }) {
                HStack {
                    Text("Upgrade plan")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Image(systemName: "arrow.up.right.square")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            Button(action: {
                safariURL = IdentifiableURL(url: URL(string: "https://allternit.com/credits")!)
            }) {
                HStack {
                    Text("Buy credits")
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Image(systemName: "creditcard")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        } header: {
            Text("Actions")
        } footer: {
            Text("Billing is handled on allternit.com for now. In-app purchases will be available in a future update.")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
        }
    }
}
