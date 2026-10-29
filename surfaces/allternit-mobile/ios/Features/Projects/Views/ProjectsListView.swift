import SwiftUI

/// The Projects section as the phone opens it from the drawer (allternit-ai
/// ProjectsScreen → ProjectView `phoneChrome` → ProjectsOverview): ☰ ·
/// "Projects" · layout/filter menu on top, "N open" under it, the projects
/// as List · Grid · Deck cards, and a floating search + "New project" bar.
/// Each card's ⋯ menu has Open, Pin, Edit details, Archive and Delete.
///
/// Data: chat projects from `ProjectStore` (`/api/v1/cowork/projects`) and
/// bot projects from `/api/v1/bot-projects` (ProjectsLibraryStore).
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
    @State private var isCreateSheetPresented = false
    @State private var editing: LibraryProject? = nil
    @State private var deleting: LibraryProject? = nil
    @State private var createError: String? = nil

    /// The Show menu (ProjectView CATEGORY_CONFIG, the kinds iOS has).
    private enum Category: String, CaseIterable {
        case all, chat, bot

        var label: String {
            switch self {
            case .all: return "All Projects"
            case .chat: return "Chat"
            case .bot: return "Bot"
            }
        }

        var icon: String {
            switch self {
            case .all: return "folder"
            case .chat: return "bubble.left"
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

    private var filtersOn: Bool { category != .all || sortBy != .recent || showArchived }

    var body: some View {
        NavigationStack(path: $navigator.path) {
            ZStack(alignment: .bottom) {
                VStack(spacing: 0) {
                    LibrarySectionHeader(title: "Projects", onMenu: openMenu) { filterMenu }
                    countsLine
                    content
                }
                LibraryFloatingBar(
                    query: $searchText,
                    placeholder: "Search projects",
                    newLabel: "New project",
                    wide: .new,
                    onNew: {
                        UIImpactFeedbackGenerator(style: .light).impactOccurred()
                        isCreateSheetPresented = true
                    }
                )
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .toolbar(.hidden, for: .navigationBar)
            .navigationDestination(item: $detailProject) { project in
                ProjectDetailView(
                    project: project,
                    onNewChat: onNewChatInProject,
                    onOpenChat: onOpenChat
                )
            }
            .navigationDestination(for: BotRoute.self) { route in
                BotRouteDestination(route: route)
            }
        }
        .environmentObject(navigator)
        .sheet(isPresented: $isCreateSheetPresented) {
            NewProjectSheet { title, description in
                createProject(title: title, description: description)
            }
        }
        .sheet(item: $editing) { project in
            EditProjectDetailsSheet(project: project) { title, description in
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
            if launchArgumentEnabled("open-new-project") {
                isCreateSheetPresented = true
            }
        }
        #endif
    }

    private func openMenu() {
        if let onOpenSidebar {
            UIImpactFeedbackGenerator(style: .medium).impactOccurred()
            onOpenSidebar()
        } else {
            dismiss()
        }
    }

    // MARK: - Filter menu (Layout · Show · Sort by · Show archived)

    private var filterMenu: some View {
        Menu {
            Section("Layout") {
                ForEach(LibraryLayout.allCases) { option in
                    LibraryMenuCheck(title: option.label, icon: option.icon, checked: layout == option) {
                        layoutRaw = option.rawValue
                    }
                }
            }
            Section("Show") {
                ForEach(Category.allCases, id: \.self) { cat in
                    LibraryMenuCheck(title: cat.label, icon: cat.icon, hint: String(count(cat)), checked: category == cat) {
                        category = cat
                    }
                }
            }
            Section("Sort by") {
                ForEach(Sort.allCases, id: \.self) { sort in
                    LibraryMenuCheck(title: sort.label, checked: sortBy == sort) { sortBy = sort }
                }
            }
            Section {
                LibraryMenuCheck(title: "Show archived", hint: String(allProjects.filter(\.isArchived).count), checked: showArchived) {
                    showArchived.toggle()
                }
            }
        } label: {
            LibraryRoundLabel(icon: layout.icon, dot: filtersOn)
        }
        .accessibilityLabel("Layout: \(layout.label). Change layout, filter and sort")
    }

    // MARK: - Content

    /// "1 open · 2 working · 1 needs you" (SessionOverview counts).
    private var countsLine: some View {
        let shown = filteredProjects
        let working = shown.filter { $0.working > 0 }.count
        let waiting = shown.filter { $0.waiting > 0 }.count
        let parts = ["\(shown.count) open",
                     working > 0 ? "\(working) working" : nil,
                     waiting > 0 ? "\(waiting) need\(waiting == 1 ? "s" : "") you" : nil].compactMap { $0 }
        return Text(parts.joined(separator: " · "))
            .font(.system(size: 12))
            .foregroundColor(Color("TextTertiary"))
            .lineLimit(1)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 16)
            .padding(.bottom, 4)
    }

    /// Errors lead the list (a failed bot load must not blank the rest).
    @ViewBuilder
    private var errors: some View {
        let botError = (category == .all || category == .bot) ? library.botProjectsError : nil
        let loadError = projectStore.loadError.map { _ in "Couldn't load projects. Check the connection and try again." }
        ForEach([loadError, botError, createError].compactMap { $0 }.uniqued(), id: \.self) { message in
            Text(message)
                .font(.system(size: 13))
                .foregroundColor(Theme.statusError)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.top, 16)
                .padding(.horizontal, 4)
        }
    }

    @ViewBuilder
    private var content: some View {
        let projects = filteredProjects
        if projectStore.isLoading && allProjects.isEmpty {
            Spacer()
            ProgressView()
            Spacer()
        } else if projects.isEmpty {
            ScrollView {
                errors.padding(.horizontal, 10)
                Text(searchText.isEmpty
                     ? "No projects yet. Create one to organize your chats, tasks, code workspaces and design canvases."
                     : "No projects match your search.")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextTertiary"))
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 40)
            }
            .refreshable { await reload() }
        } else {
            switch layout {
            case .grid:
                ScrollView {
                    VStack(spacing: 0) {
                        errors
                        LazyVGrid(columns: [GridItem(.flexible(), spacing: 8), GridItem(.flexible(), spacing: 8)], spacing: 8) {
                            ForEach(projects) { project in card(project, variant: .grid) }
                        }
                        .padding(.top, 4)
                    }
                    .padding(.horizontal, 10)
                    .padding(.bottom, 112)
                }
                .refreshable { await reload() }
            case .list:
                ScrollView {
                    LazyVStack(spacing: 6) {
                        errors
                        ForEach(projects) { project in card(project, variant: .list) }
                    }
                    .padding(.horizontal, 10)
                    .padding(.top, 4)
                    .padding(.bottom, 112)
                }
                .refreshable { await reload() }
            case .deck:
                ScrollView(.horizontal, showsIndicators: false) {
                    LazyHStack(spacing: 12) {
                        ForEach(projects) { project in card(project, variant: .deck) }
                    }
                    .scrollTargetLayout()
                    .padding(.horizontal, 24)
                    .padding(.top, 8)
                    .padding(.bottom, 112)
                }
                .scrollTargetBehavior(.viewAligned)
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
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        if let cowork = project.cowork {
            detailProject = cowork
        } else if let bot = project.bot {
            navigator.path = [.project(bot)]
        }
    }

    private func reload() async {
        projectStore.fetchProjectsIfNeeded(force: true)
        await library.load()
    }

    /// New projects open straight into their project page (Claude parity).
    private func createProject(title: String, description: String?) {
        Task {
            do {
                createError = nil
                let created = try await projectStore.createProject(title: title, description: description)
                detailProject = created
            } catch {
                createError = "Couldn't create the project: \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }

    private func delete(_ project: LibraryProject) {
        Task {
            do {
                try await library.delete(project)
            } catch {
                createError = "Couldn't delete \(project.title): \(FriendlyErrorMessage.from(error.localizedDescription))"
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

// MARK: - Card

/// One project on the overview (SessionOverview OverviewCard, phone sizes):
/// a 196pt grid card, a 60pt list row or a 226pt-wide deck card.
private struct ProjectOverviewCard<Menu: View>: View {
    let project: LibraryProject
    let variant: LibraryLayout
    @ViewBuilder let menu: () -> Menu
    let onOpen: () -> Void

    private var icon: String {
        project.mode == .bot ? "cpu" : "bubble.left"
    }

    private var tone: (label: String, color: Color)? {
        if project.isArchived { return ("Archived", Color("TextTertiary")) }
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
        if let description = project.description, !description.isEmpty { return description }
        return project.countLine.prefix(1).uppercased() + project.countLine.dropFirst()
    }

    var body: some View {
        Button(action: onOpen) {
            Group {
                if variant == .list { row } else { tile }
            }
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: variant == .list ? 12 : 16, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: variant == .list ? 12 : 16, style: .continuous)
                    .stroke(ring, lineWidth: 1)
            )
            .shadow(color: variant == .deck ? .black.opacity(0.10) : .clear, radius: 15, x: 0, y: 10)
            .opacity(project.isArchived ? 0.6 : 1)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .contextMenu { menu() }
        .accessibilityLabel(project.title + (tone.map { ", \($0.label)" } ?? ""))
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

    private var iconView: some View {
        Image(systemName: icon)
            .font(.system(size: 15))
            .foregroundColor(Color("TextSecondary"))
            .frame(width: 24, height: 24)
            .padding(.trailing, 4)
    }

    private var note: some View {
        HStack(spacing: 4) {
            if project.isFavorite {
                Image(systemName: "star.fill")
                    .font(.system(size: 10))
                    .foregroundColor(Color(hex: "#EAB308"))
                    .accessibilityLabel("Pinned")
            }
            Text(project.countLine.prefix(1).uppercased() + project.countLine.dropFirst())
                .lineLimit(1)
        }
        .font(.system(size: 10.5))
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
            .font(.system(size: 10.5, weight: .semibold))
            .foregroundColor(tone.color)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Capsule().fill(tone.color.opacity(0.12)))
        }
    }

    private var meta: some View {
        Text(libraryShortAgo(project.updatedAt))
            .font(.system(size: 10.5).monospacedDigit())
            .foregroundColor(Color("TextTertiary"))
    }

    private var row: some View {
        HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 8) {
                    iconView
                    Text(project.title)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                }
                note
                Text(preview)
                    .font(.system(size: 12.5))
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

    private var tile: some View {
        let deck = variant == .deck
        return VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .center, spacing: 4) {
                iconView
                Text(project.title)
                    .font(.system(size: deck ? 15.5 : 12.5, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(2)
                    .frame(maxWidth: .infinity, alignment: .leading)
                moreButton
            }
            note.padding(.top, 2)
            Text(preview)
                .font(.system(size: deck ? 13 : 11))
                .lineSpacing(deck ? 6 : 4)
                .foregroundColor(Color("TextSecondary"))
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .clipped()
                .padding(.top, 6)
            HStack(spacing: 6) {
                pill
                Spacer(minLength: 0)
                meta
            }
            .padding(.top, 4)
        }
        .padding(deck ? 14 : 10)
        .frame(width: deck ? 226 : nil, height: deck ? 420 : 196)
    }
}

// MARK: - Edit details

/// EditProjectDetailsDialog: name and description.
private struct EditProjectDetailsSheet: View {
    let project: LibraryProject
    let onSave: (String, String) async throws -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var title: String
    @State private var details: String
    @State private var saving = false
    @State private var error: String? = nil

    init(project: LibraryProject, onSave: @escaping (String, String) async throws -> Void) {
        self.project = project
        self.onSave = onSave
        _title = State(initialValue: project.title)
        _details = State(initialValue: project.description ?? "")
    }

    var body: some View {
        NavigationStack {
            Form {
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
            .navigationTitle("Edit details")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save") { save() }
                        .disabled(saving || title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
        }
        .presentationDetents([.medium, .large])
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
