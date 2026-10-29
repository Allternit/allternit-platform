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
/// List · Grid · Deck switch, "Make something new", "Or start from your
/// connected tools", then "Your artifacts"; a floating search pill and a
/// round + (New artifact menu) at the bottom. Records are the account's
/// Artifacts v2 library, the same ones Desktop and the web show.
struct ArtifactsLibraryView: View {
    @Binding var isSidebarOpen: Bool

    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @StateObject private var store = ArtifactLibraryStore.shared
    @StateObject private var launcher = ArtifactStudioLauncher.shared
    @AppStorage("allternit-overview-layout:phone:artifacts") private var layoutRaw = LibraryLayout.grid.rawValue
    @State private var openArtifact: ArtifactV2Summary?
    @State private var studio: ArtifactStudioSession?
    @State private var notice: String?
    @State private var showingOfficeDocuments = false
    @State private var showingConnectors = false
    @State private var searchDraft = ""
    @State private var tilesShown = false
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
                LibrarySectionHeader(title: nil, onMenu: { openLibraryDrawer($isSidebarOpen) }) { filterMenu }
                ScrollView {
                    VStack(alignment: .leading, spacing: 20) {
                        titleRow
                        if let notice { noticeBar(notice) }
                        makeSection
                        toolsSection
                        yourArtifacts
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 4)
                    .padding(.bottom, 112)
                }
                .scrollDismissesKeyboard(.interactively)
                .refreshable { await store.reload() }
            }
            LibraryFloatingBar(
                query: $searchDraft,
                placeholder: "Search artifacts",
                newLabel: "New artifact",
                wide: .search
            ) {
                newMenuItems
            }
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .sheet(item: $openArtifact) { summary in
            V2ArtifactView(artifactId: summary.id, initialTitle: summary.title)
        }
        .fullScreenCover(item: $studio, onDismiss: { Task { await store.reload() } }) { session in
            ArtifactStudioView(studio: session)
                .environmentObject(modeStore)
                .environmentObject(agentModeStore)
        }
        .sheet(isPresented: $showingOfficeDocuments) {
            OfficeDocumentsView()
        }
        .sheet(isPresented: $showingConnectors) {
            ConnectorsListView()
        }
        .fileImporter(isPresented: $importingPDF, allowedContentTypes: [.pdf]) { result in
            if case .success(let url) = result { Task { await uploadPDF(url) } }
        }
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
        .sheet(item: $debugOfficeDocumentId) { id in
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
        }
        #endif
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

    private var titleRow: some View {
        HStack(spacing: 12) {
            Text("Artifacts")
                .font(.system(size: 30, weight: .medium, design: .serif))
                .tracking(-0.3)
                .foregroundColor(Color("TextPrimary"))
            Spacer()
            LibraryLayoutSwitch(layout: Binding(get: { layout }, set: { layoutRaw = $0.rawValue }))
        }
        .padding(.top, 4)
    }

    private var filtersOn: Bool { store.scope != .all || store.kind != nil }

    private var filterMenu: some View {
        Menu {
            Section("Show") {
                LibraryMenuCheck(title: "Saved work", checked: !store.archived) { setArchived(false) }
                LibraryMenuCheck(title: "Archive", icon: "archivebox", checked: store.archived) { setArchived(true) }
            }
            Section("Owner") {
                ForEach(ArtifactLibraryStore.Scope.allCases) { scope in
                    LibraryMenuCheck(title: scope.label, checked: store.scope == scope) {
                        store.scope = scope
                        Task { await store.reload() }
                    }
                }
            }
            Section("Kind") {
                LibraryMenuCheck(title: "All kinds", checked: store.kind == nil) {
                    store.kind = nil
                    Task { await store.reload() }
                }
                ForEach(ArtifactV2Kind.filterKinds) { kind in
                    LibraryMenuCheck(title: kind.label, icon: kind.systemImage, checked: store.kind == kind) {
                        store.kind = kind
                        Task { await store.reload() }
                    }
                }
            }
        } label: {
            LibraryRoundLabel(icon: "slider.horizontal.3", dot: filtersOn)
        }
        .accessibilityLabel("Filter and sort")
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

    private var makeSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline) {
                sectionLabel("Make something new")
                Spacer()
                Text("\(tiles.count) types")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
            }
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(alignment: .top, spacing: 12) {
                    ForEach(Array(tiles.enumerated()), id: \.element) { index, kind in
                        Button { start(kind) } label: { createCard(kind) }
                            .buttonStyle(.plain)
                            // amt2-enter: the row slides in on open, 45 ms apart.
                            .opacity(tilesShown ? 1 : 0)
                            .offset(x: tilesShown ? 0 : 28)
                            .animation(.timingCurve(0.2, 0.8, 0.2, 1, duration: 0.42).delay(Double(min(index, 6)) * 0.045), value: tilesShown)
                            .disabled(launcher.launching != nil)
                            .accessibilityLabel("New \(kind.label)")
                            .accessibilityHint(kind.description)
                    }
                }
                .padding(.horizontal, 16)
            }
            .padding(.horizontal, -16)
            .onAppear {
                if UIAccessibility.isReduceMotionEnabled { tilesShown = true } else { DispatchQueue.main.async { tilesShown = true } }
            }
        }
    }

    private func createCard(_ kind: ArtifactStudioKind) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            ZStack(alignment: .topTrailing) {
                ArtifactCreateTileArt(kind: kind)
                    .clipShape(RoundedRectangle(cornerRadius: 9, style: .continuous))
                    .padding(3)
                if launcher.launching == kind {
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .fill(Color("BgPanel").opacity(0.7))
                        .overlay(ProgressView().controlSize(.small))
                } else if kind.isBeta {
                    Text("Beta")
                        .font(.system(size: 10, weight: .medium))
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.horizontal, 6)
                        .padding(.vertical, 1)
                        .background(RoundedRectangle(cornerRadius: 6).fill(Color("BgPanel")))
                        .shadow(color: .black.opacity(0.06), radius: 1, y: 1)
                        .padding(6)
                }
            }
            .frame(width: 136, height: 95)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
            .shadow(color: .black.opacity(0.05), radius: 1, y: 1)

            VStack(alignment: .leading, spacing: 2) {
                Text(kind.label)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text(kind.description)
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
                    .lineLimit(2)
                    .multilineTextAlignment(.leading)
            }
            .padding(.horizontal, 2)
            .frame(width: 136, alignment: .leading)
        }
    }

    private func start(_ kind: ArtifactStudioKind) {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
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

    // MARK: - Connected tools

    private var toolsSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            sectionLabel("Or start from your connected tools")
            Button(action: { showingConnectors = true }) {
                HStack(spacing: 6) {
                    Image(systemName: "powerplug")
                        .font(.system(size: 13))
                    Text("Connect your tools")
                        .font(.system(size: 14))
                }
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 14)
                .frame(height: 34)
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("BgPanel")))
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Color("BorderStrong"), lineWidth: 1))
            }
            .buttonStyle(.plain)
        }
    }

    // MARK: - New artifact menu

    @ViewBuilder
    private var newMenuItems: some View {
        Section("Make something new") {
            ForEach(tiles) { kind in
                Button { start(kind) } label: {
                    Label(kind.isBeta ? "\(kind.label)  Beta" : kind.label, systemImage: kind.systemImage)
                }
                .disabled(launcher.launching != nil)
            }
        }
        Section("Upload") {
            Button { importingPDF = true } label: { Label("PDF", systemImage: "square.and.arrow.up") }
            Button { choosingPhoto = true } label: { Label("Image", systemImage: "square.and.arrow.up") }
            Button { choosingVideo = true } label: { Label("Video", systemImage: "square.and.arrow.up") }
        }
        Section("Start from your connected tools") {
            Button { showingConnectors = true } label: { Label("Connect your tools", systemImage: "powerplug") }
        }
        Section {
            Button { showingOfficeDocuments = true } label: { Label("Allternit Office", systemImage: "doc.text") }
        }
    }

    // MARK: - Your artifacts

    private var filterLabel: String? {
        let parts = [store.kind?.label,
                     store.scope == .mine ? "Mine" : store.scope == .shared ? "Shared with me" : nil].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    private var filtered: Bool { store.kind != nil || !store.searchText.isEmpty || store.scope != .all }

    private var items: [ArtifactV2Summary] { store.artifacts.compactMap(\.summary) }

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
            sectionLabel("Your artifacts").padding(.bottom, -4)
            libraryBody
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
            if items.isEmpty {
                stateMessage(
                    icon: nil,
                    title: filtered ? "Nothing matches" : store.archived ? "No archived artifacts" : "No artifacts yet",
                    body: filtered
                        ? "Try another filter or search."
                        : store.archived
                            ? "Archived work stays available here. Restore an artifact to return it to Saved work."
                            : "Ask for a doc, deck, sheet or page in any chat, pick an Output in the message box, or tap + to make something new.",
                    action: filtered ? ("Clear filters", clearFilters) : nil
                )
            } else if store.kind != nil || layout != .list {
                group(items)
            } else {
                // List groups by kind; Grid and Deck are one gallery.
                ForEach(kindGroups(items), id: \.self) { kind in
                    VStack(alignment: .leading, spacing: 8) {
                        Text(ArtifactV2Kind(rawValue: kind)?.label ?? kind.capitalized)
                            .font(.system(size: 12, weight: .medium))
                            .foregroundColor(Color("TextTertiary"))
                        group(items.filter { $0.kind == kind })
                    }
                }
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

    private func kindGroups(_ items: [ArtifactV2Summary]) -> [String] {
        var seen = Set<String>()
        return items.map(\.kind).filter { seen.insert($0).inserted }
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
            GeometryReader { proxy in
                ScrollView(.horizontal, showsIndicators: false) {
                    LazyHStack(spacing: 12) {
                        ForEach(artifacts) { tile($0).frame(width: proxy.size.width * 0.78) }
                    }
                    .scrollTargetLayout()
                }
                .scrollTargetBehavior(.viewAligned)
            }
            .frame(height: 300)
        }
    }

    private func tile(_ summary: ArtifactV2Summary) -> some View {
        ArtifactLibraryCard(
            summary: summary,
            layout: layout,
            archived: store.archived,
            onOpen: {
                UIImpactFeedbackGenerator(style: .light).impactOccurred()
                openArtifact = summary
            },
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
    let onOpen: () -> Void
    let onArchive: () -> Void
    let onDelete: () -> Void

    @State private var confirmDelete = false

    private var kindLabel: String { summary.knownKind?.noun ?? summary.kind.capitalized }

    private var edited: String {
        guard let date = summary.updatedDate else { return "" }
        return "Edited \(date.formatted(.relative(presentation: .named)))"
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

    private var thumbnail: some View {
        ZStack {
            Color("FillNeutral")
            if let raw = summary.thumbnailURL, let url = URL(string: raw) {
                AsyncImage(url: url) { phase in
                    if let image = phase.image {
                        image.resizable().scaledToFill()
                    } else {
                        kindGlyph
                    }
                }
            } else {
                kindGlyph
            }
        }
        .clipped()
    }

    private var kindGlyph: some View {
        Image(systemName: ArtifactV2Kind.systemImage(for: summary.kind))
            .font(.system(size: 24, weight: .light))
            .foregroundColor(Color("TextSecondary"))
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
                .frame(width: 96, height: 72)
                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            VStack(alignment: .leading, spacing: 4) {
                Text(summary.title)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(2)
                Text(edited)
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
            ownerAvatar
            more
        }
        .padding(9)
        .frame(minHeight: 90)
    }

    private var vertical: some View {
        VStack(alignment: .leading, spacing: 0) {
            ZStack(alignment: .topLeading) {
                thumbnail
                    .frame(height: layout == .deck ? 180 : 110)
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
            Image(systemName: ArtifactV2Kind.systemImage(for: summary.kind))
                .font(.system(size: 15, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
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
