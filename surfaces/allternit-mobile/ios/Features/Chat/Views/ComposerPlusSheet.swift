import AVFoundation
import Photos
import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

/// The composer "+" sheet (web ComposerPlusSheet, phone form): an icon grid
/// (Files · GitHub · Web · Project · Connectors), the GitHub URL field and
/// project list inline, the Web search / Research toggles, then Capture to
/// brain · Plugins · Skills · Permission settings. Picks stage into the
/// shared `AttachmentStore` and show as previews above the composer text.
struct ComposerPlusSheet: View {
    @ObservedObject var attachmentStore: AttachmentStore
    /// The chat on screen — Permission settings acts on it (or the pending
    /// choice for a new chat).
    var sessionId: String? = nil

    @StateObject private var toolOptions = ToolOptionsStore.shared
    @StateObject private var projectStore = ProjectStore.shared
    @StateObject private var brainStore = BrainStore.shared
    @Environment(\.dismiss) private var dismiss

    @State private var selectedPhotoItems: [PhotosPickerItem] = []
    @State private var isPhotosPickerPresented = false
    @State private var isCameraPresented = false
    @State private var isFilePickerPresented = false
    /// Files: iOS offers the same three sources a web file input does.
    @State private var isFileSourceDialogPresented = false
    @State private var isConnectorsPresented = false
    @State private var isBrainCapturePresented = false
    @State private var isNewProjectPresented = false
    @State private var capabilitiesKind: CapabilitiesClient.Kind? = nil
    @State private var isPermissionSettingsPresented = false
    /// Set to present the app-owned priming sheet before a system prompt.
    @State private var primingPermission: AppPermission? = nil
    /// Non-permission failures (camera unavailable, unreadable file).
    @State private var pickerError: String? = nil

    @State private var githubUrl = ""
    @State private var isGitHubInputVisible = false
    @State private var isGitHubLoading = false
    @State private var isProjectMenuOpen = false

