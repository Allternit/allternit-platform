import SwiftUI

struct ArtifactRecord: Identifiable, Hashable, Sendable, Codable {
    let id: String
    let title: String
    let fileType: String
    let codeContent: String
    /// Typed `artifact.created` event metadata; `nil` for locally built records.
    let artifactId: String?
    let artifactType: String?
    let url: String?
    /// Inline content carried by the event itself (contract `inlinePreview`);
    /// `nil` when the event only references content by `url` / `artifactId`.
    let inlinePreview: String?
    /// Backend canvas version; nil for local and legacy artifacts.
    let version: Int?

    init(id: String,
         title: String,
         fileType: String,
         codeContent: String = "",
         artifactId: String? = nil,
         artifactType: String? = nil,
         url: String? = nil,
         inlinePreview: String? = nil,
         version: Int? = nil) {
        self.id = id
        self.title = title
        self.fileType = fileType
        self.codeContent = codeContent
        self.artifactId = artifactId
        self.artifactType = artifactType
        self.url = url
        self.inlinePreview = inlinePreview
        self.version = version
    }

    /// Attachment built from a typed `artifact.created` SSE event. Content
    /// resolution (url → inline preview → artifacts endpoint) is handled by
    /// `ArtifactContentLoader` when the details view opens.
    ///
    /// QUARANTINED with the replies-runtime contract (see ReplyEvent.swift) —
    /// the live path builds records via `init(agentChat:)` below.
    init(event: ArtifactCreatedEvent) {
        self.init(
            id: event.artifactId,
            title: event.title,
            fileType: event.artifactType,
            artifactId: event.artifactId,
            artifactType: event.artifactType,
            url: event.url,
            inlinePreview: event.inlinePreview
        )
    }

    /// Attachment built from an agent-chat artifact frame (LIVE path). The
    /// frame is pre-normalized by `AgentChatEvent` (kind whitelist, title
    /// fallback, image data-URL handling) exactly like the web parser; the
    /// frame's `content` maps to `inlinePreview` for ArtifactContentLoader.
    init(agentChat artifact: AgentChatEvent.Artifact) {
        self.init(
            id: artifact.artifactId,
            title: artifact.title,
            fileType: artifact.kind,
            artifactId: artifact.artifactId,
            artifactType: artifact.kind,
            url: artifact.url,
            inlinePreview: artifact.content
        )
    }

    /// Reconstructs an artifact card from a persisted message part
    /// (`metadata.parts` on `AgentSessionMessage`). Used when loading history
    /// so file/URL attachments don't disappear after a foreground reconcile.
    init(part: AgentSessionMessage.Part, messageId: String) {
        let title = part.filename
            ?? part.url?.split(separator: "/").last.map(String.init)
            ?? "Attachment"
        let derivedFileType: String
        if let filename = part.filename,
           let ext = filename.split(separator: ".").last {
            derivedFileType = String(ext).lowercased()
        } else if let urlString = part.url,
                  let ext = URL(string: urlString)?.pathExtension, !ext.isEmpty {
            derivedFileType = ext.lowercased()
        } else {
            derivedFileType = "text"
        }
        self.init(
            id: "\(messageId)-\(title)-\(part.url ?? "")",
            title: title,
            fileType: derivedFileType,
            artifactId: nil,
            artifactType: derivedFileType,
            url: part.url,
            inlinePreview: part.text
        )
    }

    var isPreviewable: Bool {
        let types = ["html", "svg", "css", "javascript"]
        return types.contains(fileType.lowercased())
    }
}

/// Library rows open the v2 viewer; chat-frame cards (no v2 record) keep
/// this local viewer with the calm bar (phone calm I12): back, share (the
/// content as a file, or the link) and ⋯ (Open in browser for a web link,
/// Copy content). Frame cards have no comments: those live on v2 records.
struct ArtifactDetailsView: View {
    let artifact: ArtifactRecord
    @Environment(\.dismiss) var dismiss
    @Environment(\.openURL) private var openURL
    @State private var selectedTab = 0
    @State private var sharedFile: SharedArtifactFile?
    @State private var shareError: String?
    @StateObject private var loader = ArtifactContentLoader()

    /// An https link the card points at, for Open in browser.
    private var webURL: URL? {
        guard let raw = artifact.url, let url = URL(string: raw), url.scheme?.lowercased() == "https" else { return nil }
        return url
    }

    var body: some View {
        // Library rows (bot pages included) are Artifacts v2 records: open
        // the rendered v2 view. Chat-frame cards keep the local viewer.
        if artifact.id.hasPrefix("art_") {
            V2ArtifactView(artifactId: artifact.id, initialTitle: artifact.title)
        } else {
            frameBody
        }
    }

