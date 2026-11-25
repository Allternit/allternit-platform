import SwiftUI
import PhotosUI
import UniformTypeIdentifiers

#if DEBUG
/// String wrapper for `.sheet(item:)` debug deep-links.
private struct IdentifiableString: Identifiable {
    var wrappedValue: String
    var id: String { wrappedValue }
}
#endif

/// The Artifacts section as the phone opens it from the drawer (allternit-ai
/// src/views/library/ArtifactsView.tsx `phoneChrome`): a round ☰ and filter
/// (Show · Owner · Kind) on top, the serif "Artifacts" title with the
/// List · Grid · Deck switch, then "Your artifacts" (empty: the kinds); a
/// floating search pill and a round + (New artifact: the deck's mode list
/// sheet) at the bottom. Records are the account's
/// Artifacts v2 library, the same ones Desktop and the web show.
struct ArtifactsLibraryView: View {
    @Binding var isSidebarOpen: Bool

    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @StateObject private var store = ArtifactLibraryStore.shared
    @State private var barHidden = false
    @StateObject private var launcher = ArtifactStudioLauncher.shared
    @AppStorage("allternit-overview-layout:phone:artifacts") private var layoutRaw = LibraryLayout.grid.rawValue
    @State private var openArtifact: ArtifactV2Summary?
    @State private var openOnShare = false
    @State private var show: ArtifactShowChip = .all
    @StateObject private var appsModel = AppsShelfModel()
    @ObservedObject private var pinsStore = DeckPinsStore.shared
    @Environment(\.openURL) private var openURL
    @State private var studio: ArtifactStudioSession?
    @State private var notice: String?
    @State private var deletingSession: ArtifactStudioSession?
    @State private var showingOfficeDocuments = false
    @State private var choosingNew = false
    @State private var searchDraft = ""
    @State private var searching = false
    @State private var importingPDF = false
    @State private var photoItem: PhotosPickerItem?
    @State private var choosingPhoto = false
    @State private var videoItem: PhotosPickerItem?
    @State private var choosingVideo = false
    #if DEBUG
    @State private var debugOfficeDocumentId: IdentifiableString? = nil
    #endif

    private var layout: LibraryLayout { LibraryLayout(rawValue: layoutRaw) ?? .grid }

