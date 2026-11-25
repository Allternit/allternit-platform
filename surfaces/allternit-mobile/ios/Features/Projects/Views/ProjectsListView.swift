import SwiftUI

/// The Projects section as on m.allternit.com (allternit-ai ProjectView with
/// `phoneChrome`): ☰ · "Projects" · filter menu (Show, Sort by, Show archived),
/// "N open · N needs you" beside the List · Grid · Deck switch, the project
/// cards, and the floating search and "New project". Each card's ⋯ menu has
/// Open, Pin, Edit details, Archive and Delete.
///
/// Data: chat projects from `ProjectStore` (`/api/v1/cowork/projects`), bot
/// projects from `/api/v1/bot-projects` and code and design projects from
/// `/api/v1/project-workspaces` (ProjectsLibraryStore).
struct ProjectsListView: View {
    /// Detail's "+ New chat": selects the project and starts a fresh Chats
    /// conversation stamped with it (the shell owns the session binding).
    let onNewChatInProject: (CoworkProject) -> Void
    /// Tapping a chat row opens it in Chats (same wiring as the history list).
    let onOpenChat: (AgentSession) -> Void
    /// Opens the shell drawer (☰).
    var onOpenSidebar: (() -> Void)? = nil

    @StateObject private var projectStore = ProjectStore.shared
    @StateObject private var library = ProjectsLibraryStore.shared
    @StateObject private var navigator = BotNavigator()
    @Environment(\.dismiss) private var dismiss

    @AppStorage("allternit-overview-layout:phone:projects") private var layoutRaw = LibraryLayout.grid.rawValue
    @State private var searchText = ""
    @State private var category: Category = .all
    @State private var sortBy: Sort = .recent
    @State private var showArchived = false
    @State private var detailProject: CoworkProject? = nil
    @State private var codeDetail: WorkspaceLibraryProject? = nil
    @State private var designCanvas: WorkspaceLibraryProject? = nil
    @State private var newDesignProjectId: String? = nil
    /// The code project just made here: it opens asking for a folder.
    @State private var newCodeProjectId: String? = nil
    /// Which kind the New project sheet is making.
    @State private var newMode: LibraryProject.Mode = .chat
    @State private var isCreateSheetPresented = false
    @State private var isSearching = false
    @State private var showExplainer = false
    /// "Cover and pad": the detail opens with its cover and pad editor.
    @State private var openLookProjectId: String? = nil
    @ReducedMotion private var reduced
    @State private var editing: LibraryProject? = nil
    @State private var deleting: LibraryProject? = nil
    @State private var createError: String? = nil
    /// Bot picked in New project with a computer: the bot form opens next.
    @State private var opensBotForm = false
    @State private var isNewBotPresented = false

    /// The Show menu (ProjectView CATEGORY_CONFIG, the kinds iOS has).
    private enum Category: String, CaseIterable {
        case all, chat, code, design, bot

        var label: String {
            switch self {
            case .all: return "All Projects"
            case .chat: return "Chat"
            case .code: return "Code"
            case .design: return "Design"
            case .bot: return "Bot"
            }
        }

        var icon: String {
            switch self {
            case .all: return "folder"
            case .chat: return "bubble.left"
            case .code: return "chevron.left.forwardslash.chevron.right"
            case .design: return "paintpalette"
            case .bot: return "cpu"
            }
        }
    }

    /// PROJECT_SORT_OPTIONS.
    private enum Sort: String, CaseIterable {
        case recent, name, created

        var label: String {
            switch self {
            case .recent: return "Recently updated"
            case .name: return "Name"
            case .created: return "Date created"
            }
        }
    }

    private var layout: LibraryLayout {
        get { LibraryLayout(rawValue: layoutRaw) ?? .grid }
    }

    private var allProjects: [LibraryProject] {
        library.projects(from: projectStore.projects)
    }

