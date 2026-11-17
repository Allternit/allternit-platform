import SwiftUI

/// What the Code overview shows: threads, or terminal sessions while the
/// terminal is on screen (code-overview-kind.tsx).
enum CodeOverviewKind: String, Identifiable {
    case threads, terminals
    var id: String { rawValue }
}

/// The Code header's sessions button: a task list, opening the sessions
/// overview (open threads, and the way to Terminal and Shell).
struct CodeSessionsButton: View {
    var tint: Color = Color("TextPrimary")
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: "checklist")
                .font(.system(size: 18))
                .foregroundColor(tint)
                .frame(width: 44, height: 44)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Code sessions")
    }
}

/// The Code sessions overview (PhoneOverviewOverlay / SessionOverview on the
/// phone): ☰ · "Code sessions" · "N open", the layout switch under it;
/// every open session as a card, and a floating
/// "Search code" pill with New at the bottom. Closing a card archives the
/// session.
struct CodeOverviewSheet: View {
    let kind: CodeOverviewKind
    let sessions: [AgentSession]
    @ObservedObject var terminals: CodeTerminalStore
    let onOpenSession: (AgentSession) -> Void
    let onNew: () -> Void
    let onArchive: (AgentSession) -> Void
    let onOpenMenu: () -> Void

    @Environment(\.dismiss) private var dismiss
    @AppStorage("allternit.code.overview-layout") private var layout = "grid"
    @State private var query = ""
    @State private var previews: [String: [MessageRecordPreview]] = [:]
    @FocusState private var searchFocused: Bool

