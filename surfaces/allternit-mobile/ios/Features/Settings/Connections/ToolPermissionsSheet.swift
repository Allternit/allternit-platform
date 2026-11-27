import SwiftUI

/// The account's tool policies, shared by every connection row's summary line
/// and the Tool permissions sheet. Ask is the default, so only Allow and Block
/// are held. A failed save puts the old value back and keeps the change for Retry.
@MainActor
final class ToolPoliciesStore: ObservableObject {
    static let shared = ToolPoliciesStore()

    enum LoadState: Equatable { case idle, loading, ready, unsupported, failed }

    @Published private(set) var policies: [String: ToolPolicy] = [:]
    @Published private(set) var state: LoadState = .idle
    /// The last save that failed: what to say, and what Retry repeats.
    @Published private(set) var saveError: String?
    private var failedChange: (owner: String, tools: [String], policy: ToolPolicy)?
    private let client = ToolPoliciesClient()

    private static func key(_ owner: String, _ tool: String) -> String { "\(owner)|\(tool)" }

    func policy(_ owner: String, _ tool: String) -> ToolPolicy { policies[Self.key(owner, tool)] ?? .ask }

    func load(force: Bool = false) async {
        guard state != .loading, force || state == .idle || state == .failed else { return }
        state = .loading
        do {
            apply(try await client.list())
            state = .ready
        } catch {
            state = ToolPoliciesClient.isNotFound(error) ? .unsupported : .failed
        }
    }

    private func apply(_ rows: [ToolPolicyRow]) {
        policies = Dictionary(rows.map { (Self.key($0.ownerId, $0.toolId), $0.policy) }, uniquingKeysWith: { _, new in new })
    }

    /// "2 allowed, 1 blocked" for one connection, nil when every tool is Ask.
    func summary(owner: String) -> String? {
        let prefix = "\(owner)|"
        let mine = policies.filter { $0.key.hasPrefix(prefix) }.values
        let allowed = mine.filter { $0 == .allow }.count
        let blocked = mine.filter { $0 == .block }.count
        let parts = [allowed > 0 ? "\(allowed) allowed" : nil, blocked > 0 ? "\(blocked) blocked" : nil].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: ", ")
    }

    func set(owner: String, tools: [String], policy: ToolPolicy) async {
        guard state == .ready else { return }
        let before = policies
        for tool in tools { policies[Self.key(owner, tool)] = policy == .ask ? nil : policy }
        saveError = nil
        failedChange = nil
        do {
            let rows = tools.map { ToolPolicyRow(ownerId: owner, toolId: $0, policy: policy) }
            apply(try await client.save(rows))
        } catch {
            policies = before
            if ToolPoliciesClient.isNotFound(error) {
                state = .unsupported
            } else {
                failedChange = (owner, tools, policy)
                saveError = tools.count == 1
                    ? "Could not save. \(tools[0]) is back to \(self.policy(owner, tools[0]).title)."
                    : "Could not save. Those tools are back to how they were."
            }
        }
    }

    func retry() async {
        guard let change = failedChange else { return }
        await set(owner: change.owner, tools: change.tools, policy: change.policy)
    }
}

/// One line under a connected app: "Tool permissions" and, when not every tool
/// is Ask, the summary. Hidden when signed out (policies live on the account).
struct ToolPermissionsButton: View {
    let entry: ConnEntry
    @ObservedObject private var auth = AuthManager.shared
    @ObservedObject private var store = ToolPoliciesStore.shared
    @State private var showing = false

