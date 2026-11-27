import SwiftUI

/// The checklist under the turn after `load_skill`: "Skill · <name> · <from>" and
/// the skill's numbered steps. Ticks come only from a later todo tool in the same
/// turn; with no todo it is a plain numbered list.
struct SkillChecklistCard: View {
    let name: String
    let result: SkillResult
    let ticks: [Bool]?

    var body: some View {
        if !result.steps.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                Text("Skill \u{00B7} \(name) \u{00B7} \(result.from)")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(Color("TextSecondary"))
                ForEach(Array(result.steps.enumerated()), id: \.offset) { index, step in
                    let done = (ticks?.indices.contains(index) ?? false) && (ticks?[index] ?? false)
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        if ticks == nil {
                            Text("\(index + 1).")
                                .font(.system(size: 13).monospacedDigit())
                                .foregroundColor(Color("TextSecondary"))
                                .frame(minWidth: 20, alignment: .trailing)
                        } else {
                            Image(systemName: done ? "checkmark.circle.fill" : "circle")
                                .font(.system(size: 13))
                                .foregroundColor(done ? Color("TextPrimary") : Color("TextSecondary"))
                        }
                        Text(step)
                            .font(.system(size: 13))
                            .foregroundColor(done ? Color("TextSecondary") : Color("TextPrimary"))
                            .strikethrough(done, color: Color("TextSecondary"))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityValue(ticks == nil ? "" : (done ? "done" : "not done"))
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 14).fill(Color("FillNeutral")))
            .accessibilityIdentifier("skillChecklistCard")
        }
    }
}
