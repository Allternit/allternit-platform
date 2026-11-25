import SwiftUI

/// Colours the team map shares. Amber marks "waiting on someone", blue "working".
enum TeamMapPalette {
    static let waiting = Color.orange
    static let working = Color.blue
    static let line = Color("BorderStrong")

    static func ring(_ state: String, waiting: Bool) -> Color {
        if waiting || state == "waiting_approval" || state == "needs_input" { return Self.waiting }
        if state == "working" { return Self.working }
        return Self.line
    }
}

/// A person (circle with initials) or a bot (rounded square with a chip glyph).
struct TeamNodeAvatar: View {
    let node: TeamNode
    var ring: Color = TeamMapPalette.line
    var badge: Int = 0
    @ScaledMetric(relativeTo: .body) private var size: CGFloat = 40

    var body: some View {
        ZStack {
            if node.isBot {
                RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
                    .fill(Color("FillNeutral"))
                    .overlay(RoundedRectangle(cornerRadius: size * 0.28, style: .continuous).strokeBorder(ring, lineWidth: 2))
                Image(systemName: "cpu")
                    .font(.system(size: size * 0.42, weight: .medium))
                    .foregroundStyle(Color("TextPrimary"))
            } else {
                Circle()
                    .fill(Color("FillNeutral"))
                    .overlay(Circle().strokeBorder(ring, lineWidth: 2))
                Text(node.initials)
                    .font(.system(size: size * 0.34, weight: .semibold))
                    .foregroundStyle(Color("TextPrimary"))
            }
        }
        .frame(width: size, height: size)
        .overlay(alignment: .topTrailing) {
            if badge > 0 {
                Text("\(badge)")
                    .font(.caption2.weight(.bold))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 5)
                    .frame(minWidth: 18, minHeight: 18)
                    .background(Capsule().fill(TeamMapPalette.waiting))
                    .offset(x: 7, y: -7)
            }
        }
        .accessibilityHidden(true)
    }
}

/// One rule line: lock + "Always on" for a built-in rule, else On/Off.
struct TeamRuleRow: View {
    let rule: TeamRule
    var onDelete: (() -> Void)?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: rule.locked ? "lock.fill" : "checkmark.shield")
                .font(.footnote)
                .foregroundStyle(Color("TextSecondary"))
                .accessibilityHidden(true)
            Text(rule.text)
                .font(.subheadline)
                .foregroundStyle(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
            Text(statusText)
                .font(.caption)
                .foregroundStyle(Color("TextSecondary"))
            if let onDelete, !rule.locked {
                Button(role: .destructive, action: onDelete) {
                    Image(systemName: "trash")
                        .font(.footnote)
                        .frame(minWidth: 44, minHeight: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.borderless)
                .foregroundStyle(.red)
                .accessibilityLabel("Delete rule")
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(rule.text). \(statusText)")
    }

    private var statusText: String {
        rule.locked ? "Always on" : (rule.enabled ? "On" : "Off")
    }
}

/// An ask on the map (Cowork, Factory, Cloud or platform). Approve / Deny
/// only when the viewer may decide it and the store has a resolve route.
struct TeamAskRow: View {
    let ask: TeamAsk
    let map: TeamMap
    /// Returns nil on success, else the sentence to show.
    var onDecide: ((Bool) async -> String?)?
    /// "Who approves this": opens the approver's node.
    var onWhoApproves: (() -> Void)?

    @State private var busy = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Circle().fill(TeamMapPalette.waiting).frame(width: 8, height: 8).accessibilityHidden(true)
                Text(headline)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(Color("TextPrimary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let summary = ask.summary, !summary.isEmpty {
                Text(summary)
                    .font(.footnote)
                    .foregroundStyle(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let rule = map.rule(ask.ruleId) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Image(systemName: "checkmark.shield").font(.footnote).accessibilityHidden(true)
                    Text("Rule: ") + Text(rule.text).bold()
                }
                .font(.footnote)
                .foregroundStyle(Color("TextPrimary"))
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
            }
            detailLine("Approver", map.approverName(ask) ?? "Not named yet")
            if let requester = requesterLine { detailLine("Asked by", requester) }
            if let level = map.levelLine(ask) { detailLine("Level", level) }
            if let onWhoApproves {
                Button("Who approves this", action: onWhoApproves)
                    .buttonStyle(.borderless)
                    .font(.footnote.weight(.semibold))
                    .frame(minHeight: 44)
                    .accessibilityHint("Opens the approver on the team map")
            }
            if onDecide != nil, map.canDecide(ask) {
                HStack(spacing: 10) {
                    Button { decide(true) } label: { Text("Approve").frame(maxWidth: .infinity) }
                        .buttonStyle(.borderedProminent)
                        .tint(.green)
                    Button { decide(false) } label: { Text("Deny").frame(maxWidth: .infinity) }
                        .buttonStyle(.bordered)
                        .tint(.red)
                }
                .disabled(busy)
                .controlSize(.regular)
            } else if ask.resolve == nil {
                Text("Decide this on the platform.")
                    .font(.caption)
                    .foregroundStyle(Color("TextSecondary"))
            }
            if let error {
                Text(error)
                    .font(.footnote)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.vertical, 4)
    }

    private var headline: String {
        if let actor = map.name(ask.actor) { return "\(ask.title) · \(actor)" }
        return ask.title
    }

    private var requesterLine: String? {
        guard let actor = ask.actor else { return nil }
        if actor.hasPrefix("p:") { return map.name(actor) }
        let bot = map.name(actor) ?? "a bot"
        if let owner = map.requesterName(ask) { return "\(owner), through \(bot)" }
        return bot
    }

    private func detailLine(_ key: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(key)
                .foregroundStyle(Color("TextSecondary"))
                .frame(minWidth: 72, alignment: .leading)
            Text(value)
                .foregroundStyle(Color("TextPrimary"))
                .fixedSize(horizontal: false, vertical: true)
        }
        .font(.footnote)
        .accessibilityElement(children: .combine)
    }

    private func decide(_ approve: Bool) {
        guard let onDecide, !busy else { return }
        busy = true
        error = nil
        Task {
            error = await onDecide(approve)
            busy = false
        }
    }
}
