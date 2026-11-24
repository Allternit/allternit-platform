import SwiftUI

// MARK: - Session context

/// One row of the context window (gizzi `/session/:id/context` categories).
typealias SessionContextCategory = SessionContextBreakdown.Category

/// `GET /api/v1/agent-sessions/:id/context?limits=false` (fetchSessionContext).
enum SessionContextClient {
    /// The envelope with the runtime's compaction count (gizzi
    /// `ContextUsage.Result.compactions`; 0 on runtimes that omit it).
    struct Usage: Decodable, Sendable {
        let breakdown: SessionContextBreakdown?
        let compactions: Int

        private enum CodingKeys: String, CodingKey { case breakdown, compactions }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            breakdown = try c.decodeIfPresent(SessionContextBreakdown.self, forKey: .breakdown)
            compactions = max(0, ((try? c.decodeIfPresent(Int.self, forKey: .compactions)) ?? nil) ?? 0)
        }
    }

    static func fetch(sessionId: String) async throws -> SessionContextBreakdown? {
        try await fetchUsage(sessionId: sessionId).breakdown
    }

    static func fetchUsage(sessionId: String) async throws -> Usage {
        let escaped = sessionId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? sessionId
        return try await APIClient.shared.get(path: "agent-sessions/\(escaped)/context?limits=false")
    }

    /// A session the runtime knows (useSessionContext isRuntimeSessionId).
    static func isRuntimeSession(_ sessionId: String?) -> Bool {
        guard let sessionId, !sessionId.isEmpty else { return false }
        return !sessionId.hasPrefix("temp-")
    }
}

/// "Compacted 3 times" (ContextWindowCard compaction line); nil at 0.
enum ContextCompactions {
    static func label(_ count: Int) -> String? {
        guard count > 0 else { return nil }
        return count == 1 ? "Compacted 1 time" : "Compacted \(count) times"
    }
}

/// 144600 → "144.6k", 1000000 → "1M" (ContextUsage.tsx formatTokens).
enum ContextTokens {
    static func format(_ tokens: Double) -> String {
        let n = max(0, tokens.rounded())
        func trim(_ value: Double) -> String {
            let s = String(format: "%.1f", value)
            return s.hasSuffix(".0") ? String(s.dropLast(2)) : s
        }
        if n >= 1_000_000 { return "\(trim(n / 1_000_000))M" }
        if n >= 1_000 { return "\(trim(n / 1_000))k" }
        return String(Int(n))
    }
}

// MARK: - Meter

/// The composer's context and usage meter (ContextUsage.tsx
/// `ComposerUsageMeter`): five HUD blocks beside the model, filled by the
/// plan window used (else how full the context is), with the context
/// percent once the session has one. A tap opens what fills the context
/// and the plan's limits. Hidden with neither.
struct ComposerUsageMeter: View {
    let sessionId: String?
    /// Changes when a turn lands, so the meter follows the conversation.
    var refreshKey: String = ""

    @ObservedObject private var usage = UsageStore.shared
    @State private var breakdown: SessionContextBreakdown? = nil
    @State private var compactions = 0
    @State private var isDetailsPresented = false

    private var hasSession: Bool { SessionContextClient.isRuntimeSession(sessionId) }
    private var planFraction: Double? { usage.percentUsed.map { $0 / 100 } }
    private var contextPercent: Int? { breakdown.map { Int(($0.utilization * 100).rounded()) } }

    private var label: String {
        var parts: [String] = []
        if let breakdown, let pct = contextPercent {
            parts.append("Context: \(pct)% used (\(ContextTokens.format(breakdown.usedTokens)) of \(ContextTokens.format(breakdown.contextWindow)))")
        }
        if let compacted = ContextCompactions.label(compactions) { parts.append(compacted) }
        if let text = usage.percentText { parts.append("Plan usage: \(text)") }
        return parts.isEmpty ? "Context and usage" : parts.joined(separator: ". ")
    }

