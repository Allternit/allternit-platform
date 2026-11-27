import SwiftUI

extension Notification.Name {
    /// The + sheet's "Server skills" row posts this; the composer types `/` to open the list.
    static let composerOpenSkills = Notification.Name("composerOpenSkills")
}

/// The list above the composer while the text starts with `/`: enabled server
/// skills filtered by prefix. Tap fills `/<id> `. Hidden when nothing matches.
struct SkillSlashList: View {
    let text: String
    let skills: [ServerSkill]
    let onPick: (ServerSkill) -> Void

    static func matches(text: String, skills: [ServerSkill]) -> [ServerSkill] {
        guard text.hasPrefix("/"), !text.contains(" ") else { return [] }
        let prefix = String(text.dropFirst()).lowercased()
        return skills.filter { $0.enabled && (prefix.isEmpty || $0.id.lowercased().hasPrefix(prefix)) }
    }

    var body: some View {
        let list = Self.matches(text: text, skills: skills)
        if !list.isEmpty {
            ScrollView {
                VStack(spacing: 0) {
                    ForEach(Array(list.prefix(8).enumerated()), id: \.element.id) { index, skill in
                        if index > 0 { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
                        Button { onPick(skill) } label: {
                            HStack(alignment: .top, spacing: 10) {
                                Image(systemName: "sparkles")
                                    .font(.system(size: 14))
                                    .foregroundColor(Color("TextSecondary"))
                                    .padding(.top, 2)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text("/\(skill.id)")
                                        .font(.system(size: 14, weight: .bold, design: .monospaced))
                                        .foregroundColor(Color("TextPrimary"))
                                    if !skill.description.isEmpty {
                                        Text(skill.description)
                                            .font(.system(size: 12))
                                            .foregroundColor(Color("TextSecondary"))
                                            .lineLimit(2)
                                    }
                                }
                                Spacer(minLength: 8)
                                Text(skill.from)
                                    .font(.system(size: 12))
                                    .foregroundColor(Color("TextSecondary"))
                                    .lineLimit(1)
                            }
                            .padding(.horizontal, 14)
                            .padding(.vertical, 10)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("/\(skill.id), from \(skill.from)")
                    }
                }
            }
            .frame(maxHeight: 280)
            .background(RoundedRectangle(cornerRadius: 14).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle"), lineWidth: 1))
            .padding(.horizontal, 12)
            .padding(.bottom, 6)
            .accessibilityIdentifier("skillSlashList")
        }
    }
}