    var body: some View {
        if auth.isSignedIn {
            Button { showing = true } label: {
                HStack(spacing: 6) {
                    Text("Tool permissions").font(.system(size: 13, weight: .semibold))
                    if let summary = store.summary(owner: "connectors:\(entry.id)") {
                        Text("· \(summary)").font(.system(size: 12.5)).foregroundColor(Color("TextSecondary"))
                    }
                }
                .foregroundColor(Color("TextPrimary"))
                .frame(minHeight: 44, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("tool-permissions-\(entry.id)")
            .task { await store.load() }
            .sheet(isPresented: $showing) { ToolPermissionsSheet(entry: entry) }
        }
    }
}

/// Read-only tools and write/delete tools, each with a group default. The split
/// is a guess from the tool's name (web `WRITE_VERB`), as on the web.
struct ToolPermissionsSheet: View {
    let entry: ConnEntry
    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var store = ToolPoliciesStore.shared
    @State private var tools: [String]?
    @State private var toolsFailed = false

    private var owner: String { "connectors:\(entry.id)" }
    private static let writeVerb = try! NSRegularExpression(
        pattern: "^(create|update|delete|remove|write|edit|send|post|put|patch|set|add|upload|move|rename|archive|publish|run|execute)",
        options: .caseInsensitive)

    private func isWrite(_ name: String) -> Bool {
        Self.writeVerb.firstMatch(in: name, range: NSRange(name.startIndex..., in: name)) != nil
    }

    private var unsupported: Bool { store.state == .unsupported }
    private var groups: [(title: String, tools: [String])] {
        let all = tools ?? []
        return [("Read-only tools", all.filter { !isWrite($0) }), ("Write/delete tools", all.filter { isWrite($0) })]
            .filter { !$0.1.isEmpty }.map { (title: $0.0, tools: $0.1) }
    }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    if unsupported {
                        Text("This server can't save tool permissions yet.").foregroundColor(Color("TextSecondary"))
                    } else if store.state == .failed || toolsFailed {
                        HStack {
                            Text("Could not load tools.").foregroundColor(Color("TextSecondary"))
                            Spacer()
                            Button("Retry") { Task { await reload() } }.frame(minHeight: 44)
                        }
                    } else if tools == nil || store.state == .loading || store.state == .idle {
                        HStack { ProgressView(); Text("Loading").foregroundColor(Color("TextSecondary")) }
                    } else if tools?.isEmpty == true {
                        Text("This connection has no tools to set.").foregroundColor(Color("TextSecondary"))
                    }
                } header: {
                    Text("\(entry.name) · Choose when agents may use these tools.").textCase(nil)
                }
                if store.state == .ready || unsupported {
                    ForEach(groups, id: \.title) { group in
                        Section {
                            groupRow(group)
                            ForEach(group.tools, id: \.self) { tool in
                                HStack {
                                    Text(tool).lineLimit(2)
                                    Spacer()
                                    picker(current: store.policy(owner, tool)) { p in
                                        Task { await store.set(owner: owner, tools: [tool], policy: p) }
                                    }
                                }
                            }
                        } header: { Text(group.title).textCase(nil) }
                    }
                }
                if let error = store.saveError {
                    Section {
                        HStack(spacing: 8) {
                            Image(systemName: "exclamationmark.circle").accessibilityHidden(true)
                            Text(error).font(.system(size: 13)).frame(maxWidth: .infinity, alignment: .leading)
                            Button("Retry") { Task { await store.retry() } }.frame(minHeight: 44)
                        }
                        .foregroundColor(Theme.statusError)
                        .accessibilityElement(children: .combine)
                    }
                }
            }
            .listStyle(.insetGrouped)
            .navigationTitle("Tool permissions")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() }.fontWeight(.semibold) } }
            .task { await reload() }
        }
        .presentationDetents([.large])
        .accessibilityIdentifier("tool-permissions-sheet")
    }

    private func groupRow(_ group: (title: String, tools: [String])) -> some View {
        let values = Set(group.tools.map { store.policy(owner, $0) })
        // Mixed tools show Ask, the way the mockup does.
        let shown: ToolPolicy = values.count == 1 ? values.first! : .ask
        return HStack {
            Text("\(group.title) default").fontWeight(.semibold)
            Spacer()
            picker(current: shown) { p in
                Task { await store.set(owner: owner, tools: group.tools, policy: p) }
            }
        }
    }

    private func picker(current: ToolPolicy, onPick: @escaping (ToolPolicy) -> Void) -> some View {
        Menu {
            ForEach(ToolPolicy.allCases, id: \.self) { p in
                Button(role: p == .block ? .destructive : nil) { onPick(p) } label: {
                    if p == current { Label(p.title, systemImage: "checkmark") } else { Text(p.title) }
                }
            }
        } label: {
            HStack(spacing: 4) {
                Text(current.title)
                Image(systemName: "chevron.up.chevron.down").font(.system(size: 11))
            }
            .foregroundColor(Color("TextSecondary"))
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .disabled(unsupported || store.state != .ready)
        .accessibilityLabel("Policy, \(current.title)")
    }

    private func reload() async {
        toolsFailed = false
        async let policies: Void = store.load(force: store.state == .failed)
        do {
            let connectors = try await ConnectorsClient().listConnectors()
            tools = connectors.first { $0.id == entry.id }?.toolNames ?? []
        } catch {
            toolsFailed = true
        }
        await policies
    }
}
