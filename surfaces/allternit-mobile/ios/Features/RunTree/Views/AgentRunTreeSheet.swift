import SwiftUI

/// The live agent run tree for one reply, as a sheet — port of
/// allternit-ai src/components/agent-run-tree/AgentRunTree.tsx in its
/// narrow (one indented list) layout. The root card is the agent that split
/// the work out; each card under it is a subagent: name, model, status,
/// task, spend (dollars, or tokens for $0 subscription brains) and time.
/// Anything a subagent spawned stacks under it on a rail. A failed card's
/// status opens the reason with Dismiss and Retry; Retry asks the parent
/// agent to run the task again, so it shows in the chat.
struct AgentRunTreeSheet: View {
    /// The reply's tool calls; agent steps become the tree's children.
    let steps: [ToolCallRecord]
    /// The chat's gizzi session, for the task tree (model, cost, nesting).
    let sessionId: String?
    let isStreaming: Bool
    var rootName: String = "Main agent"
    /// Sends a prompt to the parent agent (Retry). No Retry when nil.
    var onRetry: ((String) -> Void)? = nil

    @StateObject private var tasksStore: SessionTasksStore
    @Environment(\.dismiss) private var dismiss

    init(steps: [ToolCallRecord], sessionId: String?, isStreaming: Bool, rootName: String = "Main agent", onRetry: ((String) -> Void)? = nil) {
        self.steps = steps
        self.sessionId = sessionId
        self.isStreaming = isStreaming
        self.rootName = rootName
        self.onRetry = onRetry
        _tasksStore = StateObject(wrappedValue: SessionTasksStore(sessionId: sessionId))
    }

    private var root: RunNode {
        RunTreeBuilder.tree(rootName: rootName, steps: steps, tasks: tasksStore.tasks)
    }

    var body: some View {
        NavigationStack {
            let tree = root
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    totalsLine(tree)
                    if tree.children.isEmpty {
                        emptyState
                    } else {
                        TimelineView(.periodic(from: .now, by: 1)) { context in
                            VStack(alignment: .leading, spacing: 10) {
                                RunNodeCard(node: tree, isRoot: true, now: context.date, onRetry: nil)
                                ForEach(tree.children) { child in
                                    RunBranch(node: child, now: context.date, onRetry: retryHandler)
                                }
                            }
                        }
                    }
                    if let note = tasksStore.loadError, tree.children.isEmpty {
                        Text(note)
                            .font(.caption)
                            .foregroundColor(Color("TextTertiary"))
                    }
                }
                .padding(16)
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle("Agents")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .onAppear { tasksStore.start() }
        .onDisappear { tasksStore.stop() }
    }

    private var retryHandler: ((RunNode) -> Void)? {
        guard let onRetry else { return nil }
        return { node in
            onRetry(RunTreeBuilder.retryPrompt(node))
            dismiss()
        }
    }

    private func totalsLine(_ tree: RunNode) -> some View {
        let totals = RunTotals(root: tree)
        var parts = ["\(totals.agents) \(totals.agents == 1 ? "agent" : "agents")"]
        if totals.running > 0 { parts.append("\(totals.running) running") }
        if totals.failed > 0 { parts.append("\(totals.failed) failed") }
        if let money = RunFormat.spend(cost: totals.cost, tokens: totals.tokens) { parts.append(money) }
        return Text(parts.joined(separator: " · "))
            .font(.footnote)
            .foregroundColor(Color("TextSecondary"))
    }

    @ViewBuilder
    private var emptyState: some View {
        VStack(spacing: 8) {
            if !tasksStore.hasLoaded {
                ProgressView()
                Text("Loading agents…")
            } else {
                Image(systemName: "point.3.connected.trianglepath.dotted")
                    .font(.title2)
                Text(isStreaming ? "No agents started yet." : "This reply didn't hand work to other agents.")
            }
        }
        .font(.subheadline)
        .foregroundColor(Color("TextSecondary"))
        .frame(maxWidth: .infinity)
        .padding(.vertical, 40)
    }
}

/// A card and, on a rail under it, everything it spawned.
private struct RunBranch: View {
    let node: RunNode
    let now: Date
    let onRetry: ((RunNode) -> Void)?
    @State private var isOpen = true

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            RunNodeCard(node: node, isRoot: false, now: now, onRetry: onRetry, isOpen: isOpen, onToggle: node.children.isEmpty ? nil : {
                withAnimation(.easeInOut(duration: 0.2)) { isOpen.toggle() }
            })
            if isOpen, !node.children.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(node.children) { child in
                        RunBranch(node: child, now: now, onRetry: onRetry)
                    }
                }
                .padding(.leading, 16)
                .overlay(alignment: .leading) {
                    Rectangle()
                        .fill(Color("BorderSubtle"))
                        .frame(width: 1.5)
                        .padding(.leading, 6)
                }
            }
        }
    }
}

