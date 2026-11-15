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

struct ArtifactDetailsView: View {
    let artifact: ArtifactRecord
    @Environment(\.dismiss) var dismiss
    @State private var selectedTab = 0
    @StateObject private var loader = ArtifactContentLoader()

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
            .toolbar {
                ToolbarItem(placement: .navigationBarLeading) {
                    Button("Close") {
                        dismiss()
                    }
                    .foregroundColor(Color("AccentPrimary"))
                }

                ToolbarItem(placement: .navigationBarTrailing) {
                    Button(action: {
                        UIPasteboard.general.string = loader.content
                        let generator = UINotificationFeedbackGenerator()
                        generator.notificationOccurred(.success)
                    }) {
                        Image(systemName: "doc.on.doc")
                            .foregroundColor(Color("TextPrimary"))
                    }
                    .disabled(loader.content == nil)
                    .accessibilityLabel("Copy content")
                }
            }
            .preferredColorScheme(.dark)
        }
        .task {
            await loader.load(artifact: artifact)
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
                    icon: "doc.text",
                    title: "This artifact is empty",
                    message: "Nothing was captured for this artifact."
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
