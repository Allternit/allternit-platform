import AVFoundation
import Photos
import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

/// The composer "+" sheet, "Add to chat" (phone calm pass step 22, contract
/// C2, brief item 5; web ComposerPlusSheet). A Claude-style grouped list:
/// a strip of recent photos with a Photos link in the title row, then
/// 1. Camera · Add files · GitHub link · Sketch,
/// 2. Permission › · Computer › · Project ›,
/// 3. Web search · Research · Memory (toggles) · Connectors › · Skills › · Plugins ›.
/// Permission, Computer and Project push a page inside the sheet. Run on my
/// phone and Capture to brain live in the chat's ⋯ menu; hosts without one
/// (`chatMenuHoldsExtras` false) keep them here in a fourth group. Picks
/// stage into the shared `AttachmentStore` and show above the composer text.
struct ComposerPlusSheet: View {
    @ObservedObject var attachmentStore: AttachmentStore
    /// The chat on screen. Permission acts on it (or on the pending choice
    /// for a chat that doesn't exist yet).
    var sessionId: String? = nil
    /// Chats that can send to a linked phone show "Run on my phone" (in the
    /// fourth group, only when the host has no chat ⋯ menu).
    var allowsPhoneRun: Bool = false
    /// "Permission · Auto ›" (a chat that has the choice). Off (bot threads,
    /// Code), the row is "Permission settings ›" as before.
    var showsPermission: Bool = true
    /// The host's ⋯ menu holds Run on my phone and Capture to brain.
    var chatMenuHoldsExtras: Bool = false
    /// Reduced motion: the host presents with no slide and the sheet fades in.
    var fadesIn: Bool = false

    @StateObject private var toolOptions = ToolOptionsStore.shared
    @StateObject private var projectStore = ProjectStore.shared
    @StateObject private var brainStore = BrainStore.shared
    @StateObject private var recentPhotos = RecentPhotosLoader()
    @ObservedObject private var permissionStore = ChatPermissionStore.shared
    @ObservedObject private var chipStore = ComputerChipStore.shared
    @ObservedObject private var relay = RuntimeRelayStore.shared
    @ObservedObject private var settings = SettingsStore.shared
    @ReducedMotion private var reduceMotion
    @Environment(\.dismiss) private var dismiss

    @State private var page: ComposerPlusPage
    /// Which way the last page change went (push = forward).
    @State private var pushing = true
    @State private var appeared = false

    @State private var selectedPhotoItems: [PhotosPickerItem] = []
    @State private var isPhotosPickerPresented = false
    @State private var isCameraPresented = false
    @State private var isFilePickerPresented = false
    @State private var isSketchPresented = false
    @State private var isConnectorsPresented = false
    @State private var isBrainCapturePresented = false
    @State private var isNewProjectPresented = false
    @State private var capabilitiesKind: CapabilitiesClient.Kind? = nil
    @State private var isPermissionSettingsPresented = false
    @State private var isConnectComputerPresented = false
    @State private var isComputeGatePresented = false
    /// Set to present the app-owned priming sheet before a system prompt.
    @State private var primingPermission: AppPermission? = nil
    /// Non-permission failures (camera unavailable, unreadable file).
    @State private var pickerError: String? = nil

    @State private var githubUrl = ""
    @State private var isGitHubInputVisible = false
    @State private var isGitHubLoading = false

    init(
        attachmentStore: AttachmentStore,
        sessionId: String? = nil,
        allowsPhoneRun: Bool = false,
        showsPermission: Bool = true,
        chatMenuHoldsExtras: Bool = false,
        fadesIn: Bool = false
    ) {
        self.attachmentStore = attachmentStore
        self.sessionId = sessionId
        self.allowsPhoneRun = allowsPhoneRun
        self.showsPermission = showsPermission
        self.chatMenuHoldsExtras = chatMenuHoldsExtras
        self.fadesIn = fadesIn
        _page = State(initialValue: Self.initialPage(permissionAvailable: showsPermission))
    }

