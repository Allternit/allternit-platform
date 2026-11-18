import SwiftUI

/// Phase-1 Runtime Operations hub for iOS.
///
/// Mirrors the web's RuntimeOperationsView: aggregates budget, replay, prewarm,
/// and execution-mode status into summary cards. Detail management screens are
/// deferred to items #62-64.
struct RuntimeOperationsView: View {
    @StateObject private var store = RuntimeOperationsStore.shared
    @Environment(\.dismiss) private var dismiss
    @State private var isBudgetDashboardPresented = false
    @State private var isReplayManagerPresented = false
    @State private var isPrewarmManagerPresented = false

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().background(Color("BorderSubtle"))
            content
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .navigationDestination(isPresented: $isBudgetDashboardPresented) {
            BudgetDashboardView()
        }
        .navigationDestination(isPresented: $isReplayManagerPresented) {
            ReplayManagerView()
        }
        .navigationDestination(isPresented: $isPrewarmManagerPresented) {
            PrewarmManagerView()
        }
        .task {
            store.fetchIfNeeded()
        }
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Runtime Operations")
                    .font(.system(.title3, design: .serif))
                    .fontWeight(.medium)
                    .foregroundColor(Color("TextPrimary"))
                Text("Budget, replay, prewarm, and execution mode")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }

            Spacer()

            Button(action: {
                Haptics.light()
                store.fetchIfNeeded(force: true)
            }) {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 32, height: 32)
                    .background(Color("BgPanel"))
                    .clipShape(Circle())
            }
            .accessibilityLabel("Refresh runtime operations")

            Button(action: { dismiss() }) {
                Image(systemName: "xmark")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 32, height: 32)
                    .background(Color("BgPanel"))
                    .clipShape(Circle())
            }
            .accessibilityLabel("Close")
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        if store.isLoading && store.budget == nil {
            Spacer()
            ProgressView()
            Spacer()
        } else if let loadError = store.loadError, store.budget == nil {
            Spacer()
            FriendlyStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "Couldn't load runtime ops",
                message: FriendlyErrorMessage.from(loadError),
                actionTitle: "Retry",
                action: { store.fetchIfNeeded(force: true) }
            )
            Spacer()
        } else {
            ScrollView {
                VStack(spacing: 16) {
                    alertsRow
                    executionModeCard
                    budgetCard
                    replayCard
                    prewarmCard
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 20)
            }
            .refreshable {
                await store.refresh()
            }
        }
    }

    // MARK: - Alerts

    private var alertsRow: some View {
        VStack(spacing: 8) {
            ForEach(store.budgetAlerts.prefix(2)) { alert in
                HStack(spacing: 10) {
                    Image(systemName: alert.level == .critical ? "exclamationmark.triangle.fill" : alert.level == .warning ? "exclamationmark.circle" : "info.circle")
                        .foregroundColor(alertColor(for: alert.level))
                    VStack(alignment: .leading, spacing: 2) {
                        Text(alert.title)
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                        Text(alert.message)
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(2)
                    }
                    Spacer()
                }
                .padding(12)
                .background(alertColor(for: alert.level).opacity(0.08))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusMD)
                        .stroke(alertColor(for: alert.level).opacity(0.3), lineWidth: 1)
                )
            }
        }
    }

    private func alertColor(for level: RuntimeBudgetAlert.Level) -> Color {
        switch level {
        case .critical: return Theme.statusError
        case .warning: return Theme.statusWarning
        case .info: return Theme.statusInfo
        }
    }

    // MARK: - Execution mode card

    private var executionModeCard: some View {
        let meta = executionModeMeta(store.executionMode?.mode)
        return card {
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Image(systemName: "switch.2")
                        .font(.system(size: 18, weight: .medium))
                        .foregroundColor(Color("AccentPrimary"))
                    Text("Execution Mode")
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    Text(meta.badgeLabel)
                        .font(.system(size: 10, weight: .bold))
                        .tracking(0.8)
                        .foregroundColor(meta.badgeColor)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(meta.badgeColor.opacity(0.12))
                        .clipShape(Capsule())
                }

                Text(meta.title)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                Text(meta.detail)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(2)
            }
        }
    }

    private func executionModeMeta(_ mode: RuntimeExecutionMode?) -> (title: String, detail: String, badgeLabel: String, badgeColor: Color) {
        switch mode {
        case .plan:
            return ("Plan rail", "Generate and inspect a plan before runtime changes are allowed.", "Plan", Theme.statusInfo)
        case .safe:
            return ("Safe rail", "Require runtime rails and policy checks before direct execution.", "Safe", Theme.statusWarning)
        case .auto:
            return ("Auto rail", "Execute directly when the runtime and rails permit the request.", "Auto", Theme.statusSuccess)
        case .none:
            return ("Syncing runtime rail", "Waiting for the runtime to report the shared execution default.", "Syncing", Color("TextSecondary"))
        }
    }

    // MARK: - Budget card

    private var budgetCard: some View {
        Button(action: { isBudgetDashboardPresented = true }) {
            budgetCardContent
        }
        .buttonStyle(.plain)
    }

    private var budgetCardContent: some View {
        card {
            VStack(alignment: .leading, spacing: 12) {
                HStack {
                    Image(systemName: "wallet")
                        .font(.system(size: 18, weight: .medium))
                        .foregroundColor(Color("AccentPrimary"))
                    Text("Budget")
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Spacer()
                    if let budget = store.budget {
                        Text("\(Int(budget.creditsRemaining)) credits")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }

                if let budget = store.budget {
                    HStack(spacing: 12) {
                        budgetMiniStat("Hourly", value: "\(Int(budget.creditsConsumedThisHour))")
                        budgetMiniStat("Projected", value: "\(Int(budget.projectedHourlyCost))")
                        budgetMiniStat("Status", value: budget.status.capitalized)
                    }

                    LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 8) {
                        ForEach(store.budgetMetrics) { metric in
                            HStack {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(metric.label)
                                        .font(.caption)
                                        .foregroundColor(Color("TextSecondary"))
                                    Text(metric.detail)
                                        .font(.system(size: 12, weight: .semibold))
                                        .foregroundColor(Color("TextPrimary"))
                                }
                                Spacer()
                                Circle()
                                    .fill(toneColor(metric.tone))
                                    .frame(width: 8, height: 8)
                            }
                            .padding(10)
                            .background(Color("BgSecondary"))
                            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM))
                        }
                    }
                } else {
                    FriendlyInlineStateView(
                        style: .empty,
                        icon: "wallet",
                        title: "Budget data unavailable",
                        message: "Connect a billing source to see runtime budget."
                    )
                }
            }
        }
    }

    private func budgetMiniStat(_ label: String, value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
            Text(value)
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func toneColor(_ tone: RuntimeBudgetMetric.Tone) -> Color {
        switch tone {
        case .healthy: return Theme.statusSuccess
        case .warning: return Theme.statusWarning
        case .critical: return Theme.statusError
        }
    }

    // MARK: - Replay card

    private var replayCard: some View {
        Button(action: { isReplayManagerPresented = true }) {
            card {
                VStack(alignment: .leading, spacing: 12) {
                    HStack {
                        Image(systemName: "clock.arrow.circlepath")
                            .font(.system(size: 18, weight: .medium))
                            .foregroundColor(Color("AccentPrimary"))
                        Text("Replay")
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                        Spacer()
                        Text("\(store.replayManifests.count) manifests")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }

                    if let first = store.replayManifests.first {
                        HStack {
                            Text("Latest")
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
                            Spacer()
                            Text(first.runId.prefix(18))
                                .font(.system(size: 12, design: .monospaced))
                                .foregroundColor(Color("TextPrimary"))
                                .lineLimit(1)
                        }
                        HStack {
                            Text("Outputs")
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
                            Spacer()
                            Text("\(first.outputCount)")
                                .font(.system(size: 12, weight: .semibold))
                                .foregroundColor(Color("TextPrimary"))
                        }
                    } else {
                        Text("No replay manifests available")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
            }
        }
        .buttonStyle(.plain)
    }

    // MARK: - Prewarm card

    private var prewarmCard: some View {
        Button(action: { isPrewarmManagerPresented = true }) {
            card {
                VStack(alignment: .leading, spacing: 12) {
                    HStack {
                        Image(systemName: "flame")
                            .font(.system(size: 18, weight: .medium))
                            .foregroundColor(Color("AccentPrimary"))
                        Text("Prewarm")
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                        Spacer()
                        if let status = store.prewarmStatus {
                            Text(status.enabled ? "Enabled" : "Disabled")
                                .font(.caption)
                                .foregroundColor(status.enabled ? Theme.statusSuccess : Color("TextSecondary"))
                        }
                    }

                    HStack(spacing: 12) {
                        prewarmMiniStat("Pools", value: "\(store.poolStats.totalPools)")
                        prewarmMiniStat("Instances", value: "\(store.poolStats.totalInstances)")
                        prewarmMiniStat("Available", value: "\(store.poolStats.totalAvailable)")
                        prewarmMiniStat("In Use", value: "\(store.poolStats.totalInUse)")
                    }

                    if let status = store.prewarmStatus, !status.pools.isEmpty {
                        Text("Pools: \(status.pools.map(\.name).joined(separator: ", "))")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(1)
                    }
                }
            }
        }
        .buttonStyle(.plain)
    }

    private func prewarmMiniStat(_ label: String, value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
            Text(value)
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    // MARK: - Card wrapper

    private func card<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        content()
            .padding(14)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radiusLG)
                    .stroke(Theme.borderWarmDefault, lineWidth: 1)
            )
    }
}