    private var filtered: [AgentSession] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return sessions }
        return sessions.filter { ($0.name ?? "").lowercased().contains(q) }
    }

    private var openCount: Int { kind == .terminals ? terminals.tiles.count : sessions.count }

    var body: some View {
        VStack(spacing: 0) {
            header
            if kind == .threads {
                HStack {
                    Spacer()
                    OverviewLayoutSwitch(raw: $layout, fallback: .grid)
                }
                .padding(.horizontal, 12)
                .padding(.bottom, 4)
            }
            if kind == .threads, layout == "deck", !filtered.isEmpty {
                // Deck (SessionOverview deck, phone): tall 226-wide cards the
                // full height, in a sideways row that snaps card by card.
                GeometryReader { geo in
                    ScrollView(.horizontal, showsIndicators: false) {
                        LazyHStack(spacing: 12) {
                            ForEach(filtered) { card($0).frame(width: OverviewDeck.width, height: max(240, geo.size.height - 104)) }
                        }
                        .padding(.horizontal, 24)
                        .padding(.top, 8)
                        .scrollTargetLayout()
                    }
                    .scrollTargetBehavior(.viewAligned)
                }
            } else {
            ScrollView {
                if kind == .terminals {
                    terminalCards
                } else if filtered.isEmpty {
                    Text(query.isEmpty ? "No code threads yet. Start one and it shows here." : "No code threads match.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("BrandAccent"))
                        .frame(maxWidth: .infinity)
                        .padding(.top, 40)
                        .padding(.horizontal, 24)
                } else if layout == "grid" {
                    LazyVGrid(columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible(), spacing: 12)], spacing: 12) {
                        ForEach(filtered) { card($0) }
                    }
                    .padding(.horizontal, 12)
                    .padding(.top, 8)
                    .padding(.bottom, 96)
                } else {
                    LazyVStack(spacing: 8) {
                        ForEach(filtered) { card($0) }
                    }
                    .padding(.horizontal, 12)
                    .padding(.top, 8)
                    .padding(.bottom, 96)
                }
            }
            }
        }
        .background(Color("BgPrimary"))
        .overlay(alignment: .bottom) {
            if kind == .threads { bottomBar }
        }
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 10) {
            roundButton("line.3.horizontal", label: "Open menu") {
                dismiss()
                onOpenMenu()
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(kind == .terminals ? "Terminal sessions" : "Code sessions")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text("\(openCount) open")
                    .font(.system(size: 12))
                    .foregroundColor(Color("BrandAccent"))
            }
            .padding(.top, 4)
            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.top, 12)
        .padding(.bottom, 8)
    }

    private func roundButton(_ icon: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(.system(size: 15, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
                .frame(width: 36, height: 36)
                .background(Color("BgTertiary"))
                .clipShape(Circle())
                .frame(width: 44, height: 44)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }

    /// One session card: title, ⋯ menu, close (archive), the last exchange,
    /// and its age.
    private func card(_ session: AgentSession) -> some View {
        let title = session.name ?? "Untitled Session"
        let tall = layout == "deck"
        return Button(action: {
            dismiss()
            onOpenSession(session)
        }) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 4) {
                    Text(title)
                        .font(.system(size: tall ? 15.5 : 13, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    Menu {
                        Button(action: {
                            dismiss()
                            onOpenSession(session)
                        }) { Label("Open", systemImage: "arrow.up.right") }
                        Button(role: .destructive, action: { onArchive(session) }) { Label("Archive", systemImage: "archivebox") }
                    } label: {
                        Image(systemName: "ellipsis")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextTertiary"))
                            .frame(width: 28, height: 28)
                    }
                    .accessibilityLabel("More actions for \(title)")
                    Button(action: { onArchive(session) }) {
                        Image(systemName: "xmark")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundColor(Color("TextTertiary"))
                            .frame(width: 28, height: 28)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Close \(title)")
                }
                preview(for: session)
                    .frame(maxWidth: .infinity, minHeight: layout == "list" ? 0 : 100, maxHeight: tall ? .infinity : nil, alignment: .topLeading)
                HStack {
                    Spacer()
                    Text(CodeSessionDates.age(session))
                        .font(.system(size: 10))
                        .foregroundColor(Color("BrandAccent"))
                }
            }
            .padding(tall ? 14 : 10)
            .frame(maxHeight: tall ? .infinity : nil, alignment: .top)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: 14))
            .overlay(RoundedRectangle(cornerRadius: 14).stroke(Color("BorderSubtle"), lineWidth: 1))
            // Deck cards lift off the page (0 10px 30px rgba(0,0,0,.10)).
            .shadow(color: .black.opacity(tall ? 0.10 : 0), radius: 15, y: 10)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .task(id: session.id) { await loadPreview(session) }
    }

    @ViewBuilder
    private func preview(for session: AgentSession) -> some View {
        let lines = previews[session.id] ?? []
        VStack(alignment: .leading, spacing: 6) {
            ForEach(lines) { line in
                if line.isUser {
                    HStack {
                        Spacer(minLength: 16)
                        Text(line.text)
                            .font(.system(size: layout == "deck" ? 12 : 10))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(2)
                            .padding(.horizontal, 8)
                            .padding(.vertical, 5)
                            .background(Color("BgTertiary"))
                            .clipShape(RoundedRectangle(cornerRadius: 10))
                    }
                } else {
                    Text(line.text)
                        .font(.system(size: layout == "deck" ? 13 : 10))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(layout == "list" ? 2 : (layout == "deck" ? nil : 5))
                }
            }
        }
        .mask(LinearGradient(colors: [.black, .black, .black.opacity(0.2)], startPoint: .top, endPoint: .bottom))
    }

    private var terminalCards: some View {
        LazyVGrid(columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible(), spacing: 12)], spacing: 12) {
            ForEach(terminals.tiles) { tile in
                Button(action: {
                    terminals.selectedId = tile.id
                    dismiss()
                }) {
                    VStack(alignment: .leading, spacing: 8) {
                        HStack {
                            Text(tile.name)
                                .font(.system(size: 13, weight: .semibold, design: .monospaced))
                                .foregroundColor(TerminalPalette.foreground)
                            Spacer()
                            Button(action: { terminals.closeTile(tile.id) }) {
                                Image(systemName: "xmark")
                                    .font(.system(size: 11, weight: .semibold))
                                    .foregroundColor(TerminalPalette.secondary)
                                    .frame(width: 28, height: 28)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Close \(tile.name)")
                        }
                        Text("$ █")
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundColor(TerminalPalette.secondary)
                        Spacer(minLength: 40)
                    }
                    .padding(10)
                    .background(TerminalPalette.background)
                    .clipShape(RoundedRectangle(cornerRadius: 14))
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
    }

    private var bottomBar: some View {
        HStack(spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextSecondary"))
                TextField("Search code", text: $query)
                    .font(.system(size: 16))
                    .focused($searchFocused)
                    .submitLabel(.search)
            }
            .padding(.horizontal, 16)
            .frame(height: 48)
            .background(Color("BgPanel"))
            .clipShape(Capsule())
            .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
            .shadow(color: .black.opacity(0.08), radius: 12, y: 4)
            Button(action: {
                dismiss()
                onNew()
            }) {
                Image(systemName: "plus")
                    .font(.system(size: 18, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 48, height: 48)
                    .background(Color("BgPanel"))
                    .clipShape(Circle())
                    .overlay(Circle().stroke(Color("BorderSubtle"), lineWidth: 1))
                    .shadow(color: .black.opacity(0.08), radius: 12, y: 4)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("New code thread")
        }
        .padding(.horizontal, 14)
        .padding(.bottom, 12)
    }

    /// The card's last exchange: the newest user message and the reply after it.
    @MainActor
    private func loadPreview(_ session: AgentSession) async {
        guard previews[session.id] == nil, session.messageCount > 0 else { return }
        guard let records = try? await AgentChatClient().listMessages(sessionId: session.id) else { return }
        let lastUser = records.lastIndex { $0.role == "user" }
        let slice = lastUser.map { Array(records[$0...].prefix(2)) } ?? Array(records.suffix(1))
        previews[session.id] = slice.enumerated().compactMap { index, record in
            let text = record.content.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !text.isEmpty else { return nil }
            return MessageRecordPreview(id: "\(session.id)-\(index)", text: text, isUser: record.role == "user")
        }
    }
}

struct MessageRecordPreview: Identifiable, Equatable {
    let id: String
    let text: String
    let isUser: Bool
}

// MARK: - Code sessions view (phone calm I9)

/// Which screen the Code sessions view shows.
enum CodeSessionsScreen: Equatable {
    case sessions, scheduled, start
}

/// Where the Code sessions view opens (DEBUG launch arguments set these).
struct CodeSessionsOpening: Equatable {
    var screen: CodeSessionsScreen = .sessions
    var filter: CodeSessionFilter = .all
    /// Open the first computer's device sheet once the list is read.
    var deviceSheet = false
    var connectSheet = false
}

/// The Code sessions view (brief item 7, contract C6; web
/// `PhoneCodeSessionsView`). Code home stays; its top-right icon opens this,
/// zoomed out of the icon (CodeModeView). It shows:
/// - Computers (`RuntimeRelayStore`, the list the chip reads), + opens the
///   connect kit with its pair paths. A row opens the device sheet.
/// - Sessions with their C6 status icon (spinner Working, hand Needs input,
///   eye Ready for review, check Done) and the unread dot. Opening one goes
///   to Code, where `CodeModeView.open` clears Ready for review (9a).
/// - The filter menu (glass, grows from its button; the button fills while a
///   filter other than All is on) and the clock (Scheduled).
/// - A floating glass "+ New session": the start screen (`CodeStartScreen`).
struct CodeSessionsView: View {
    /// Every Code-surface session, archived ones too.
    let sessions: [AgentSession]
    let folder: CodeFolderInfo?
    var opening = CodeSessionsOpening()
    let onClose: () -> Void
    let onOpenSession: (AgentSession) -> Void
    let onArchive: (AgentSession) -> Void
    let onStart: (String) -> Void
    let onRefresh: () async -> Void
    /// Opens the card overview (Grid / Deck cards), still reachable from the
    /// new list (step 27 carry-over from step 26).
    var onOpenCards: (() -> Void)? = nil

    @ObservedObject private var relay = RuntimeRelayStore.shared
    @ObservedObject private var shellSessions = ShellSessionsStore.shared
    @ReducedMotion private var reduced
    @State private var screen: CodeSessionsScreen = .sessions
    @State private var filter: CodeSessionFilter = .all
    @State private var menuOpen = false
    @State private var sheet: Sheet? = nil
    @State private var didApplyOpening = false

    enum Sheet: Identifiable, Equatable {
        case connect
        case device(String)
        var id: String {
            switch self {
            case .connect: return "connect"
            case .device(let id): return "device-\(id)"
            }
        }
    }

    private func status(_ session: AgentSession) -> CodeSessionFilterStatus {
        codeSessionFilterStatus(session, streaming: shellSessions.isStreaming(session.id))
    }

    private var screenAnimation: Animation {
        reduced ? Motion.fade : .spring(response: 0.34, dampingFraction: 0.88)
    }

    var body: some View {
        ZStack {
            Color("BgPrimary").ignoresSafeArea()
            switch screen {
            case .sessions:
                sessionsScreen
                    .transition(.opacity)
            case .scheduled:
                CodeScheduledScreen(onBack: { go(.sessions) })
                    .transition(reduced ? .opacity : .move(edge: .trailing).combined(with: .opacity))
            case .start:
                CodeStartScreen(folder: folder,
                                onBack: { go(.sessions) },
                                onConnect: { sheet = .connect },
                                onStart: onStart)
                    .transition(reduced ? .opacity : .scale(scale: 0.96, anchor: .bottom).combined(with: .opacity))
            }
        }
        .accessibilityAddTraits(.isModal)
        .sheet(item: $sheet) { sheet in
            switch sheet {
            case .connect:
                ConnectComputerSheet(showsPairPaths: true)
                    .presentationDragIndicator(.visible)
            case .device(let id):
                if let device = relay.devices.first(where: { $0.id == id }) {
                    CodeDeviceSheet(
                        device: device,
                        isActive: relay.activeRuntimeId == id,
                        folder: folder,
                        sessions: activeSessions,
                        status: status,
                        onOpen: { session in
                            self.sheet = nil
                            onOpenSession(session)
                        },
                        onNewSession: {
                            if device.isOnline { relay.select(device.id) }
                            self.sheet = nil
                            go(.start)
                        }
                    )
                    .presentationDetents([.medium, .large])
                    .presentationDragIndicator(.visible)
                }
            }
        }
        .task {
            await relay.refresh()
            applyOpening()
        }
    }

    private func go(_ next: CodeSessionsScreen) {
        menuOpen = false
        withAnimation(screenAnimation) { screen = next }
    }

    /// Sessions running or waiting on the computer Code runs on.
    private var activeSessions: [AgentSession] {
        sessions.filter {
            let s = status($0)
            return s == .working || s == .needsInput
        }
    }

    private func applyOpening() {
        guard !didApplyOpening else { return }
        didApplyOpening = true
        screen = opening.screen
        filter = opening.filter
        if opening.connectSheet {
            sheet = .connect
        } else if opening.deviceSheet, let first = relay.activeDevice ?? relay.devices.first {
            sheet = .device(first.id)
        }
    }

    // MARK: Sessions screen

    private var sessionsScreen: some View {
        let result = filterCodeSessionRows(sessions, filter: filter, status: status)
        return VStack(spacing: 0) {
            header(counts: result.counts)
            List {
                computersSection
                Section {
                    if result.items.isEmpty {
                        Text(filter.emptyText)
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextSecondary"))
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.vertical, 12)
                            .plainCodeRow()
                    } else {
                        ForEach(result.items) { session in
                            sessionRow(session)
                        }
                    }
                } header: {
                    sectionLabel(filter == .all ? "Sessions" : "Sessions · \(filter.label)")
                }
                Color.clear.frame(height: 72).plainCodeRow()
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .refreshable {
                await onRefresh()
                await relay.refresh()
            }
            .animation(reduced ? nil : .spring(response: 0.3, dampingFraction: 0.86), value: filter)
        }
        .overlay(alignment: .topTrailing) { filterMenu(counts: result.counts) }
        .overlay(alignment: .bottom) {
            CodeFloatingAction(label: "New session") { go(.start) }
                .padding(.bottom, 12)
        }
    }

    private func header(counts: CodeSessionFilterCounts) -> some View {
        HStack(spacing: 8) {
            CodeGlassRoundButton(icon: "chevron.left", label: "Back to Code", action: onClose)
            Spacer()
            HStack(spacing: 0) {
                Button(action: { go(.scheduled) }) {
                    Image(systemName: "clock")
                        .font(.system(size: 16))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(width: 44, height: 40)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Scheduled")
                if let onOpenCards {
                    Button {
                        Haptics.light()
                        onOpenCards()
                    } label: {
                        Image(systemName: "square.grid.2x2")
                            .font(.system(size: 15))
                            .foregroundColor(Color("TextPrimary"))
                            .frame(width: 44, height: 40)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Cards")
                    .accessibilityHint("Opens the card overview with Grid and Deck layouts")
                }
                Button(action: toggleMenu) {
                    Image(systemName: "line.3.horizontal.decrease")
                        .font(.system(size: 15, weight: filter == .all ? .regular : .semibold))
                        .foregroundColor(filter == .all ? Color("TextPrimary") : Color("BgPrimary"))
                        .frame(width: 32, height: 32)
                        .background(Circle().fill(filter == .all ? Color.clear : Color("TextPrimary")))
                        .frame(width: 44, height: 40)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Filter: \(filter.label)")
                .accessibilityHint("Opens the session filters")
            }
            .padding(.horizontal, 4)
            .glassSurface(cornerRadius: 22)
        }
        .overlay {
            Text("Code sessions")
                .font(.system(size: 17, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .accessibilityAddTraits(.isHeader)
                .allowsHitTesting(false)
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
        .padding(.bottom, 4)
    }

    private func toggleMenu() {
        Haptics.light()
        withAnimation(reduced ? Motion.fade : .spring(response: 0.28, dampingFraction: 0.82)) { menuOpen.toggle() }
    }

    /// The filter menu: glass, grows from the filter button's corner.
    @ViewBuilder
    private func filterMenu(counts: CodeSessionFilterCounts) -> some View {
        ZStack(alignment: .topTrailing) {
            if menuOpen {
                Color.black.opacity(0.001)
                    .ignoresSafeArea()
                    .onTapGesture { toggleMenu() }
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(CodeSessionFilter.allCases) { option in
                        Button {
                            Haptics.selection()
                            withAnimation(reduced ? Motion.fade : .spring(response: 0.28, dampingFraction: 0.82)) {
                                filter = option
                                menuOpen = false
                            }
                        } label: {
                            HStack(spacing: 10) {
                                Image(systemName: "checkmark")
                                    .font(.system(size: 13, weight: .semibold))
                                    .opacity(filter == option ? 1 : 0)
                                    .frame(width: 16)
                                Text(option.label)
                                    .font(.system(size: 15))
                                Spacer(minLength: 16)
                                Text("\(counts[option])")
                                    .font(.system(size: 13))
                                    .monospacedDigit()
                                    .foregroundColor(Color("TextSecondary"))
                            }
                            .foregroundColor(Color("TextPrimary"))
                            .padding(.horizontal, 14)
                            .frame(minHeight: 44)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityAddTraits(filter == option ? .isSelected : [])
                        .accessibilityLabel("\(option.label), \(counts[option])")
                    }
                }
                .padding(.vertical, 6)
                .frame(width: 250)
                .glassSurface(cornerRadius: 18)
                .shadow(color: .black.opacity(0.12), radius: 20, y: 8)
                .padding(.top, 56)
                .padding(.trailing, 14)
                .transition(reduced ? .opacity : .scale(scale: 0.6, anchor: .topTrailing).combined(with: .opacity))
                .accessibilityElement(children: .contain)
                .accessibilityLabel("Filter sessions")
            }
        }
    }

    private func sectionLabel(_ text: String, trailing: AnyView? = nil) -> some View {
        HStack {
            Text(text)
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextSecondary"))
            Spacer()
            if let trailing { trailing }
        }
        .textCase(nil)
        .padding(.top, 8)
    }

    // MARK: Computers

    @ViewBuilder
    private var computersSection: some View {
        Section {
            if relay.devices.isEmpty {
                Button(action: { sheet = .connect }) {
                    HStack(spacing: 12) {
                        Image(systemName: "desktopcomputer")
                            .font(.system(size: 15))
                            .foregroundColor(Color("TextSecondary"))
                            .frame(width: 24)
                        Text("No computers yet")
                            .font(.system(size: 14))
                            .foregroundColor(Color("TextSecondary"))
                        Spacer()
                        Text("Connect")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                    }
                    .frame(minHeight: 48)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .plainCodeRow()
            } else {
                ForEach(relay.devices) { device in
                    computerRow(device)
                }
            }
        } header: {
            sectionLabel("Computers", trailing: AnyView(
                Button(action: { sheet = .connect }) {
                    Image(systemName: "plus")
                        .font(.system(size: 15, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(width: 44, height: 36)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Connect a computer")
            ))
        }
    }

    private func computerRow(_ device: RelayRuntimeDevice) -> some View {
        let active = relay.activeRuntimeId == device.id
        let state = device.isOnline ? (device.status == "busy" ? "Busy" : "Online") : "Offline"
        let line = [state, active ? "Code runs here" : nil].compactMap { $0 }.joined(separator: " · ")
        return Button(action: { sheet = .device(device.id) }) {
            HStack(spacing: 12) {
                Image(systemName: "laptopcomputer")
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 24)
                VStack(alignment: .leading, spacing: 2) {
                    Text(device.displayName)
                        .font(.system(size: 15))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text(line)
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
                Spacer()
                Circle()
                    .fill(device.isOnline ? Theme.statusSuccess : Color("TextTertiary").opacity(0.5))
                    .frame(width: 8, height: 8)
                    .accessibilityHidden(true)
                Image(systemName: "chevron.right")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(Color("TextTertiary"))
            }
            .frame(minHeight: 52)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .plainCodeRow()
        .accessibilityLabel("\(device.displayName), \(line)")
    }

    // MARK: Sessions

    private func sessionRow(_ session: AgentSession) -> some View {
        let s = status(session)
        let unread = shellSessions.isUnread(session) && s != .working
        return Button(action: { onOpenSession(session) }) {
            CodeSessionStatusRow(session: session, status: s, unread: unread)
        }
        .buttonStyle(.plain)
        .plainCodeRow()
        .swipeActions(edge: .trailing, allowsFullSwipe: true) {
            if s != .archived {
                Button(role: .destructive) { onArchive(session) } label: {
                    Label("Archive", systemImage: "archivebox")
                }
            }
        }
    }
}

/// One Code session row: status icon, title, "Status · age", unread dot.
/// Shared by the sessions list and the device sheet.
struct CodeSessionStatusRow: View {
    let session: AgentSession
    let status: CodeSessionFilterStatus
    var unread = false

    private var title: String { session.name ?? "Untitled Session" }
    private var meta: String {
        [status.label, CodeSessionDates.age(session)].filter { !$0.isEmpty }.joined(separator: " · ")
    }

    var body: some View {
        HStack(spacing: 12) {
            CodeSessionStatusIcon(status: status)
                .frame(width: 24)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 15, weight: unread ? .semibold : .regular))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                if !meta.isEmpty {
                    Text(meta)
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            if unread {
                Circle().fill(Color.blue).frame(width: 8, height: 8)
                    .accessibilityHidden(true)
            }
        }
        .frame(minHeight: 52)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityLabel([title, status.label, unread ? "New" : ""].filter { !$0.isEmpty }.joined(separator: ", "))
    }
}

/// The C6 status icon: spinner Working, hand Needs input, eye Ready for
/// review, check Done; a plain dot for idle and archived.
struct CodeSessionStatusIcon: View {
    let status: CodeSessionFilterStatus

    var body: some View {
        switch status {
        case .working:
            ProgressView()
                .controlSize(.small)
                .tint(GizziCodeWordmark.coral)
        case .needsInput:
            Image(systemName: "hand.raised.fill")
                .font(.system(size: 14))
                .foregroundColor(Theme.statusWarning)
        case .readyForReview:
            Image(systemName: "eye")
                .font(.system(size: 14))
                .foregroundColor(Color("AccentPrimary"))
        case .done:
            Image(systemName: "checkmark.circle")
                .font(.system(size: 15))
                .foregroundColor(Theme.statusSuccess)
        case .idle, .archived:
            Circle()
                .strokeBorder(Color("TextTertiary"), lineWidth: 1.5)
                .frame(width: 10, height: 10)
        }
    }
}

/// The floating glass action at the bottom ("+ New session", "+ New task").
struct CodeFloatingAction: View {
    let label: String
    let action: () -> Void

    var body: some View {
        Button(action: {
            Haptics.light()
            action()
        }) {
            HStack(spacing: 6) {
                Image(systemName: "plus").font(.system(size: 14, weight: .bold))
                Text(label).font(.system(size: 15, weight: .semibold))
            }
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 20)
            .frame(height: 48)
            .glassSurface(cornerRadius: 24)
            .shadow(color: .black.opacity(0.10), radius: 14, y: 6)
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
    }
}

private extension View {
    /// A List row with no inset background or separator (the view draws its own).
    func plainCodeRow() -> some View {
        self.listRowBackground(Color.clear)
            .listRowSeparator(.hidden)
            .listRowInsets(EdgeInsets(top: 0, leading: 16, bottom: 0, trailing: 16))
    }
}

// MARK: - Device sheet

/// A computer's sheet (web DeviceSheetBody): its status, the folder, the
/// sessions running or waiting on it, and New session. Sessions don't record
/// their computer, so only the computer Code runs on lists them.
struct CodeDeviceSheet: View {
    let device: RelayRuntimeDevice
    let isActive: Bool
    let folder: CodeFolderInfo?
    let sessions: [AgentSession]
    let status: (AgentSession) -> CodeSessionFilterStatus
    let onOpen: (AgentSession) -> Void
    let onNewSession: () -> Void

    @Environment(\.dismiss) private var dismiss

    private var here: [AgentSession] { isActive ? sessions : [] }

    private var statusLine: String {
        let state = device.isOnline ? "Online" : (device.isWakeable ? "Asleep · wakes when used" : "Offline")
        return [state, device.hostname].compactMap { $0 }.joined(separator: " · ")
    }

    /// "…/allternit-workspace/allternit-ai"
    private var folderText: String {
        guard isActive, let folder else { return "No folder picked" }
        let parts = folder.directory.split(separator: "/")
        return parts.count > 2 ? "…/" + parts.suffix(2).joined(separator: "/") : folder.directory
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                Button(action: { dismiss() }) {
                    Image(systemName: "xmark")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 32, height: 32)
                        .background(Color("FillNeutral"), in: Circle())
                        .frame(width: 44, height: 44)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Close")
                Spacer()
            }
            .overlay {
                Text(device.displayName)
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                    .padding(.horizontal, 52)
                    .accessibilityAddTraits(.isHeader)
            }
            Text(statusLine)
                .font(.system(size: 12))
                .foregroundColor(Color("TextSecondary"))
            VStack(alignment: .leading, spacing: 4) {
                Text("Folder")
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextSecondary"))
                HStack(spacing: 8) {
                    Image(systemName: "folder").font(.system(size: 14))
                    Text(folderText).font(.system(size: 14)).lineLimit(1).truncationMode(.middle)
                    Spacer()
                }
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 12)
                .frame(minHeight: 44)
                .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            }
            if here.isEmpty {
                VStack(spacing: 6) {
                    Image(systemName: "laptopcomputer")
                        .font(.system(size: 18))
                        .foregroundColor(Color("TextTertiary"))
                    Text("No active sessions on this computer")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 18)
            } else {
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(here) { session in
                            Button(action: { onOpen(session) }) {
                                CodeSessionStatusRow(session: session, status: status(session))
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
            }
            Button(action: {
                Haptics.light()
                onNewSession()
            }) {
                HStack(spacing: 6) {
                    Image(systemName: "plus").font(.system(size: 14, weight: .bold))
                    Text("New session").font(.system(size: 15, weight: .semibold))
                }
                .foregroundColor(Color("BgPrimary"))
                .frame(maxWidth: .infinity, minHeight: 48)
                .background(Color("TextPrimary"), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            }
            .buttonStyle(.plain)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 16)
        .padding(.top, 8)
        .background(Color("BgPanel").ignoresSafeArea())
    }
}

// MARK: - Scheduled

/// Clock → Scheduled: the routines on the computer Code runs on (gizzi
/// `v1/automations/routines`, `RoutineStore`, the same server Code threads
/// run on). A refused or failed read shows the empty state, never error text
/// (C5). New task opens the existing New routine sheet.
struct CodeScheduledScreen: View {
    let onBack: () -> Void

    @ObservedObject private var store = RoutineStore.shared
    @State private var creating = false
    @State private var loaded = false
    @State private var createError: String? = nil

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                CodeGlassRoundButton(icon: "chevron.left", label: "Back to Code sessions", action: onBack)
                Spacer()
            }
            .overlay {
                Text("Scheduled")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .accessibilityAddTraits(.isHeader)
            }
            .padding(.horizontal, 12)
            .padding(.top, 8)

            if !store.routines.isEmpty {
                List {
                    ForEach(store.routines) { routine in
                        HStack(spacing: 12) {
                            Image(systemName: "clock")
                                .font(.system(size: 15))
                                .foregroundColor(Color("TextSecondary"))
                                .frame(width: 24)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(routine.name)
                                    .font(.system(size: 15))
                                    .foregroundColor(Color("TextPrimary"))
                                    .lineLimit(1)
                                Text(Self.line(routine))
                                    .font(.system(size: 12))
                                    .foregroundColor(Color("TextSecondary"))
                                    .lineLimit(1)
                            }
                        }
                        .frame(minHeight: 52)
                        .plainCodeRow()
                    }
                    Color.clear.frame(height: 72).plainCodeRow()
                }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
                .refreshable { await store.refresh() }
            } else if loaded {
                Spacer()
                VStack(spacing: 8) {
                    Image(systemName: "clock")
                        .font(.system(size: 22))
                        .foregroundColor(Color("TextTertiary"))
                    Text("No scheduled tasks")
                        .font(.system(size: 15, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                    Text("Scheduled tasks on your computer show here.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                }
                .accessibilityElement(children: .combine)
                Spacer()
            } else {
                Spacer()
                ProgressView()
                Spacer()
            }
            if let createError {
                Text(createError)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 24)
                    .padding(.bottom, 72)
            }
        }
        .overlay(alignment: .bottom) {
            CodeFloatingAction(label: "New task") { creating = true }
                .padding(.bottom, 12)
        }
        .task {
            await store.refresh()
            loaded = true
        }
        .sheet(isPresented: $creating) {
            CreateRoutineSheet { name, steps, trigger, schedule in
                Task {
                    do {
                        createError = nil
                        try await store.createRoutine(name: name, steps: steps, trigger: trigger, schedule: schedule)
                    } catch {
                        createError = plainErrorText(error, fallback: "Couldn't create the task.")
                    }
                }
            }
        }
    }

    /// "Every day 9:00 · Running", "Manual · Done".
    static func line(_ routine: Routine) -> String {
        let when = routine.schedule.flatMap { $0.isEmpty ? nil : $0 } ?? routine.trigger.flatMap { $0.isEmpty ? nil : $0 } ?? "Manual"
        let state: String?
        switch routine.state {
        case "running": state = "Running"
        case "failed": state = "Stopped"
        case "completed": state = "Done"
        default: state = nil
        }
        return [when, state].compactMap { $0 }.joined(separator: " · ")
    }
}