    private var filteredProjects: [LibraryProject] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return allProjects
            .filter { project in
                if !showArchived && project.isArchived { return false }
                if category != .all && project.mode.rawValue != category.rawValue { return false }
                if !query.isEmpty && !project.title.lowercased().contains(query) { return false }
                return true
            }
            .sorted { a, b in
                switch sortBy {
                case .recent:
                    if a.isFavorite != b.isFavorite { return a.isFavorite }
                    return (a.updatedAt ?? .distantPast) > (b.updatedAt ?? .distantPast)
                case .name:
                    return a.title.localizedCompare(b.title) == .orderedAscending
                case .created:
                    return (a.createdAt ?? .distantPast) > (b.createdAt ?? .distantPast)
                }
            }
    }

    private func count(_ category: Category) -> Int {
        allProjects.filter { category == .all || $0.mode.rawValue == category.rawValue }.count
    }


    var body: some View {
        NavigationStack(path: $navigator.path) {
            ZStack(alignment: .bottom) {
                PolishedTabPage(
                    title: "Projects",
                    description: "Keep a job's chats, tasks and board together, and see what is working.",
                    introKey: .projects,
                    hero: { ProjectsIntroHero() }
                ) {
                    pageContent
                }
                .refreshable { await reload() }
                GlassPill(title: "New project", systemImage: "plus") { newProjectTapped() }
                    .padding(.bottom, 14)
                if showExplainer {
                    ProjectsExplainerCard(
                        onOK: {
                            ProjectsExplainer.markSeen()
                            withAnimation(reduced ? Motion.fade : .easeOut(duration: 0.2)) { showExplainer = false }
                            startNew(.chat)
                        },
                        onDismiss: {
                            ProjectsExplainer.markSeen()
                            withAnimation(reduced ? Motion.fade : .easeOut(duration: 0.2)) { showExplainer = false }
                        }
                    )
                    .zIndex(5)
                }
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    PhoneMenuButton(round: true, onOpen: openMenu)
                }
                ToolbarItem(placement: .topBarTrailing) {
                    ProjectsPill(searching: $isSearching, filtersActive: filtersOn) { filterPopover }
                }
            }
            .navigationDestination(item: $detailProject) { project in
                ProjectDetailView(
                    project: project,
                    onNewChat: onNewChatInProject,
                    onOpenChat: onOpenChat,
                    opensLook: project.id == openLookProjectId
                )
            }
            .navigationDestination(item: $codeDetail) { project in
                CodeProjectDetailView(project: project, asksForFolder: project.id == newCodeProjectId)
            }
            .navigationDestination(for: BotRoute.self) { route in
                BotRouteDestination(route: route)
            }
        }
        .environmentObject(navigator)
        .sheet(isPresented: $isCreateSheetPresented, onDismiss: {
            if opensBotForm { opensBotForm = false; isNewBotPresented = true }
        }) {
            NewProjectSheet(
                onCreate: { title, description in
                    createProject(mode: newMode, title: title, description: description)
                },
                onCreateAndStyle: { title, description in
                    createProject(mode: .chat, title: title, description: description, opensLook: true)
                },
                kind: $newMode
            )
        }
        .sheet(isPresented: $isNewBotPresented) {
            NewBotSheet { _ in Task { await library.load() } }
        }
        .fullScreenCover(item: $designCanvas) { project in
            DesignProjectCanvasView(project: project, createdHere: project.id == newDesignProjectId)
        }
        .sheet(item: $editing) { project in
            EditProjectDetailsSheet(project: project, onCoverAndPad: project.cowork.map { cowork in
                { detailProject = cowork; openLookProjectId = cowork.id }
            }) { title, description in
                try await library.updateDetails(project, title: title, description: description)
            }
        }
        .alert("Delete Project", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } })) {
            Button("Cancel", role: .cancel) { deleting = nil }
            Button("Delete", role: .destructive) {
                if let project = deleting { delete(project) }
                deleting = nil
            }
        } message: {
            Text(deleting.map { "Delete \"\($0.title)\"? This cannot be undone." } ?? "")
        }
        .task { await library.load() }
        #if DEBUG
        .onAppear {
            // `-open-project-detail` (DEBUG only): deep-links into the first
            // project so the detail view can be screenshot-verified.
            if launchArgumentEnabled("open-project-detail"), detailProject == nil {
                detailProject = projectStore.projects.first
            }
            // `-open-new-project` (DEBUG only): opens the create sheet.
            if launchArgumentEnabled("open-new-project") || launchArgumentEnabled("calm-new-project") {
                isCreateSheetPresented = true
            }
            // `-calm-projects-pill` (DEBUG only): opens the search + filter pill's popover state (search field shown).
            if launchArgumentEnabled("calm-projects-pill") { isSearching = true }
            // `-calm-projects-explainer` (DEBUG only): shows the explainer regardless of the once-key.
            if launchArgumentEnabled("calm-projects-explainer") { showExplainer = true }
        }
        #endif
    }

    private func openMenu() {
        if let onOpenSidebar {
            Haptics.medium()
            onOpenSidebar()
        } else {
            dismiss()
        }
    }

    // MARK: - Filter menu (Show · Sort by · Show archived)

    private var filtersOn: Bool { category != .all || sortBy != .recent || showArchived }

    /// Popover behind the pill's filter button: List · Grid · Deck (the shared
    /// OverviewLayoutSwitch) plus the existing Show · Sort by · Archived filters.
    private var filterPopover: some View {
        VStack(alignment: .leading, spacing: 4) {
            OverviewLayoutSwitch(raw: $layoutRaw, fallback: .grid)
                .padding(.bottom, 6)
            Text("Show").font(.caption.weight(.semibold)).foregroundColor(Color("TextSecondary"))
            ForEach(Category.allCases, id: \.self) { option in
                checked("\(option.label)  \(count(option))", on: category == option) { category = option }
            }
            Text("Sort by").font(.caption.weight(.semibold)).foregroundColor(Color("TextSecondary")).padding(.top, 6)
            ForEach(Sort.allCases, id: \.self) { option in
                checked(option.label, on: sortBy == option) { sortBy = option }
            }
            checked("Show archived  \(allProjects.filter(\.isArchived).count)", on: showArchived) { showArchived.toggle() }
                .padding(.top, 6)
        }
        .padding(16)
        .frame(minWidth: 260)
    }

    private func checked(_ title: String, on: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack {
                Text(title).font(.system(size: 15))
                Spacer(minLength: 12)
                if on { Image(systemName: "checkmark").font(.system(size: 13, weight: .semibold)) }
            }
            .foregroundColor(Color("TextPrimary"))
            .frame(minHeight: 40)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - Content

    /// "2 working · 1 needs you" beside the "Your projects · N open" label
    /// (SessionOverview counts).
    private var countsDetail: String {
        let shown = filteredProjects
        let working = shown.filter { $0.working > 0 }.count
        let waiting = shown.filter { $0.waiting > 0 }.count
        return [working > 0 ? "\(working) working" : nil,
                waiting > 0 ? "\(waiting) need\(waiting == 1 ? "s" : "") you" : nil]
            .compactMap { $0 }.joined(separator: " · ")
    }

    /// No computer: the computer's lists are empty by design, not failing.
    private var noComputer: Bool {
        library.noComputer || ProjectsLibraryStore.isNoComputerMessage(projectStore.loadError)
    }

    /// Notices lead the list (a failed bot load must not blank the rest).
    /// No computer reads as one neutral line; a real failure is a calm line
    /// with Retry. Nothing here is red.
    @ViewBuilder
    private var errors: some View {
        let noComputer = self.noComputer
        let botError = (category == .all || category == .bot) && !noComputer ? library.botProjectsError : nil
        let codeError = (category == .all || category == .code || category == .design) && !noComputer ? library.workspaceProjectsError : nil
        let loadError = projectStore.loadError.flatMap { message in
            noComputer || ProjectsLibraryStore.isNoComputerMessage(message) ? nil : "Couldn't load projects. \(FriendlyErrorMessage.from(message))"
        }
        let accountError = noComputer ? library.accountProjectsError : nil
        if noComputer {
            NeedsComputerLine(text: "Files and runs need a computer", style: .card)
        }
        let failures = [loadError, botError, codeError, accountError].compactMap { $0 }.uniqued()
        if !failures.isEmpty {
            FriendlyInlineStateView(
                style: .offline,
                icon: "wifi.exclamationmark",
                title: "Couldn't load everything",
                message: failures.joined(separator: " "),
                actionTitle: "Retry",
                action: { Task { await reload() } }
            )
            .padding(.top, 8)
        }
        if let createError {
            Text(createError)
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.top, 12)
                .padding(.horizontal, 4)
        }
    }

    @ViewBuilder
    private var pageContent: some View {
        let projects = filteredProjects
        if isSearching || !searchText.isEmpty {
            ProjectsSearchField(query: $searchText)
                .padding(.horizontal, -14)
        }
        errors
        if projectStore.isLoading && allProjects.isEmpty {
            ProgressView()
                .frame(maxWidth: .infinity)
                .padding(.vertical, 60)
        } else {
            SectionLabel("Your projects · \(projects.count) open") {
                if !countsDetail.isEmpty {
                    Text(countsDetail)
                        .font(.system(size: 12.5))
                        .foregroundStyle(Color("TextSecondary"))
                        .lineLimit(1)
                }
            }
            if projects.isEmpty {
                if searchText.isEmpty {
                    EmptyBox(title: "No projects yet",
                             line: "Tap New project to start one. Its chats, tasks and files stay together.",
                             art: .sheets,
                             actionTitle: "New project",
                             action: { newProjectTapped() })
                } else {
                    Text("No projects match your search.")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextTertiary"))
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 40)
                }
            } else {
                switch layout {
                case .grid:
                    LazyVGrid(columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible(), spacing: 12)], spacing: 12) {
                        ForEach(projects) { project in card(project, variant: .grid) }
                    }
                case .list:
                    LazyVStack(spacing: 8) {
                        ForEach(projects) { project in card(project, variant: .list) }
                    }
                case .deck:
                    ScrollView(.horizontal, showsIndicators: false) {
                        LazyHStack(spacing: 12) {
                            ForEach(projects) { project in card(project, variant: .deck) }
                        }
                        .scrollTargetLayout()
                        .padding(.horizontal, 14)
                    }
                    .scrollTargetBehavior(.viewAligned)
                    .padding(.horizontal, -14)
                }
            }
        }
    }

    private func card(_ project: LibraryProject, variant: LibraryLayout) -> some View {
        ProjectOverviewCard(
            project: project,
            variant: variant,
            menu: { menuItems(project) },
            onOpen: { open(project) }
        )
    }

    @ViewBuilder
    private func menuItems(_ project: LibraryProject) -> some View {
        Button { open(project) } label: { Label("Open", systemImage: "arrow.up.right.square") }
        Button { library.toggleFavorite(project) } label: {
            Label(project.isFavorite ? "Unpin" : "Pin", systemImage: project.isFavorite ? "pin.slash" : "pin")
        }
        Button { editing = project } label: { Label("Edit details", systemImage: "pencil") }
        Button { library.toggleArchive(project) } label: {
            Label(project.isArchived ? "Unarchive" : "Archive", systemImage: "archivebox")
        }
        Divider()
        Button(role: .destructive) { deleting = project } label: { Label("Delete", systemImage: "trash") }
    }

    // MARK: - Actions

    private func open(_ project: LibraryProject) {
        Haptics.light()
        if let cowork = project.cowork {
            detailProject = cowork
        } else if let row = project.workspace {
            if project.mode == .design { designCanvas = row } else { codeDetail = row }
        } else if let bot = project.bot {
            navigator.path = [.project(bot)]
        } else if project.account != nil {
            // Code, design and bot work runs on a computer: with none, the
            // gate explains; with one, the row is on its way there.
            if noComputer { ComputeStatusStore.shared.noteComputeRequired() } else { Task { await reload() } }
        }
    }

    private func reload() async {
        projectStore.fetchProjectsIfNeeded(force: true)
        await library.load()
    }

    /// First tap shows the explainer; OK opens create. Later taps open create.
    private func newProjectTapped() {
        if ProjectsExplainer.seen() {
            startNew(.chat)
        } else {
            withAnimation(reduced ? Motion.fade : .easeOut(duration: 0.2)) { showExplainer = true }
        }
    }

    private func startNew(_ mode: LibraryProject.Mode) {
        Haptics.light()
        newMode = mode
        isCreateSheetPresented = true
    }

    /// New projects open straight into their project page (Claude parity).
    private func createProject(mode: LibraryProject.Mode, title: String, description: String?, opensLook: Bool = false) {
        // Bot with a computer: the bot form takes over once this sheet closes.
        if mode == .bot && !noComputer {
            opensBotForm = true
            return
        }
        Task {
            do {
                createError = nil
                if mode == .bot {
                    // Saved to the account; the brief runs once a computer connects.
                    try await library.createAccountProject(kind: .bot, title: title, description: nil, brief: description)
                } else if noComputer {
                    let created = try await library.createAccountProject(
                        kind: AccountProject.Kind(rawValue: mode.rawValue) ?? .chat, title: title, description: description
                    )
                    if created.mode == .chat, let row = library.projects(from: projectStore.projects).first(where: { $0.account?.id == created.id }) {
                        if opensLook { openLookProjectId = created.id }
                        detailProject = row.cowork
                    }
                } else if mode == .code {
                    let created = try await library.createCodeProject(title: title, description: description)
                    newCodeProjectId = created.id
                    codeDetail = created
                } else if mode == .design {
                    let created = try await library.createDesignProject(title: title, description: description)
                    newDesignProjectId = created.id
                    designCanvas = created
                } else {
                    let created = try await projectStore.createProject(title: title, description: description)
                    if opensLook { openLookProjectId = created.id }
                    detailProject = created
                }
            } catch {
                createError = "Couldn't create the project. \(FriendlyErrorMessage.from(error))"
            }
        }
    }

    private func delete(_ project: LibraryProject) {
        Task {
            do {
                try await library.delete(project)
            } catch {
                createError = "Couldn't delete \(project.title). \(FriendlyErrorMessage.from(error))"
            }
        }
    }
}