    var body: some View {
        Group {
            if planFraction != nil || hasSession {
                Button { isDetailsPresented = true } label: {
                    HStack(spacing: 4) {
                        UsageHudBlocks(fraction: planFraction ?? breakdown?.utilization ?? 0)
                        if let contextPercent {
                            Text("\(contextPercent)%")
                                .font(.system(size: 11, weight: .medium).monospacedDigit())
                                .foregroundColor(Color("TextTertiary"))
                        }
                    }
                    .padding(.horizontal, 4)
                    .frame(minWidth: 32, minHeight: 44)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("\(label). Open details.")
            }
        }
        .task(id: "\(sessionId ?? "")|\(refreshKey)") { await load() }
        .sheet(isPresented: $isDetailsPresented) {
            ContextUsageSheet(breakdown: breakdown, compactions: compactions)
        }
    }

    private func load() async {
        usage.fetchUsageIfNeeded()
        guard hasSession, let sessionId else {
            breakdown = nil
            compactions = 0
            return
        }
        // Keep the last good breakdown on a transient failure.
        if let fresh = try? await SessionContextClient.fetchUsage(sessionId: sessionId) {
            breakdown = fresh.breakdown
            compactions = fresh.compactions
        }
    }
}

/// Five HUD blocks (HudBlocks count=5): amber from 80%, red at the limit.
struct UsageHudBlocks: View {
    let fraction: Double

    private var tone: Color {
        if fraction >= 1 { return Theme.statusError }
        if fraction >= 0.8 { return Theme.statusWarning }
        return Color("TextSecondary")
    }

    var body: some View {
        let filled = Int((min(max(fraction, 0), 1) * 5).rounded(.up))
        HStack(spacing: 2) {
            ForEach(0..<5, id: \.self) { index in
                RoundedRectangle(cornerRadius: 1)
                    .fill(index < filled ? tone : Color("BorderStrong"))
                    .frame(width: 4, height: 10)
            }
        }
        .accessibilityHidden(true)
    }
}

/// The meter's details (ContextUsagePanel): the context window by row,
/// who measured it, then the plan's usage limits.
private struct ContextUsageSheet: View {
    let breakdown: SessionContextBreakdown?
    var compactions: Int = 0
    @Environment(\.dismiss) private var dismiss

    /// The panel's rows in order; a brain that does not fill one is left out.
    private static let order: [(id: String, name: String)] = [
        ("messages", "Messages"), ("system_tools", "System tools"), ("memory_files", "Memory files"),
        ("skills", "Skills"), ("system_prompt", "System prompt"), ("mcp_tools", "MCP tools"),
        ("mcp_instructions", "MCP server instructions"), ("autocompact_buffer", "Autocompact buffer"),
        ("free_space", "Free space"),
    ]

    private struct Row: Hashable {
        let name: String
        let tokens: Double
    }

    private var rows: [Row] {
        guard let categories = breakdown?.categories else { return [] }
        let byId = Dictionary(categories.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        return Self.order.compactMap { row in
            guard let match = byId[row.id], match.tokens > 0 else { return nil }
            return Row(name: match.name ?? row.name, tokens: match.tokens)
        }
    }

    private var measured: String {
        guard let breakdown else { return "" }
        if breakdown.source == "brain" { return "Reported by \(breakdown.brainName ?? "the brain")" }
        return breakdown.basis == "estimated" ? "Estimated" : "Measured by the provider"
    }

    private func compactionRow(_ text: String) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "arrow.down.right.and.arrow.up.left")
                .foregroundColor(Color("TextTertiary"))
                .accessibilityHidden(true)
            Text(text)
            Spacer()
        }
        .font(.subheadline)
        .foregroundColor(Color("TextSecondary"))
        .accessibilityElement(children: .combine)
        .accessibilityHint("Older messages were summarized to make room.")
    }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    if let breakdown {
                        HStack {
                            Text("\(ContextTokens.format(breakdown.usedTokens)) of \(ContextTokens.format(breakdown.contextWindow))")
                            Spacer()
                            Text("\(Int((breakdown.utilization * 100).rounded()))%")
                                .foregroundColor(Color("TextSecondary"))
                                .monospacedDigit()
                        }
                        ForEach(rows, id: \.name) { row in
                            HStack {
                                Text(row.name).foregroundColor(Color("TextSecondary"))
                                Spacer()
                                Text(ContextTokens.format(row.tokens))
                                    .foregroundColor(Color("TextSecondary"))
                                    .monospacedDigit()
                            }
                            .font(.subheadline)
                        }
                        if let compacted = ContextCompactions.label(compactions) {
                            compactionRow(compacted)
                        }
                    } else {
                        Text("The context window shows once this chat has a reply.")
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                    }
                } header: {
                    Text("Context window")
                } footer: {
                    if breakdown != nil { Text(measured) }
                }
                Section("Usage") {
                    PlanUsageRows()
                }
            }
            .navigationTitle("Context and usage")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }
}
