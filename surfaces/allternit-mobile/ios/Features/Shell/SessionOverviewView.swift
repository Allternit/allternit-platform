import SwiftUI
import UIKit

/// The current mode's open sessions as a list, grid or deck of cards, full
/// screen (allternit-ai src/fabric-session/phone/screens/PhoneOverviewOverlay.tsx).
/// Opened from the chat header's count button or "See all" over Recents in
/// the drawer. Section-screen chrome: ☰ · title · layout/filter, floating
/// search + New. Bots have their own (a bot's Threads tab).
struct SessionOverviewView: View {
    let mode: ShellMode
    let onOpen: (AgentSession) -> Void
    let onNew: () -> Void
    /// ☰ leaves the overview for the drawer.
    let onMenu: () -> Void

    enum Layout: String, CaseIterable {
        case list = "List", grid = "Grid", deck = "Deck"

        var icon: String {
            switch self {
            case .list: return "list.bullet"
            case .grid: return "square.grid.2x2"
            case .deck: return "rectangle.split.3x1"
            }
        }
    }

    @ObservedObject private var store = ShellSessionsStore.shared
    @ObservedObject private var projects = ProjectStore.shared
    /// The chosen layout, kept across launches (useOverviewLayout).
    @AppStorage("allternit-phone-overview-layout") private var layoutRaw = Layout.grid.rawValue
    @State private var query = ""
    /// nil = all projects.
    @State private var group: String? = nil

    private var layout: Layout { Layout(rawValue: layoutRaw) ?? .grid }

    private var title: String {
        switch mode {
        case .code: return "Code"
        case .aci: return "Agent runs"
        case .home, .bots: return "Chats"
        }
    }

    private var newLabel: String {
        switch mode {
        case .code: return "New thread"
        case .aci: return "New session"
        case .home, .bots: return "New chat"
        }
    }

    private var emptyText: String {
        switch mode {
        case .code: return "No code threads yet. Start one and it shows here."
        case .aci: return "No agent runs yet."
        case .home, .bots: return "No open chats. Start one and it shows here."
        }
    }

    private var items: [AgentSession] { store.openSessions(for: mode) }

    /// Group label: the session's project ("No project" otherwise).
    private func groupLabel(_ session: AgentSession) -> String {
        guard let id = session.projectId, let project = projects.projects.first(where: { $0.id == id }) else {
            return "No project"
        }
        return project.title
    }

    private var groups: [(label: String, count: Int)] {
        var counts: [String: Int] = [:]
        var order: [String] = []
        for item in items {
            let label = groupLabel(item)
            if counts[label] == nil { order.append(label) }
            counts[label, default: 0] += 1
        }
        return order.map { ($0, counts[$0] ?? 0) }
    }

    private var shown: [AgentSession] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return items.filter { item in
            if let group, groupLabel(item) != group { return false }
            guard !needle.isEmpty else { return true }
            return (item.name ?? "").lowercased().contains(needle)
                || (item.description ?? "").lowercased().contains(needle)
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            PhoneSectionHeader(title: title, onMenu: onMenu) {
                filterMenu
            }
            ZStack(alignment: .bottom) {
                content
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                PhoneFloatingBar(
                    query: $query,
                    placeholder: "Search \(title.lowercased())",
                    newLabel: newLabel,
                    wide: .search,
                    onNew: onNew
                )
            }
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .accessibilityIdentifier("phoneOverview")
    }

    private var filterMenu: some View {
        let groupNoun = mode == .code ? "repos" : "projects"
        return Menu {
            Section("Layout") {
                ForEach(Layout.allCases, id: \.self) { option in
                    Button {
                        layoutRaw = option.rawValue
                    } label: {
                        Label(option.rawValue, systemImage: layout == option ? "checkmark" : option.icon)
                    }
                }
            }
            if groups.count > 1 {
                Section("Show") {
                    Button {
                        group = nil
                    } label: {
                        checkLabel("All \(groupNoun) (\(items.count))", on: group == nil)
                    }
                    ForEach(groups, id: \.label) { entry in
                        Button {
                            group = entry.label
                        } label: {
                            checkLabel("\(entry.label) (\(entry.count))", on: group == entry.label)
                        }
                    }
                }
            }
        } label: {
            PhoneFilterButtonLabel(active: group != nil, systemName: layout.icon)
        }
        .accessibilityLabel("Layout: \(layout.rawValue). Change layout, filter and sort")
    }