    var body: some View {
        VStack(spacing: 0) {
            header

            ScrollView {
                ZStack(alignment: .top) {
                    pageContent
                        .id(page)
                        .transition(pageTransition)
                }
                .padding(.horizontal, 12)
                .padding(.bottom, 20)
            }
        }
        .opacity(fadesIn && !appeared ? 0 : 1)
        .onAppear {
            // The + itself plays the light haptic as it opens the sheet.
            if fadesIn { withAnimation(Motion.fade) { appeared = true } } else { appeared = true }
        }
        .modifier(PlusSheetBackground())
        .presentationDetents(Self.sheetDetents)
        .presentationCornerRadius(28)
        .presentationDragIndicator(.hidden)
        .photosPicker(
            isPresented: $isPhotosPickerPresented,
            selection: $selectedPhotoItems,
            maxSelectionCount: 5,
            matching: .images
        )
        .sheet(isPresented: $isCameraPresented) {
            CameraPicker { image in stageCameraImage(image) }
                .ignoresSafeArea()
        }
        .sheet(isPresented: $isFilePickerPresented) {
            DocumentPicker { url in stageFile(url) }
        }
        .fullScreenCover(isPresented: $isSketchPresented) {
            SketchSheet { image in stageSketch(image) }
        }
        .sheet(isPresented: $isConnectorsPresented) {
            ConnectorsListView()
        }
        .sheet(isPresented: $isBrainCapturePresented) {
            BrainCaptureSheet()
        }
        .sheet(isPresented: $isNewProjectPresented) {
            NewProjectSheet { title, description in
                Task {
                    if let project = try? await projectStore.createProject(title: title, description: description) {
                        projectStore.selectedProjectId = project.id
                    }
                }
            }
        }
        .sheet(item: $capabilitiesKind) { kind in
            NavigationStack {
                CapabilitiesSettingsView(kind: kind)
                    .toolbar {
                        ToolbarItem(placement: .topBarTrailing) {
                            Button("Done") { capabilitiesKind = nil }
                        }
                    }
            }
            .environmentObject(AuthManager.shared)
        }
        .sheet(isPresented: $isPermissionSettingsPresented) {
            PermissionDefaultsSheet(sessionId: sessionId)
        }
        .sheet(isPresented: $isConnectComputerPresented) {
            ConnectComputerSheet()
        }
        .sheet(isPresented: $isComputeGatePresented) {
            ComputeGateSheet()
        }
        .sheet(item: $primingPermission) { permission in
            PermissionPrimingSheet(permission: permission) {
                handlePrimingContinue(permission)
            }
        }
        .onChange(of: selectedPhotoItems) { _, items in
            guard !items.isEmpty else { return }
            stagePickedPhotos(items)
            selectedPhotoItems = []
        }
    }

    /// DEBUG: the `-open-plus-*` launch arguments pin the sheet to .large so
    /// screenshot verification sees every group (simctl can't scroll sheets).
    private static var sheetDetents: Set<PresentationDetent> {
        #if DEBUG
        if launchArgumentEnabled(LaunchArgument.openPlusSheet)
            || launchArgumentEnabled(LaunchArgument.openPlusPermission)
            || launchArgumentEnabled(LaunchArgument.openPlusComputer) {
            return [.large]
        }
        #endif
        return [.medium, .large]
    }

    private static func initialPage(permissionAvailable: Bool) -> ComposerPlusPage {
        #if DEBUG
        return ComposerPlusPage.initial(
            openPermission: launchArgumentEnabled(LaunchArgument.openPlusPermission),
            openComputer: launchArgumentEnabled(LaunchArgument.openPlusComputer),
            permissionAvailable: permissionAvailable
        )
        #else
        return .root
        #endif
    }

    // MARK: - Pages

    private var pageTransition: AnyTransition {
        if reduceMotion { return .opacity }
        return .asymmetric(
            insertion: .move(edge: pushing ? .trailing : .leading).combined(with: .opacity),
            removal: .move(edge: pushing ? .leading : .trailing).combined(with: .opacity)
        )
    }

    private func go(_ next: ComposerPlusPage) {
        guard next != page else { return }
        Haptics.light()
        pushing = next != .root
        withAnimation(Motion.pick(.easeOut(duration: 0.2), reduced: reduceMotion)) { page = next }
    }