private extension Array where Element: Hashable {
    func uniqued() -> [Element] {
        var seen = Set<Element>()
        return filter { seen.insert($0).inserted }
    }
}

// MARK: - Folder look cache

/// In-memory folder looks for the cards, loaded lazily per visible card.
/// A missing record, a failed request or no paired computer all leave nil and
/// the card keeps the default palette cover; nothing here blocks the list.
@MainActor
final class ProjectLooksCache: ObservableObject {
    static let shared = ProjectLooksCache()

    private var looks: [String: Appearance] = [:]
    private var settled: Set<String> = []
    private var inFlight: [String: Task<Appearance?, Never>] = [:]
    private let client = AppearanceClient()

    /// The web's key for the same project: `["project", <mode>, <id>]`, where
    /// chat projects keep their pre-fold `cowork` mode (web
    /// `projectWorkspaceMode`) so iOS reads the look the web saved.
    static func key(for project: LibraryProject) -> [String] {
        let mode = project.mode == .chat ? "cowork" : project.mode.rawValue
        return AppearanceClient.projectKey(mode: mode, id: project.nativeId)
    }

    func cached(_ key: [String]) -> Appearance? { looks[AppearanceClient.keyString(key)] }

    func appearance(for key: [String]) async -> Appearance? {
        let id = AppearanceClient.keyString(key)
        if settled.contains(id) { return looks[id] }
        if let task = inFlight[id] { return await task.value }
        let client = self.client
        let task = Task { try? await client.appearance(key: key) }
        inFlight[id] = task
        let result = await task.value
        inFlight[id] = nil
        // A failed request is not cached, so the next visible pass retries it.
        if result != nil { looks[id] = result; settled.insert(id) }
        return result
    }
}

