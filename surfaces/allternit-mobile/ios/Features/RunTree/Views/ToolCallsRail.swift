import SwiftUI

/// Tool calls on a thin rail — the phone port of the web ProcessBlock's
/// Arc-style rail (allternit-ai #850, src/components/ai-elements/process).
///
/// Working: "Using 3 tools · 14s" (· "1 failed", or "Waiting for approval"
/// while a step waits on the person), then the last six rows on the rail,
/// "+N earlier" above them, the running one with its orb. A failed row says
/// why under it, with Retry on the latest reply; a row whose call waits on
/// a permission carries Deny / Always / Allow.
/// Done: one line "Used 4 tools · 12s ›"; a tap opens every row.
/// Two or more subagents add an "Agents" row that opens the run tree.
struct ToolCallsRail: View {
    let calls: [ToolCallRecord]
    let isStreaming: Bool
    /// Pending permissions for this chat; one whose `toolCallId` matches a
    /// row draws its buttons on that row.
    var permissions: [AgentPermissionAsk] = []
    /// Opens the agent run tree sheet.
    var onOpenRunTree: () -> Void = {}
    /// Runs the turn again; only given on the latest finished reply.
    var onRetry: (() -> Void)? = nil

    @State private var isExpanded = false
    @State private var openRowId: String? = nil

    /// Rows the working view keeps on screen (web: last 6).
    private static let liveWindow = 6
    /// Subagents needed before the reply gets an Agents row (web MIN_AGENTS_FOR_TREE).
    private static let minAgentsForTree = 2

    private var visibleCalls: [ToolCallRecord] {
        calls.filter { !$0.isTodo }.map { call in
            // A finished reply can't still be running a tool: it lost its
            // final update, so it settles instead of spinning forever.
            guard !isStreaming, call.state == .running else { return call }
            var settled = call
            settled.state = .done
            return settled
        }
    }

    var body: some View {
        let shown = visibleCalls
        if shown.isEmpty {
            EmptyView()
        } else if isStreaming {
            // Only a live reply ticks; a finished one draws once.
            TimelineView(.periodic(from: .now, by: 1)) { context in
                content(shown, now: context.date)
            }
        } else {
            content(shown, now: Date())
        }
    }