    /// A pick on a sub-page shows its check, then returns to the list.
    private func returnToRoot() {
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(220))
            go(.root)
        }
    }

    @ViewBuilder
    private var pageContent: some View {
        switch page {
        case .root: rootPage
        case .permission: permissionPage
        case .computer: computerPage
        case .project: projectPage
        }
    }

    // MARK: - Header

    private var header: some View {
        VStack(spacing: 6) {
            Capsule()
                .fill(Color("BorderStrong"))
                .frame(width: 36, height: 5)
            ZStack {
                Text(page.title)
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .accessibilityAddTraits(.isHeader)
                HStack {
                    Button(action: {
                        if page == .root { dismiss() } else { go(.root) }
                    }) {
                        Image(systemName: page == .root ? "xmark" : "chevron.left")
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .frame(width: 34, height: 34)
                            .background(Color("FillNeutral"), in: Circle())
                            .frame(width: 44, height: 44)
                            .contentShape(Circle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(page == .root ? "Close" : "Back")
                    .accessibilityIdentifier(page == .root ? "plusSheetClose" : "")
                    Spacer()
                    if page == .root {
                        Button(action: {
                            Haptics.light()
                            photosTapped()
                        }) {
                            Text("Photos")
                                .font(.system(size: 14, weight: .medium))
                                .foregroundColor(Color("TextSecondary"))
                                .frame(minWidth: 44, minHeight: 44)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("plusSheetPhotos")
                    }
                }
            }
        }
        .padding(.horizontal, 10)
        .padding(.top, 8)
        .padding(.bottom, 6)
    }

    // MARK: - Root

    private var rootPage: some View {
        VStack(alignment: .leading, spacing: 12) {
            RecentPhotosStrip(loader: recentPhotos, onPick: stageRecentPhoto, onOpenLibrary: photosTapped)

            if let pickerError {
                FriendlyInlineStateView(
                    style: .error,
                    icon: "exclamationmark.triangle.fill",
                    title: "Something went wrong",
                    message: pickerError
                )
            }

            // Group 1: add something.
            PlusGroup {
                PlusRow(icon: "camera", title: "Camera", testId: "plusSheetCamera") { cameraTapped() }
                PlusDivider()
                PlusRow(icon: "doc.badge.plus", title: "Add files", testId: "plusSheetAddFiles") { isFilePickerPresented = true }
                PlusDivider()
                PlusRow(icon: "link", title: "GitHub link", isActive: isGitHubInputVisible, testId: "plusSheetGithub") {
                    withAnimation(Motion.pick(Motion.deck, reduced: reduceMotion)) { isGitHubInputVisible.toggle() }
                }
                if isGitHubInputVisible {
                    githubField
                }
                PlusDivider()
                PlusRow(icon: "scribble.variable", title: "Sketch") { isSketchPresented = true }
                if IslandBarFlag.isOn {
                    PlusDivider()
                    // The island bar (Features/IslandBar): run one tool of a connected app.
                    PlusRow(icon: "bolt", title: "Do with an app", testId: "plusSheetIslandBar") {
                        dismiss()
                        NotificationCenter.default.post(name: .islandBarOpen, object: nil)
                    }
                }
            }

            // Group 2: how and where this chat runs.
            PlusGroup {
                if showsPermission {
                    PlusRow(
                        icon: "checkmark.shield",
                        title: "Permission",
                        value: permissionStore.mode(for: sessionId).shortLabel,
                        chevron: true
                    ) { go(.permission) }
                } else {
                    PlusRow(icon: "checkmark.shield", title: "Permission settings", chevron: true) {
                        isPermissionSettingsPresented = true
                    }
                }
                PlusDivider()
                PlusRow(
                    icon: "desktopcomputer",
                    title: "Computer",
                    value: computerRowValue(chipStore.state, activeName: relay.activeDevice?.displayName),
                    chevron: true,
                    showsDot: chipStore.showsDot
                ) { go(.computer) }
                PlusDivider()
                PlusRow(icon: "folder", title: "Project", value: selectedProjectTitle, chevron: true) { go(.project) }
            }

            // Group 3: tools.
            PlusGroup {
                Group {
                    PlusToggleRow(icon: "globe", title: "Web search",
                                  hint: "Let the agent search the web for current information",
                                  isOn: $toolOptions.webSearch)
                    PlusDivider()
                    PlusToggleRow(icon: "magnifyingglass", title: "Research",
                                  hint: "Deeper, multi-source research before answering",
                                  isOn: $toolOptions.research)
                    PlusDivider()
                    PlusToggleRow(icon: "brain", title: "Memory",
                                  hint: "Let Allternit learn from your chats to personalize replies",
                                  isOn: $settings.generateMemoryFromHistory)
                }
                PlusDivider()
                PlusRow(icon: "bolt", title: "Connectors", chevron: true, testId: "plusSheetConnectors") { isConnectorsPresented = true }
                PlusDivider()
                PlusRow(icon: "graduationcap", title: "Skills", chevron: true) { capabilitiesKind = .skill }
                PlusDivider()
                PlusRow(icon: "puzzlepiece", title: "Plugins", chevron: true) { capabilitiesKind = .plugin }
            }

            // Hosts with no chat ⋯ menu keep these here.
            if !chatMenuHoldsExtras {
                if allowsPhoneRun {
                    RunOnPhoneRow(onArmed: { dismiss() })
                }
                if brainStore.hasBrain {
                    PlusGroup {
                        PlusRow(
                            icon: "brain.head.profile",
                            title: "Capture to brain",
                            value: brainStore.pendingPush ? "Waiting to sync" : nil,
                            chevron: true
                        ) { isBrainCapturePresented = true }
                    }
                }
            }
        }
    }

    private var selectedProjectTitle: String {
        guard let id = projectStore.selectedProjectId else { return "None" }
        return projectStore.projects.first(where: { $0.id == id })?.title ?? "None"
    }

    // MARK: - GitHub

    private var githubField: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                TextField("github.com/user/repo/blob/main/file", text: $githubUrl)
                    .font(.system(size: 16))
                    .foregroundColor(Color("TextPrimary"))
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .keyboardType(.URL)
                    .submitLabel(.done)
                    .onSubmit { fetchGitHubFile() }
                if isGitHubLoading {
                    ProgressView()
                        .controlSize(.small)
                } else {
                    let empty = githubUrl.trimmingCharacters(in: .whitespaces).isEmpty
                    Button(action: fetchGitHubFile) {
                        Text("Add")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .frame(minWidth: 44, minHeight: 36)
                    }
                    .disabled(empty)
                    .opacity(empty ? 0.4 : 1)
                }
            }
            .padding(.horizontal, 10)
            .background(Color("BgPrimary"), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            Text("Paste a GitHub file URL to fetch its raw contents as an attachment.")
                .font(.system(size: 11))
                .foregroundColor(Color("TextSecondary"))
        }
        .padding(.horizontal, 14)
        .padding(.bottom, 10)
        .transition(reduceMotion ? .opacity : .move(edge: .top).combined(with: .opacity))
    }

    private func fetchGitHubFile() {
        let rawUrl = githubUrl.trimmingCharacters(in: .whitespaces)
        guard !rawUrl.isEmpty else { return }
        isGitHubLoading = true
        Task {
            do {
                let withScheme = rawUrl.hasPrefix("http") ? rawUrl : "https://" + rawUrl
                let raw = withScheme
                    .replacingOccurrences(of: "https://github.com/", with: "https://raw.githubusercontent.com/")
                    .replacingOccurrences(of: "/blob/", with: "/")
                guard let url = URL(string: raw) else { throw URLError(.badURL) }
                let (data, response) = try await URLSession.shared.data(from: url)
                guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
                    throw URLError(.badServerResponse)
                }
                let filename = rawUrl.components(separatedBy: "/").last?.components(separatedBy: "?").first ?? "github-file"
                attachmentStore.add(StagedAttachment(thumbnail: nil, data: data, filename: filename, mediaType: "text/plain"))
                githubUrl = ""
                isGitHubInputVisible = false
            } catch {
                pickerError = "Couldn't fetch that GitHub file."
            }
            isGitHubLoading = false
        }
    }

    // MARK: - Permission page (C2)

    private var permissionPage: some View {
        VStack(alignment: .leading, spacing: 10) {
            PlusGroup {
                let current = permissionStore.mode(for: sessionId)
                ForEach(Array(ChatPermissionMode.allCases.enumerated()), id: \.element) { index, option in
                    if index > 0 { PlusDivider() }
                    Button(action: {
                        Haptics.selection()
                        permissionStore.choose(option, sessionId: sessionId)
                        returnToRoot()
                    }) {
                        HStack(spacing: 12) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(option.label)
                                    .font(.system(size: 15, weight: .medium))
                                    .foregroundColor(Color("TextPrimary"))
                                Text(option.description)
                                    .font(.system(size: 12))
                                    .foregroundColor(Color("TextSecondary"))
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                            Spacer(minLength: 8)
                            if option == current {
                                Image(systemName: "checkmark")
                                    .font(.system(size: 14, weight: .semibold))
                                    .foregroundColor(Color("TextPrimary"))
                            }
                        }
                        .padding(.horizontal, 14)
                        .padding(.vertical, 11)
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(option == current ? [.isSelected] : [])
                }
            }
            Text(sessionId == nil ? "Applies when this chat starts." : "Applies to this chat.")
                .font(.system(size: 12))
                .foregroundColor(Color("TextSecondary"))
                .padding(.horizontal, 6)
            // The defaults page stays reachable ("Permission settings").
            Button(action: {
                Haptics.light()
                isPermissionSettingsPresented = true
            }) {
                Text("Permission settings")
                    .font(.system(size: 13))
                    .underline()
                    .foregroundColor(Color("TextSecondary"))
                    .frame(maxWidth: .infinity, minHeight: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
    }

    // MARK: - Computer page (C4 fix place)

    private var computerPage: some View {
        VStack(alignment: .leading, spacing: 10) {
            let state = chipStore.state
            if state.kind != .none {
                PlusGroup {
                    HStack(spacing: 12) {
                        Image(systemName: "desktopcomputer")
                            .font(.system(size: 16))
                            .foregroundColor(Color("TextSecondary"))
                            .frame(width: 24)
                            .accessibilityHidden(true)
                        Text(state.text)
                            .font(.system(size: 14))
                            .foregroundColor(Color("TextPrimary"))
                            .frame(maxWidth: .infinity, alignment: .leading)
                        if state.showsProgress {
                            ProgressView().controlSize(.small)
                        } else if let label = state.actionLabel, let action = state.action {
                            Button(action: {
                                Haptics.light()
                                if action == .gate { isComputeGatePresented = true } else { isConnectComputerPresented = true }
                            }) {
                                Text(label)
                                    .font(.system(size: 13, weight: .semibold))
                                    .foregroundColor(Color("TextPrimary"))
                                    .padding(.horizontal, 12)
                                    .frame(minHeight: 36)
                                    .background(Color("BgPrimary"), in: Capsule())
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 10)
                }
            }

            PlusGroup {
                if relay.devices.isEmpty {
                    if relay.isLoading {
                        ProgressView()
                            .frame(maxWidth: .infinity, minHeight: 52)
                    } else {
                        PlusPickRow(icon: "desktopcomputer", title: "No computer",
                                    subtitle: "Plan chats work without one", isSelected: true) {}
                    }
                } else {
                    PlusPickRow(icon: "sparkles", title: "Automatic",
                                subtitle: "Your default computer, else the first one online",
                                isSelected: relay.selectedRuntimeId == nil) { pickComputer(nil) }
                    ForEach(relay.devices) { device in
                        PlusDivider()
                        PlusPickRow(
                            icon: device.isWakeable ? "cloud" : "desktopcomputer",
                            title: device.displayName,
                            subtitle: computerStatusLabel(status: device.status, wakeable: device.isWakeable),
                            isSelected: relay.selectedRuntimeId == device.id
                        ) { pickComputer(device.id) }
                    }
                }
            }

            PlusGroup {
                PlusRow(icon: "plus", title: "Add a computer", chevron: true) { isConnectComputerPresented = true }
            }
            Text("Open Allternit Desktop and sign in with this account, or run `gizzi pair` in a terminal.")
                .font(.system(size: 12))
                .foregroundColor(Color("TextSecondary"))
                .padding(.horizontal, 6)
        }
        .onAppear {
            // The dot on + leads here: bring the closed chip back.
            chipStore.restore()
        }
        .task { await relay.refresh() }
    }

    private func pickComputer(_ id: String?) {
        Haptics.selection()
        relay.select(id)
        chipStore.restore()
        returnToRoot()
    }

    // MARK: - Project page

    private var projectPage: some View {
        VStack(alignment: .leading, spacing: 10) {
            PlusGroup {
                PlusPickRow(icon: "tray", title: "None", subtitle: nil,
                            isSelected: projectStore.selectedProjectId == nil) { pickProject(nil) }
                if projectStore.isLoading && projectStore.projects.isEmpty {
                    PlusDivider()
                    ProgressView()
                        .frame(maxWidth: .infinity, minHeight: 52)
                } else if let loadError = projectStore.loadError, projectStore.projects.isEmpty {
                    PlusDivider()
                    FriendlyInlineStateView(
                        style: .error,
                        icon: "exclamationmark.triangle",
                        title: "Couldn't load projects",
                        message: FriendlyErrorMessage.from(loadError),
                        actionTitle: "Retry",
                        action: { projectStore.fetchProjectsIfNeeded(force: true) }
                    )
                    .padding(10)
                }
                ForEach(projectStore.projects) { project in
                    PlusDivider()
                    PlusPickRow(icon: "folder", title: project.title, subtitle: nil,
                                isSelected: projectStore.selectedProjectId == project.id) { pickProject(project.id) }
                }
            }
            PlusGroup {
                PlusRow(icon: "plus", title: "New project", chevron: true) { isNewProjectPresented = true }
            }
        }
        .onAppear { projectStore.fetchProjectsIfNeeded() }
    }

    private func pickProject(_ id: String?) {
        Haptics.selection()
        projectStore.selectedProjectId = id
        returnToRoot()
    }

    // MARK: - Recent photo strip

    private func stageRecentPhoto(_ id: String) {
        Task {
            guard let picked = await recentPhotos.imageData(for: id) else {
                pickerError = "Couldn't read that photo."
                return
            }
            let type = UTType(picked.uti) ?? .jpeg
            attachmentStore.add(StagedAttachment(
                thumbnail: UIImage(data: picked.data),
                data: picked.data,
                filename: "photo.\(type.preferredFilenameExtension ?? "jpg")",
                mediaType: type.preferredMIMEType ?? "image/jpeg"
            ))
            dismiss()
        }
    }

    // MARK: - Permission flows

    private func photosTapped() {
        if PHPhotoLibrary.authorizationStatus(for: .readWrite) == .notDetermined,
           !AppPermission.photos.hasPrimed {
            primingPermission = .photos
            return
        }
        requestPhotoAccess()
    }

    private func requestPhotoAccess() {
        guard PHPhotoLibrary.authorizationStatus(for: .readWrite) == .notDetermined else {
            isPhotosPickerPresented = true
            return
        }
        PHPhotoLibrary.requestAuthorization(for: .readWrite) { _ in
            Task { @MainActor in
                recentPhotos.load()
                isPhotosPickerPresented = true
            }
        }
    }

    private func cameraTapped() {
        guard UIImagePickerController.isSourceTypeAvailable(.camera) else {
            pickerError = "Camera isn't available on this device."
            return
        }
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .notDetermined:
            if !AppPermission.camera.hasPrimed {
                primingPermission = .camera
            } else {
                requestCameraAccess()
            }
        case .authorized:
            isCameraPresented = true
        default:
            pickerError = "Camera access is off. Enable it in Settings to take photos."
        }
    }

    private func requestCameraAccess() {
        AVCaptureDevice.requestAccess(for: .video) { granted in
            Task { @MainActor in
                if granted {
                    isCameraPresented = true
                } else {
                    pickerError = "Camera access is off. Enable it in Settings to take photos."
                }
            }
        }
    }

    private func handlePrimingContinue(_ permission: AppPermission) {
        switch permission {
        case .photos: requestPhotoAccess()
        case .camera: requestCameraAccess()
        case .microphone: break
        case .notifications: break
        }
    }

    // MARK: - Staging (a pick closes the sheet, as on the web)

    private func stagePickedPhotos(_ items: [PhotosPickerItem]) {
        Task {
            for (index, item) in items.enumerated() {
                guard let data = try? await item.loadTransferable(type: Data.self) else { continue }
                let contentType = item.supportedContentTypes.first ?? .jpeg
                let mediaType = contentType.preferredMIMEType ?? "image/jpeg"
                let ext = contentType.preferredFilenameExtension ?? "jpg"
                attachmentStore.add(StagedAttachment(
                    thumbnail: UIImage(data: data),
                    data: data,
                    filename: "photo-\(index + 1).\(ext)",
                    mediaType: mediaType
                ))
            }
            dismiss()
        }
    }

    private func stageCameraImage(_ image: UIImage) {
        guard let data = image.jpegData(compressionQuality: 0.85) else { return }
        attachmentStore.add(StagedAttachment(
            thumbnail: image,
            data: data,
            filename: "camera.jpg",
            mediaType: "image/jpeg"
        ))
        dismiss()
    }

    private func stageSketch(_ image: UIImage) {
        guard let data = image.pngData() else { return }
        attachmentStore.add(StagedAttachment(
            thumbnail: image,
            data: data,
            filename: "sketch.png",
            mediaType: "image/png"
        ))
        dismiss()
    }

    private func stageFile(_ url: URL) {
        guard let data = try? Data(contentsOf: url) else {
            pickerError = "Couldn't read that file."
            return
        }
        let mediaType = UTType(filenameExtension: url.pathExtension)?.preferredMIMEType
            ?? "application/octet-stream"
        attachmentStore.add(StagedAttachment(
            thumbnail: nil,
            data: data,
            filename: url.lastPathComponent,
            mediaType: mediaType
        ))
        dismiss()
    }
}

// MARK: - Grouped list parts (white + neutral fill, never tan)

/// One rounded group of rows (Claude's grouped list).
private struct PlusGroup<Content: View>: View {
    @ViewBuilder var content: () -> Content

    var body: some View {
        VStack(spacing: 0) { content() }
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
            .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
    }
}

private struct PlusDivider: View {
    var body: some View {
        Rectangle()
            .fill(Color("BorderSubtle"))
            .frame(height: 1)
            .padding(.leading, 48)
    }
}

/// Icon, title, an optional value and chevron. The whole row is the button.
private struct PlusRow: View {
    let icon: String
    let title: String
    var value: String? = nil
    var chevron: Bool = false
    var isActive: Bool = false
    /// The folded computer chip's dot, on the Computer row.
    var showsDot: Bool = false
    /// Shared Maestro/Android test id (maestro/ids.md); empty = none.
    var testId: String = ""
    let action: () -> Void

    var body: some View {
        Button(action: {
            Haptics.light()
            action()
        }) {
            HStack(spacing: 12) {
                Image(systemName: icon)
                    .font(.system(size: 17))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 24)
                    .overlay(alignment: .topTrailing) {
                        if showsDot {
                            Circle()
                                .fill(Color("TextPrimary"))
                                .frame(width: 7, height: 7)
                                .offset(x: 3, y: -2)
                                .accessibilityHidden(true)
                        }
                    }
                Text(title)
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Spacer(minLength: 8)
                if let value {
                    Text(value)
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                if chevron {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundColor(Color("TextSecondary"))
                        .opacity(0.7)
                }
            }
            .padding(.horizontal, 14)
            .frame(minHeight: 48)
            .background(isActive ? Color("BorderSubtle").opacity(0.6) : Color.clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(value.map { "\(title), \($0)" } ?? title)
        .accessibilityAddTraits(isActive ? [.isSelected] : [])
        .accessibilityIdentifier(testId)
    }
}

/// A toggle row. The old subtitle is the VoiceOver hint.
private struct PlusToggleRow: View {
    let icon: String
    let title: String
    let hint: String
    @Binding var isOn: Bool

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: icon)
                .font(.system(size: 17))
                .foregroundColor(Color("TextPrimary"))
                .frame(width: 24)
                .accessibilityHidden(true)
            Toggle(isOn: $isOn) {
                Text(title)
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextPrimary"))
            }
            .tint(Color("AccentPrimary"))
            .accessibilityHint(hint)
        }
        .padding(.horizontal, 14)
        .frame(minHeight: 48)
        .onChange(of: isOn) { _, _ in Haptics.selection() }
    }
}

/// A choice on a sub-page: title, optional subtitle, a check when picked.
private struct PlusPickRow: View {
    let icon: String
    let title: String
    let subtitle: String?
    let isSelected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                Image(systemName: icon)
                    .font(.system(size: 16))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 24)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.system(size: 15))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    if let subtitle {
                        Text(subtitle)
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(2)
                    }
                }
                Spacer(minLength: 8)
                if isSelected {
                    Image(systemName: "checkmark")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .frame(minHeight: 48)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(isSelected ? [.isSelected] : [])
    }
}

