import PhotosUI
import QuickLook
import SwiftUI
import UniformTypeIdentifiers
import AllternitFilesKit

/// Big full-width button from the design's sheets.
struct FilesBigButton: View {
    let title: String
    var systemImage: String? = nil
    var primary = true
    var role: ButtonRole? = nil
    let action: () -> Void

    var body: some View {
        Button(role: role, action: action) {
            HStack(spacing: 8) {
                if let systemImage { Image(systemName: systemImage) }
                Text(title).font(.system(size: 16, weight: .semibold))
            }
            .frame(maxWidth: .infinity, minHeight: 50)
            .foregroundColor(role == .destructive ? Theme.statusError : (primary ? Color("BgPrimary") : Color("TextPrimary")))
            .background(primary && role != .destructive ? Color("TextPrimary") : Color("FillNeutral"))
            .clipShape(RoundedRectangle(cornerRadius: 14))
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Wake sheet (design 4C "Wake your cloud computer?")

struct FilesWakeSheet: View {
    @ObservedObject var model: FilesComputerModel
    let prompt: FilesComputerModel.WakePrompt
    @State private var remember = false
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "moon.fill")
                .font(.system(size: 22))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 44, height: 44)
                .background(Color("FillNeutral"))
                .clipShape(RoundedRectangle(cornerRadius: 12))
            Text("Wake \(model.name)?")
                .font(.system(size: 17, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
            Text("\(model.name) is asleep. Waking takes about 40 seconds, and while it is awake it bills credits at your plan's awake rate. Browsing the saved list stays free.")
                .font(.system(size: 14))
                .foregroundColor(Color("TextSecondary"))
                .multilineTextAlignment(.center)
            Toggle(isOn: $remember) {
                Text("Wake automatically from now on")
                    .font(.system(size: 14))
                    .foregroundColor(Color("TextPrimary"))
            }
            .tint(Color("TextPrimary"))
            .padding(.vertical, 4)
            FilesBigButton(title: "Wake", systemImage: "bolt.fill") {
                let remember = remember
                dismiss()
                Task { await model.wake(prompt, remember: remember) }
            }
            FilesBigButton(title: "Not now", primary: false) {
                model.wakePrompt = nil
                dismiss()
            }
            Text("You can change this in Settings > Files.")
                .font(.system(size: 12))
                .foregroundColor(Color("TextTertiary"))
        }
        .padding(.horizontal, 20)
        .padding(.top, 24)
        .padding(.bottom, 12)
        .presentationDetents([.medium])
        .presentationDragIndicator(.visible)
        .background(Color("BgPrimary"))
    }
}

// MARK: - One file (tap a row: explicit buttons, no menus)

struct FilesItemSheet: View {
    @ObservedObject var model: FilesComputerModel
    let item: Item
    let rootKind: RootKind?

    @Environment(\.dismiss) private var dismiss
    @State private var previewURL: URL?
    @State private var shareURL: FilesShareFile?
    @State private var keptOffline = false
    @State private var isBusy = false
    @State private var confirmTrash = false
    @State private var isImportOpen = false

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    header
                    if let task = model.pendingTask(for: item) { inboxApproval(task) }
                    VStack(spacing: 10) {
                        FilesBigButton(title: "Open", systemImage: "eye") { run(.openFile) { previewURL = try await model.localCopy(of: item) } }
                        FilesBigButton(title: "Share…", systemImage: "square.and.arrow.up", primary: false) {
                            run(.openFile) { shareURL = FilesShareFile(url: try await model.localCopy(of: item)) }
                        }
                        if rootKind == .models {
                            FilesBigButton(title: "Use on this iPhone", systemImage: "cpu", primary: false) { isImportOpen = true }
                        }
                    }
                    Toggle(isOn: $keptOffline) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("Keep on this iPhone").font(.system(size: 16)).foregroundColor(Color("TextPrimary"))
                            Text("Opens without the network, in this app and the Files app.")
                                .font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                        }
                    }
                    .tint(Color("TextPrimary"))
                    .padding(14)
                    .background(Color("FillNeutral"))
                    .clipShape(RoundedRectangle(cornerRadius: 12))
                    .onChange(of: keptOffline) { _, value in
                        guard value != model.isKeptOffline(item) else { return }
                        Task { do { try await model.setKeptOffline(item, value) } catch { model.notice = FilesComputerModel.message(error); keptOffline = !value } }
                    }
                    if item.permissions.delete {
                        FilesBigButton(title: "Move to Trash", systemImage: "trash", primary: false, role: .destructive) { confirmTrash = true }
                    }
                    if isBusy { ProgressView().frame(maxWidth: .infinity) }
                }
                .padding(20)
            }
            .background(Color("BgPrimary"))
            .navigationTitle(item.name)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
            .confirmationDialog("Move \(item.name) to Trash?", isPresented: $confirmTrash, titleVisibility: .visible) {
                Button("Move to Trash", role: .destructive) {
                    run(.openFile) { try await model.trash(item); dismiss() }
                }
            } message: {
                Text("It stays in the computer's trash, so you can restore it.")
            }
        }
        .quickLookPreview($previewURL)
        .sheet(item: $shareURL) { file in ArtifactShareSheet(items: [file.url]) }
        .sheet(isPresented: $isImportOpen) { FilesModelImportSheet(model: model, item: item) }
        .onAppear { keptOffline = model.isKeptOffline(item) }
    }

    private var header: some View {
        HStack(spacing: 12) {
            Image(systemName: FilesFolderScreen.symbol(for: item))
                .font(.system(size: 22))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 44, height: 44)
                .background(Color("FillNeutral"))
                .clipShape(RoundedRectangle(cornerRadius: 12))
            VStack(alignment: .leading, spacing: 2) {
                Text("\(FilesFormat.bytes(item.size)) · \(FilesFormat.date(item.modifiedAt))")
                    .font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
                if let by = item.modifiedBy?.displayName, !by.isEmpty {
                    Text("Changed by \(by)").font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
                }
                if item.provenance?.aiGenerated == true {
                    Text("Made by your agent").font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
                }
            }
        }
    }

    /// Inbox files are untrusted: nothing reads them until the owner says so.
    private func inboxApproval(_ task: FilesInboxTask) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("\(task.droppedByLabel) dropped this in Inbox. Your agent hasn't read it.")
                .font(.system(size: 14)).foregroundColor(Color("TextPrimary"))
            Text("If you approve, the agent reads only this file, summarizes it with no other tools, and saves the result next to it.")
                .font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
            HStack(spacing: 10) {
                FilesBigButton(title: "Ask my agent to read it") { Task { await model.approve(task); dismiss() } }
                FilesBigButton(title: "Reject", primary: false) { Task { await model.reject(task) } }
                    .frame(maxWidth: 110)
            }
        }
        .padding(14)
        .background(Color("FillNeutral"))
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }

    private func run(_ reason: WakeReason, _ action: @escaping @MainActor () async throws -> Void) {
        isBusy = true
        Task {
            await model.needingWake(reason, action)
            isBusy = false
        }
    }
}