// MARK: - Card

/// One project on the overview (SessionOverview OverviewCard, phone sizes):
/// a 196pt grid card, a 60pt list row or a 226pt-wide deck card.
private struct ProjectOverviewCard<Menu: View>: View {
    let project: LibraryProject
    let variant: LibraryLayout
    @ViewBuilder let menu: () -> Menu
    let onOpen: () -> Void

    @State private var appearance: Appearance?
    @ScaledMetric(relativeTo: .body) private var listCoverWidth: CGFloat = 30

    private var icon: String {
        switch project.mode {
        case .chat: return "bubble.left"
        case .code: return "chevron.left.forwardslash.chevron.right"
        case .design: return "paintpalette"
        case .bot: return "cpu"
        }
    }

    private var modeLabel: String {
        switch project.mode {
        case .chat: return "Chat"
        case .code: return "Code"
        case .design: return "Design"
        case .bot: return "Bot"
        }
    }

    private var status: WorkspaceLibraryProject.Status? { project.workspace?.status }

    /// The note line's chip (ProjectStateChip): Closed and Blocked show as the
    /// card's status instead, and On track says nothing.
    private var stateChip: (label: String, color: Color)? {
        guard let status, !status.closed else { return nil }
        switch status.state {
        case "at_risk": return ("At risk", Theme.statusWarning)
        case "needs_decision": return ("Needs decision", Theme.statusWarning)
        case "done": return ("Done", Color("TextTertiary"))
        default: return nil
        }
    }