/// Glass sheet: Liquid Glass on iOS 26 (the system sheet), a thick material
/// before, solid under Reduce Transparency.
private struct PlusSheetBackground: ViewModifier {
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency

    func body(content: Content) -> some View {
        if reduceTransparency {
            content.presentationBackground(Color("BgPrimary"))
        } else {
            modern(content)
        }
    }

    @ViewBuilder private func modern(_ content: Content) -> some View {
        #if compiler(>=6.2)
        if #available(iOS 26.0, *) {
            content
        } else {
            content.presentationBackground(.thickMaterial)
        }
        #else
        content.presentationBackground(.thickMaterial)
        #endif
    }
}

// MARK: - Grow from the + (iOS 18 zoom; plain sheet before)

private let composerPlusZoomID = "composer-plus"

extension View {
    /// Marks the composer's + as the place the sheet grows from.
    @ViewBuilder
    func composerPlusZoomSource(_ namespace: Namespace.ID, enabled: Bool) -> some View {
        if #available(iOS 18.0, *), enabled {
            self.matchedTransitionSource(id: composerPlusZoomID, in: namespace)
        } else {
            self
        }
    }

    /// The sheet side of the zoom from the +.
    @ViewBuilder
    func composerPlusZoomDestination(_ namespace: Namespace.ID, enabled: Bool) -> some View {
        if #available(iOS 18.0, *), enabled {
            self.navigationTransition(.zoom(sourceID: composerPlusZoomID, in: namespace))
        } else {
            self
        }
    }
}