    var body: some View {
        ZStack(alignment: .bottom) {
            VStack(spacing: 0) {
                LibrarySectionHeader(title: nil, onMenu: { openLibraryDrawer($isSidebarOpen) }) {
                    ProjectsPill(searching: $searching, filtersActive: filtersOn, searchLabel: "Search artifacts") { filterPopover }
                }
                if searching || !searchDraft.isEmpty {
                    ProjectsSearchField(query: $searchDraft, placeholder: "Search artifacts")
                }
                // No nav bar here: the title docks into LibrarySectionHeader's
                // centre, measured from this page's top (the search field can
                // sit between them).
                ACIDockReader { dock in
                    PolishedTabPage(title: "Artifacts", chrome: .overlay(barCenter: dock.barCenter),
                                    introKey: .artifacts, hero: { ArtifactsIntroHero() }) {
                        if let notice { noticeBar(notice) }
                        yourArtifacts
                    }
                    .scrollDismissesKeyboard(.interactively)
                    .refreshable { await store.reload() }
                }
            }
            // Floating "New artifact": keeps every item the old bar's menu had.
            Button { Haptics.light(); choosingNew = true } label: {
                NewArtifactPill()
            }
            .buttonStyle(.plain)
            .accessibilityLabel("New artifact")
            .padding(.bottom, 14)
            .hidesOnScroll(barHidden)
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .tracksScrollForHide($barHidden)
        .fullScreenCover(item: $openArtifact) { summary in
            V2ArtifactView(artifactId: summary.id, initialTitle: summary.title, startOnShare: openOnShare)
        }
        .sheet(item: $appsModel.consent) { request in
            AppsConsentSheet(request: request,
                             onAllow: { Task { await appsModel.confirmInstall(request) } },
                             onCancel: { appsModel.consent = nil })
                .presentationDetents([.medium])
        }
        .alert("Delete this \(deletingSession?.kind.noun.lowercased() ?? "artifact")?", isPresented: Binding(
            get: { deletingSession != nil },
            set: { if !$0 { deletingSession = nil } }
        )) {
            Button("Cancel", role: .cancel) { deletingSession = nil }
            Button("Delete", role: .destructive) {
                if let session = deletingSession { deleteEditorSession(session) }
                deletingSession = nil
            }
        } message: {
            Text("Its chat and file are deleted. People it's shared with lose access. This can't be undone.")
        }
        .fullScreenCover(item: $studio, onDismiss: { Task { await store.reload() } }) { session in
            ArtifactStudioView(studio: session)
                .id(session.sessionId)
                .environmentObject(modeStore)
                .environmentObject(agentModeStore)
        }
        .sheet(isPresented: $showingOfficeDocuments) {
            OfficeDocumentsView()
        }
        .sheet(isPresented: $choosingNew) {
            ArtifactNewModeSheet(modes: newModes, busy: launcher.launching != nil, onPick: pick,
                                 onUpload: { kind in
                                     afterSheet { switch kind { case .image: choosingPhoto = true; case .video: choosingVideo = true } }
                                 },
                                 onOffice: { afterSheet { showingOfficeDocuments = true } })
        }
        .fileImporter(isPresented: $importingPDF, allowedContentTypes: [.pdf]) { result in
            if case .success(let url) = result { Task { await uploadPDF(url) } }
        }
        // The deck's PDF mode lands here: open the file picker straight away.
        .task { if ItCanStore.shared.takePDFPick() { importingPDF = true } }
        .photosPicker(isPresented: $choosingPhoto, selection: $photoItem, matching: .images)
        .onChange(of: photoItem) { _, item in
            guard let item else { return }
            photoItem = nil
            Task { await uploadImage(item) }
        }
        .photosPicker(isPresented: $choosingVideo, selection: $videoItem, matching: .videos)
        .onChange(of: videoItem) { _, item in
            guard let item else { return }
            videoItem = nil
            Task { await uploadVideo(item) }
        }
        #if DEBUG
        .task {
            // `-calm-artifacts-filter=<kind>` / `-calm-artifacts-list` (DEBUG only).
            if launchArgumentEnabled("calm-artifacts-list") { layoutRaw = LibraryLayout.list.rawValue }
            if let value = launchArgumentValue("calm-artifacts-filter"), let kind = ArtifactTileFilter.kind(fromArgument: value) {
                store.kind = kind
                await store.reload()
            }
        }
        .fullScreenCover(item: $debugOfficeDocumentId) { id in
            V2ArtifactView(artifactId: id.wrappedValue)
        }
        .task {
            // `-open-office-documents` / `-open-office-document-id <id>`
            // (DEBUG only): drive the Office surfaces without tap injection.
            let args = CommandLine.arguments
            if args.contains("-open-office-documents") {
                showingOfficeDocuments = true
            }
            if let idx = args.firstIndex(of: "-open-office-document-id"), args.indices.contains(idx + 1) {
                debugOfficeDocumentId = .init(wrappedValue: args[idx + 1])
            }
            // `-calm-artifact-viewer|comments|share[=<id>]` (phone calm I12).
            if CalmArtifactLaunch.isActive {
                debugOfficeDocumentId = .init(wrappedValue: CalmArtifactLaunch.artifactId)
            }
        }
        #endif
        .task { await pinsStore.load() }
        .task {
            searchDraft = store.searchText
            if store.phase == .idle || store.artifacts.isEmpty { await store.reload() }
        }
        .task(id: searchDraft) {
            // Debounced server-side search (`q` matches titles).
            guard searchDraft != store.searchText else { return }
            try? await Task.sleep(for: .milliseconds(300))
            guard !Task.isCancelled else { return }
            store.searchText = searchDraft
            await store.reload()
        }
    }

    // MARK: - Chrome

    private var filtersOn: Bool { store.scope != .all || store.kind != nil }

    private var filterPopover: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 4) {
                OverviewLayoutSwitch(raw: $layoutRaw, fallback: .grid)
                    .padding(.bottom, 6)
                popoverLabel("Show")
                popoverCheck("Saved work", on: !store.archived) { setArchived(false) }
                popoverCheck("Archive", on: store.archived) { setArchived(true) }
                popoverLabel("Owner")
                ForEach(ArtifactLibraryStore.Scope.allCases) { scope in
                    popoverCheck(scope.label, on: store.scope == scope) {
                        store.scope = scope
                        Task { await store.reload() }
                    }
                }
                popoverLabel("Kind")
                popoverCheck("All kinds", on: store.kind == nil) { setKind(nil) }
                ForEach(ArtifactV2Kind.filterKinds) { kind in
                    popoverCheck(kind.label, on: store.kind == kind) { setKind(kind) }
                }
            }
            .padding(16)
        }
        .frame(minWidth: 260, maxHeight: 460)
    }

    private func popoverLabel(_ text: String) -> some View {
        Text(text).font(.caption.weight(.semibold)).foregroundColor(Color("TextSecondary")).padding(.top, 6)
    }

    private func popoverCheck(_ title: String, on: Bool, action: @escaping () -> Void) -> some View {
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

    private func setKind(_ kind: ArtifactV2Kind?) {
        guard store.kind != kind else { return }
        store.kind = kind
        Task { await store.reload() }
    }

    private func setArchived(_ value: Bool) {
        guard store.archived != value else { return }
        store.archived = value
        Task { await store.reload() }
    }

    private func noticeBar(_ text: String) -> some View {
        HStack(spacing: 12) {
            Text(text)
                .font(.system(size: 13))
                .foregroundColor(Color("TextPrimary"))
                .frame(maxWidth: .infinity, alignment: .leading)
            Button(action: { notice = nil }) {
                Image(systemName: "xmark")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 24, height: 24)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Dismiss")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
    }

    private func sectionLabel(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 13, weight: .medium))
            .foregroundColor(Color("TextTertiary"))
    }

    // MARK: - Make something new

    /// The web's gallery order (GALLERY_TILES), minus kinds the org turned off.
    private var tiles: [ArtifactStudioKind] {
        ArtifactStudioKind.galleryOrder.filter { kind in
            guard let template = kind.templateKind, let settings = store.orgSettings else { return true }
            return settings.allows(kind: template)
        }
    }

    /// The deck's mode list (same order), minus kinds the org turned off.
    private var newModes: [ChatDeckMode] {
        ArtifactNewModes.list { kind in
            guard let template = kind.templateKind, let settings = store.orgSettings else { return true }
            return settings.allows(kind: template)
        }
    }

    /// A row in the + New sheet or the empty library: PDF opens the file
    /// picker; every other kind starts its artifact session.
    private func pick(_ mode: ChatDeckMode) {
        if let kind = DeckLaunch.kind(for: mode.entry) {
            startMode(kind, mode)
        } else if mode.entry.pick == "pdf" {
            afterSheet { importingPDF = true }
        }
    }

    /// A picker or sheet can't present while the + New sheet is still dismissing.
    private func afterSheet(_ work: @escaping () -> Void) {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.4, execute: work)
    }

    private func startMode(_ kind: ArtifactStudioKind, _ mode: ChatDeckMode) {
        Haptics.light()
        let can = ItCanStore.shared.choices(for: mode.entry)
        Task {
            do {
                studio = try await launcher.launch(kind, can: can, recordKind: mode.entry.id == "app" ? "app" : nil)
            } catch is CancellationError {
                return
            } catch {
                notice = "Couldn't start a \(kind.noun.lowercased()) session. \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }

    private var selectedTile: ArtifactStudioKind? {
        ArtifactTileFilter.selectedTile(for: store.kind, in: tiles)
    }

    private func start(_ kind: ArtifactStudioKind) {
        Haptics.light()
        Task {
            do {
                studio = try await launcher.launch(kind)
            } catch is CancellationError {
                return
            } catch {
                notice = "Couldn't start a \(kind.noun.lowercased()) session. \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }

    // MARK: - Upload (web createPdfArtifact / createImageArtifact / createVideoArtifact)

    /// Server enforces 16 MiB decoded; leave margin for the base64 transport.
    private static let maxUploadBytes = 15 * 1024 * 1024
    /// Video footage goes to cloud storage, with a sanity cap.
    private static let maxVideoUploadBytes = 500 * 1024 * 1024

    private func uploadPDF(_ url: URL) async {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        guard let data = try? Data(contentsOf: url) else {
            notice = "Couldn't read that PDF."
            return
        }
        guard data.count <= Self.maxUploadBytes else {
            notice = "That PDF is over 15 MB. Split or shrink it and try again."
            return
        }
        let title = url.deletingPathExtension().lastPathComponent
        await upload(name: url.lastPathComponent, input: ArtifactV2CreateInput(
            kind: "pdf", title: title.isEmpty ? "Untitled PDF" : title, body: data.base64EncodedString(),
            bodyFormat: "application/pdf", meta: ["filename": .string(url.lastPathComponent)],
            origin: ArtifactV2Origin(surface: "library")), failure: "Couldn't upload that PDF.")
    }

    private func uploadImage(_ item: PhotosPickerItem) async {
        guard let data = try? await item.loadTransferable(type: Data.self) else {
            notice = "Couldn't read that image."
            return
        }
        guard data.count <= Self.maxUploadBytes else {
            notice = "That image is over 15 MB. Shrink it and try again."
            return
        }
        let type = item.supportedContentTypes.first { $0.conforms(to: .image) }
        let mime = type?.preferredMIMEType ?? "image/jpeg"
        let name = "Image.\(type?.preferredFilenameExtension ?? "jpg")"
        // Small images embed as a data URL; larger ones go to Allternit Cloud
        // storage, the same place generated image URLs come from.
        let body: String
        if await CloudFilesClient.shared.shouldUpload(size: data.count) {
            notice = "Uploading \(name)…"
            do {
                body = try await CloudFilesClient.shared.upload(data, name: name, contentType: mime).url
            } catch {
                notice = error.localizedDescription
                return
            }
        } else {
            body = "data:\(mime);base64,\(data.base64EncodedString())"
        }
        await upload(name: name, input: ArtifactV2CreateInput(
            kind: "image", title: "Untitled image", body: body,
            bodyFormat: "text/uri-list", meta: ["filename": .string(name)],
            origin: ArtifactV2Origin(surface: "library"), capabilities: ["storage": .bool(true)]),
            failure: "Couldn't upload that image.")
    }

    /// A video becomes a video-kind artifact: the footage goes to Allternit
    /// Cloud storage and the body is the empty FilmCraft project (meta carries
    /// the footage ref), the same shape generated videos get.
    private func uploadVideo(_ item: PhotosPickerItem) async {
        guard let movie = try? await item.loadTransferable(type: PickedMovie.self) else {
            notice = "Only video files (MP4, MOV, WebM) can be uploaded as video artifacts."
            return
        }
        defer { try? FileManager.default.removeItem(at: movie.url.deletingLastPathComponent()) }
        let size = (try? FileManager.default.attributesOfItem(atPath: movie.url.path)[.size] as? Int) ?? 0
        guard size <= Self.maxVideoUploadBytes else {
            notice = "That video is over 500 MB. Export a smaller version and try again."
            return
        }
        let name = movie.url.lastPathComponent
        notice = "Uploading \(name)…"
        guard await CloudFilesClient.shared.shouldUpload(size: max(size, CloudFilesClient.minCloudBytes)) else {
            notice = "Sign in to upload videos — the footage is stored in Allternit Cloud."
            return
        }
        let mime = UTType(filenameExtension: movie.url.pathExtension)?.preferredMIMEType ?? "video/mp4"
        let url: String
        do {
            url = try await CloudFilesClient.shared.upload(fileAt: movie.url, name: name, contentType: mime).url
        } catch {
            notice = error.localizedDescription
            return
        }
        let base = movie.url.deletingPathExtension().lastPathComponent.trimmingCharacters(in: .whitespaces)
        let title = base.isEmpty ? "Untitled video" : base
        await upload(name: name, input: ArtifactV2CreateInput(
            kind: "video", title: title, body: Self.initialVideoProject(title),
            bodyFormat: "application/vnd.allternit.video+json",
            meta: ["filename": .string(name), "media": .object(["url": .string(url), "mime": .string(mime)])],
            origin: ArtifactV2Origin(surface: "library"), capabilities: ["storage": .bool(true)]),
            failure: "Couldn't upload that video.")
    }

    /// web buildInitialVideoProject: the current `.fcproj` envelope with an
    /// empty project tree (schema 11).
    static func initialVideoProject(_ title: String) -> String {
        let project: [String: Any] = [
            "schema_version": 11,
            "generator": "Allternit",
            "project": [
                "name": title,
                "settings": [
                    "renderer": "FilmCraft GPU Acceleration (wgpu)",
                    "video_display": "Timecode",
                    "audio_display_samples": true,
                    "default_still_duration": 1270080000000,
                    "default_transition_duration_frames": 24,
                    "default_audio_transition_duration": 254016000000,
                ] as [String: Any],
                "root": ["id": 0, "name": title, "children": [Any]()] as [String: Any],
                "items": [String: Any](),
                "next_id": 1,
            ] as [String: Any],
        ]
        let data = (try? JSONSerialization.data(withJSONObject: project, options: [.sortedKeys])) ?? Data("{}".utf8)
        return String(decoding: data, as: UTF8.self)
    }

    private func upload(name: String, input: ArtifactV2CreateInput, failure: String) async {
        notice = "Uploading \(name)…"
        do {
            let created = try await ArtifactsV2Client.shared.create(input)
            notice = nil
            await store.reload()
            openArtifact = store.artifacts.compactMap(\.summary).first { $0.id == created.id }
        } catch {
            notice = "\(failure) \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }

    // MARK: - Your artifacts

    private var filterLabel: String? {
        let parts = [store.kind?.label,
                     store.scope == .mine ? "Mine" : store.scope == .shared ? "Shared with me" : nil].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    private var filtered: Bool { store.kind != nil || !store.searchText.isEmpty || store.scope != .all }

    private var allItems: [ArtifactV2Summary] { store.artifacts.compactMap(\.summary) }

    /// Artifact ids that are apps (the apps shelf rows made from an artifact).
    private var appIds: Set<String> {
        Set(appsModel.yours.filter { $0.kind == "artifact" }.map(\.slug))
    }

    /// The Apps chip lists only apps.
    private var items: [ArtifactV2Summary] {
        show == .apps ? allItems.filter { appIds.contains($0.id) } : allItems
    }

    private var appMeta: [String: (updated: Date?, visibility: String)] {
        Dictionary(allItems.map { ($0.id, (updated: $0.updatedDate, visibility: $0.visibility)) }, uniquingKeysWith: { a, _ in a })
    }

    private func showChanged(_ chip: ArtifactShowChip) {
        let scope: ArtifactLibraryStore.Scope = chip == .shared ? .shared : .all
        guard store.scope != scope else { return }
        store.scope = scope
        Task { await store.reload() }
    }

    private func makeApp() {
        guard let mode = newModes.first(where: { $0.entry.id == "app" }) else {
            notice = "Apps are turned off for your organization."
            return
        }
        pick(mode)
    }

    private func openApp(_ row: AppsShelfRow) {
        if row.kind == "artifact", let summary = allItems.first(where: { $0.id == row.slug }) {
            openOnShare = false
            openArtifact = summary
            return
        }
        Task {
            if let link = await appsModel.openLink(for: row), let url = AppsShelfLinks.openURL(for: link) {
                openURL(url)
            } else {
                notice = "Couldn’t open \(row.name). Try again."
            }
        }
    }

    @ViewBuilder
    private var yourArtifacts: some View {
        VStack(alignment: .leading, spacing: 12) {
            if store.archived || filterLabel != nil {
                HStack(spacing: 8) {
                    Text("Showing \([store.archived ? "Archive" : nil, filterLabel].compactMap { $0 }.joined(separator: " · "))")
                        .font(.system(size: 12.5))
                        .foregroundColor(Color("TextSecondary"))
                    Button("Clear") {
                        store.archived = false
                        store.scope = .all
                        store.kind = nil
                        Task { await store.reload() }
                    }
                    .font(.system(size: 12.5, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                }
            }
            ArtifactsShowChips(value: $show, onChange: showChanged)
            if (show == .all || show == .apps) && !store.archived && store.phase != .signedOut {
                ArtifactAppsRow(model: appsModel, meta: appMeta, busy: launcher.launching != nil,
                                onMakeApp: makeApp, onOpen: openApp)
                    .padding(.vertical, 4)
            }
            if show != .plans { sectionLabel(show == .apps ? "Apps" : show == .shared ? "Shared with you" : "Your artifacts").padding(.bottom, -4) }
            if show != .plans, let tile = selectedTile { newRow(tile) }
            if show != .plans && !store.archived && !filtered {
                ForEach(launcher.editorSessions) { session in
                    editorSessionRow(session)
                }
            }
            if show == .plans {
                PlansListView { summary in
                    openOnShare = false
                    openArtifact = summary
                }
            } else if !(items.isEmpty && store.phase == .loaded && !filtered && !store.archived && !launcher.editorSessions.isEmpty) {
                libraryBody
            }
        }
    }

    /// A Docs, Sheets, Slides or Design session: reopens its chat and editor.
    /// The ⋯ beside it holds Delete.
    private func editorSessionRow(_ session: ArtifactStudioSession) -> some View {
        HStack(spacing: 4) {
            Button {
                Haptics.light()
                studio = session
            } label: {
                HStack(spacing: 10) {
                    Image(session.kind.iconAsset)
                        .resizable()
                        .scaledToFit()
                        .frame(width: 24, height: 24)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(session.title)
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                        Text(session.kind.label)
                            .font(.system(size: 12.5))
                            .foregroundColor(Color("TextTertiary"))
                    }
                    Spacer(minLength: 0)
                    Text(libraryShortAgo(session.startedAt))
                        .font(.system(size: 12).monospacedDigit())
                        .foregroundColor(Color("TextTertiary"))
                }
                .padding(.leading, 12)
                .frame(minHeight: 60)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            Menu {
                Button(role: .destructive) { deletingSession = session } label: { Label("Delete", systemImage: "trash") }
            } label: {
                Image(systemName: "ellipsis")
                    .font(.system(size: 14, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("More for \(session.title)")
            .padding(.trailing, 4)
        }
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    private func deleteEditorSession(_ session: ArtifactStudioSession) {
        Task {
            do {
                try await launcher.delete(session)
                _ = ShellSessionsStore.shared.remove(id: session.sessionId)
            } catch {
                notice = "Couldn't delete “\(session.title)”. \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }

    @ViewBuilder
    private var libraryBody: some View {
        switch store.phase {
        case .idle, .loading:
            skeleton
        case .signedOut:
            stateMessage(
                icon: nil,
                title: "Sign in to see your artifacts",
                body: "Artifacts are saved to your Allternit account, so they follow you to every device.",
                action: ("Sign in", signIn)
            )
        case .failed(let message):
            let offline = message.localizedCaseInsensitiveContains("offline") || message.localizedCaseInsensitiveContains("internet")
            stateMessage(
                icon: offline ? "wifi.slash" : nil,
                title: offline ? "You’re offline" : "Couldn’t load your artifacts",
                body: offline ? "Your artifacts will show here when you’re back online." : FriendlyErrorMessage.from(message),
                action: ("Try again", retry)
            )
        case .loaded:
            if items.isEmpty, let kind = store.kind, !store.archived, store.searchText.isEmpty, store.scope == .all {
                Text(ArtifactTileFilter.emptyText(kind: kind, tile: selectedTile))
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 24)
                    .accessibilityAddTraits(.updatesFrequently)
            } else if items.isEmpty, !filtered, !store.archived {
                ArtifactEmptyKinds(modes: newModes, busy: launcher.launching != nil, onPick: pick)
            } else if items.isEmpty {
                stateMessage(
                    icon: nil,
                    title: filtered ? "Nothing matches" : store.archived ? "No archived artifacts" : "No artifacts yet",
                    body: filtered
                        ? "Try another filter or search."
                        : store.archived
                            ? "Archived work stays available here. Restore an artifact to return it to Saved work."
                            : "Ask for a doc, deck, sheet or page in any chat, pick an Output in the message box, or tap New artifact.",
                    action: filtered ? ("Clear filters", clearFilters) : nil
                )
            } else {
                // Rows carry "Kind · Edited <time>", so list, grid and deck are one flat set.
                group(items)
            }
            if store.isLoadingMore {
                Text("Loading…")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextTertiary"))
                    .frame(maxWidth: .infinity)
            }
        }
    }

    private func signIn() {
        AuthManager.shared.isPresentingAuth = true
    }

    private func retry() {
        Task { await store.reload() }
    }

    private func clearFilters() {
        store.scope = .all
        store.kind = nil
        searchDraft = ""
        store.searchText = ""
        Task { await store.reload() }
    }

    /// D8: while filtered, the list starts with "New <Kind>" (the tile's old create action).
    private func newRow(_ tile: ArtifactStudioKind) -> some View {
        Button { start(tile) } label: {
            HStack(spacing: 12) {
                Image(systemName: "plus")
                    .font(.system(size: 18, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 96, height: 58)
                    .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
                Text(ArtifactTileFilter.newRowTitle(tile))
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                Spacer(minLength: 0)
                if launcher.launching == tile { ProgressView().controlSize(.small) }
            }
            .padding(9)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(launcher.launching != nil)
        .accessibilityLabel(ArtifactTileFilter.newRowTitle(tile))
    }

    @ViewBuilder
    private func group(_ artifacts: [ArtifactV2Summary]) -> some View {
        switch layout {
        case .list:
            LazyVStack(spacing: 10) {
                ForEach(artifacts) { tile($0) }
            }
        case .grid:
            LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)], spacing: 10) {
                ForEach(artifacts) { tile($0) }
            }
        case .deck:
            ScrollView(.horizontal, showsIndicators: false) {
                LazyHStack(spacing: 12) {
                    ForEach(artifacts) { tile($0).frame(width: OverviewDeck.width, height: OverviewDeck.height) }
                }
                .scrollTargetLayout()
            }
            .scrollTargetBehavior(.viewAligned)
            .frame(height: OverviewDeck.height)
        }
    }

    private func tile(_ summary: ArtifactV2Summary) -> some View {
        ArtifactLibraryCard(
            summary: summary,
            layout: layout,
            archived: store.archived,
            appRow: appsModel.yours.first { $0.kind == "artifact" && $0.slug == summary.id },
            pinned: pinsStore.isPinned(summary.id),
            onOpen: {
                Haptics.light()
                openOnShare = false
                openArtifact = summary
            },
            onShare: {
                openOnShare = true
                openArtifact = summary
            },
            onInstallApp: { if let row = appsModel.yours.first(where: { $0.kind == "artifact" && $0.slug == summary.id }) { appsModel.requestInstall(row) } },
            onCopyAppLink: {
                if let link = appsModel.yours.first(where: { $0.kind == "artifact" && $0.slug == summary.id })?.link {
                    UIPasteboard.general.string = link
                    notice = "App link copied."
                } else {
                    notice = "This app has no link yet."
                }
            },
            onTogglePin: { Task { await pinsStore.toggle(id: summary.id, kind: summary.kind, title: summary.title) } },
            onArchive: { changeArchive(summary) },
            onDelete: { delete(summary) }
        )
        .task {
            if let saved = store.artifacts.first(where: { $0.id == summary.id }) {
                await store.loadMoreIfNeeded(current: saved)
            }
        }
    }

    private func changeArchive(_ summary: ArtifactV2Summary) {
        let restoring = store.archived
        Task {
            do {
                try await store.setArchived(id: summary.id, archived: !restoring)
                notice = restoring ? "Restored to saved work." : "Archived. You can restore it from Archive."
            } catch {
                notice = "Couldn’t update your archive. Try again."
            }
        }
    }

    private func delete(_ summary: ArtifactV2Summary) {
        Task {
            do {
                try await store.delete(id: summary.id)
            } catch {
                notice = FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
    }

    private var skeleton: some View {
        let count = layout == .grid ? 6 : layout == .deck ? 2 : 5
        let height: CGFloat = layout == .grid ? 190 : layout == .deck ? 260 : 90
        return Group {
            if layout == .grid {
                LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)], spacing: 10) {
                    ForEach(0..<count, id: \.self) { _ in skeletonBlock(height) }
                }
            } else {
                VStack(spacing: 10) {
                    ForEach(0..<count, id: \.self) { _ in skeletonBlock(height) }
                }
            }
        }
        .accessibilityLabel("Loading artifacts")
    }

    private func skeletonBlock(_ height: CGFloat) -> some View {
        RoundedRectangle(cornerRadius: 16, style: .continuous)
            .fill(Color("FillNeutral"))
            .frame(height: height)
    }

    /// StateMessage: a dashed card with a title, a line and an action.
    private func stateMessage(icon: String?, title: String, body: String, action: (String, @MainActor () -> Void)?) -> some View {
        VStack(spacing: 8) {
            if let icon {
                Image(systemName: icon)
                    .font(.system(size: 20))
                    .foregroundColor(Color("TextTertiary"))
            }
            Text(title)
                .font(.system(size: 15, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
            Text(body)
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
                .multilineTextAlignment(.center)
            if let action {
                Button(action: { action.1() }) {
                    Text(action.0)
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 12)
                        .frame(height: 34)
                        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("FillNeutral")))
                }
                .buttonStyle(.plain)
                .padding(.top, 8)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 24)
        .padding(.vertical, 56)
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(Color("BorderStrong"), style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
        )
    }
}

// MARK: - Card

/// One saved artifact (ArtifactTile → ArtifactPreviewCard): a thumbnail
/// card in Grid / Deck, a horizontal thumbnail card in the phone's List.
/// The kind is the card's badge, so the line is just when it was edited.
private struct ArtifactLibraryCard: View {
    let summary: ArtifactV2Summary
    let layout: LibraryLayout
    let archived: Bool
    /// The apps shelf row when this artifact is an app.
    let appRow: AppsShelfRow?
    let pinned: Bool
    let onOpen: () -> Void
    let onShare: () -> Void
    let onInstallApp: () -> Void
    let onCopyAppLink: () -> Void
    let onTogglePin: () -> Void
    let onArchive: () -> Void
    let onDelete: () -> Void

    @State private var confirmDelete = false

    private var isApp: Bool { appRow != nil }

    private var kindLabel: String { isApp ? "App" : (summary.knownKind?.noun ?? summary.kind.capitalized) }

    /// Kinds that run as a page can become apps; Docs, Sheets and Slides cannot.
    private var canBecomeApp: Bool { ["page", "dashboard", "ops", "app", "build", "code", "html"].contains(summary.kind) }

    private var edited: String {
        guard let date = summary.updatedDate else { return "" }
        let when = date.formatted(.relative(presentation: .named))
        if isApp { return "Updated \(when) · \(ArtifactAppsRow.visibilityLabel(summary.visibility))" }
        return "Edited \(when)"
    }

    var body: some View {
        Button(action: onOpen) {
            Group {
                if layout == .list { horizontal } else { vertical }
            }
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .contextMenu { menuItems }
        .alert("Delete permanently?", isPresented: $confirmDelete) {
            Button("Cancel", role: .cancel) {}
            Button("Delete permanently", role: .destructive, action: onDelete)
        } message: {
            Text("This can’t be undone.")
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(summary.title), \(kindLabel)")
    }

    @ViewBuilder
    private var menuItems: some View {
        Button(action: onOpen) { Label("Open", systemImage: "arrow.up.right.square") }
        if summary.isMine {
            Button(action: onShare) { Label("Share", systemImage: "square.and.arrow.up") }
            if isApp {
                Button(action: onInstallApp) { Label("Install app", systemImage: "arrow.down.circle") }
                Button(action: onCopyAppLink) { Label("Copy app link", systemImage: "link") }
            } else if canBecomeApp {
                Button(action: onShare) { Label("Make it an app", systemImage: "plus.circle") }
            }
        }
        Divider()
        Button(action: onTogglePin) {
            Label(pinned ? "Unpin" : "Pin", systemImage: pinned ? "pin.slash" : "pin")
        }
        Button(action: onArchive) {
            Label(archived ? "Restore to saved work" : "Archive",
                  systemImage: archived ? "arrow.uturn.backward" : "archivebox")
        }
        if summary.isMine {
            Divider()
            Button(role: .destructive) { confirmDelete = true } label: { Label("Delete", systemImage: "trash") }
        }
    }

    private var more: some View {
        Menu { menuItems } label: {
            Image(systemName: "ellipsis")
                .font(.system(size: 14, weight: .bold))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 28, height: 28)
                .background(Circle().fill(Color("BgPanel").opacity(layout == .list ? 0 : 0.92)))
        }
        .accessibilityLabel("More for \(summary.title)")
    }

    /// The record's own content (ArtifactCardPreview): the list endpoint
    /// sends no picture, so the card draws one from the body.
    private var thumbnail: some View {
        ArtifactCardPreview(summary: summary)
    }

    private var badge: some View {
        Text(kindLabel)
            .font(.system(size: 10.5, weight: .medium))
            .foregroundColor(Color("TextSecondary"))
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(RoundedRectangle(cornerRadius: 6).fill(Color("BgPanel").opacity(0.92)))
    }

    @ViewBuilder
    private var ownerAvatar: some View {
        if !summary.isMine {
            let name = summary.owner.name
            Text(String((name ?? "?").prefix(1)).uppercased())
                .font(.system(size: 10.5, weight: .semibold))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 24, height: 24)
                .background(Circle().fill(Color("BorderSubtle")))
                .accessibilityLabel(name.map { "Shared by \($0)" } ?? "Shared with you")
        }
    }

    private var horizontal: some View {
        HStack(spacing: 12) {
            thumbnail
                .frame(width: 96, height: 58)
                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            VStack(alignment: .leading, spacing: 4) {
                Text(summary.title)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(2)
                Text(ArtifactTileFilter.subtitle(kindLabel: kindLabel, edited: edited))
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
            ownerAvatar
            more
        }
        .padding(9)
        .frame(minHeight: 76)
    }

    private var vertical: some View {
        VStack(alignment: .leading, spacing: 0) {
            ZStack(alignment: .topLeading) {
                thumbnail
                    .frame(height: layout == .deck ? OverviewDeck.height * 0.6 : 110)
                HStack {
                    badge
                    Spacer()
                    more
                }
                .padding(6)
            }
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .top, spacing: 6) {
                    Text(summary.title)
                        .font(.system(size: 13.5, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(2)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    ownerAvatar
                }
                Text(edited)
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
                    .lineLimit(1)
            }
            .padding(10)
        }
    }
}

/// One library row (the studio pane's artifact list).
struct ArtifactLibraryRow: View {
    let summary: ArtifactV2Summary

    var body: some View {
        HStack(spacing: 12) {
            Image(ArtifactV2Kind.iconAsset(for: summary.kind))
                .resizable()
                .frame(width: 22, height: 22)
                .frame(width: 34, height: 34)
                .background(Color("FillNeutral"))
                .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            VStack(alignment: .leading, spacing: 2) {
                Text(summary.title)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text(metadata)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
            Image(systemName: "chevron.right")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
        }
        .padding(.horizontal, 12)
        .frame(minHeight: 58)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }

    private var metadata: String {
        var parts: [String] = [summary.knownKind?.noun ?? summary.kind.capitalized]
        if !summary.isMine {
            parts.append(summary.owner.name.map { "Shared by \($0)" } ?? "Shared")
        } else if summary.isLegacy {
            parts.append("Legacy")
        }
        if let date = summary.updatedDate {
            parts.append(date.formatted(.relative(presentation: .named)))
        }
        return parts.joined(separator: " · ")
    }
}


/// A picked video copied out of Photos to a temporary file, so the upload
/// streams from disk instead of holding the footage in memory.
struct PickedMovie: Transferable {
    let url: URL

    static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(contentType: .movie) { movie in
            SentTransferredFile(movie.url)
        } importing: { received in
            // Keep the original file name (it becomes the title) in a
            // folder of its own.
            let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            let copy = folder.appendingPathComponent(received.file.lastPathComponent)
            try FileManager.default.copyItem(at: received.file, to: copy)
            return PickedMovie(url: copy)
        }
    }
}


// MARK: - Polish pieces

/// Tiles shrink a little on press (polish contract, pressPill 0.94).
private struct ArtifactTilePressStyle: ButtonStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? PolishTokens.pressPill : 1)
            .animation(reduceMotion ? nil : PolishTokens.press, value: configuration.isPressed)
    }
}

/// The floating "New artifact" label: a glass pill (the Menu keeps every item).
private struct NewArtifactPill: View {
    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "plus").font(.system(size: 15, weight: .semibold))
            Text("New artifact").font(.system(size: 16, weight: .semibold))
        }
        .foregroundStyle(PolishTokens.ink)
        .padding(.horizontal, 22)
        .frame(height: 48)
        .polishGlass(cornerRadius: 24)
        .contentShape(Capsule())
    }
}

/// First-visit intro hero: the real Artifacts page. Docs gets tapped, then
/// three artifacts appear under "Your artifacts". Settles on the end state
/// with Reduce Motion.
private struct ArtifactsIntroHero: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var docsPressed = false
    @State private var shown = 0

    private let types = ["Docs", "Sheets", "Slides"]
    private let mine: [(String, String)] = [("Q4 plan", "Doc"), ("Budget 2027", "Sheet"), ("Investor deck", "Slides")]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            label("Everything you make, in one place", trailing: nil)
            HStack(spacing: 7) {
                ForEach(Array(types.enumerated()), id: \.offset) { i, name in
                    VStack(alignment: .leading, spacing: 3) {
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .fill(PolishTokens.neutralFill)
                            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(PolishTokens.glassBorder, lineWidth: 1))
                            .overlay(thumb(i, progress: 1).padding(.horizontal, 22).padding(.vertical, 6))
                            .frame(height: 38)
                            .scaleEffect(i == 0 && docsPressed ? PolishTokens.pressPill : 1)
                        Text(name).font(.system(size: 10, weight: .semibold)).foregroundStyle(PolishTokens.ink)
                    }
                }
            }
            label("Recent", trailing: nil).padding(.top, 6)
            HStack(spacing: 7) {
                ForEach(Array(mine.enumerated()), id: \.offset) { i, item in
                    VStack(alignment: .leading, spacing: 3) {
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .fill(PolishTokens.neutralFill)
                            .overlay(thumb(i, progress: 1).padding(6))
                            .frame(height: 46)
                        Text(item.0).font(.system(size: 10, weight: .semibold)).foregroundStyle(PolishTokens.ink).lineLimit(1)
                        Text("\(item.1) · just now").font(.system(size: 9)).foregroundStyle(Color("TextTertiary")).lineLimit(1)
                    }
                    .opacity(shown > i ? 1 : 0)
                    .offset(y: shown > i ? 0 : 12)
                    .scaleEffect(shown > i ? 1 : 0.9)
                }
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPanel"))
        .task { await play() }
    }

    private func label(_ text: String, trailing: String?) -> some View {
        HStack {
            Text(text).font(.system(size: 10, weight: .medium)).foregroundStyle(Color("TextSecondary"))
            Spacer()
            if let trailing { Text(trailing).font(.system(size: 10)).foregroundStyle(Color("TextSecondary")) }
        }
    }

    /// A tiny white page: lines for a doc, cells for a sheet, a bar for slides.
    private func thumb(_ i: Int, progress: CGFloat) -> some View {
        RoundedRectangle(cornerRadius: 4, style: .continuous)
            .fill(Color.white)
            .overlay(alignment: .top) {
                Group {
                    if i == 0 {
                        VStack(alignment: .leading, spacing: 3) {
                            ForEach(0..<3, id: \.self) { j in
                                Capsule().fill(j == 0 ? PolishTokens.ink : Color(hex: "#D4D4D4")).frame(height: 2.5)
                                    .padding(.trailing, CGFloat(j) * 8)
                            }
                        }
                    } else if i == 1 {
                        LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 2), count: 4), spacing: 2) {
                            ForEach(0..<12, id: \.self) { j in
                                RoundedRectangle(cornerRadius: 1).fill([2, 5, 8].contains(j) ? Color(hex: "#15803D") : Color(hex: "#E5E5E5")).frame(height: 4)
                            }
                        }
                    } else {
                        VStack(alignment: .leading, spacing: 3) {
                            Capsule().fill(PolishTokens.ink).frame(height: 3).padding(.trailing, 14)
                            RoundedRectangle(cornerRadius: 2).fill(PolishTokens.coral.opacity(0.2)).frame(height: 9)
                        }
                    }
                }
                .padding(5)
            }
            .shadow(color: .black.opacity(0.08), radius: 1.5, y: 1)
    }

    private func play() async {
        if reduceMotion { shown = 3; return }
        shown = 0
        try? await Task.sleep(nanoseconds: 600_000_000)
        withAnimation(PolishTokens.press) { docsPressed = true }
        try? await Task.sleep(nanoseconds: 200_000_000)
        withAnimation(PolishTokens.press) { docsPressed = false }
        try? await Task.sleep(nanoseconds: 350_000_000)
        for n in 1...3 {
            withAnimation(PolishTokens.base) { shown = n }
            try? await Task.sleep(nanoseconds: 450_000_000)
        }
    }
}