    @ViewBuilder
    private func content(_ shown: [ToolCallRecord], now: Date) -> some View {
        let failed = shown.filter { $0.state == .failed }.count
        let waiting = shown.contains { call in permissions.contains { $0.toolCallId == call.id } }
        let agentSteps = shown.filter(\.isAgentStep)
        let railOpen = isStreaming || isExpanded

        VStack(alignment: .leading, spacing: 6) {
            Button {
                guard !isStreaming else { return }
                withAnimation(.easeInOut(duration: 0.2)) { isExpanded.toggle() }
            } label: {
                header(count: shown.count, failed: failed, waiting: waiting, elapsed: totalElapsed(shown, now: now))
            }
            .buttonStyle(.plain)
            .accessibilityHint(isStreaming ? "" : (isExpanded ? "Hides the steps" : "Shows every step"))

            if railOpen {
                let window = isStreaming ? Array(shown.suffix(Self.liveWindow)) : shown
                VStack(alignment: .leading, spacing: 2) {
                    if window.count < shown.count {
                        Text("+\(shown.count - window.count) earlier")
                            .font(.caption2)
                            .foregroundColor(Color("TextTertiary"))
                            .padding(.leading, 18)
                            .padding(.vertical, 2)
                    }
                    ForEach(window) { call in
                        row(call, now: now, isLast: call.id == shown.last?.id)
                    }
                }
                .padding(.leading, 9)
                .overlay(alignment: .leading) {
                    Rectangle()
                        .fill(Color("BorderSubtle"))
                        .frame(width: 1.5)
                        .padding(.leading, 9)
                        .padding(.vertical, 6)
                }
                .transition(.opacity)
            }

            if agentSteps.count >= Self.minAgentsForTree {
                agentsRow(agentSteps)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    // MARK: - Header

    private func header(count: Int, failed: Int, waiting: Bool, elapsed: TimeInterval?) -> some View {
        HStack(spacing: 6) {
            Group {
                if isStreaming {
                    ThinkingOrb(state: .forToolName(visibleCalls.last?.toolName ?? ""), size: .inline)
                } else {
                    Image(systemName: "wrench.and.screwdriver")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            .frame(width: 20, height: 20)

            Text(headerText(count: count, failed: failed, waiting: waiting, elapsed: elapsed))
                .font(.caption.weight(.medium))
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(1)

            if !isStreaming {
                Image(systemName: "chevron.right")
                    .font(.caption2.weight(.semibold))
                    .foregroundColor(Color("TextTertiary"))
                    .rotationEffect(.degrees(isExpanded ? 90 : 0))
            }
        }
        .frame(minHeight: 28)
        .contentShape(Rectangle())
    }

    private func headerText(count: Int, failed: Int, waiting: Bool, elapsed: TimeInterval?) -> String {
        if isStreaming && waiting { return "Waiting for approval" }
        var parts = ["\(isStreaming ? "Using" : "Used") \(count) \(count == 1 ? "tool" : "tools")"]
        // The elapsed-time readout: the wire carries no result size, so time
        // is the honest number to show.
        if let elapsed, elapsed >= 1 { parts.append(RunFormat.elapsed(elapsed)) }
        if failed > 0 { parts.append("\(failed) failed") }
        return parts.joined(separator: " · ")
    }

    private func totalElapsed(_ shown: [ToolCallRecord], now: Date) -> TimeInterval? {
        guard let start = shown.compactMap(\.startedAt).min() else { return nil }
        let end: Date? = isStreaming ? now : shown.compactMap(\.finishedAt).max()
        return end.map { max(0, $0.timeIntervalSince(start)) }
    }

    // MARK: - Rows

    @ViewBuilder
    private func row(_ call: ToolCallRecord, now: Date, isLast: Bool) -> some View {
        let summary = call.summary
        let ask = permissions.first { $0.toolCallId == call.id }
        let isOpen = openRowId == call.id

        VStack(alignment: .leading, spacing: 4) {
            Button {
                if call.isAgentStep {
                    onOpenRunTree()
                } else {
                    withAnimation(.easeInOut(duration: 0.15)) { openRowId = isOpen ? nil : call.id }
                }
            } label: {
                HStack(spacing: 8) {
                    rowIcon(call, symbol: summary.symbol)
                    rowWords(summary)
                    Spacer(minLength: 4)
                    if let elapsed = call.elapsed(now: now), call.state != .running || elapsed >= 1 {
                        Text(RunFormat.duration(elapsed))
                            .font(.caption2.monospacedDigit())
                            .foregroundColor(Color("TextTertiary"))
                    }
                    stateMark(call.state)
                }
                .frame(minHeight: 32)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityElement(children: .combine)
            .accessibilityLabel("\(summary.verb) \(summary.detail), \(call.state == .running ? "running" : call.state == .failed ? "failed" : "done")")

            if call.state == .failed {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(call.error ?? "It stopped with an error and gave no reason.")
                        .font(.caption)
                        .foregroundColor(.orange)
                        .lineLimit(isOpen ? nil : 2)
                        .fixedSize(horizontal: false, vertical: true)
                    if isLast, let onRetry {
                        Button("Retry", action: onRetry)
                            .font(.caption.weight(.semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .frame(minHeight: 32)
                    }
                }
                .padding(.leading, 26)
            }

            if let ask, isStreaming {
                PermissionAskView(ask: ask, compact: true)
                    .padding(.leading, 26)
            }

            if isOpen, call.state != .failed {
                rowDetail(call)
                    .padding(.leading, 26)
                    .transition(.opacity)
            }
        }
        .padding(.leading, 6)
    }

    @ViewBuilder
    private func rowIcon(_ call: ToolCallRecord, symbol: String) -> some View {
        if call.isAgentStep {
            SubagentFaceView(subagent: call.summary.subagent, name: call.summary.verb,
                             running: call.state == .running, size: 18)
                .overlay(Circle().stroke(Color("BorderSubtle"), lineWidth: 1))
        } else {
            plainRowIcon(call, symbol: symbol)
        }
    }

    private func plainRowIcon(_ call: ToolCallRecord, symbol: String) -> some View {
        ZStack {
            Circle().fill(Color("BgPrimary"))
            if call.isAgentStep {
                Text(String(call.summary.verb.prefix(1)))
                    .font(.system(size: 9, weight: .bold))
                    .foregroundColor(Color("TextPrimary"))
            } else {
                Image(systemName: symbol)
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .frame(width: 18, height: 18)
        .overlay(Circle().stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    private func rowWords(_ summary: ToolDescription) -> some View {
        (Text(summary.verb).foregroundColor(Color("TextPrimary"))
         + Text(summary.detail.isEmpty ? "" : (summary.subagent != nil ? " · " : " ") + summary.detail)
            .foregroundColor(Color("TextSecondary")))
            .font(.caption)
            .lineLimit(1)
            .truncationMode(.tail)
    }

    @ViewBuilder
    private func stateMark(_ state: ToolCallRecord.State) -> some View {
        switch state {
        case .running:
            ProgressView()
                .controlSize(.mini)
                .frame(width: 14, height: 14)
        case .done:
            Image(systemName: "checkmark")
                .font(.system(size: 10, weight: .bold))
                .foregroundColor(Color("TextTertiary"))
                .frame(width: 14, height: 14)
        case .failed:
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 10))
                .foregroundColor(.orange)
                .frame(width: 14, height: 14)
        }
    }

    /// The step's input, a line per field — what the tool was given.
    private func rowDetail(_ call: ToolCallRecord) -> some View {
        let fields = call.input
            .filter { !$0.value.isEmpty }
            .sorted { $0.key < $1.key }
            .prefix(6)
        return VStack(alignment: .leading, spacing: 2) {
            if let title = call.title, !title.isEmpty {
                Text(title).font(.caption).foregroundColor(Color("TextSecondary"))
            }
            ForEach(Array(fields), id: \.key) { field in
                Text("\(field.key): \(field.value)")
                    .font(.system(.caption2, design: .monospaced))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(3)
            }
            if fields.isEmpty && (call.title ?? "").isEmpty {
                Text(call.toolName)
                    .font(.system(.caption2, design: .monospaced))
                    .foregroundColor(Color("TextTertiary"))
            }
        }
        .padding(8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPrimary"))
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
    }

    // MARK: - Agents row

    private func agentsRow(_ steps: [ToolCallRecord]) -> some View {
        let running = steps.filter { $0.state == .running }.count
        let failed = steps.filter { $0.state == .failed }.count
        var parts = ["\(steps.count) agents"]
        if running > 0 { parts.append("\(running) running") }
        if failed > 0 { parts.append("\(failed) failed") }
        return Button(action: onOpenRunTree) {
            HStack(spacing: 6) {
                Image(systemName: "point.3.connected.trianglepath.dotted")
                    .font(.caption.weight(.semibold))
                    .frame(width: 20, height: 20)
                Text("Agents")
                    .font(.caption.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text(parts.joined(separator: " · "))
                    .font(.caption)
                Image(systemName: "chevron.right")
                    .font(.caption2.weight(.semibold))
            }
            .foregroundColor(Color("TextSecondary"))
            .frame(minHeight: 32)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityHint("Opens the agent run tree")
    }
}