// MARK: - Recent photos

/// The newest photos for the strip at the top of the sheet. Reads the
/// library only when access was already given; otherwise the strip shows one
/// tile that runs the normal priming → system prompt → picker flow.
@MainActor
final class RecentPhotosLoader: ObservableObject {
    @Published private(set) var ids: [String] = []
    @Published private(set) var thumbs: [String: UIImage] = [:]
    private var assets: [String: PHAsset] = [:]

    static var hasAccess: Bool {
        let status = PHPhotoLibrary.authorizationStatus(for: .readWrite)
        return status == .authorized || status == .limited
    }

    func load(limit: Int = 12) {
        guard Self.hasAccess else {
            ids = []
            return
        }
        let options = PHFetchOptions()
        options.sortDescriptors = [NSSortDescriptor(key: "creationDate", ascending: false)]
        options.fetchLimit = limit
        let result = PHAsset.fetchAssets(with: .image, options: options)
        var found: [PHAsset] = []
        for index in 0..<result.count { found.append(result.object(at: index)) }
        assets = Dictionary(found.map { ($0.localIdentifier, $0) }, uniquingKeysWith: { first, _ in first })
        ids = found.map(\.localIdentifier)

        let request = PHImageRequestOptions()
        request.deliveryMode = .opportunistic
        request.resizeMode = .fast
        request.isNetworkAccessAllowed = true
        let side: CGFloat = 234
        for asset in found {
            let id = asset.localIdentifier
            PHImageManager.default().requestImage(
                for: asset,
                targetSize: CGSize(width: side, height: side),
                contentMode: .aspectFill,
                options: request
            ) { @Sendable [weak self] image, _ in
                guard let image else { return }
                Task { @MainActor in self?.thumbs[id] = image }
            }
        }
    }