    private var tone: (label: String, color: Color)? {
        if project.isArchived { return ("Archived", Color("TextTertiary")) }
        if status?.closed == true { return ("Closed", Color("TextTertiary")) }
        if status?.state == "blocked" { return ("Blocked", Theme.statusWarning) }
        if project.waiting > 0 { return ("\(project.waiting) waiting", Theme.statusWarning) }
        if project.working > 0 { return ("\(project.working) working", Theme.accentChat) }
        return nil
    }

    private var ring: Color {
        if project.waiting > 0 && !project.isArchived { return Theme.statusWarning }
        if project.working > 0 && !project.isArchived { return Theme.accentChat }
        return Color("BorderSubtle")
    }

    private var preview: String {
        if let next = status?.nextAction, !next.isEmpty { return next }
        if let description = project.description, !description.isEmpty { return description }
        return project.countLine.prefix(1).uppercased() + project.countLine.dropFirst()
    }

    var body: some View {
        Button(action: onOpen) {
            Group {
                if variant == .list { row } else { tile }
            }
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: variant == .list ? 12 : 14, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: variant == .list ? 12 : 14, style: .continuous)
                    .stroke(ring, lineWidth: 1)
            )
            .shadow(color: variant == .deck ? .black.opacity(0.10) : .clear, radius: 15, x: 0, y: 10)
            .opacity(project.isArchived || status?.closed == true ? 0.6 : 1)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .contextMenu { menu() }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(project.title + ", \(modeLabel) project" + (tone.map { ", \($0.label)" } ?? ""))
        .task(id: project.id) {
            let key = ProjectLooksCache.key(for: project)
            if let hit = ProjectLooksCache.shared.cached(key) { appearance = hit; return }
            appearance = await ProjectLooksCache.shared.appearance(for: key)
        }
    }

    private var moreButton: some View {
        SwiftUI.Menu { menu() } label: {
            Image(systemName: "ellipsis")
                .font(.system(size: variant == .deck ? 15 : 13, weight: .bold))
                .foregroundColor(Color("TextTertiary"))
                .frame(width: 24, height: 24)
                .contentShape(Rectangle())
        }
        .accessibilityLabel("More for \(project.title)")
    }

    private var note: some View {
        HStack(spacing: 4) {
            if project.isFavorite {
                Image(systemName: "star.fill")
                    .font(.system(size: 10))
                    .foregroundColor(Color(hex: "#EAB308"))
                    .accessibilityLabel("Pinned")
            }
            Text(modeLabel)
                .lineLimit(1)
            Text("·")
            Text(project.countLine)
                .lineLimit(1)
            if let stateChip {
                Text(stateChip.label)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundColor(stateChip.color)
                    .lineLimit(1)
            }
        }
        .font(.caption)
        .foregroundColor(Color("TextTertiary"))
    }

    @ViewBuilder
    private var pill: some View {
        if let tone {
            HStack(spacing: 4) {
                if project.waiting > 0 && !project.isArchived {
                    Image(systemName: "hand.raised").font(.system(size: 10))
                }
                Text(tone.label).lineLimit(1)
            }
            .font(.caption2.weight(.semibold))
            .foregroundColor(tone.color)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Capsule().fill(tone.color.opacity(0.12)))
        }
    }

    private var meta: some View {
        Text(libraryShortAgo(project.updatedAt))
            .font(.caption.monospacedDigit())
            .foregroundColor(Color("TextTertiary"))
    }

    private var row: some View {
        HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 8) {
                    FolderCoverView(title: project.title, mode: project.mode.rawValue,
                                    appearance: appearance, width: listCoverWidth, flat: true)
                        .accessibilityHidden(true)
                    Text(project.title)
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                }
                note
                Text(preview)
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
            VStack(alignment: .trailing, spacing: 4) {
                meta
                pill
            }
            .padding(.top, 2)
            moreButton
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .frame(minHeight: 60)
    }

    /// Folder cover on top, then the title, note, preview line and status.
    private var tile: some View {
        let deck = variant == .deck
        return VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .top, spacing: 4) {
                Spacer(minLength: 0)
                FolderCoverView(title: project.title, mode: project.mode.rawValue,
                                appearance: appearance, width: deck ? 132 : 100)
                    .accessibilityHidden(true)
                Spacer(minLength: 0)
                moreButton
                    .padding(.trailing, -4)
            }
            Text(project.title)
                .font(deck ? .headline : .subheadline.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(2)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.top, 8)
            note.padding(.top, 2)
            Text(preview)
                .font(deck ? .subheadline : .footnote)
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(deck ? 3 : 2)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .padding(.top, 4)
            HStack(spacing: 6) {
                pill
                Spacer(minLength: 0)
                meta
            }
            .padding(.top, 4)
        }
        .padding(variant == .grid ? 12 : 16)
        .frame(width: deck ? OverviewDeck.width : nil, height: deck ? OverviewDeck.height : nil)
        .frame(minHeight: deck ? nil : 250, alignment: .top)
    }
}

