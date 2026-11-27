import SwiftUI

/// "From your servers" in Customize › Skills: one switch per skill the person's
/// MCP servers (and installed apps) serve. Hidden when there are none. A failed
/// save puts the switch back and says so.
struct ServerSkillsSection: View {
    @ObservedObject private var store = ServerSkillsStore.shared

    var body: some View {
        Group {
            if store.hasSkills {
                VStack(alignment: .leading, spacing: 0) {
                    Text("From your servers")
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.bottom, 4)
                    ForEach(Array(store.skills.enumerated()), id: \.element.id) { index, skill in
                        if index > 0 { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
                        Toggle(isOn: Binding(get: { skill.enabled }, set: { store.set(skill, enabled: $0) })) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(skill.name)
                                    .font(.system(size: 15))
                                    .foregroundColor(Color("TextPrimary"))
                                Text(skill.sourceLine)
                                    .font(.system(size: 12))
                                    .foregroundColor(Color("TextSecondary"))
                            }
                        }
                        .padding(.vertical, 10)
                        .accessibilityIdentifier("serverSkillSwitch")
                    }
                    if let error = store.saveError {
                        Text(error)
                            .font(.system(size: 13))
                            .foregroundColor(Theme.statusError)
                            .padding(.top, 6)
                    }
                }
                .padding(.bottom, 24)
            }
        }
        .task { store.loadIfNeeded() }
    }
}
