import SwiftUI
import UIKit

/// Full-screen search over chats, code threads, ACI runs, projects and bots
/// (the drawer's round search button; allternit-ai phone-nav.ts `searchOpen`).
/// Matches titles locally against what the drawer already loaded.
struct ShellSearchView: View {
    let sessions: [AgentSession]
    let onOpenSession: (AgentSession) -> Void
    let onOpenProject: (CoworkProject) -> Void
    let onOpenBot: (AgentRecord) -> Void

    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @StateObject private var projectStore = ProjectStore.shared
    @State private var query = ""
    @FocusState private var focused: Bool

    private var needle: String {
        query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    }

    private var matchedSessions: [AgentSession] {
        guard !needle.isEmpty else { return [] }
        return sessions.filter { session in
            (session.name ?? "").lowercased().contains(needle)
                || (session.description ?? "").lowercased().contains(needle)
        }
        .prefix(40)
        .map { $0 }
    }

    private var matchedProjects: [CoworkProject] {
        guard !needle.isEmpty else { return [] }
        return projectStore.projects.filter { $0.title.lowercased().contains(needle) }
    }

    private var matchedBots: [AgentRecord] {
        guard !needle.isEmpty else { return [] }
        return agentModeStore.agents.filter { $0.name.lowercased().contains(needle) }
    }

    var body: some View {
        NavigationStack {
            List {
                if needle.isEmpty {
                    Text("Search chats, code threads, projects and bots.")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                        .listRowBackground(Color.clear)
                } else if matchedSessions.isEmpty && matchedProjects.isEmpty && matchedBots.isEmpty {
                    FriendlyInlineStateView(
                        style: .empty,
                        icon: "magnifyingglass",
                        title: "No results",
                        message: "Nothing matches “\(query)”."
                    )
                    .frame(maxWidth: .infinity)
                    .listRowBackground(Color.clear)
                } else {
                    if !matchedSessions.isEmpty {
                        Section("Chats & threads") {
                            ForEach(matchedSessions) { session in
                                row(icon: Self.icon(for: session), title: session.name ?? "Untitled", subtitle: Self.surfaceLabel(for: session)) {
                                    dismiss()
                                    onOpenSession(session)
                                }
                            }
                        }
                    }
                    if !matchedProjects.isEmpty {
                        Section("Projects") {
                            ForEach(matchedProjects) { project in
                                row(icon: "folder", title: project.title, subtitle: nil) {
                                    dismiss()
                                    onOpenProject(project)
                                }
                            }
                        }
                    }
                    if !matchedBots.isEmpty {
                        Section("Bots") {
                            ForEach(matchedBots) { bot in
                                row(icon: "cpu", title: bot.name, subtitle: nil) {
                                    dismiss()
                                    onOpenBot(bot)
                                }
                            }
                        }
                    }
                }
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary").ignoresSafeArea())
            .scrollDismissesKeyboard(.interactively)
            .safeAreaInset(edge: .top) {
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass")
                        .foregroundColor(Color("TextSecondary"))
                    TextField("Search", text: $query)
                        .focused($focused)
                        .submitLabel(.search)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("shellSearchField")
                    if !query.isEmpty {
                        Button { query = "" } label: {
                            Image(systemName: "xmark.circle.fill").foregroundColor(Color("TextSecondary"))
                        }
                        .accessibilityLabel("Clear search")
                    }
                }
                .padding(.horizontal, 12)
                .frame(height: 40)
                .background(Capsule().fill(Color("BgPanel")))
                .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
                .background(Color("BgPrimary"))
            }
            .navigationTitle("Search")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Close") { dismiss() }
                }
            }
            .onAppear {
                focused = true
                projectStore.fetchProjectsIfNeeded()
                agentModeStore.fetchAgentsIfNeeded()
            }
        }
    }

    private func row(icon: String, title: String, subtitle: String?, action: @escaping () -> Void) -> some View {
        Button {
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
            action()
        } label: {
            HStack(spacing: 12) {
                Image(systemName: icon)
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 22)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    if let subtitle {
                        Text(subtitle)
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
                Spacer(minLength: 0)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .listRowBackground(Color("BgPanel"))
    }

    static func icon(for session: AgentSession) -> String {
        if session.agentId != nil { return "cpu" }
        switch session.originSurface ?? "" {
        case "code": return "terminal"
        case "browser": return "globe"
        default: return "bubble.left"
        }
    }

    private static func surfaceLabel(for session: AgentSession) -> String {
        if session.agentId != nil { return "Bots" }
        switch session.originSurface ?? "" {
        case "code": return "Code"
        case "browser": return "ACI"
        default: return "Home"
        }
    }
}
