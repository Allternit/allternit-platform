import SwiftUI

/// Phase-1 Compute Nodes manager for iOS.
///
/// Mirrors the web's NodesView list tab: stat cards, node cards, refresh,
/// delete, and a join-token generator. Terminal and deploy wizard are deferred.
struct NodesManagerView: View {
    @StateObject private var store = NodesStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var nodeToDelete: NodeRecord? = nil
    @State private var deleteError: String? = nil
    @State private var tokenError: String? = nil

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().background(Color("BorderSubtle"))
            content
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .task {
            store.fetchIfNeeded()
        }
        .confirmationDialog(
            "Remove node?",
            isPresented: Binding(
                get: { nodeToDelete != nil },
                set: { if !$0 { nodeToDelete = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Remove Node", role: .destructive) {
                if let node = nodeToDelete {
                    performDelete(node)
                }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            if let node = nodeToDelete {
                Text("Disconnect \(node.hostname) from the control plane. The node agent will need to be reconfigured to reconnect.")
            }
        }
        .alert("Node Join Token", isPresented: Binding<Bool>(
            get: { store.generatedToken != nil },
            set: { if !$0 { store.clearGeneratedToken() } }
        )) {
            Button("Copy & Close") {
                if let command = store.generatedToken?.installCommand {
                    UIPasteboard.general.string = command
                }
                store.clearGeneratedToken()
            }
        } message: {
            if let token = store.generatedToken {
                Text("Run this on the node you want to connect:\n\n\(token.installCommand)")
            }
        }
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Compute Nodes")
                    .font(.system(.title3, design: .serif))
                    .fontWeight(.medium)
                    .foregroundColor(Color("TextPrimary"))
                Text("Manage your compute infrastructure")
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
        if store.isLoading && store.nodes.isEmpty {
            Spacer()
            ProgressView()
            Spacer()
        } else if let loadError = store.loadError, store.nodes.isEmpty {
            Spacer()
            let offline = isConnectionFailure(loadError)
            FriendlyStateView(
                style: offline ? .offline : .error,
                icon: offline ? "wifi.slash" : "exclamationmark.triangle",
                title: "Failed to load nodes",
                message: FriendlyErrorMessage.from(loadError),
                actionTitle: "Retry",
                action: { store.fetchIfNeeded(force: true) }
            )
            Spacer()
        } else {
            ScrollView {
                VStack(spacing: 16) {
                    actionBar
                    statCards
                    if let deleteError {
                        errorBanner(deleteError)
                    }
                    if let tokenError {
                        errorBanner(tokenError)
                    }
                    nodeList
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 20)
            }
            .refreshable {
                await store.refresh()
            }
        }
    }

    // MARK: - Action bar

    private var actionBar: some View {
        HStack(spacing: 12) {
            Button(action: { store.fetchIfNeeded(force: true) }) {
                HStack(spacing: 6) {
                    Image(systemName: "arrow.clockwise")
                        .font(.system(size: 12, weight: .semibold))
                    Text("Refresh")
                        .font(.system(size: 13, weight: .semibold))
                }
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusMD)
                        .stroke(Theme.borderWarmDefault, lineWidth: 1)
                )
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Refresh")

            Button(action: generateToken) {
                HStack(spacing: 6) {
                    if store.isGeneratingToken {
                        ProgressView()
                            .scaleEffect(0.7)
                    } else {
                        Image(systemName: "plus")
                            .font(.system(size: 12, weight: .semibold))
                    }
                    Text("Connect Node")
                        .font(.system(size: 13, weight: .semibold))
                }
                .foregroundColor(Color("AccentPrimary"))
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .background(Color("AccentPrimary").opacity(0.12))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusMD)
                        .stroke(Color("AccentPrimary").opacity(0.25), lineWidth: 1)
                )
            }
            .buttonStyle(.plain)
            .disabled(store.isGeneratingToken)

            Spacer()
        }
    }

    // MARK: - Stat cards

    private var statCards: some View {
        LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible()), GridItem(.flexible())], spacing: 12) {
            statCard(label: "Total Nodes", value: "\(store.nodes.count)", icon: "externaldrive.connected.to.line.below", color: Color("AccentPrimary"))
            statCard(label: "Online", value: "\(store.onlineCount)", icon: "checkmark.circle", color: Theme.statusSuccess)
            statCard(label: "Offline", value: "\(store.offlineCount)", icon: "xmark.circle", color: Color("TextSecondary"))
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

    // MARK: - Node list

    @ViewBuilder
    private var nodeList: some View {
        if store.nodes.isEmpty {
            emptyState
        } else {
            VStack(spacing: 12) {
                ForEach(store.nodes) { node in
                    nodeRow(node)
                }
            }
        }
    }

    private var emptyState: some View {
        FriendlyStateView(
            style: .empty,
            icon: "externaldrive.connected.to.line.below",
            title: "No nodes connected",
            message: "Add compute nodes to run agents and workloads. You can deploy new cloud instances or connect existing machines.",
            actionTitle: "Connect Existing Node",
            action: generateToken,
            art: .network,
            size: .panel
        )
        .padding(.vertical, 24)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
    }

    private func nodeRow(_ node: NodeRecord) -> some View {
        let connected = store.isConnected(node.id)
        return VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                ZStack(alignment: .bottomTrailing) {
                    Image(systemName: "externaldrive")
                        .font(.system(size: 24))
                        .foregroundColor(Color("TextSecondary"))
                    Circle()
                        .fill(connected ? Theme.statusSuccess : Color("TextSecondary"))
                        .frame(width: 10, height: 10)
                        .overlay(Circle().stroke(Color("BgSecondary"), lineWidth: 1.5))
                }

                VStack(alignment: .leading, spacing: 2) {
                    Text(node.hostname)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                    Text(node.id)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }

                Spacer(minLength: 8)

                statusBadge(node.status)
            }

            HStack(spacing: 10) {
                nodeMiniStat("CPU", value: "\(node.cpuCores) cores")
                nodeMiniStat("Memory", value: formatBytes(node.memoryGB))
                nodeMiniStat("Disk", value: formatBytes(node.diskGB))
            }

            HStack(spacing: 8) {
                nodeTag("\(node.os) / \(node.arch)")
                if node.dockerAvailable {
                    nodeTag("Docker")
                }
                if node.gpuAvailable {
                    nodeTag("GPU")
                }
            }

            HStack {
                Text("Last seen: \(formatLastSeen(node.lastSeenAt)) • Version: \(node.version)")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                Spacer()
                Button(action: { nodeToDelete = node }) {
                    Image(systemName: "trash")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(Theme.statusError)
                        .padding(8)
                        .background(Theme.statusError.opacity(0.08))
                        .clipShape(Circle())
                }
                .buttonStyle(.plain)
                .disabled(store.isDeletingNodeId == node.id)
            }
        }
        .padding(14)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.borderWarmDefault, lineWidth: 1)
        )
        .overlay(
            Rectangle()
                .fill(statusColor(node.status))
                .frame(width: 4)
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG)),
            alignment: .leading
        )
    }

    private func nodeMiniStat(_ label: String, value: String) -> some View {
        VStack(spacing: 4) {
            Text(value)
                .font(.system(size: 14, weight: .semibold, design: .rounded))
                .foregroundColor(Color("TextPrimary"))
            Text(label.uppercased())
                .font(.system(size: 8, weight: .semibold))
                .tracking(1)
                .foregroundColor(Color("TextSecondary"))
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 10)
        .background(Color("BgSecondary"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
    }

    private func nodeTag(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 10, weight: .semibold))
            .foregroundColor(Color("TextSecondary"))
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(Color("BgSecondary"))
            .clipShape(Capsule())
            .overlay(
                Capsule()
                    .stroke(Theme.borderWarmDefault, lineWidth: 1)
            )
    }

    private func statusBadge(_ status: NodeStatus) -> some View {
        HStack(spacing: 4) {
            Image(systemName: statusIcon(status))
                .font(.system(size: 10))
            Text(statusLabel(status))
                .font(.system(size: 10, weight: .semibold))
        }
        .foregroundColor(statusColor(status))
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(statusColor(status).opacity(0.12))
        .clipShape(Capsule())
    }

    private func errorBanner(_ message: String) -> some View {
        HStack(spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 14))
                .foregroundColor(Theme.statusError)
            Text(message)
                .font(.caption)
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
            Spacer()
            Button(action: {
                deleteError = nil
                tokenError = nil
            }) {
                Image(systemName: "xmark")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
            }
            .buttonStyle(.plain)
        }
        .padding(12)
        .background(Theme.statusError.opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radiusLG)
                .stroke(Theme.statusError.opacity(0.3), lineWidth: 1)
        )
    }

    // MARK: - Helpers

    private func isConnectionFailure(_ error: String) -> Bool {
        let lowered = error.lowercased()
        return lowered.contains("could not connect")
            || lowered.contains("failed to connect")
            || lowered.contains("offline")
            || lowered.contains("no network")
            || lowered.contains("network connection was lost")
    }

    private func statusColor(_ status: NodeStatus) -> Color {
        switch status {
        case .online: return Theme.statusSuccess
        case .offline: return Color("TextSecondary")
        case .busy: return Theme.statusWarning
        case .maintenance: return Theme.statusInfo
        case .error: return Theme.statusError
        }
    }

    private func statusIcon(_ status: NodeStatus) -> String {
        switch status {
        case .online: return "checkmark.circle"
        case .offline: return "xmark.circle"
        case .busy: return "waveform.path.ecg"
        case .maintenance: return "clock"
        case .error: return "exclamationmark.triangle"
        }
    }

    private func statusLabel(_ status: NodeStatus) -> String {
        switch status {
        case .online: return "Online"
        case .offline: return "Offline"
        case .busy: return "Busy"
        case .maintenance: return "Maintenance"
        case .error: return "Error"
        }
    }

    private func formatBytes(_ gb: Int) -> String {
        if gb >= 1024 {
            return String(format: "%.1f TB", Double(gb) / 1024)
        }
        return "\(gb) GB"
    }

    private func formatLastSeen(_ dateString: String?) -> String {
        guard let dateString,
              let date = ISO8601DateFormatter().date(from: dateString) else {
            return "Never"
        }
        let diff = Date().timeIntervalSince(date)
        if diff < 60 { return "Just now" }
        if diff < 3600 { return "\(Int(diff / 60))m ago" }
        if diff < 86400 { return "\(Int(diff / 3600))h ago" }
        return "\(Int(diff / 86400))d ago"
    }

    // MARK: - Actions

    private func performDelete(_ node: NodeRecord) {
        deleteError = nil
        Task {
            do {
                try await store.deleteNode(node.id)
            } catch {
                deleteError = error.localizedDescription
            }
        }
    }

    private func generateToken() {
        tokenError = nil
        Task {
            do {
                try await store.generateToken()
            } catch {
                tokenError = error.localizedDescription
            }
        }
    }
}
