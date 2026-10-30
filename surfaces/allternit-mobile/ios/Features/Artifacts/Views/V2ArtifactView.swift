import AVKit
import PDFKit
import SwiftUI

/// Opens one Artifacts v2 record (`GET /api/v2/artifacts/:id`) and renders
/// its body by kind, matching the web viewers (viewers.tsx):
///
/// - page / design (HTML) / doc / diagram → `ArtifactPageWebView`
/// - sheet / slides / code → native grid, cards, monospaced text
/// - image / video → fetched media (API-hosted media with the bearer)
/// - pdf → PDFKit, with Sign (PDFSignView) on a temp copy
/// - Office records imported from the old sectioned store → "Open in
///   editor" (OfficeDocumentView + its WKWebView editor)
/// - card / dashboard / ops / motion, canvas designs, unrendered video
///   projects, pages that use storage/AI/connectors and any kind the app
///   doesn't draw → `ArtifactLiveWebView`: the web's own viewer, signed in
///   as the user, with the page runtime bridge.
///
/// Stored artifacts open in the web's real `ArtifactWindow` (the kind's editor,
/// or its viewer without edit access), the same window Desktop and
/// m.allternit.com use, with its own toolbar for versions, share and export.
/// The native renderers below remain for records it doesn't serve.
///
/// Docs (including docs with several tabs), sheets and decks the user can
/// edit get an Edit button (V2ArtifactEditorView), which saves a new v2
/// version. PDFs, images and videos get Save or share (the share sheet:
/// Save to Files / Photos); the live web viewer handles its own exports.
struct V2ArtifactView: View {
    let artifactId: String
    var initialTitle: String? = nil
    /// Called after an archive or delete so the caller can drop the row.
    var onRemoved: (() -> Void)? = nil

    @Environment(\.dismiss) private var dismiss
    @State private var artifact: ArtifactV2?
    @State private var error: String?
    @State private var isLoading = true
    @State private var confirmDelete = false
    @State private var actionError: String?
    @State private var signURL: IdentifiableURL?
    @State private var showingLegacyEditor = false
    @State private var editModel: ArtifactBodyEditing.Model?
    @State private var showingEditor = false
    @State private var needsSignIn = false
    @State private var isExporting = false
    @State private var sharedFile: SharedArtifactFile?


    var body: some View {
        NavigationStack {
            ZStack {
                Color("BgPrimary").ignoresSafeArea()
                content
            }
            .navigationTitle(artifact?.title ?? initialTitle ?? "Artifact")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { toolbar }
            .toolbar(usesWebWindow ? .hidden : .automatic, for: .navigationBar)
            .navigationDestination(isPresented: $showingLegacyEditor) {
                if let legacyId = artifact?.origin?.legacyId {
                    OfficeDocumentView(artifactId: legacyId)
                }
            }
            .confirmationDialog("Delete this artifact for everyone?", isPresented: $confirmDelete, titleVisibility: .visible) {
                Button("Delete", role: .destructive) { Task { await deleteArtifact() } }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("People it's shared with lose access too. This can't be undone.")
            }
            .alert("Couldn't do that", isPresented: Binding(get: { actionError != nil }, set: { if !$0 { actionError = nil } })) {
                Button("OK", role: .cancel) {}
            } message: {
                Text(actionError ?? "")
            }
            .sheet(item: $signURL) { item in PDFSignView(sourceURL: item.url) }
            .sheet(item: $sharedFile) { file in
                ArtifactShareSheet(items: [file.url])
                    .presentationDetents([.medium, .large])
            }
            .overlay(alignment: .bottom) {
                if isExporting {
                    ArtifactExportProgressPill(label: "Preparing file…")
                        .padding(.bottom, 24)
                }
            }
            .fullScreenCover(isPresented: $showingEditor) {
                if let artifact, let editModel {
                    V2ArtifactEditorView(
                        artifact: artifact,
                        model: editModel,
                        onSaved: { saved in self.artifact = saved },
                        onConflict: { Task { await load() } }
                    )
                }
            }
        }
        .task { await load() }
    }

    /// Whether this record opens in the web's artifact window.
    private var usesWebWindow: Bool {
        guard let artifact else { return false }
        return artifact.id.hasPrefix("art_") || SampleWorkspace.isEnabled
    }

