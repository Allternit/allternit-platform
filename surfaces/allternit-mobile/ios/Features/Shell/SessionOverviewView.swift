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
    /// The card whose title is being renamed in place (⋯ → Rename).
    @State private var editingId: String? = nil
    @State private var draft = ""
    @FocusState private var renameFocused: Bool
    @State private var actionError: String? = nil
    /// Deck card being swiped up to close.
    @State private var swipeId: String? = nil
    @State private var swipeOffset: CGFloat = 0

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

    /// Working sessions first, then the most recent (useModeOverview `order`).
    private var items: [AgentSession] {
        store.openSessions(for: mode).enumerated().sorted { a, b in
            let wa = store.isStreaming(a.element.id), wb = store.isStreaming(b.element.id)
            return wa != wb ? wa : a.offset < b.offset
        }.map(\.element)
    }

    /// Agent runs can't be closed or renamed here (web passes no onClose / onRename for ACI).
    private var editable: Bool { mode != .aci }

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
                    .animation(.spring(response: 0.35, dampingFraction: 0.86), value: shown.map(\.id))
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
        .alert("Couldn't update the chat", isPresented: Binding(get: { actionError != nil }, set: { if !$0 { actionError = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(actionError ?? "")
        }
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
                            listRow(session)
                                .onTapGesture { if editingId != session.id { open(session) } }
                                .transition(.opacity.combined(with: .move(edge: .top)))
                        }
                    }
                    .padding(.top, 4)
                    .padding(.bottom, 96)
                }
            case .grid:
                ScrollView {
                    LazyVGrid(columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible(), spacing: 12)], spacing: 12) {
                        ForEach(shown) { session in
                            card(session, tall: false)
                                .onTapGesture { if editingId != session.id { open(session) } }
                                .transition(.opacity.combined(with: .scale(scale: 0.96)))
                        }
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 4)
                    .padding(.bottom, 96)
                }
            case .deck:
                TabView {
                    ForEach(shown) { session in
                        card(session, tall: true)
                            .onTapGesture { if editingId != session.id { open(session) } }
                            .offset(y: swipeId == session.id ? swipeOffset : 0)
                            .opacity(swipeId == session.id ? Double(1 + swipeOffset / 400) : 1)
                            .simultaneousGesture(swipeUpToClose(session))
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
                statusPill(session)
            }
            if editable {
                // List rows scroll up and down, so they close by the X.
                HStack(spacing: 0) {
                    moreMenu(session, deck: false)
                    closeButton(session, deck: false)
                }
                .padding(.trailing, -4)
            }
        }
        .padding(.horizontal, 20)
        .frame(minHeight: 60)
        .contentShape(Rectangle())
        .accessibilityElement(children: .contain)
        .accessibilityLabel(accessibilityTitle(session))
        .accessibilityAddTraits(.isButton)
    }

    private func card(_ session: AgentSession, tall: Bool) -> some View {
        let working = store.isStreaming(session.id)
        return VStack(alignment: .leading, spacing: 6) {
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
            // The name stays at the top, with ⋯ and the X beside it.
            HStack(alignment: .top, spacing: 4) {
                titleView(session, size: tall ? 15.5 : 15, weight: .semibold, lines: 2)
                if editable {
                    moreMenu(session, deck: tall)
                    closeButton(session, deck: tall)
                }
            }
            statusPill(session)
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
        .overlay(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .stroke(working ? Self.coral : Color("BorderSubtle"), lineWidth: 1)
        )
        // A working card gets a soft coral halo (3px at 14%).
        .background(
            RoundedRectangle(cornerRadius: 19, style: .continuous)
                .stroke(Self.coral.opacity(working && !tall ? 0.14 : 0), lineWidth: 3)
                .padding(-1.5)
        )
        .shadow(color: .black.opacity(tall ? 0.10 : 0), radius: 15, y: 10)
        .contentShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        .accessibilityElement(children: .contain)
        .accessibilityLabel(accessibilityTitle(session))
        .accessibilityAddTraits(.isButton)
    }

    // MARK: - Status, rename, close (SessionOverview OverviewCard)

    /// The web's working accent (CORAL #D97757).
    static let coral = Color(red: 0xD9 / 255, green: 0x77 / 255, blue: 0x57 / 255)

    /// OverviewStatusPill: "Working" (coral, pulsing dot) or "New reply".
    @ViewBuilder
    private func statusPill(_ session: AgentSession) -> some View {
        if store.isStreaming(session.id) {
            HStack(spacing: 4) {
                Circle().fill(Self.coral).frame(width: 6, height: 6)
                    .phaseAnimator([1.0, 0.35]) { dot, phase in dot.opacity(phase) } animation: { _ in .easeInOut(duration: 0.9) }
                Text("Working")
            }
            .font(.system(size: 10.5, weight: .semibold))
            .foregroundColor(Self.coral)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Capsule().fill(Self.coral.opacity(0.13)))
        } else if store.isUnread(session) {
            Text("New reply")
                .font(.system(size: 10.5, weight: .semibold))
                .foregroundColor(Color("TextTertiary"))
                .padding(.vertical, 2)
        }
    }

    private func accessibilityTitle(_ session: AgentSession) -> String {
        let name = session.name ?? "Untitled"
        if store.isStreaming(session.id) { return "\(name), Working" }
        if store.isUnread(session) { return "\(name), New reply" }
        return name
    }

    /// The title, or the rename field in its place.
    @ViewBuilder
    private func titleView(_ session: AgentSession, size: CGFloat, weight: Font.Weight, lines: Int) -> some View {
        if editingId == session.id {
            TextField("Name", text: $draft)
                .font(.system(size: size, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .textFieldStyle(.plain)
                .submitLabel(.done)
                .focused($renameFocused)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(RoundedRectangle(cornerRadius: 6).fill(Color("BgPrimary")))
                .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color("AccentPrimary"), lineWidth: 1))
                .onChange(of: draft) { _, value in if value.count > 120 { draft = String(value.prefix(120)) } }
                .onSubmit { commitRename(session) }
                .onChange(of: renameFocused) { _, focused in if !focused, editingId == session.id { commitRename(session) } }
                .accessibilityLabel("Rename")
                .frame(maxWidth: .infinity)
        } else {
            Text(session.name ?? "Untitled")
                .font(.system(size: size, weight: weight))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(lines)
                .multilineTextAlignment(.leading)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    /// ⋯: Open, Rename, Archive (a tap opens it; no long-press menus).
    private func moreMenu(_ session: AgentSession, deck: Bool) -> some View {
        Menu {
            Button { open(session) } label: { Label("Open", systemImage: "arrow.up.right.square") }
            Button { startRename(session) } label: { Label("Rename", systemImage: "pencil") }
            Button { close(session) } label: { Label("Archive", systemImage: "archivebox") }
        } label: {
            Image(systemName: "ellipsis")
                .font(.system(size: deck ? 15 : 13, weight: .bold))
                .foregroundColor(Color("TextTertiary"))
                .frame(width: 28, height: 28)
                .contentShape(Rectangle())
        }
        .accessibilityLabel("More for \(session.name ?? "Untitled")")
    }

    /// The X: archives the session.
    private func closeButton(_ session: AgentSession, deck: Bool) -> some View {
        Button { close(session) } label: {
            Image(systemName: "xmark")
                .font(.system(size: deck ? 13 : 11, weight: .medium))
                .foregroundColor(Color("TextTertiary"))
                .frame(width: 28, height: 28)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Archive \(session.name ?? "Untitled")")
    }

    /// Phones: swipe a deck card up to close it.
    private func swipeUpToClose(_ session: AgentSession) -> some Gesture {
        DragGesture(minimumDistance: 16)
            .onChanged { value in
                guard editable, editingId == nil, abs(value.translation.height) > abs(value.translation.width) else { return }
                swipeId = session.id
                swipeOffset = min(0, value.translation.height) + max(0, value.translation.height) * 0.05
            }
            .onEnded { value in
                defer { withAnimation(.spring(response: 0.3, dampingFraction: 0.85)) { swipeOffset = 0; swipeId = nil } }
                guard swipeId == session.id, value.translation.height < -90 else { return }
                close(session)
            }
    }

    private func startRename(_ session: AgentSession) {
        draft = session.name ?? ""
        editingId = session.id
        // After the menu has closed, or its dismissal takes focus back.
        DispatchQueue.main.async { renameFocused = true }
    }

    private func commitRename(_ session: AgentSession) {
        guard editingId == session.id else { return }
        let next = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        editingId = nil
        renameFocused = false
        guard !next.isEmpty, next != (session.name ?? "") else { return }
        Task {
            do { try await store.rename(id: session.id, to: next) }
            catch { actionError = FriendlyErrorMessage.from(error.localizedDescription) }
        }
    }

    private func close(_ session: AgentSession) {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        Task {
            // archive() takes the card out at once and puts it back if the write fails.
            do { try await store.archive(id: session.id) }
            catch { actionError = FriendlyErrorMessage.from(error.localizedDescription) }
        }
    }

    private func open(_ session: AgentSession) {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        onOpen(session)
    }
}