    @ViewBuilder
    private func checkLabel(_ text: String, on: Bool) -> some View {
        if on { Label(text, systemImage: "checkmark") } else { Text(text) }
    }

    @ViewBuilder
    private var content: some View {
        if !store.hasLoaded && store.isLoading {
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if shown.isEmpty {
            Text(query.isEmpty ? emptyText : "Nothing matches “\(query)”.")
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
                .multilineTextAlignment(.center)
                .padding(.horizontal, 32)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            switch layout {
            case .list:
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(shown) { session in
                            Button { open(session) } label: { listRow(session) }
                                .buttonStyle(.plain)
                        }
                    }
                    .padding(.top, 4)
                    .padding(.bottom, 96)
                }
            case .grid:
                ScrollView {
                    LazyVGrid(columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible(), spacing: 12)], spacing: 12) {
                        ForEach(shown) { session in
                            Button { open(session) } label: { card(session, tall: false) }
                                .buttonStyle(.plain)
                        }
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 4)
                    .padding(.bottom, 96)
                }
            case .deck:
                TabView {
                    ForEach(shown) { session in
                        Button { open(session) } label: { card(session, tall: true) }
                            .buttonStyle(.plain)
                            .padding(.horizontal, 28)
                            .padding(.top, 8)
                            .padding(.bottom, 96)
                    }
                }
                .tabViewStyle(.page(indexDisplayMode: .never))
            }
        }
    }

    private func listRow(_ session: AgentSession) -> some View {
        HStack(spacing: 14) {
            Image(systemName: ShellSearchView.icon(for: session))
                .font(.system(size: 17))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 22)
            VStack(alignment: .leading, spacing: 1) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(session.name ?? "Untitled")
                        .font(.system(size: 16.5))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Spacer(minLength: 0)
                    Text(ShellSessionsStore.relativeTime(ShellSessionsStore.timestamp(of: session)))
                        .font(.system(size: 12))
                        .monospacedDigit()
                        .foregroundColor(Color("TextSecondary"))
                }
                if let preview = session.description, !preview.isEmpty {
                    Text(preview)
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
            }
        }
        .padding(.horizontal, 20)
        .frame(minHeight: 52)
        .contentShape(Rectangle())
    }

    private func card(_ session: AgentSession, tall: Bool) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: ShellSearchView.icon(for: session))
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                Text(groupLabel(session))
                    .font(.system(size: 11, weight: .medium))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                Spacer(minLength: 0)
                Text(ShellSessionsStore.relativeTime(ShellSessionsStore.timestamp(of: session)))
                    .font(.system(size: 11))
                    .monospacedDigit()
                    .foregroundColor(Color("TextSecondary"))
            }
            Text(session.name ?? "Untitled")
                .font(.system(size: 15, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(2)
                .multilineTextAlignment(.leading)
            if let preview = session.description, !preview.isEmpty {
                Text(preview)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(tall ? 12 : 4)
                    .multilineTextAlignment(.leading)
            }
            Spacer(minLength: 0)
            Text("\(session.messageCount) message\(session.messageCount == 1 ? "" : "s")")
                .font(.system(size: 11))
                .foregroundColor(Color("TextTertiary"))
        }
        .padding(12)
        .frame(maxWidth: .infinity, minHeight: tall ? nil : 150, maxHeight: tall ? .infinity : nil, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("BgPanel")))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .contentShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        .accessibilityElement(children: .combine)
    }

    private func open(_ session: AgentSession) {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        onOpen(session)
    }
}