    /// The full image for a tapped tile, with its type identifier.
    func imageData(for id: String) async -> (data: Data, uti: String)? {
        guard let asset = assets[id] else { return nil }
        let options = PHImageRequestOptions()
        options.isNetworkAccessAllowed = true
        options.deliveryMode = .highQualityFormat
        return await withCheckedContinuation { continuation in
            PHImageManager.default().requestImageDataAndOrientation(for: asset, options: options) { @Sendable data, uti, _, _ in
                if let data {
                    continuation.resume(returning: (data, uti ?? UTType.jpeg.identifier))
                } else {
                    continuation.resume(returning: nil)
                }
            }
        }
    }
}

private struct RecentPhotosStrip: View {
    @ObservedObject var loader: RecentPhotosLoader
    let onPick: (String) -> Void
    let onOpenLibrary: () -> Void

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 4) {
                if loader.ids.isEmpty {
                    Button(action: {
                        Haptics.light()
                        onOpenLibrary()
                    }) {
                        VStack(spacing: 6) {
                            Image(systemName: "photo.on.rectangle")
                                .font(.system(size: 20))
                            Text(RecentPhotosLoader.hasAccess ? "Photos" : "Allow photos")
                                .font(.system(size: 12, weight: .medium))
                        }
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 112, height: 78)
                        .background(Color("FillNeutral"))
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                } else {
                    ForEach(loader.ids, id: \.self) { id in
                        Button(action: {
                            Haptics.light()
                            onPick(id)
                        }) {
                            ZStack {
                                Color("FillNeutral")
                                if let thumb = loader.thumbs[id] {
                                    Image(uiImage: thumb)
                                        .resizable()
                                        .scaledToFill()
                                }
                            }
                            .frame(width: 78, height: 78)
                            .clipped()
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Recent photo")
                    }
                }
            }
        }
        .frame(height: 78)
        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        .onAppear { loader.load() }
    }
}