    var body: some View {
        VStack(spacing: 0) {
            ZStack {
                Capsule()
                    .fill(Color("BorderStrong"))
                    .frame(width: 36, height: 5)
                HStack {
                    Spacer()
                    Button(action: { dismiss() }) {
                        Image(systemName: "xmark")
                            .font(.system(size: 13, weight: .bold))
                            .foregroundColor(Color("TextSecondary"))
                            .frame(width: 32, height: 32)
                            .contentShape(Circle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Close")
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 8)
            .padding(.bottom, 4)

            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    if let pickerError {
                        FriendlyInlineStateView(
                            style: .error,
                            icon: "exclamationmark.triangle.fill",
                            title: "Something went wrong",
                            message: pickerError
                        )
                    }

                    iconGrid

                    if isGitHubInputVisible {
                        githubPanel
                    }

                    if isProjectMenuOpen {
                        projectPanel
                    }

                    VStack(spacing: 4) {
                        toggleRow(
                            icon: "globe",
                            title: "Web search",
                            subtitle: "Let the agent search the web for current information",
                            isOn: $toolOptions.webSearch
                        )
                        Rectangle()
                            .fill(Color("BorderSubtle"))
                            .frame(height: 1)
                            .padding(.leading, 44)
                        toggleRow(
                            icon: "pencil.tip",
                            title: "Research",
                            subtitle: "Deeper, multi-source research before answering",
                            isOn: $toolOptions.research
                        )
                    }
                    .plusPanel()

                    VStack(spacing: 8) {
                        if brainStore.hasBrain {
                            listRow(
                                icon: "brain",
                                title: "Capture to brain",
                                subtitle: brainStore.pendingPush
                                    ? "Waiting to sync — will retry automatically"
                                    : "Save an idea or pain to your second brain"
                            ) { isBrainCapturePresented = true }
                        }
                        listRow(icon: "puzzlepiece", title: "Plugins", subtitle: "Browse and manage plugins") {
                            capabilitiesKind = .plugin
                        }
                        listRow(icon: "graduationcap", title: "Skills", subtitle: "Browse and manage skills") {
                            capabilitiesKind = .skill
                        }
                        listRow(icon: "checkmark.shield", title: "Permission settings", subtitle: "Review permission defaults") {
                            isPermissionSettingsPresented = true
                        }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 20)
            }
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .presentationDetents(Self.sheetDetents)
        .presentationCornerRadius(28)
        .presentationDragIndicator(.hidden)
        .confirmationDialog("Add files", isPresented: $isFileSourceDialogPresented, titleVisibility: .hidden) {
            Button("Photo Library") { photosTapped() }
            Button("Take Photo") { cameraTapped() }
            Button("Choose File") { isFilePickerPresented = true }
        }
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

    /// DEBUG: `-open-plus-sheet` pins the sheet to .large so screenshot
    /// verification can see every section (simctl can't scroll sheets).
    private static var sheetDetents: Set<PresentationDetent> {
        #if DEBUG
        if launchArgumentEnabled("open-plus-sheet") { return [.large] }
        #endif
        return [.medium, .large]
    }

    // MARK: - Icon grid

    private var iconGrid: some View {
        let columns = Array(repeating: GridItem(.flexible(), spacing: 8), count: 4)
        return LazyVGrid(columns: columns, spacing: 8) {
            gridButton(icon: "photo", label: "Files") {
                isFileSourceDialogPresented = true
            }
            gridButton(icon: "chevron.left.forwardslash.chevron.right", label: "GitHub", isActive: isGitHubInputVisible) {
                withAnimation(.easeInOut(duration: 0.2)) {
                    isGitHubInputVisible.toggle()
                    isProjectMenuOpen = false
                }
            }
            gridButton(icon: "globe", label: "Web", isActive: toolOptions.webSearch, showCheck: toolOptions.webSearch) {
                toolOptions.webSearch.toggle()
                isProjectMenuOpen = false
            }
            gridButton(icon: "folder", label: "Project", isActive: isProjectMenuOpen || projectStore.selectedProjectId != nil) {
                withAnimation(.easeInOut(duration: 0.2)) {
                    isProjectMenuOpen.toggle()
                    isGitHubInputVisible = false
                }
            }
            gridButton(icon: "bolt", label: "Connectors") {
                isConnectorsPresented = true
            }
        }
    }

    private func gridButton(
        icon: String,
        label: String,
        isActive: Bool = false,
        showCheck: Bool = false,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: {
            hapticLight()
            action()
        }) {
            VStack(spacing: 6) {
                Image(systemName: icon)
                    .font(.system(size: 18))
                    .foregroundColor(isActive ? Color("TextPrimary") : Color("TextSecondary"))
                    .frame(width: 36, height: 36)
                    .background(isActive ? Color("BorderStrong") : Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 8))
                Text(label)
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                    .minimumScaleFactor(0.85)
            }
            .frame(maxWidth: .infinity)
            .padding(8)
            .background(isActive ? Color("FillNeutral") : Color("FillNeutral").opacity(0.5), in: RoundedRectangle(cornerRadius: 12))
            .overlay(
                RoundedRectangle(cornerRadius: 12)
                    .stroke(isActive ? Color("BorderStrong") : Color.clear, lineWidth: 1)
            )
            .overlay(alignment: .topTrailing) {
                if showCheck {
                    Image(systemName: "checkmark")
                        .font(.system(size: 7, weight: .bold))
                        .foregroundColor(Color("OnAccent"))
                        .frame(width: 14, height: 14)
                        .background(Color("AccentPrimary"), in: Circle())
                        .padding(6)
                }
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
        .accessibilityAddTraits(isActive ? [.isSelected] : [])
    }

    // MARK: - GitHub

    private var githubPanel: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Image(systemName: "link")
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
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
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundColor(Color("TextPrimary"))
                    }
                    .disabled(empty)
                    .opacity(empty ? 0.4 : 1)
                }
            }
            Text("Paste a GitHub file URL to fetch its raw contents as an attachment.")
                .font(.system(size: 11))
                .foregroundColor(Color("TextSecondary"))
        }
        .plusPanel()
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