// MARK: - Edit details

/// EditProjectDetailsDialog: name and description.
private struct EditProjectDetailsSheet: View {
    let project: LibraryProject
    /// Nil for projects without a pad (code, design, bot).
    let onCoverAndPad: (() -> Void)?
    let onSave: (String, String) async throws -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var title: String
    @State private var details: String
    @State private var saving = false
    @State private var error: String? = nil

    init(project: LibraryProject, onCoverAndPad: (() -> Void)? = nil, onSave: @escaping (String, String) async throws -> Void) {
        self.project = project
        self.onCoverAndPad = onCoverAndPad
        self.onSave = onSave
        _title = State(initialValue: project.title)
        _details = State(initialValue: project.description ?? "")
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    HStack {
                        Spacer()
                        FolderCoverView(title: title.isEmpty ? project.title : title, mode: project.mode.rawValue,
                                        appearance: nil, width: 110, flat: true)
                            .accessibilityElement(children: .ignore)
                            .accessibilityLabel("Folder cover for \(title)")
                        Spacer()
                    }
                    .listRowBackground(Color.clear)
                }
                if let onCoverAndPad {
                    Section {
                        Button {
                            Haptics.light()
                            dismiss()
                            onCoverAndPad()
                        } label: {
                            HStack {
                                Text("Cover and pad")
                                Spacer()
                                Image(systemName: "chevron.right").font(.system(size: 12, weight: .semibold))
                            }
                            .frame(minHeight: 44)
                        }
                        .foregroundColor(Color("TextPrimary"))
                    }
                }
                Section("Name") {
                    TextField("Project name", text: $title)
                }
                Section("Description") {
                    TextField("What is this project about?", text: $details, axis: .vertical)
                        .lineLimit(3...6)
                }
                if let error {
                    Section { Text(error).foregroundColor(Theme.statusError).font(.footnote) }
                }
            }
            .scrollContentBackground(.hidden)
            .background(Color("BgPrimary"))
            .navigationTitle("Project settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button { dismiss() } label: { Image(systemName: "xmark") }
                        .accessibilityLabel("Close")
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button { save() } label: { Image(systemName: "checkmark") }
                        .accessibilityLabel("Save")
                        .disabled(saving || title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
        }
        .onAppear { Haptics.light() }
        .presentationDetents([.large])
    }

    private func save() {
        saving = true
        Task {
            do {
                try await onSave(title.trimmingCharacters(in: .whitespacesAndNewlines), details)
                dismiss()
            } catch {
                self.error = "Couldn't save. \(FriendlyErrorMessage.from(error.localizedDescription))"
                saving = false
            }
        }
    }
}


