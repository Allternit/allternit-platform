import SwiftUI
import UIKit

/// The current mode's open sessions as a list, grid or deck of cards, full
/// screen (allternit-ai src/fabric-session/phone/screens/PhoneOverviewOverlay.tsx).
/// Opened from the chat header's count button or "See all" over Recents in
/// the drawer. Section-screen chrome: ☰ · title · filter, the List · Grid · Deck switch, floating
/// search + New. Bots have their own (a bot's Threads tab).
///
/// ⋯ ("Recents options", phone calm step 25, web ai #1151): Select chats /
/// Select threads, then the All · Pinned · Archived views. Select mode swaps
/// to checkbox rows (chats and code threads only), Cancel + Select all / Clear
/// up top and a glass bar of Archive (Unarchive) · Pin (Unpin) · Delete. The
/// rules are `reduceOverviewSelect` in OverviewSelect.swift.
struct SessionOverviewView: View {
    let mode: ShellMode
    let onOpen: (AgentSession) -> Void
    let onNew: () -> Void
    /// ☰ leaves the overview for the drawer.
    let onMenu: () -> Void

    typealias Layout = OverviewLayout

    @ObservedObject private var store = ShellSessionsStore.shared
    @ObservedObject private var projects = ProjectStore.shared
    @ObservedObject private var pins = RailPinsStore.shared
    @State private var select = INITIAL_OVERVIEW_SELECT
    @State private var barHidden = false
    @State private var optionsOpen = false
    @State private var layoutOpen = false
    @State private var seeded = false
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

    private var layout: Layout {
        // Select mode is always the list (checkbox rows).
        select.selecting ? .list : (Layout(stored: layoutRaw) ?? .grid)
    }

    /// ⋯ and select mode are for chats and code threads (web `PhoneChatMenu`).
    private var hasOptions: Bool { mode == .home || mode == .code }

    private func selectable(_ session: AgentSession) -> Bool { hasOptions && !session.isTerminal }