private struct RunNodeCard: View {
    let node: RunNode
    let isRoot: Bool
    let now: Date
    let onRetry: ((RunNode) -> Void)?
    var isOpen = true
    var onToggle: (() -> Void)? = nil

    @State private var showsFailure = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                face
                Text(node.name)
                    .font(.subheadline.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                if let onToggle, node.descendantCount > 0 {
                    Button(action: onToggle) {
                        HStack(spacing: 2) {
                            Text("\(node.descendantCount)")
                            Image(systemName: "chevron.down")
                                .rotationEffect(.degrees(isOpen ? 0 : -90))
                        }
                        .font(.caption2.weight(.semibold))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(minWidth: 32, minHeight: 32)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("\(isOpen ? "Hide" : "Show") \(node.descendantCount) agents under \(node.name)")
                }
                Spacer(minLength: 4)
                statusPill
            }
            if let model = node.model {
                Text(model)
                    .font(.caption)
                    .foregroundColor(Color("TextTertiary"))
            }
            if !node.task.isEmpty {
                Text(node.task)
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(3)
                    .fixedSize(horizontal: false, vertical: true)
            }
            footer
            if showsFailure {
                failureBox
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(isRoot ? Color("BgPrimary") : Color("BgSecondary"))
        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .stroke(node.status == .failed ? Color.orange.opacity(0.5) : Color("BorderSubtle"), lineWidth: 1)
        )
    }

    private var face: some View {
        ZStack {
            Circle().fill(isRoot ? Color("BgSecondary") : Color("AccentPrimary").opacity(0.14))
            if isRoot {
                Image(systemName: "point.3.connected.trianglepath.dotted")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
            } else {
                Text(String(node.name.prefix(1)))
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(Color("TextPrimary"))
            }
        }
        .frame(width: 24, height: 24)
        .accessibilityHidden(true)
    }

    @ViewBuilder
    private var statusPill: some View {
        let label = HStack(spacing: 4) {
            statusIcon
            Text(node.status.label)
            if node.status == .failed {
                Image(systemName: "chevron.down")
                    .font(.system(size: 8, weight: .bold))
                    .rotationEffect(.degrees(showsFailure ? 180 : 0))
            }
        }
        .font(.caption2.weight(.semibold))
        .foregroundColor(statusColor)
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(Capsule().fill(statusColor.opacity(0.12)))

        if node.status == .failed {
            Button {
                withAnimation(.easeInOut(duration: 0.15)) { showsFailure.toggle() }
            } label: {
                label.frame(minHeight: 32)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("\(node.name) failed. \(showsFailure ? "Hide" : "Show") the reason")
        } else {
            label
        }
    }

    @ViewBuilder
    private var statusIcon: some View {
        switch node.status {
        case .running: ProgressView().controlSize(.mini)
        case .queued: Image(systemName: "clock")
        case .done: Image(systemName: "checkmark.circle.fill")
        case .failed: Image(systemName: "exclamationmark.circle.fill")
        case .stopped: Image(systemName: "stop.circle")
        }
    }

    private var statusColor: Color {
        switch node.status {
        case .running, .queued: return Color("AccentPrimary")
        case .done: return Color("TextSecondary")
        case .failed: return .orange
        case .stopped: return Color("TextTertiary")
        }
    }

    @ViewBuilder
    private var footer: some View {
        let subtree = node.subtreeSpend
        let spend: (Double?, Int) = isRoot ? (subtree.cost, subtree.tokens) : (node.cost, node.tokens)
        let money = RunFormat.spend(cost: spend.0, tokens: spend.1)
        let elapsed = node.elapsed(now: now).flatMap { $0 >= 1 ? RunFormat.elapsed($0) : nil }
        if money != nil || elapsed != nil {
            HStack(spacing: 10) {
                if let money { Text(money) }
                if let elapsed { Text(elapsed).monospacedDigit() }
            }
            .font(.caption)
            .foregroundColor(Color("TextTertiary"))
        }
    }

    private var failureBox: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(node.error ?? "It stopped with an error and gave no reason.")
                .font(.footnote)
                .foregroundColor(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) {
                Button("Dismiss") {
                    withAnimation(.easeInOut(duration: 0.15)) { showsFailure = false }
                }
                .buttonStyle(QuestionCardButtonStyle(kind: .quiet))
                if let onRetry {
                    Button("Retry") { onRetry(node) }
                        .buttonStyle(QuestionCardButtonStyle(kind: .primary))
                }
            }
        }
        .padding(10)
        .background(Color.orange.opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
    }
}