extension CapabilitiesClient.Kind: Identifiable {
    public var id: String { String(describing: self) }
}

/// "Permission settings": how freely this chat (or the next new one) works.
/// The defaults page the + sheet's Permission page links to (C2), and the
/// whole row where the chat has no Permission choice.
private struct PermissionDefaultsSheet: View {
    let sessionId: String?
    @ObservedObject private var store = ChatPermissionStore.shared
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section {
                    ForEach(ChatPermissionMode.allCases, id: \.self) { option in
                        Button(action: {
                            store.choose(option, sessionId: sessionId)
                            dismiss()
                        }) {
                            HStack(spacing: 12) {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(option.label)
                                        .font(.system(size: 15))
                                        .foregroundColor(Color("TextPrimary"))
                                    Text(option.description)
                                        .font(.system(size: 12))
                                        .foregroundColor(Color("TextSecondary"))
                                }
                                Spacer()
                                if store.mode(for: sessionId) == option {
                                    Image(systemName: "checkmark")
                                        .foregroundColor(Color("TextPrimary"))
                                }
                            }
                        }
                    }
                } footer: {
                    Text(sessionId == nil ? "Applies when this chat starts." : "Applies to this chat.")
                }
            }
            .navigationTitle("Permissions")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .presentationDetents([.medium])
    }
}

