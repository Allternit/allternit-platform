import SwiftUI

/// Phase-1 Prewarm Manager for iOS.
///
/// Mirrors the web's PrewarmManagerView: pool-size slider, warmup action,
/// health chips, pool topology, and recent GUI-driven activity.
struct PrewarmManagerView: View {
    @StateObject private var store = RuntimeOperationsStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var draftPoolSize: Double = 2
    @State private var activities: [PrewarmActivity] = []
    @State private var lastActionError: String? = nil

    private var status: PrewarmStatus? { store.prewarmStatus }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().background(Color("BorderSubtle"))
            content
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .task {
            store.fetchIfNeeded()
            if let poolSize = status?.poolSize {
                draftPoolSize = Double(poolSize)
            }
        }
        .onChange(of: status?.poolSize) { _, newValue in
            if let newValue, !store.isSettingPrewarmPoolSize {
                draftPoolSize = Double(newValue)
            }
        }
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Prewarm Pool Manager")
                    .font(.system(.title3, design: .serif))
                    .fontWeight(.medium)
                    .foregroundColor(Color("TextPrimary"))
                Text("Tune pool size and trigger warmup")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }

            Spacer()

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
        if store.isLoading && status == nil {
            Spacer()
            ProgressView()
            Spacer()
        } else if let loadError = store.loadError, status == nil {
            Spacer()
            let offline = isConnectionFailure(loadError)
            FriendlyStateView(
                style: offline ? .offline : .error,
                icon: offline ? "wifi.slash" : "exclamationmark.triangle",
                title: "Failed to load prewarm status",
                message: FriendlyErrorMessage.from(loadError),
                actionTitle: "Retry",
                action: { store.fetchIfNeeded(force: true) }
            )
            Spacer()
        } else {
            ScrollView {
                VStack(spacing: 16) {
                    statusHeader
                    statCards
                    controlsCard
                    topologyCard
                    activityCard
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 20)
            }
            .refreshable {
                await store.refresh()
            }
        }
    }

    // MARK: - Status header

    private var statusHeader: some View {
        HStack {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 6) {
                    Image(systemName: "flame")
                        .font(.system(size: 12))
                        .foregroundColor(Theme.statusWarning)
                    Text("Launch Latency Control")
                        .font(.system(size: 10, weight: .semibold))
                        .tracking(2.4)
                        .foregroundColor(Color("TextSecondary"))
                }
                Text("Prewarm Pool Manager")
                    .font(.system(size: 22, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text("Tune the runtime pool size and trigger warmup for the live prewarm service.")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(2)
            }

            Spacer()

            if let status {
                Text(status.enabled ? "Enabled" : "Disabled")
                    .font(.system(size: 10, weight: .bold))
                    .tracking(0.8)
                    .foregroundColor(status.enabled ? Theme.statusSuccess : Color("TextSecondary"))
                    .padding(.horizontal, 10)
                    .padding(.vertical, 4)
                    .background((status.enabled ? Theme.statusSuccess : Color("TextSecondary")).opacity(0.12))
                    .clipShape(Capsule())
            }
        }
        .padding(16)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    // MARK: - Stat cards

    private var statCards: some View {
        LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible()), GridItem(.flexible()), GridItem(.flexible())], spacing: 12) {
            statCard(
                label: "Pool Size",
                value: "\(status?.poolSize ?? 0)",
                icon: "gauge.with.dots.needle.67percent",
                color: Theme.statusWarning
            )
            statCard(
                label: "Available",
                value: "\(store.poolStats.totalAvailable)",
                icon: "checkmark.circle",
                color: Theme.statusSuccess
            )
            statCard(
                label: "In Use",
                value: "\(store.poolStats.totalInUse)",
                icon: "waveform.path.ecg",
                color: Color("AccentPrimary")
            )
            statCard(
                label: "Managed Pools",
                value: "\(store.poolStats.totalPools)",
                icon: "clock",
                color: Theme.statusInfo
            )
        }
    }

    private func statCard(label: String, value: String, icon: String, color: Color) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Image(systemName: icon)
                    .font(.system(size: 16, weight: .medium))
                    .foregroundColor(color)
                Spacer()
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(label.uppercased())
                    .font(.system(size: 9, weight: .semibold))
                    .tracking(1)
                    .foregroundColor(Color("TextSecondary"))
                Text(value)
                    .font(.system(size: 22, weight: .heavy, design: .rounded))
                    .foregroundColor(Color("TextPrimary"))
            }
        }
        .padding(14)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    // MARK: - Controls card

    private var controlsCard: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 8) {
                Image(systemName: "flame")
                    .font(.system(size: 16))
                    .foregroundColor(Theme.statusWarning)
                Text("Runtime Pool Controls")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
            }

            Text("The backend currently manages a shared pool size and warmup action. Adjust those values here.")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))

            VStack(alignment: .leading, spacing: 12) {
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Default pool size")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                        Text("Increase this when cold starts hurt throughput. Lower it to reclaim capacity.")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                    Spacer()
                    Text("\(Int(draftPoolSize))")
                        .font(.system(size: 20, weight: .semibold, design: .rounded))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                        .background(Color("BgSecondary"))
                        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                }

                Slider(value: $draftPoolSize, in: 1...12, step: 1)
                    .tint(Color("AccentPrimary"))

                HStack {
                    Text("Lean")
                        .font(.system(size: 9, weight: .semibold))
                        .tracking(1)
                        .foregroundColor(Color("TextSecondary"))
                    Spacer()
                    Text("Balanced")
                        .font(.system(size: 9, weight: .semibold))
                        .tracking(1)
                        .foregroundColor(Color("TextSecondary"))
                    Spacer()
                    Text("Aggressive")
                        .font(.system(size: 9, weight: .semibold))
                        .tracking(1)
                        .foregroundColor(Color("TextSecondary"))
                }

                HStack(spacing: 12) {
                    Button(action: applyPoolSize) {
                        HStack(spacing: 6) {
                            if store.isSettingPrewarmPoolSize {
                                ProgressView()
                                    .scaleEffect(0.7)
                            } else {
                                Image(systemName: "checkmark.circle")
                                    .font(.system(size: 12, weight: .semibold))
                            }
                            Text("Apply Pool Size")
                                .font(.system(size: 13, weight: .semibold))
                        }
                        .foregroundColor(Theme.statusSuccess)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 10)
                        .background(Theme.statusSuccess.opacity(0.12))
                        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                        .overlay(
                            RoundedRectangle(cornerRadius: Theme.radiusMD)
                                .stroke(Theme.statusSuccess.opacity(0.25), lineWidth: 1)
                        )
                    }
                    .buttonStyle(.plain)
                    .disabled(store.isSettingPrewarmPoolSize || Int(draftPoolSize) == (status?.poolSize ?? 0))

                    Button(action: triggerWarmup) {
                        HStack(spacing: 6) {
                            if store.isWarmingPrewarmPool {
                                ProgressView()
                                    .scaleEffect(0.7)
                            } else {
                                Image(systemName: "thermometer")
                                    .font(.system(size: 12, weight: .semibold))
                            }
                            Text("Trigger Warmup")
                                .font(.system(size: 13, weight: .semibold))
                        }
                        .foregroundColor(Theme.statusWarning)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 10)
                        .background(Theme.statusWarning.opacity(0.12))
                        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                        .overlay(
                            RoundedRectangle(cornerRadius: Theme.radiusMD)
                                .stroke(Theme.statusWarning.opacity(0.25), lineWidth: 1)
                        )
                    }
                    .buttonStyle(.plain)
                    .disabled(store.isWarmingPrewarmPool)
                }
                .frame(maxWidth: .infinity, alignment: .trailing)
            }
            .padding(14)
            .background(Color("BgSecondary"))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))

            HStack(spacing: 12) {
                healthChip(label: "Healthy", count: healthyPools.count, color: Theme.statusSuccess)
                healthChip(label: "Degraded", count: degradedPools.count, color: Theme.statusWarning)
                healthChip(label: "Empty", count: emptyPools.count, color: Theme.statusInfo)
            }
        }
        .padding(14)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    private func healthChip(label: String, count: Int, color: Color) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(label.uppercased())
                .font(.system(size: 9, weight: .semibold))
                .tracking(1)
                .foregroundColor(color)
            Text("\(count)")
                .font(.system(size: 20, weight: .semibold, design: .rounded))
                .foregroundColor(Color("TextPrimary"))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(color.opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusMD)
                .stroke(color.opacity(0.25), lineWidth: 1)
        )
    }

    // MARK: - Pool topology

    private var topologyCard: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 8) {
                Image(systemName: "network")
                    .font(.system(size: 16))
                    .foregroundColor(Color("AccentPrimary"))
                Text("Pool Topology")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
            }

            Text("Live status from `/api/v1/runtime/prewarm/status`.")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))

            if let status = status, status.pools.isEmpty {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "thermometer",
                    title: "No prewarm pools reported",
                    message: "Increase the pool size to initialize the default pool."
                )
                .frame(maxWidth: .infinity)
                .padding(.vertical, 16)
                .background(Color("BgSecondary"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
            } else if let status {
                VStack(spacing: 12) {
                    ForEach(status.pools, id: \.name) { pool in
                        poolRow(pool)
                    }
                }
            }
        }
        .padding(14)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    private func poolRow(_ pool: BackendPoolStatus) -> some View {
        let health = poolHealth(pool)
        let utilization = pool.poolSize == 0 ? 0 : Double(pool.inUse) / Double(pool.poolSize)

        return VStack(alignment: .leading, spacing: 10) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(pool.name)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text("runtime/default")
                        .font(.system(size: 9, weight: .semibold))
                        .tracking(1)
                        .foregroundColor(Color("TextSecondary"))
                }
                Spacer()
                HStack(spacing: 4) {
                    Image(systemName: healthIcon(for: health))
                        .font(.system(size: 11))
                        .foregroundColor(healthColor(for: health))
                    Text(healthLabel(for: health))
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundColor(healthColor(for: health))
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(healthColor(for: health).opacity(0.12))
                .clipShape(Capsule())
            }

            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    Text("Utilization")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                    Spacer()
                    Text("\(pool.inUse) / \(pool.poolSize)")
                        .font(.caption)
                        .foregroundColor(Color("TextPrimary"))
                }
                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        RoundedRectangle(cornerRadius: 4)
                            .fill(Color("BgSecondary"))
                        RoundedRectangle(cornerRadius: 4)
                            .fill(healthColor(for: health))
                            .frame(width: max(0, geo.size.width * CGFloat(utilization)))
                    }
                }
                .frame(height: 6)
            }

            HStack(spacing: 10) {
                poolMiniStat("Available", value: "\(pool.available)", color: Theme.statusSuccess)
                poolMiniStat("In Use", value: "\(pool.inUse)", color: Theme.statusInfo)
                poolMiniStat("Capacity", value: "\(pool.poolSize)", color: Color("TextSecondary"))
            }
        }
        .padding(12)
        .background(Color("BgSecondary"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
    }

    private func poolMiniStat(_ label: String, value: String, color: Color) -> some View {
        VStack(spacing: 4) {
            Text(value)
                .font(.system(size: 16, weight: .semibold, design: .rounded))
                .foregroundColor(Color("TextPrimary"))
            Text(label.uppercased())
                .font(.system(size: 8, weight: .semibold))
                .tracking(1)
                .foregroundColor(color)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 10)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
    }

    // MARK: - Activity card

    private var activityCard: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 8) {
                Image(systemName: "clock.arrow.2.circlepath")
                    .font(.system(size: 16))
                    .foregroundColor(Color("AccentPrimary"))
                Text("Recent Activity")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
            }

            if activities.isEmpty {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "clock.arrow.2.circlepath",
                    title: "No recent activity",
                    message: "No recent GUI-driven prewarm activity.",
                    art: .log
                )
                .padding(.vertical, 8)
            } else {
                VStack(spacing: 8) {
                    ForEach(activities.prefix(10)) { activity in
                        HStack(spacing: 10) {
                            Image(systemName: "clock")
                                .font(.system(size: 12))
                                .foregroundColor(Color("TextSecondary"))
                            VStack(alignment: .leading, spacing: 2) {
                                HStack(spacing: 6) {
                                    Text(activity.poolName)
                                        .font(.system(size: 12, weight: .semibold))
                                        .foregroundColor(Color("TextPrimary"))
                                    Text(activity.activityType)
                                        .font(.system(size: 9, weight: .semibold))
                                        .tracking(0.5)
                                        .foregroundColor(Color("TextSecondary"))
                                        .padding(.horizontal, 6)
                                        .padding(.vertical, 2)
                                        .background(Color("BgSecondary"))
                                        .clipShape(Capsule())
                                }
                                Text(activity.details)
                                    .font(.caption)
                                    .foregroundColor(Color("TextSecondary"))
                            }
                            Spacer()
                            Text(activity.timestamp, style: .time)
                                .font(.system(size: 10))
                                .foregroundColor(Color("TextSecondary"))
                        }
                        .padding(10)
                        .background(Color("BgSecondary"))
                        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                    }
                }
            }
        }
        .padding(14)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    // MARK: - Health helpers

    private func isConnectionFailure(_ error: String) -> Bool {
        let lowered = error.lowercased()
        return lowered.contains("could not connect")
            || lowered.contains("failed to connect")
            || lowered.contains("offline")
            || lowered.contains("no network")
            || lowered.contains("network connection was lost")
    }

    private var healthyPools: [BackendPoolStatus] {
        (status?.pools ?? []).filter { poolHealth($0) == .healthy }
    }

    private var degradedPools: [BackendPoolStatus] {
        (status?.pools ?? []).filter { poolHealth($0) == .degraded }
    }

    private var emptyPools: [BackendPoolStatus] {
        (status?.pools ?? []).filter { poolHealth($0) == .empty }
    }

    private func poolHealth(_ pool: BackendPoolStatus) -> PoolHealth {
        if pool.available <= 0 {
            return .empty
        }
        let ratio = Double(pool.available) / Double(max(pool.poolSize, 1))
        if ratio < 0.35 {
            return .degraded
        }
        return .healthy
    }

    private func healthColor(for health: PoolHealth) -> Color {
        switch health {
        case .healthy: return Theme.statusSuccess
        case .degraded: return Theme.statusWarning
        case .empty: return Theme.statusInfo
        }
    }

    private func healthIcon(for health: PoolHealth) -> String {
        switch health {
        case .healthy: return "checkmark.circle"
        case .degraded: return "exclamationmark.triangle"
        case .empty: return "thermometer"
        }
    }

    private func healthLabel(for health: PoolHealth) -> String {
        switch health {
        case .healthy: return "Healthy"
        case .degraded: return "Degraded"
        case .empty: return "Empty"
        }
    }

    // MARK: - Actions

    private func applyPoolSize() {
        lastActionError = nil
        let selectedPoolName = status?.pools.first?.name ?? "default"
        Task {
            do {
                try await store.setPrewarmPoolSize(Int(draftPoolSize))
                activities.insert(
                    PrewarmActivity(
                        timestamp: Date(),
                        poolName: selectedPoolName,
                        activityType: "InstanceCreated",
                        details: "Set pool size to \(Int(draftPoolSize))"
                    ),
                    at: 0
                )
            } catch {
                lastActionError = error.localizedDescription
            }
        }
    }

    private func triggerWarmup() {
        lastActionError = nil
        let selectedPoolName = status?.pools.first?.name ?? "default"
        Task {
            do {
                try await store.warmupPrewarmPool()
                activities.insert(
                    PrewarmActivity(
                        timestamp: Date(),
                        poolName: selectedPoolName,
                        activityType: "WarmupStarted",
                        details: "Warmup triggered from the GUI"
                    ),
                    at: 0
                )
            } catch {
                lastActionError = error.localizedDescription
            }
        }
    }
}

// MARK: - Local activity model

private struct PrewarmActivity: Identifiable {
    let id = UUID()
    let timestamp: Date
    let poolName: String
    let activityType: String
    let details: String
}