// MARK: - Intro hero

/// First-visit intro hero (`introProjectsSeenAt`): the project list as it
/// looks, then the open project's Board with a card moving from Working to
/// Done. Reduce Motion keeps the list still.
private struct ProjectsIntroHero: View {
    private struct Row: Identifiable {
        let id: String
        let icon: String
        let note: String
        let pill: (String, Color)?
        let hand: Bool
        let ring: Color?
        let ago: String
    }

    private static let amber = Color(hex: "#B45309")
    private static let rows: [Row] = [
        Row(id: "Launch site", icon: "chevron.left.forwardslash.chevron.right", note: "Code · 2 sessions",
            pill: ("2 working", PolishTokens.coral), hand: false, ring: PolishTokens.coral.opacity(0.55), ago: "now"),
        Row(id: "Weekly report", icon: "cpu", note: "Bot · 3 bots · Al coordinates",
            pill: ("1 waiting", amber), hand: true, ring: Color(hex: "#FBBF24"), ago: "1h"),
        Row(id: "Q4 plan", icon: "bubble.left", note: "Chat · 4 threads", pill: nil, hand: false, ring: nil, ago: "2d")
    ]

    @State private var phase = 0
    @Namespace private var cards
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var moved: Bool { phase == 2 }

    private var columns: [(String, [String])] {
        [("Queued", ["Write the FAQ"]),
         ("Working", moved ? ["Hero copy"] : ["Hero copy", "Fix checkout"]),
         ("Done", moved ? ["Fix checkout", "Pick domain"] : ["Pick domain"])]
    }