    private var selectLabel: String { mode == .code ? "Select threads" : "Select chats" }

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
        switch select.view {
        case .pinned: return "No pinned \(mode == .code ? "threads" : "chats") yet."
        case .archived: return "Nothing archived."
        case .all: break
        }
        switch mode {
        case .code: return "No code threads yet. Start one and it shows here."
        case .aci: return "No agent runs yet."
        case .home, .bots: return "No open chats. Start one and it shows here."
        }
    }

    /// Working sessions first, then the most recent (useModeOverview `order`).
    private var items: [AgentSession] {
        if select.view == .archived { return store.archivedSessions(for: mode) }
        let open = store.openSessions(for: mode).filter { select.view != .pinned || pins.isPinned($0.id) }
        return open.enumerated().sorted { a, b in
            let wa = store.isStreaming(a.element.id), wb = store.isStreaming(b.element.id)
            return wa != wb ? wa : a.offset < b.offset
        }.map(\.element)
    }

    /// Agent runs can't be closed or renamed here (web passes no onClose / onRename for ACI).
    private var editable: Bool { mode != .aci && !select.selecting && select.view != .archived }

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
            if select.selecting {
                selectHeader
            } else {
                // ☰ leaves the overview and opens the shell drawer.
                PhoneHeader(title: title, wideSides: groups.count > 1, sectionStyle: true, onMenu: onMenu) {
                    if groups.count > 1 { filterMenu }
                    if hasOptions { optionsButton } else { layoutButton }
                }
            }
            // The Bots home's List · Grid · Deck switch, on every mode's overview.
            HStack {
                Text("\(shown.count) \(select.view == .archived ? "ARCHIVED" : (select.view == .pinned ? "PINNED" : "OPEN"))")
                    .font(.system(size: 12, weight: .semibold))
                    .tracking(0.72)
                    .foregroundColor(Color("TextTertiary"))
                Spacer()
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 8)
            ZStack(alignment: .bottom) {
                content
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .animation(.spring(response: 0.35, dampingFraction: 0.86), value: shown.map(\.id))
                if select.selecting {
                    selectBar
                } else {
                    PhoneFloatingBar(
                        query: $query,
                        placeholder: "Search \(title.lowercased())",
                        newLabel: newLabel,
                        wide: .search,
                        onNew: onNew
                    )
                    .hidesOnScroll(barHidden)
                }
            }
            .tracksScrollForHide($barHidden)
        }
        .onChange(of: shown.map(\.id)) { _, ids in
            select = reduceOverviewSelect(select, .visible(ids))
            seedForLaunchArguments()
        }
        .onAppear {
            #if DEBUG
            if launchArgumentEnabled(LaunchArgument.calmOverviewMenu) { optionsOpen = true }
            seedForLaunchArguments()
            #endif
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
            PhoneFilterButtonLabel(active: group != nil)
        }
        .accessibilityLabel("Filter")
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
            VStack(spacing: 8) {
                EmptyArt(kind: .search, size: .panel)
                Text(query.isEmpty ? emptyText : "Nothing matches “\(query)”.")
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextSecondary"))
                    .multilineTextAlignment(.center)
            }
            .padding(.horizontal, 32)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            switch layout {
            case .list:
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(shown) { session in
                            listRow(session)
                                .onTapGesture { if editingId != session.id { tap(session) } }
                                .transition(.opacity.combined(with: .move(edge: .top)))
                        }
                    }
                    .padding(.top, 4)
                    .padding(.bottom, 96)
                    .reportsScrollOffset()
                }
            case .grid:
                ScrollView {
                    LazyVGrid(columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible(), spacing: 12)], spacing: 12) {
                        ForEach(shown) { session in
                            card(session, tall: false)
                                .onTapGesture { if editingId != session.id { tap(session) } }
                                .transition(.opacity.combined(with: .scale(scale: 0.96)))
                        }
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 4)
                    .padding(.bottom, 96)
                    .reportsScrollOffset()
                }
            case .deck:
                ScrollView(.horizontal, showsIndicators: false) {
                    LazyHStack(spacing: 12) {
                        ForEach(shown) { session in
                            card(session, tall: true)
                                .frame(width: OverviewDeck.width, height: OverviewDeck.height)
                                .onTapGesture { if editingId != session.id { tap(session) } }
                                .offset(y: swipeId == session.id ? swipeOffset : 0)
                                .opacity(swipeId == session.id ? Double(1 + swipeOffset / 400) : 1)
                                .simultaneousGesture(swipeUpToClose(session))
                        }
                    }
                    .scrollTargetLayout()
                    .padding(.horizontal, 24)
                    .padding(.top, 8)
                    .padding(.bottom, 96)
                }
                .scrollTargetBehavior(.viewAligned)
            }
        }
    }

    private func listRow(_ session: AgentSession) -> some View {
        HStack(spacing: 14) {
            if select.selecting { checkbox(session) }
            Image(systemName: ShellSearchView.icon(for: session))
                .font(.system(size: 17))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 22)
            VStack(alignment: .leading, spacing: 1) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(session.displayTitle)
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
                placeBadge(session)
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
        .opacity(select.selecting && !selectable(session) ? 0.45 : 1)
        .accessibilityLabel(accessibilityTitle(session))
        .accessibilityValue(select.selecting && selectable(session) ? (select.selected.contains(session.id) ? "Selected" : "Not selected") : "")
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
            placeBadge(session)
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

    // MARK: - Select mode (OverviewSelect.swift)

    private var optionsButton: some View {
        Button {
            Haptics.light()
            optionsOpen = true
        } label: {
            PhoneFilterButtonLabel(active: select.view != .all, systemName: "ellipsis")
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Recents options")
        .accessibilityIdentifier("recentsOptions")
        .popover(isPresented: $optionsOpen) {
            optionsList.presentationCompactAdaptation(.popover)
        }
    }

    /// Modes with no select/archive options still get the layout switch,
    /// in a one-row popover behind a sliders button.
    private var layoutButton: some View {
        Button {
            Haptics.light()
            layoutOpen = true
        } label: {
            PhoneFilterButtonLabel(active: false, systemName: "slider.horizontal.3")
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Layout")
        .accessibilityIdentifier("overviewLayout")
        .popover(isPresented: $layoutOpen) {
            OverviewLayoutSwitch(raw: $layoutRaw, fallback: .grid)
                .padding(12)
                .presentationCompactAdaptation(.popover)
        }
    }

    private var optionsList: some View {
        VStack(alignment: .leading, spacing: 0) {
            OverviewLayoutSwitch(raw: $layoutRaw, fallback: .grid)
                .padding(.horizontal, 8)
                .padding(.bottom, 4)
            Divider().padding(.vertical, 4)
            optionRow(selectLabel, icon: "checkmark.circle", on: false, disabled: shown.isEmpty) {
                optionsOpen = false
                select = reduceOverviewSelect(select, .start)
            }
            Divider().padding(.vertical, 4)
            ForEach(OverviewView.allCases, id: \.self) { view in
                optionRow(view.label, icon: nil, on: select.view == view, disabled: false) {
                    optionsOpen = false
                    select = reduceOverviewSelect(select, .view(view))
                }
            }
        }
        .padding(8)
        .frame(width: 230)
    }

    private func optionRow(_ text: String, icon: String?, on: Bool, disabled: Bool, action: @escaping () -> Void) -> some View {
        Button {
            Haptics.selection()
            action()
        } label: {
            HStack(spacing: 10) {
                if let icon { Image(systemName: icon).frame(width: 20) }
                Text(text)
                Spacer(minLength: 0)
                if on { Image(systemName: "checkmark") }
            }
            .font(.system(size: 16))
            .foregroundColor(Color("TextPrimary"))
            .frame(minHeight: 44)
            .padding(.horizontal, 8)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.4 : 1)
    }

    /// Cancel · "N selected" · Select all / Clear.
    private var selectHeader: some View {
        let ids = shown.filter(selectable).map(\.id)
        let all = !ids.isEmpty && ids.allSatisfy { select.selected.contains($0) }
        return HStack {
            Button("Cancel") {
                Haptics.light()
                select = reduceOverviewSelect(select, .cancel)
            }
            .frame(minWidth: 72, minHeight: 44, alignment: .leading)
            Spacer()
            Text(select.selected.isEmpty ? selectLabel : "\(select.selected.count) selected")
                .font(.system(size: 17, weight: .semibold))
                .accessibilityAddTraits(.isHeader)
            Spacer()
            Button(all ? "Clear" : "Select all") {
                Haptics.selection()
                select = reduceOverviewSelect(select, .selectAll(ids))
            }
            .frame(minWidth: 72, minHeight: 44, alignment: .trailing)
        }
        .font(.system(size: 16))
        .foregroundColor(Color("TextPrimary"))
        .frame(height: 52)
        .padding(.horizontal, 16)
        .background(Color("BgPrimary"))
    }

    private func checkbox(_ session: AgentSession) -> some View {
        let on = select.selected.contains(session.id)
        return Image(systemName: on ? "checkmark.circle.fill" : "circle")
            .font(.system(size: 22))
            .foregroundColor(on ? Color("AccentPrimary") : Color("TextTertiary"))
            .opacity(selectable(session) ? 1 : 0)
            .frame(width: 26)
            .accessibilityHidden(true)
    }

    /// Glass bottom bar: Archive (Unarchive) · Pin (Unpin) · Delete.
    private var selectBar: some View {
        let none = select.selected.isEmpty
        let pinAction = bulkPinAction(select.selected, pinnedIds: Set(pins.pins.map(\.id)))
        let archivedView = select.view == .archived
        return GlassContainer(spacing: 8) {
            HStack(spacing: 4) {
                barButton(archivedView ? "Unarchive" : "Archive", icon: archivedView ? "tray.and.arrow.up" : "archivebox", disabled: none) { bulkArchive() }
                barButton(pinAction == .pin ? "Pin" : "Unpin", icon: pinAction == .pin ? "pin" : "pin.slash", disabled: none) { bulkPin(pinAction) }
                holdDeleteButton(count: select.selected.count, disabled: none)
            }
            .padding(6)
            .glassSurface(cornerRadius: 28)
        }
        .padding(.horizontal, 16)
        .padding(.bottom, 12)
        .accessibilityIdentifier("overviewSelectBar")
    }

    /// Delete is press and hold, never a tap or a dialog (Allternit rule).
    private func holdDeleteButton(count: Int, disabled: Bool) -> some View {
        VStack(spacing: 2) {
            Image(systemName: "trash").font(.system(size: 18))
            Text("Hold to delete").font(.system(size: 11, weight: .medium))
        }
        .foregroundColor(Theme.statusError)
        .frame(maxWidth: .infinity, minHeight: 48)
        .contentShape(Rectangle())
        .onLongPressGesture(minimumDuration: 1.0) {
            guard !disabled else { return }
            UINotificationFeedbackGenerator().notificationOccurred(.success)
            bulkDelete()
        }
        .disabled(disabled)
        .opacity(disabled ? 0.4 : 1)
        .accessibilityLabel("Hold to delete \(count) \(count == 1 ? "item" : "items")")
        .accessibilityAddTraits(.isButton)
        .accessibilityAction { bulkDelete() }
    }

    private func barButton(_ text: String, icon: String, disabled: Bool, destructive: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(spacing: 2) {
                Image(systemName: icon).font(.system(size: 18))
                Text(text).font(.system(size: 11, weight: .medium))
            }
            .foregroundColor(destructive ? Theme.statusError : Color("TextPrimary"))
            .frame(maxWidth: .infinity, minHeight: 48)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.4 : 1)
    }

    private func tap(_ session: AgentSession) {
        if select.selecting {
            guard selectable(session) else { return }
            Haptics.selection()
            select = reduceOverviewSelect(select, .toggle(session.id))
        } else {
            open(session)
        }
    }

    /// Run `work` for each ticked row; the first failure is shown, then select mode ends.
    private func bulk(_ work: @escaping (String) async throws -> Void) {
        let ids = select.selected
        select = reduceOverviewSelect(select, .done)
        Task {
            var failure: Error?
            for id in ids {
                do { try await work(id) } catch { failure = failure ?? error }
            }
            if let failure { actionError = FriendlyErrorMessage.from(failure.localizedDescription) }
        }
    }

    private func bulkArchive() {
        Haptics.light()
        let unarchive = select.view == .archived
        bulk { id in
            if unarchive { try await store.unarchive(id: id) } else { try await store.archive(id: id) }
        }
    }

    private func bulkPin(_ action: BulkPinAction) {
        Haptics.selection()
        let kind = mode == .code ? "code" : "chat"
        for id in select.selected where pins.isPinned(id) == (action == .unpin) {
            pins.toggle(id: id, kind: kind, mode: kind)
        }
        select = reduceOverviewSelect(select, .done)
    }

    private func bulkDelete() {
        Haptics.warning()
        bulk { id in try await store.delete(id: id) }
    }

    /// `-calm-recents-select`: enter select mode with the first two rows ticked.
    private func seedForLaunchArguments() {
        #if DEBUG
        guard !seeded, launchArgumentEnabled(LaunchArgument.calmRecentsSelect) else { return }
        let ids = shown.filter(selectable).map(\.id)
        guard !ids.isEmpty else { return }
        seeded = true
        select = reduceOverviewSelect(reduceOverviewSelect(select, .start), .selectAll(Array(ids.prefix(2))))
        #endif
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
        let name = session.displayTitle
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
            Text(session.displayTitle)
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
            if ShellSessionsStore.belongs(session, to: .code) {
                let inCloud = store.cloudPlaces[session.id] != nil
                Button { teleport(session, toCloud: !inCloud) } label: {
                    Label(inCloud ? "Bring back to this Mac" : "Teleport to cloud", systemImage: inCloud ? "icloud.and.arrow.down" : "icloud.and.arrow.up")
                }
            }
            Button { close(session) } label: { Label("Archive", systemImage: "archivebox") }
        } label: {
            Image(systemName: "ellipsis")
                .font(.system(size: deck ? 15 : 13, weight: .bold))
                .foregroundColor(Color("TextTertiary"))
                .frame(width: 28, height: 28)
                .contentShape(Rectangle())
        }
        .accessibilityLabel("More for \(session.displayTitle)")
    }

    /// Teleport from ⋯: the session keeps its conversation and resumes on the other computer.
    private func teleport(_ session: AgentSession, toCloud: Bool) {
        Haptics.light()
        Task {
            do { try await store.teleport(id: session.id, toCloud: toCloud) }
            catch { actionError = FriendlyErrorMessage.from(error.localizedDescription) }
        }
    }

    /// Local / Cloud badge: where the session runs, same as the web session header.
    private func placeBadge(_ session: AgentSession) -> some View {
        let label = store.cloudPlaces[session.id]
        return HStack(spacing: 4) {
            Image(systemName: label == nil ? "laptopcomputer" : "cloud").font(.system(size: 10))
            Text(label == nil ? "Local" : "Cloud").font(.system(size: 11, weight: .medium))
        }
        .foregroundColor(Color("TextSecondary"))
        .accessibilityLabel(label.map { "Running on \($0)" } ?? "Running on this Mac")
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
        .accessibilityLabel("Archive \(session.displayTitle)")
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
        Haptics.light()
        Task {
            // archive() takes the card out at once and puts it back if the write fails.
            do { try await store.archive(id: session.id) }
            catch { actionError = FriendlyErrorMessage.from(error.localizedDescription) }
        }
    }

    private func open(_ session: AgentSession) {
        Haptics.light()
        onOpen(session)
    }
}