struct FilesShareFile: Identifiable {
    let url: URL
    var id: String { url.path }
}

// MARK: - Add (design: Photo Library, Browse Files, Take a photo or scan)

struct FilesAddSheet: View {
    @ObservedObject var model: FilesComputerModel
    let rootId: RootId
    let parentId: ItemId
    let folderName: String

    @Environment(\.dismiss) private var dismiss
    @State private var photos: [PhotosPickerItem] = []
    @State private var isImporterOpen = false
    @State private var isScannerOpen = false
    @State private var isNewFolderOpen = false
    @State private var newFolderName = ""

    var body: some View {
        NavigationStack {
            List {
                Section {
                    PhotosPicker(selection: $photos, maxSelectionCount: 20, matching: .any(of: [.images, .videos])) {
                        // PhotosPicker's label closure isn't main-actor, so plain views here.
                        HStack(spacing: 12) {
                            Image(systemName: "photo.on.rectangle").font(.system(size: 18))
                                .foregroundColor(Color("TextSecondary")).frame(width: 26)
                            Text("Photo Library").font(.system(size: 16)).foregroundColor(Color("TextPrimary"))
                            Spacer()
                        }
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                    }
                    Button { isImporterOpen = true } label: { FilesRowLabel(systemImage: "folder", title: "Browse Files") }
                    if FilesDocumentScanner.isSupported {
                        Button { isScannerOpen = true } label: {
                            FilesRowLabel(systemImage: "doc.viewfinder", title: "Scan a document", subtitle: "Saved as a PDF in Scans")
                        }
                    }
                    Button { isNewFolderOpen = true } label: { FilesRowLabel(systemImage: "folder.badge.plus", title: "New folder") }
                } header: {
                    Text("Add to \(folderName)")
                }
                .listRowBackground(Color("FillNeutral"))
            }
            .filesListStyle()
            .navigationTitle("Add")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
            .onChange(of: photos) { _, picked in
                guard !picked.isEmpty else { return }
                dismiss()
                Task { await uploadPhotos(picked) }
            }
            .fileImporter(isPresented: $isImporterOpen, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
                guard case .success(let urls) = result else { return }
                let copies = urls.compactMap(Self.copySecurityScoped)
                dismiss()
                upload(copies)
            }
            .fullScreenCover(isPresented: $isScannerOpen) {
                FilesDocumentScanner { pdf in
                    isScannerOpen = false
                    dismiss()
                    if let pdf { let (rootId, parentId) = (rootId, parentId); Task { await model.saveScan(pdf, near: rootId, parentId: parentId) } }
                }
                .ignoresSafeArea()
            }
            .alert("New folder", isPresented: $isNewFolderOpen) {
                TextField("Name", text: $newFolderName)
                Button("Cancel", role: .cancel) {}
                Button("Create") {
                    let name = newFolderName.trimmingCharacters(in: .whitespacesAndNewlines)
                    guard !name.isEmpty else { return }
                    let (rootId, parentId) = (rootId, parentId)
                    dismiss()
                    Task { await model.needingWake(.upload) { [model] in try await model.makeFolder(named: name, rootId: rootId, parentId: parentId) } }
                }
            }
        }
    }