    // MARK: - Project

    private var projectPanel: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("ADD TO PROJECT")
                .font(.system(size: 11, weight: .bold))
                .tracking(0.8)
                .foregroundColor(Color("TextTertiary"))
            VStack(spacing: 4) {
                projectRow(id: nil, title: "None")
                if projectStore.isLoading && projectStore.projects.isEmpty {
                    ProgressView()
                        .controlSize(.small)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 8)
                } else if let loadError = projectStore.loadError, projectStore.projects.isEmpty {
                    FriendlyInlineStateView(
                        style: .error,
                        icon: "exclamationmark.triangle",
                        title: "Couldn't load projects",
                        message: FriendlyErrorMessage.from(loadError),
                        actionTitle: "Retry",
                        action: { projectStore.fetchProjectsIfNeeded(force: true) }
                    )
                }
                ForEach(projectStore.projects) { project in
                    projectRow(id: project.id, title: project.title)
                }
                Button(action: {
                    hapticLight()
                    isProjectMenuOpen = false
                    isNewProjectPresented = true
                }) {
                    HStack(spacing: 8) {
                        Image(systemName: "plus")
                            .font(.system(size: 13, weight: .bold))
                        Text("New project")
                            .font(.system(size: 14))
                        Spacer()
                    }
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.horizontal, 12)
                    .frame(height: 44)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .plusPanel()
        .onAppear { projectStore.fetchProjectsIfNeeded() }
    }

    private func projectRow(id: String?, title: String) -> some View {
        let selected = projectStore.selectedProjectId == id
        return Button(action: {
            hapticLight()
            projectStore.selectedProjectId = id
            isProjectMenuOpen = false
            dismiss()
        }) {
            HStack(spacing: 8) {
                Text(title)
                    .font(.system(size: 14))
                    .lineLimit(1)
                Spacer()
                if selected {
                    Image(systemName: "checkmark")
                        .font(.system(size: 13, weight: .bold))
                }
            }
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 12)
            .frame(height: 44)
            .background(selected ? Color("FillNeutral") : Color.clear, in: RoundedRectangle(cornerRadius: 8))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - Rows

    private func toggleRow(icon: String, title: String, subtitle: String, isOn: Binding<Bool>) -> some View {
        HStack(spacing: 12) {
            rowIcon(icon)
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                Text(subtitle)
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextSecondary"))
            }
            Spacer(minLength: 8)
            Toggle(title, isOn: isOn)
                .labelsHidden()
                .tint(Color("AccentPrimary"))
        }
        .padding(.vertical, 6)
    }

    private func listRow(icon: String, title: String, subtitle: String, action: @escaping () -> Void) -> some View {
        Button(action: {
            hapticLight()
            action()
        }) {
            HStack(spacing: 12) {
                rowIcon(icon)
                VStack(alignment: .leading, spacing: 1) {
                    Text(title)
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                    Text(subtitle)
                        .font(.system(size: 11))
                        .foregroundColor(Color("TextSecondary"))
                }
                Spacer(minLength: 8)
                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .opacity(0.6)
            }
            .contentShape(Rectangle())
            .plusPanel()
        }
        .buttonStyle(.plain)
    }

    private func rowIcon(_ name: String) -> some View {
        Image(systemName: name)
            .font(.system(size: 15))
            .foregroundColor(Color("TextPrimary"))
            .frame(width: 32, height: 32)
            .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 8))
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
            Task { @MainActor in isPhotosPickerPresented = true }
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

    private func hapticLight() {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
    }
}

// MARK: - Panels

private extension View {
    /// Quiet grouped panel (web GlassPanel / ListRow chrome).
    func plusPanel() -> some View {
        self
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color("FillNeutral").opacity(0.6), in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
    }
}

extension CapabilitiesClient.Kind: Identifiable {
    public var id: String { String(describing: self) }
}

/// "Permission settings": how freely this chat (or the next new one) works —
/// the same three choices as the composer's pill.
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

