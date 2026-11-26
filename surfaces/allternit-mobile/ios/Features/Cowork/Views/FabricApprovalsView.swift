import SwiftUI

/// Fabric Transport approvals inbox + run status (consumer-packaged Cowork
/// P4.1): pending approvals with Grant/Deny (decided-by recorded
/// server-side), recent run states, and the attributed event timeline per
/// selected run. Reaches the gateway's `/api/v1/fabric/transport/*` surface
/// through the shared APIClient — legacy Cowork task views are untouched.
struct FabricApprovalsView: View {
    @StateObject private var store = FabricApprovalsStore()
    @State private var selectedRun: String? = nil
    /// Team map asks the viewer decides, with Rule · Approver · Asked by · Level (flag `team.map`).
    @StateObject private var team = TeamMapStore()
    /// The Team map sheet; `focus` is the node it opens on (the approver), nil for the whole map.
    @State private var teamMapTarget: TeamMapTarget?

    var body: some View {
        List {
            if TeamMapFlag.isOn, let map = team.map, !map.asks.isEmpty {
                Section("Team approvals") {
                    ForEach(map.asks) { ask in
                        TeamAskRow(ask: ask, map: map,
                                   onDecide: { await team.decide(ask, approve: $0) },
                                   onWhoApproves: { teamMapTarget = TeamMapTarget(focus: ask.approverUserId.map { "p:\($0)" }) })
                    }
                    if let note = TeamPartial.note(map.partial) {
                        Text(note).font(.footnote).foregroundStyle(.secondary)
                    }
                }
            }
            Section("Pending approvals") {
                if store.pending.isEmpty {
                    if let error = store.loadError {
                        Text(error).font(.footnote).foregroundStyle(.secondary)
                    } else {
                        FriendlyInlineStateView(title: "No approvals waiting.", art: .bell)
                    }
                } else {
                    ForEach(store.pending) { approval in
                        ApprovalRow(approval: approval,
                                    onWhoApproves: TeamMapFlag.isOn ? { teamMapTarget = TeamMapTarget(focus: nil) } : nil) { grant in
                            Task { await store.decide(approval, grant: grant) }
                        }
                    }
                }
            }
            Section("Recent runs") {
                if store.runs.isEmpty {
                    FriendlyInlineStateView(title: "No runs yet.", art: .bell)
                } else {
                    ForEach(store.runs) { run in
                        Button {
                            selectedRun = selectedRun == run.id ? nil : run.id
                            if selectedRun != nil { Task { await store.fetchEvents(for: run.id) } }
                        } label: {
                            HStack {
                                Circle()
                                    .fill(run.state == "completed" ? Color.green : (run.state == "failed" ? Color.red : Color.yellow))
                                    .frame(width: 8, height: 8)
                                Text(run.id.prefix(8).description)
                                Spacer()
                                Text(run.state)
                                    .font(.footnote)
                                    .foregroundStyle(.secondary)
                            }
                        }
                        if selectedRun == run.id {
                            ForEach(store.events[run.id] ?? []) { event in
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(event.eventType).font(.caption).bold()
                                    if let executor = event.executor {
                                        Text("executor \(executor)")
                                            .font(.caption2)
                                            .foregroundStyle(.secondary)
                                    }
                                }
                                .padding(.leading, 12)
                            }
                        }
                    }
                }
            }
        }
        .navigationTitle("Fabric Transport")
        .refreshable {
            await store.refresh()
            if TeamMapFlag.isOn { await team.load() }
        }
        .task {
            await store.refresh()
            if TeamMapFlag.isOn { await team.load() }
        }
        .sheet(item: $teamMapTarget, onDismiss: {
            Task { await team.load() }
        }) { target in
            TeamMapSheet(focus: target.focus)
                .presentationDetents([.large])
                .presentationDragIndicator(.visible)
        }
    }
}

/// Which node the Team map sheet opens on.
struct TeamMapTarget: Identifiable, Equatable {
    let id = UUID()
    let focus: String?
}

private struct ApprovalRow: View {
    let approval: FabricTransportClient.Approval
    /// "Who approves this": opens the Team map (flag `team.map`).
    var onWhoApproves: (() -> Void)? = nil
    let onDecide: (Bool) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(approval.capability).font(.caption).bold()
                Spacer()
                Text(approval.status)
                    .font(.caption2)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(approval.status == "pending" ? Color.yellow.opacity(0.25) : Color.green.opacity(0.2))
                    .clipShape(Capsule())
            }
            Text(approval.target)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            if let decidedBy = approval.decidedBy, !decidedBy.isEmpty {
                Text("decided by \(decidedBy)")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            if let onWhoApproves {
                Button("Who approves this", action: onWhoApproves)
                    .font(.caption.weight(.semibold))
                    .buttonStyle(.borderless)
                    .frame(minHeight: 44)
                    .accessibilityHint("Opens the team map")
            }
            if approval.status == "pending" {
                HStack(spacing: 12) {
                    Button("Grant") { onDecide(true) }
                        .buttonStyle(.borderedProminent)
                        .tint(.green)
                    Button("Deny") { onDecide(false) }
                        .buttonStyle(.bordered)
                        .tint(.red)
                }
                .controlSize(.small)
            }
        }
        .padding(.vertical, 4)
    }
}

@MainActor
final class FabricApprovalsStore: ObservableObject {
    @Published private(set) var pending: [FabricTransportClient.Approval] = []
    @Published private(set) var runs: [FabricTransportClient.Run] = []
    @Published private(set) var events: [String: [FabricTransportClient.RunEvent]] = [:]
    @Published private(set) var loadError: String? = nil

    private let client = FabricTransportClient()

    func refresh() async {
        loadError = nil
        do {
            let approvals = try await client.listApprovals()
            self.pending = approvals.filter { $0.status == "pending" }
            self.runs = Array(try await client.listRuns().prefix(20))
        } catch {
            loadError = error.localizedDescription
        }
    }

    func decide(_ approval: FabricTransportClient.Approval, grant: Bool) async {
        do {
            _ = try await client.decide(approvalId: approval.id, grant: grant)
            await refresh()
        } catch {
            loadError = error.localizedDescription
        }
    }

    func fetchEvents(for runId: String) async {
        do {
            events[runId] = try await client.listRunEvents(runId: runId)
        } catch {
            events[runId] = []
        }
    }
}