// MARK: - Camera picker (UIImagePickerController wrapper)

/// SwiftUI has no camera API — the picker stays in UIKit. Picker callbacks
/// hop to the MainActor before staging.
private struct CameraPicker: UIViewControllerRepresentable {
    let onImage: @MainActor (UIImage) -> Void

    func makeUIViewController(context: Context) -> UIImagePickerController {
        let picker = UIImagePickerController()
        picker.sourceType = .camera
        picker.delegate = context.coordinator
        return picker
    }

    func updateUIViewController(_ uiViewController: UIImagePickerController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(onImage: onImage) }

    final class Coordinator: NSObject, UIImagePickerControllerDelegate, UINavigationControllerDelegate {
        let onImage: @MainActor (UIImage) -> Void

        init(onImage: @escaping @MainActor (UIImage) -> Void) {
            self.onImage = onImage
        }

        func imagePickerController(_ picker: UIImagePickerController, didFinishPickingMediaWithInfo info: [UIImagePickerController.InfoKey: Any]) {
            let image = info[.originalImage] as? UIImage
            picker.dismiss(animated: true)
            guard let image else { return }
            Task { @MainActor in onImage(image) }
        }

        func imagePickerControllerDidCancel(_ picker: UIImagePickerController) {
            picker.dismiss(animated: true)
        }
    }
}

// MARK: - Document picker (UIDocumentPickerViewController wrapper)

/// Files source: any content type, copied into the app inbox on pick so the
/// data stays readable after the picker dismisses. Shared with the project
/// detail's "Add files" flow (ProjectDetailView).
struct DocumentPicker: UIViewControllerRepresentable {
    let onPick: @MainActor (URL) -> Void

    func makeUIViewController(context: Context) -> UIDocumentPickerViewController {
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.item], asCopy: true)
        picker.delegate = context.coordinator
        return picker
    }

    func updateUIViewController(_ uiViewController: UIDocumentPickerViewController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(onPick: onPick) }

    final class Coordinator: NSObject, UIDocumentPickerDelegate {
        let onPick: @MainActor (URL) -> Void

        init(onPick: @escaping @MainActor (URL) -> Void) {
            self.onPick = onPick
        }

        func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
            guard let url = urls.first else { return }
            Task { @MainActor in onPick(url) }
        }
    }
}

