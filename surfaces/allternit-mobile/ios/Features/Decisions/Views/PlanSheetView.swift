import SwiftUI

/// A plan waiting for approval, read-only (web PlanViewer + the plan bar in
/// ArtifactWindow, allternit-ai ar/plans 2.17 and 2.18). Steps first, then the
/// other Markdown sections. Approve answers the question's "Yes"; Revise and
/// Ask a question both answer "No" (keep planning), the same as the web bar.
struct PlanSheetView: View {
    enum Action { case approve, revise, ask }

    let plan: AgentQuestionPlan
    let agentName: String?
    let isSending: Bool
    let error: String?
    var onAction: (Action) -> Void
    var onClose: () -> Void

    private var title: String {
        let t = plan.title?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return t.isEmpty ? "Untitled plan" : t
    }

    /// The Markdown left after the title heading and the numbered steps.
    private var rest: String {
        PlanSheetView.restOfBody(plan.body ?? "", title: plan.title)
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    Text(title)
                        .font(.system(.title2, design: .serif))
                        .foregroundColor(Color("TextPrimary"))
                        .fixedSize(horizontal: false, vertical: true)
                    Text("Plan · \(plan.steps.count) \(plan.steps.count == 1 ? "step" : "steps")")
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))

                    Text("STEPS")
                        .font(.caption2.weight(.medium).monospaced())
                        .tracking(1.2)
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.top, 16)
                    ForEach(Array(plan.steps.enumerated()), id: \.offset) { i, step in
                        HStack(alignment: .top, spacing: 10) {
                            Text("\(i + 1)")
                                .font(.footnote.monospacedDigit())
                                .foregroundColor(Color("TextSecondary"))
                                .frame(width: 22, alignment: .leading)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(step.title)
                                    .font(.subheadline)
                                    .foregroundColor(Color("TextPrimary"))
                                if let d = step.detail, !d.isEmpty {
                                    Text(d)
                                        .font(.footnote)
                                        .foregroundColor(Color("TextSecondary"))
                                }
                            }
                            .fixedSize(horizontal: false, vertical: true)
                            Spacer(minLength: 0)
                        }
                        .padding(.vertical, 8)
                        Divider().opacity(i == plan.steps.count - 1 ? 0 : 1)
                    }

                    if !rest.isEmpty {
                        Text(Self.markdown(rest))
                            .font(.subheadline)
                            .foregroundColor(Color("TextPrimary"))
                            .fixedSize(horizontal: false, vertical: true)
                            .padding(.top, 16)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(Color("BgPrimary"))
            .navigationTitle(agentName.map { "\($0)'s plan" } ?? "Plan")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close", action: onClose).disabled(isSending)
                }
            }
            .safeAreaInset(edge: .bottom) { bar }
        }
        .presentationDetents([.large])
    }

    private var bar: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let error {
                Text(error)
                    .font(.caption)
                    .foregroundColor(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack(spacing: 8) {
                Button { onAction(.approve) } label: {
                    if isSending { ProgressView().controlSize(.small) } else { Text("Approve") }
                }
                .buttonStyle(QuestionCardButtonStyle(kind: .primary))
                Button("Revise") { onAction(.revise) }
                    .buttonStyle(QuestionCardButtonStyle(kind: .quiet))
                Button("Ask a question") { onAction(.ask) }
                    .buttonStyle(QuestionCardButtonStyle(kind: .quiet))
                Spacer(minLength: 0)
            }
            .disabled(isSending)
        }
        .padding(.horizontal, 16)
        .padding(.top, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPrimary"))
        .overlay(alignment: .top) { Divider() }
    }

    private static func markdown(_ s: String) -> AttributedString {
        (try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(s)
    }

    /// Port of the web `parsePlanBody` "rest": drop a leading H1 that repeats
    /// the title and the steps block (under a Steps heading, else the first
    /// numbered list).
    static func restOfBody(_ md: String, title: String?) -> String {
        var lines = md.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n")
        while let f = lines.first, f.trimmingCharacters(in: .whitespaces).isEmpty { lines.removeFirst() }
        if let f = lines.first, f.hasPrefix("# "), !f.hasPrefix("## ") {
            let h = f.dropFirst(2).replacingOccurrences(of: "**", with: "").trimmingCharacters(in: .whitespaces).lowercased()
            if title == nil || h == title!.trimmingCharacters(in: .whitespaces).lowercased() { lines.removeFirst() }
        }
        func isHeading(_ l: String) -> Bool { l.range(of: #"^#{1,6}\s+"#, options: .regularExpression) != nil }
        func isNumbered(_ l: String) -> Bool { l.range(of: #"^\s{0,3}\d+[.)]\s+"#, options: .regularExpression) != nil }
        var from = -1, to = lines.count
        if let head = lines.firstIndex(where: { $0.range(of: #"^#{1,6}\s*(?i:plan\s*)?(?i:steps?)\b"#, options: .regularExpression) != nil }) {
            from = head
            if let next = lines.indices.first(where: { $0 > head && isHeading(lines[$0]) }) { to = next }
        } else if let first = lines.firstIndex(where: isNumbered) {
            from = first
            var i = first
            while i < lines.count, isNumbered(lines[i]) || lines[i].hasPrefix(" ") || lines[i].trimmingCharacters(in: .whitespaces).isEmpty { i += 1 }
            to = i
        }
        if from >= 0 { lines.removeSubrange(from..<to) }
        return lines.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