    private func upload(_ urls: [URL]) {
        guard !urls.isEmpty else { return }
        let (rootId, parentId) = (rootId, parentId)
        Task { await model.needingWake(.upload) { [model] in try await model.upload(urls, rootId: rootId, parentId: parentId) } }
    }

    private func uploadPhotos(_ picked: [PhotosPickerItem]) async {
        var urls: [URL] = []
        for item in picked {
            if let file = try? await item.loadTransferable(type: FilesPickedFile.self) { urls.append(file.url) }
        }
        upload(urls)
    }

    /// Files picked from another provider are security scoped; copy them
    /// while access is open.
    static func copySecurityScoped(_ url: URL) -> URL? {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("FilesPick-\(UUID().uuidString)", isDirectory: true)
        let dest = folder.appendingPathComponent(FilesComputerModel.safeName(url.lastPathComponent))
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            try FileManager.default.copyItem(at: url, to: dest)
            return dest
        } catch {
            return nil
        }
    }
}

/// A photo or video from the library, copied to a temporary file (never
/// loaded into memory: videos can be gigabytes).
struct FilesPickedFile: Transferable {
    let url: URL

    static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(importedContentType: .movie) { received in try copy(received.file) }
        FileRepresentation(importedContentType: .image) { received in try copy(received.file) }
    }

    private static func copy(_ file: URL) throws -> FilesPickedFile {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("FilesPhoto-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let dest = folder.appendingPathComponent(file.lastPathComponent)
        try FileManager.default.copyItem(at: file, to: dest)
        return FilesPickedFile(url: dest)
    }
}

// MARK: - Use a model on this iPhone (SPEC section 19 hand-off)

struct FilesModelImportSheet: View {
    @ObservedObject var model: FilesComputerModel
    let item: Item

    enum Phase {
        case checking
        case ready(FilesPackageManifest)
        case downloading(done: UInt64, total: UInt64)
        case installed(String)
        case failed(String)
    }

    @Environment(\.dismiss) private var dismiss
    @State private var phase: Phase = .checking
    @State private var job: Task<Void, Never>?

    var body: some View {
        NavigationStack {
            VStack(spacing: 16) {
                switch phase {
                case .checking:
                    ProgressView("Checking the model…")
                case .ready(let manifest):
                    readyView(manifest)
                case .downloading(let done, let total):
                    ProgressView(value: total == 0 ? 0 : Double(done) / Double(total)) {
                        Text("Copying \(item.name)")
                    } currentValueLabel: {
                        Text("\(FilesFormat.bytes(done)) of \(FilesFormat.bytes(total))")
                    }
                    FilesBigButton(title: "Stop", primary: false) { job?.cancel(); dismiss() }
                case .installed(let name):
                    Image(systemName: "checkmark.circle.fill").font(.system(size: 40)).foregroundColor(Theme.statusSuccess)
                    Text("\(name) is ready under Models > On this iPhone.").multilineTextAlignment(.center)
                    FilesBigButton(title: "Done") { dismiss() }
                case .failed(let message):
                    Text(message).multilineTextAlignment(.center).foregroundColor(Color("TextPrimary"))
                    FilesBigButton(title: "Close", primary: false) { dismiss() }
                }
            }
            .padding(20)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color("BgPrimary"))
            .navigationTitle("Use on this iPhone")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { job?.cancel(); dismiss() } } }
        }
        .task { await check() }
        .onDisappear { job?.cancel() }
    }

    private func readyView(_ manifest: FilesPackageManifest) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(manifest.name).font(.system(size: 17, weight: .semibold))
            Text("\(manifest.weightsFormat.uppercased()) · \(FilesFormat.bytes(manifest.totalBytes)) · \(manifest.files.count) files")
                .font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
            Text("The files are copied from \(model.name) and checked against the checksums it sent. The model then runs on this iPhone, with nothing sent anywhere.")
                .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
            FilesBigButton(title: "Copy and install", systemImage: "arrow.down.circle") { start(manifest) }
        }
    }

    private func check() async {
        do {
            let manifest = try await FilesModelImporter.handoff(client: model.client, itemId: item.id)
            guard manifest.grant.execute else { phase = .failed(FilesModelImporter.Failure.noRunPermission.localizedDescription); return }
            phase = .ready(manifest)
        } catch {
            phase = .failed(FilesComputerModel.message(error))
        }
    }

    private func start(_ manifest: FilesPackageManifest) {
        phase = .downloading(done: 0, total: manifest.totalBytes)
        let client = model.client
        job = Task {
            do {
                try await FilesModelImporter.install(manifest, client: client) { done, total in
                    Task { @MainActor in phase = .downloading(done: done, total: total) }
                }
                phase = .installed(manifest.name)
            } catch FilesError.wakeConsentRequired {
                phase = .failed("\(model.name) is asleep. Wake it from the Files screen, then try again.")
            } catch is CancellationError {
            } catch {
                phase = .failed(FilesComputerModel.message(error))
            }
        }
    }
}