    var body: some View {
        ZStack(alignment: .top) {
            Color(hex: "#F7F7F7")
            VStack(alignment: .leading, spacing: 0) {
                if phase == 0 { list } else { board }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 11)
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .background(
                RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Color.white)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .strokeBorder(PolishTokens.glassBorder, lineWidth: 0.5)
            )
            .shadow(color: .black.opacity(0.06), radius: 12, x: 0, y: 6)
            .padding(.horizontal, 12)
            .padding(.top, 12)
        }
        .frame(height: 200)
        .clipped()
        .task { await run() }
    }

    private var list: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                BlockText("PROJECTS", height: 8, color: PolishTokens.ink)
                Spacer()
                Text("3 open").font(.system(size: 10)).foregroundStyle(Color("TextSecondary"))
            }
            .padding(.bottom, 3)
            ForEach(Self.rows) { r in
                HStack(spacing: 8) {
                    Image(systemName: r.icon)
                        .font(.system(size: 12))
                        .foregroundStyle(Color("TextSecondary"))
                        .frame(width: 16)
                    VStack(alignment: .leading, spacing: 1) {
                        HStack(spacing: 6) {
                            Text(r.id).font(.system(size: 12, weight: .semibold)).foregroundStyle(PolishTokens.ink)
                            if let pill = r.pill {
                                HStack(spacing: 3) {
                                    if r.hand { Image(systemName: "hand.raised").font(.system(size: 8)) }
                                    Text(pill.0).font(.system(size: 9.5, weight: .semibold))
                                }
                                .foregroundStyle(pill.1)
                                .padding(.horizontal, 6)
                                .padding(.vertical, 1)
                                .background(Capsule().fill(pill.1.opacity(0.12)))
                            }
                        }
                        Text(r.note)
                            .font(.system(size: 10))
                            .foregroundStyle(Color("TextTertiary"))
                            .lineLimit(1)
                    }
                    Spacer(minLength: 0)
                    Text(r.ago).font(.system(size: 10)).foregroundStyle(Color("TextTertiary"))
                }
                .padding(.horizontal, 9)
                .padding(.vertical, 7)
                .overlay(
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .strokeBorder(r.ring ?? PolishTokens.glassBorder, lineWidth: r.ring == nil ? 0.5 : 1)
                )
            }
        }
        .transition(.opacity)
    }

    private var board: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 4) {
                Text("\u{2039} Projects").foregroundStyle(Color("TextSecondary"))
                Text("/").foregroundStyle(Color("TextTertiary"))
                Text("Launch site").fontWeight(.bold).foregroundStyle(PolishTokens.ink)
            }
            .font(.system(size: 10.5))
            HStack(spacing: 14) {
                ForEach(["Overview", "Delegate", "Board"], id: \.self) { t in
                    Text(t)
                        .font(.system(size: 11, weight: t == "Board" ? .semibold : .regular))
                        .foregroundStyle(t == "Board" ? PolishTokens.ink : Color("TextSecondary"))
                        .padding(.bottom, 5)
                        .overlay(alignment: .bottom) {
                            if t == "Board" { Capsule().fill(PolishTokens.ink).frame(height: 2) }
                        }
                }
                Spacer(minLength: 0)
            }
            .overlay(alignment: .bottom) { Rectangle().fill(PolishTokens.glassBorder).frame(height: 0.5) }
            HStack(alignment: .top, spacing: 6) {
                ForEach(columns, id: \.0) { col in
                    VStack(alignment: .leading, spacing: 4) {
                        Text(col.0).font(.system(size: 9.5, weight: .semibold)).foregroundStyle(Color("TextSecondary"))
                            .padding(.bottom, 1)
                        ForEach(col.1, id: \.self) { name in
                            HStack(spacing: 3) {
                                if col.0 == "Done" {
                                    Image(systemName: "checkmark").font(.system(size: 8, weight: .bold)).foregroundStyle(Color(hex: "#16A34A"))
                                }
                                Text(name).font(.system(size: 9.5, weight: .medium)).foregroundStyle(PolishTokens.ink).lineLimit(1)
                            }
                            .padding(.horizontal, 6)
                            .padding(.vertical, 5)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(RoundedRectangle(cornerRadius: 7, style: .continuous).fill(Color.white))
                            .shadow(color: .black.opacity(0.06), radius: 1, x: 0, y: 1)
                            .matchedGeometryEffect(id: name, in: cards)
                        }
                    }
                    .padding(6)
                    .frame(maxWidth: .infinity, minHeight: 100, alignment: .topLeading)
                    .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(PolishTokens.neutralFill2))
                }
            }
        }
        .transition(.opacity)
    }

    /// List, then the Board, then the card moves; loops until the view goes.
    private func run() async {
        guard !reduceMotion else { return }
        let hold: [UInt64] = [2600, 1100, 2600]
        while !Task.isCancelled {
            try? await Task.sleep(nanoseconds: hold[phase] * 1_000_000)
            if Task.isCancelled { return }
            withAnimation(.spring(response: 0.45, dampingFraction: 0.82)) { phase = (phase + 1) % 3 }
        }
    }
}