    // MARK: - Toolbar

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .topBarLeading) {
            Button("Close") { dismiss() }
                .foregroundColor(Color("AccentPrimary"))
        }
        if let artifact {
            ToolbarItemGroup(placement: .topBarTrailing) {
                if artifact.canEdit, ArtifactBodyEditing.isEditable(kind: artifact.kind),
                   let model = ArtifactBodyEditing.model(kind: artifact.kind, body: artifact.version.body, format: artifact.version.bodyFormat) {
                    Button {
                        editModel = model
                        showingEditor = true
                    } label: { Image(systemName: "square.and.pencil") }
                    .accessibilityLabel("Edit")
                }
                if shareSource(artifact) != nil {
                    Button {
                        Task { await shareFile(artifact) }
                    } label: { Image(systemName: "square.and.arrow.up") }
                    .disabled(isExporting)
                    .accessibilityLabel("Save or share")
                }
                if let text = copyableText(artifact) {
                    Button {
                        UIPasteboard.general.string = text
                        UINotificationFeedbackGenerator().notificationOccurred(.success)
                    } label: { Image(systemName: "doc.on.doc") }
                    .accessibilityLabel("Copy content")
                }
                Button {
                    Task { await archive() }
                } label: { Image(systemName: "archivebox") }
                .accessibilityLabel("Archive")
                if artifact.isMine {
                    Button(role: .destructive) { confirmDelete = true } label: { Image(systemName: "trash") }
                        .accessibilityLabel("Delete")
                }
            }
        }
    }

    // MARK: - States

    @ViewBuilder
    private var content: some View {
        if isLoading {
            ProgressView("Loading artifact…")
        } else if error != nil, needsSignIn {
            FriendlyStateView(
                style: .empty,
                icon: "person.crop.circle.badge.questionmark",
                title: "Sign in to open this artifact",
                message: "Artifacts are saved to your Allternit account, so they open once you're signed in.",
                actionTitle: "Sign in",
                action: { AuthManager.shared.signIn() }
            )
        } else if let error {
            FriendlyStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "Couldn't open this artifact",
                message: FriendlyErrorMessage.from(error),
                actionTitle: "Retry",
                action: { Task { await load() } }
            )
        } else if let artifact, usesWebWindow {
            ArtifactLiveWebView(route: .window(id: artifact.id), onClose: { dismiss() })
                .ignoresSafeArea()
        } else if let artifact {
            VStack(spacing: 0) {
                if artifact.origin?.legacySource == "sectioned", artifact.origin?.legacyId != nil,
                   ["doc", "sheet", "slides", "pdf"].contains(artifact.kind) {
                    legacyEditorBanner
                }
                render(artifact)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }

    private var legacyEditorBanner: some View {
        Button { showingLegacyEditor = true } label: {
            HStack(spacing: 8) {
                Image(systemName: "arrow.up.forward.app")
                Text("Open in editor").font(.subheadline.weight(.semibold))
                Spacer()
                Image(systemName: "chevron.right").font(.caption)
            }
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .background(Color("BgSecondary"))
        }
        .buttonStyle(.plain)
    }

    // MARK: - Rendering by kind

    @ViewBuilder
    private func render(_ a: ArtifactV2) -> some View {
        let body = a.version.body
        let format = a.version.bodyFormat
        switch a.kind {
        case "page":
            if let uri = ArtifactBodyDecoder.firstURI(body), format == "text/uri-list" || body.trimmingCharacters(in: .whitespacesAndNewlines).hasPrefix("https://"),
               let url = URL(string: uri), url.scheme?.lowercased() == "https" {
                ArtifactPageWebView(source: .url(url))
            } else if format == "text/markdown" {
                ArtifactPageWebView(source: .html(ArtifactHTML.markdown(body)))
            } else if a.runtimeVersion != 1, a.id.hasPrefix("art_"), ArtifactLiveWebView.pageNeedsRuntime(a) {
                // Stored data, AI or connectors: the web's page runtime.
                ArtifactLiveWebView(artifact: a)
            } else {
                ArtifactPageWebView(source: .html(body))
            }
        case "doc":
            if let md = ArtifactBodyDecoder.docMarkdown(body: body, format: format) {
                ArtifactPageWebView(source: .html(ArtifactHTML.markdown(md.isEmpty ? "*Empty doc*" : md)))
            } else if format == "text/html" {
                ArtifactPageWebView(source: .html(body))
            } else {
                ArtifactLiveWebView(artifact: a)
            }
        case "design":
            if let html = ArtifactBodyDecoder.designHTML(body: body, format: format) {
                ArtifactPageWebView(source: .html(html))
            } else {
                // Canvas designs: the web draws the artboards (canvasToHtml).
                ArtifactLiveWebView(artifact: a)
            }
        case "diagram":
            if format == "image/svg+xml" || body.trimmingCharacters(in: .whitespaces).hasPrefix("<svg") {
                ArtifactPageWebView(source: .html(ArtifactHTML.svg(body)))
            } else {
                ArtifactPageWebView(source: .html(ArtifactHTML.mermaid(body)))
            }
        case "sheet":
            if let grid = ArtifactBodyDecoder.sheetGrid(body: body, format: format) {
                SheetGridView(grid: grid)
            } else {
                ArtifactLiveWebView(artifact: a)
            }
        case "slides":
            if let slides = ArtifactBodyDecoder.slides(body: body) {
                SlidesListView(slides: slides)
            } else if let md = ArtifactBodyDecoder.docMarkdown(body: body, format: format) {
                ArtifactPageWebView(source: .html(ArtifactHTML.markdown(md)))
            } else {
                ArtifactLiveWebView(artifact: a)
            }
        case "code":
            CodeBodyView(code: body)
        case "image":
            if case .string("video")? = a.version.meta?["media"], let url = ArtifactBodyDecoder.firstURI(body) {
                ArtifactVideoView(urlString: url)
            } else if let url = ArtifactBodyDecoder.firstURI(body) {
                ArtifactImageView(urlString: url)
            } else {
                unreadable("image")
            }
        case "video":
            if let url = ArtifactBodyDecoder.videoURL(body: body, meta: a.version.meta) {
                ArtifactVideoView(urlString: url)
            } else {
                // An unrendered video project: the web's viewer previews it.
                ArtifactLiveWebView(artifact: a)
            }
        case "pdf":
            if format == "application/pdf", let data = Data(base64Encoded: body, options: .ignoreUnknownCharacters),
               let document = PDFDocument(data: data) {
                VStack(spacing: 0) {
                    PDFKitView(document: document)
                    Button { openSign(data: data, title: a.title) } label: {
                        Label("Sign this PDF", systemImage: "signature")
                            .font(.subheadline.weight(.semibold))
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 12)
                    }
                    .buttonStyle(.plain)
                    .foregroundColor(Color("TextPrimary"))
                    .background(Color("BgSecondary"))
                }
            } else {
                unreadable("PDF")
            }
        default:
            // card / dashboard / ops / motion and kinds newer than this app.
            ArtifactLiveWebView(artifact: a)
        }
    }

    private func unreadable(_ label: String) -> some View {
        FriendlyStateView(
            style: .error,
            icon: "exclamationmark.triangle",
            title: "This \(label) couldn't be read",
            message: "Open it on the web or Desktop to export the source."
        )
    }


    private func copyableText(_ a: ArtifactV2) -> String? {
        switch a.kind {
        case "pdf", "image", "video": return nil
        case "doc": return ArtifactBodyDecoder.docMarkdown(body: a.version.body, format: a.version.bodyFormat) ?? a.version.body
        default: return a.version.body.isEmpty ? nil : a.version.body
        }
    }

    // MARK: - Save or share

    private enum ShareSource {
        case pdf(String)
        case media(String, isVideo: Bool)
    }

    private func shareSource(_ a: ArtifactV2) -> ShareSource? {
        let body = a.version.body
        switch a.kind {
        case "pdf":
            guard a.version.bodyFormat == "application/pdf", !body.isEmpty else { return nil }
            return .pdf(body)
        case "image":
            guard let url = ArtifactBodyDecoder.firstURI(body) else { return nil }
            if case .string("video")? = a.version.meta?["media"] { return .media(url, isVideo: true) }
            return .media(url, isVideo: false)
        case "video":
            return ArtifactBodyDecoder.videoURL(body: body, meta: a.version.meta).map { .media($0, isVideo: true) }
        default:
            return nil
        }
    }

    private func shareFile(_ a: ArtifactV2) async {
        guard let source = shareSource(a) else { return }
        isExporting = true
        defer { isExporting = false }
        do {
            let data: Data
            let ext: String
            switch source {
            case .pdf(let base64):
                guard let pdf = Data(base64Encoded: base64, options: .ignoreUnknownCharacters) else { throw ArtifactContentError.badURL }
                data = pdf
                ext = "pdf"
            case .media(let url, let isVideo):
                data = try await ArtifactMediaLoader.data(for: url)
                ext = isVideo ? Self.videoExtension(url) : Self.imageExtension(data)
            }
            let base = a.title.trimmingCharacters(in: .whitespacesAndNewlines)
            let file = try ArtifactFileShare.write(data, suggestedName: "\(base.isEmpty ? "Artifact" : base).\(ext)", mimeType: nil)
            sharedFile = SharedArtifactFile(url: file)
        } catch is CancellationError {
            return
        } catch {
            actionError = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    private static func imageExtension(_ data: Data) -> String {
        let head = [UInt8](data.prefix(12))
        if head.starts(with: [0xFF, 0xD8]) { return "jpg" }
        if head.starts(with: [0x47, 0x49, 0x46]) { return "gif" }
        if head.count >= 12, head.starts(with: [0x52, 0x49, 0x46, 0x46]), head[8...11].elementsEqual([0x57, 0x45, 0x42, 0x50]) { return "webp" }
        if let text = String(data: data.prefix(256), encoding: .utf8), text.contains("<svg") { return "svg" }
        return "png"
    }

    private static func videoExtension(_ url: String) -> String {
        let path = URL(string: url)?.pathExtension.lowercased() ?? ""
        return ["mp4", "mov", "m4v", "webm"].contains(path) ? path : "mp4"
    }

    // MARK: - Actions

    private func load() async {
        isLoading = artifact == nil
        do {
            artifact = try await ArtifactsV2Client.shared.get(id: artifactId)
            error = nil
            needsSignIn = false
        } catch is CancellationError {
            return
        } catch {
            needsSignIn = (error as? APIError)?.httpStatus == 401
            self.error = error.localizedDescription
        }
        isLoading = false
    }

    private func archive() async {
        do {
            try await ArtifactsV2Client.shared.setArchived(id: artifactId, archived: true)
            ArtifactLibraryStore.shared.dropLocally(id: artifactId)
            onRemoved?()
            dismiss()
        } catch {
            actionError = error.localizedDescription
        }
    }

    private func deleteArtifact() async {
        do {
            try await ArtifactLibraryStore.shared.delete(id: artifactId)
            onRemoved?()
            dismiss()
        } catch {
            actionError = error.localizedDescription
        }
    }

    private func openSign(data: Data, title: String) {
        let name = title.replacingOccurrences(of: "/", with: "-")
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("\(name.isEmpty ? "Document" : name).pdf")
        do {
            try data.write(to: url, options: .atomic)
            signURL = IdentifiableURL(url: url)
        } catch {
            actionError = error.localizedDescription
        }
    }
}

// MARK: - Native viewers

private struct SheetGridView: View {
    let grid: ArtifactBodyDecoder.SheetGrid

    var body: some View {
        if grid.rows.isEmpty && grid.columns.isEmpty {
            FriendlyStateView(style: .empty, icon: "tablecells", title: "No rows yet", message: "This sheet is empty.")
        } else {
            ScrollView([.horizontal, .vertical]) {
                Grid(alignment: .leading, horizontalSpacing: 0, verticalSpacing: 0) {
                    if !grid.columns.isEmpty {
                        GridRow {
                            ForEach(Array(grid.columns.enumerated()), id: \.offset) { _, column in
                                cell(column, header: true)
                            }
                        }
                    }
                    ForEach(Array(grid.rows.enumerated()), id: \.offset) { _, row in
                        GridRow {
                            ForEach(Array(row.enumerated()), id: \.offset) { _, value in
                                cell(value, header: false)
                            }
                        }
                    }
                }
                .padding(16)
            }
            .textSelection(.enabled)
        }
    }

    private func cell(_ text: String, header: Bool) -> some View {
        Text(text)
            .font(.system(.footnote, design: header ? .default : .monospaced).weight(header ? .semibold : .regular))
            .foregroundColor(Color("TextPrimary"))
            .lineLimit(3)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .frame(minWidth: 80, alignment: .leading)
            .background(header ? Color("BgSecondary") : Color.clear)
            .overlay(Rectangle().stroke(Color("BorderSubtle"), lineWidth: 0.5))
    }
}

private struct SlidesListView: View {
    let slides: [ArtifactBodyDecoder.Slide]

    var body: some View {
        ScrollView {
            LazyVStack(spacing: 14) {
                ForEach(Array(slides.enumerated()), id: \.offset) { index, slide in
                    VStack(alignment: .leading, spacing: 10) {
                        Text("\(index + 1) / \(slides.count)")
                            .font(.caption2.monospacedDigit())
                            .foregroundColor(Color("TextSecondary"))
                        if let title = slide.title, !title.isEmpty {
                            Text(title).font(.title3.weight(.semibold)).foregroundColor(Color("TextPrimary"))
                        }
                        ForEach(Array(slide.bullets.enumerated()), id: \.offset) { _, bullet in
                            HStack(alignment: .firstTextBaseline, spacing: 8) {
                                Text("•")
                                Text(bullet)
                            }
                            .font(.subheadline)
                            .foregroundColor(Color("TextPrimary"))
                        }
                        if let body = slide.body, !body.isEmpty {
                            Text(LocalizedStringKey(body)).font(.subheadline).foregroundColor(Color("TextPrimary"))
                        }
                    }
                    .frame(maxWidth: .infinity, minHeight: 160, alignment: .topLeading)
                    .padding(18)
                    .background(Color("BgPanel"))
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: Theme.radiusMD, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                }
            }
            .padding(16)
            .textSelection(.enabled)
        }
    }
}

private struct CodeBodyView: View {
    let code: String

    var body: some View {
        ScrollView([.vertical, .horizontal]) {
            Text(code)
                .font(.system(.footnote, design: .monospaced))
                .foregroundColor(Color("TextPrimary"))
                .textSelection(.enabled)
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Color("BgSecondary"))
    }
}

private struct PDFKitView: UIViewRepresentable {
    let document: PDFDocument

    func makeUIView(context: Context) -> PDFView {
        let view = PDFView()
        view.autoScales = true
        view.displayMode = .singlePageContinuous
        view.document = document
        return view
    }

    func updateUIView(_ view: PDFView, context: Context) {
        if view.document !== document { view.document = document }
    }
}

/// Loads media an artifact points at. API paths (`/api/...`) are resolved
/// against the cloud API and sent with the bearer; external URLs never get it.
enum ArtifactMediaLoader {
    static func data(for urlString: String) async throws -> Data {
        if urlString.hasPrefix("data:") {
            guard let comma = urlString.firstIndex(of: ",") else { throw ArtifactContentError.badURL }
            let meta = urlString[..<comma]
            let payload = String(urlString[urlString.index(after: comma)...])
            if meta.contains(";base64") {
                guard let data = Data(base64Encoded: payload, options: .ignoreUnknownCharacters) else { throw ArtifactContentError.badURL }
                return data
            }
            return Data((payload.removingPercentEncoding ?? payload).utf8)
        }
        var request: URLRequest
        if urlString.hasPrefix("/") {
            guard let url = URL(string: urlString, relativeTo: AppConfig.cloudAPIBaseURL)?.absoluteURL else { throw ArtifactContentError.badURL }
            request = URLRequest(url: url)
            if let token = await ArtifactsV2Client.bearerToken() {
                request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
            }
        } else {
            guard let url = URL(string: urlString), let scheme = url.scheme?.lowercased(), ["http", "https"].contains(scheme) else {
                throw ArtifactContentError.badURL
            }
            request = URLRequest(url: url)
        }
        let (data, response) = try await URLSession.shared.data(for: request)
        if let http = response as? HTTPURLResponse, !(200...299).contains(http.statusCode) {
            throw APIError.httpError(statusCode: http.statusCode, message: nil)
        }
        return data
    }
}

private struct ArtifactImageView: View {
    let urlString: String
    @State private var image: UIImage?
    @State private var failed = false

    var body: some View {
        Group {
            if let image {
                ScrollView {
                    Image(uiImage: image)
                        .resizable()
                        .scaledToFit()
                        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusSM))
                        .padding(16)
                }
            } else if failed {
                FriendlyStateView(style: .error, icon: "photo", title: "Couldn't load the image", message: "Check your connection and try again.")
            } else {
                ProgressView()
            }
        }
        .task {
            do {
                let data = try await ArtifactMediaLoader.data(for: urlString)
                if let decoded = UIImage(data: data) { image = decoded } else { failed = true }
            } catch {
                failed = true
            }
        }
    }
}

private struct ArtifactVideoView: View {
    let urlString: String
    @State private var player: AVPlayer?
    @State private var failed = false

    var body: some View {
        Group {
            if let player {
                VideoPlayer(player: player)
            } else if failed {
                FriendlyStateView(style: .error, icon: "video", title: "Couldn't load the video", message: "Check your connection and try again.")
            } else {
                ProgressView("Loading video…")
            }
        }
        .task {
            // External https footage streams; API-hosted footage needs the
            // bearer, so it's downloaded to a temp file first.
            if let url = URL(string: urlString), url.scheme?.lowercased() == "https" {
                player = AVPlayer(url: url)
                return
            }
            do {
                let data = try await ArtifactMediaLoader.data(for: urlString)
                let file = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".mp4")
                try data.write(to: file, options: .atomic)
                player = AVPlayer(url: file)
            } catch {
                failed = true
            }
        }
        .onDisappear { player?.pause() }
    }
}