    private var frameBody: some View {
        NavigationStack {
            VStack(spacing: 0) {
                // Preview/Code selector if preview is supported.
                if artifact.isPreviewable {
                    Menu {
                        Button("Preview") { selectedTab = 0 }
                        Button("Code") { selectedTab = 1 }
                    } label: {
                        HStack(spacing: 4) {
                            Text(selectedTab == 0 ? "Preview" : "Code")
                                .font(.subheadline.weight(.semibold))
                            Image(systemName: "chevron.down")
                                .font(.system(size: 10, weight: .bold))
                        }
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                        .background(Color("BgPanel"))
                        .clipShape(Capsule())
                        .overlay(
                            Capsule()
                                .stroke(Theme.borderWarmDefault, lineWidth: 1)
                        )
                    }
                    .buttonStyle(.plain)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 10)
                    .background(Color("BgPrimary"))
                }

                ZStack {
                    Color("BgSecondary").edgesIgnoringSafeArea(.all)
                    content
                }
            }
            .navigationTitle(artifact.title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar(.hidden, for: .navigationBar)
            .safeAreaInset(edge: .top, spacing: 0) { viewerBar }
            .sheet(item: $sharedFile) { file in
                ArtifactShareSheet(items: [file.url])
                    .presentationDetents([.medium, .large])
            }
            .alert("Couldn't share that", isPresented: Binding(get: { shareError != nil }, set: { if !$0 { shareError = nil } })) {
                Button("OK", role: .cancel) {}
            } message: {
                Text(shareError ?? "")
            }
        }
        .task {
            await loader.load(artifact: artifact)
        }
    }

    // MARK: - Viewer bar

    private var viewerBar: some View {
        HStack(spacing: 10) {
            Button { dismiss() } label: {
                Image(systemName: "chevron.left")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 40, height: 40)
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .glassSurface(cornerRadius: 20)
            .accessibilityLabel("Back")

            Text(artifact.title)
                .font(.subheadline.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
                .frame(maxWidth: .infinity)

            HStack(spacing: 0) {
                Button {
                    Haptics.light()
                    share()
                } label: {
                    Image(systemName: "square.and.arrow.up")
                        .font(.system(size: 16))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(width: 42, height: 40)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(loader.content?.isEmpty != false && webURL == nil)
                .accessibilityLabel("Share")

                Menu {
                    if let webURL {
                        Button { openURL(webURL) } label: { Label("Open in browser", systemImage: "arrow.up.right.square") }
                    }
                    Button {
                        UIPasteboard.general.string = loader.content
                        Haptics.success()
                    } label: { Label("Copy content", systemImage: "doc.on.doc") }
                    .disabled(loader.content == nil)
                } label: {
                    Image(systemName: "ellipsis")
                        .font(.system(size: 16))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(width: 42, height: 40)
                        .contentShape(Rectangle())
                }
                .accessibilityLabel("More")
            }
            .padding(.horizontal, 2)
            .glassSurface(cornerRadius: 20)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(Color("BgPrimary"))
    }

    /// The content as a file named after the card (Save to Files, AirDrop…),
    /// else the link.
    private func share() {
        if let text = loader.content, !text.isEmpty {
            let ext = artifact.fileType.lowercased().filter { $0.isLetter || $0.isNumber }
            let hasExtension = !(artifact.title as NSString).pathExtension.isEmpty
            let name = hasExtension ? artifact.title : "\(artifact.title).\(ext.isEmpty ? "txt" : ext)"
            do {
                sharedFile = SharedArtifactFile(url: try ArtifactFileShare.write(Data(text.utf8), suggestedName: name, mimeType: nil))
            } catch {
                shareError = plainErrorText(error)
            }
        } else if let webURL {
            sharedFile = SharedArtifactFile(url: webURL)
        }
    }

    // MARK: - Content states

    @ViewBuilder
    private var content: some View {
        switch loader.state {
        case .idle, .loading:
            VStack(spacing: 12) {
                ProgressView()
                    .tint(Color("AccentPrimary"))
                Text("Loading artifact…")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }

        case .failed(let message):
            FriendlyStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "Couldn't load this artifact",
                message: FriendlyErrorMessage.from(message),
                actionTitle: "Retry",
                action: { Task { await loader.retry(artifact: artifact) } }
            )

        case .loaded(let text):
            if text.isEmpty {
                FriendlyStateView(
                    style: .empty,
                    title: "This artifact is empty",
                    message: "Nothing was captured for this artifact.",
                    art: .sheets
                )
            } else if selectedTab == 0 && artifact.isPreviewable {
                // Sandboxed execution preview (custom scheme + CSP, no network).
                SandboxedArtifactWebView(content: text, artifactType: artifact.fileType)
            } else {
                // Scrollable plain code container
                ScrollView {
                    VStack(alignment: .leading) {
                        Text(text)
                            .font(.system(.subheadline, design: .monospaced))
                            .foregroundColor(Color("TextPrimary"))
                            .padding()
                            .textSelection(.enabled)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
    }
}
