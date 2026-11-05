import Foundation

/// "Make something new" — the studio kinds of the web's gallery
/// (allternit-ai src/views/library/artifact-gallery.ts +
/// src/lib/artifact-session/artifact-session-kinds.ts). Each tile starts an
/// artifact session: an agent session whose standing instructions say what
/// to make, with a studio pane beside the chat that shows the result.
///
/// On the web, Docs / Sheets / Slides / Design open an Office or Design
/// editor bound to a record in the old sectioned store, which the phone
/// can't host. Here those tiles start the same session making the Artifacts
/// v2 kind instead (doc / sheet / slides / design), which the app's own
/// editor opens; Ops sheets do the same (the web opens a chat with Output →
/// Ops sheets).
enum ArtifactStudioKind: String, CaseIterable, Identifiable, Sendable {
    case docs, sheets, slides, dashboard, ops, motion, design, page, research, build, image, video

    /// The web's gallery order (GALLERY_TILES).
    static let galleryOrder: [ArtifactStudioKind] = allCases

    var id: String { rawValue }

    var label: String {
        switch self {
        case .docs: return "Docs"
        case .sheets: return "Sheets"
        case .slides: return "Slides"
        case .design: return "Design"
        case .ops: return "Ops sheets"
        case .dashboard: return "Dashboards"
        case .motion: return "Motion"
        case .page: return "Page"
        case .build: return "Build"
        case .image: return "Image"
        case .video: return "Video"
        case .research: return "Deep Research"
        }
    }

    var description: String {
        switch self {
        case .docs: return "A document you write and edit in place, with Allternit drafting alongside."
        case .sheets: return "A spreadsheet for data, formulas, and charts."
        case .slides: return "A deck you can present, restyle, and export to PowerPoint."
        case .design: return "Screens, flows, and graphics laid out on a canvas in Allternit Design."
        case .ops: return "Live status, logs and actions for a deploy, site, app or server."
        case .dashboard: return "Live charts and tiles that query your connected tools."
        case .motion: return "An animated explainer you can export as video."
        case .page: return "A single web page you can share."
        case .build: return "A website or app, built and previewed live."
        case .image: return "Generate or edit an image from a prompt."
        case .video: return "A generated video clip, with the length and format you ask for."
        case .research: return "A cited research report built from many sources."
        }
    }

    var noun: String {
        switch self {
        case .docs: return "Document"
        case .sheets: return "Spreadsheet"
        case .slides: return "Presentation"
        case .design: return "Design"
        case .ops: return "Ops sheet"
        case .dashboard: return "Dashboard"
        case .motion: return "Motion"
        case .page: return "Page"
        case .build: return "Build"
        case .image: return "Image"
        case .video: return "Video"
        case .research: return "Report"
        }
    }

    var newTitle: String {
        switch self {
        case .research: return "Untitled research"
        default: return "Untitled \(noun.lowercased())"
        }
    }

    var emptyTitle: String {
        switch self {
        case .docs: return "Draft a plan, memo or brief with Allternit"
        case .sheets: return "Paste your data and turn it into a sheet"
        case .slides: return "Paste your ideas and turn them into slides"
        case .design: return "Describe an idea and turn it into a design"
        case .ops: return "Describe the deploy, site, app or server to watch"
        case .dashboard: return "Describe the numbers you want to watch"
        case .motion: return "Describe a short animated explainer"
        case .page: return "Describe a page you want to share"
        case .build: return "Describe a site or app and watch it build"
        case .image: return "Describe an image and Allternit will make it"
        case .video: return "Describe a clip, its length and format"
        case .research: return "Ask a question and get a cited report"
        }
    }

    var studioHint: String {
        switch self {
        case .docs: return "The document appears here as Allternit writes it, and opens in the editor."
        case .sheets: return "The sheet appears here as Allternit builds it, and opens in the editor."
        case .slides: return "The deck appears here as Allternit builds it, and opens in the editor."
        case .design: return "The design appears here when it's saved."
        case .ops: return "Live status, logs and actions appear here."
        case .dashboard: return "Live charts and tiles that query your connected tools appear here."
        case .motion: return "Scenes play here, and you can export the result as an MP4 on the web or Desktop."
        case .page: return "The page renders here as a live preview you can share."
        case .build: return "The site appears here as it builds, with a live preview you can click through."
        case .image: return "Each image renders here, and you can refine it or ask for variations."
        case .video: return "The clip renders here when it's ready."
        case .research: return "The report fills in here as sources are read, with citations you can open."
        }
    }

    /// The custom kind tile (DeckIcons), as the web's ArtifactKindIcon.
    var iconAsset: String {
        switch self {
        case .docs: return "deck-docs"
        case .sheets: return "deck-sheets"
        case .slides: return "deck-slides"
        case .design: return "deck-design"
        case .ops: return "deck-ops"
        case .dashboard: return "deck-dashboard"
        case .motion: return "deck-motion"
        case .page: return "deck-page"
        case .build: return "deck-build"
        case .image: return "deck-image"
        case .video: return "deck-video"
        case .research: return "deck-research"
        }
    }

    var systemImage: String {
        switch self {
        case .docs: return "doc.text"
        case .sheets: return "tablecells"
        case .slides: return "rectangle.on.rectangle"
        case .design: return "paintbrush.pointed"
        case .ops: return "gauge.with.dots.needle.33percent"
        case .dashboard: return "chart.bar.xaxis"
        case .motion: return "film.stack"
        case .page: return "globe"
        case .build: return "hammer"
        case .image: return "photo"
        case .video: return "video"
        case .research: return "text.magnifyingglass"
        }
    }

    /// The Office editor this kind opens in (`ARTIFACT_SESSION_KINDS[…].office`).
    var officeProduct: String? {
        switch self {
        case .docs, .sheets, .slides: return rawValue
        default: return nil
        }
    }

    /// Agent mode contract id (`metadata.agentModeId`), nil for none.
    var modeId: String? {
        switch self {
        case .docs: return "docs"
        case .sheets: return "data"
        case .slides: return "slides"
        case .design: return "website"
        // These make their own kind: "data" is the Sheets contract.
        case .ops, .dashboard, .motion: return nil
        case .page, .build: return "website"
        case .image: return "image"
        case .video: return "video"
        case .research: return "research"
        }
    }

    /// The v2 kind the session's result is (studio pane filter).
    var studioKind: String {
        switch self {
        case .docs: return "doc"
        case .sheets: return "sheet"
        case .slides: return "slides"
        case .design: return "design"
        case .ops: return "ops"
        case .dashboard: return "dashboard"
        case .motion: return "motion"
        case .page, .build: return "page"
        case .image: return "image"
        case .video: return "video"
        case .research: return "doc"
        }
    }

    /// v2 template kind the org can turn off (`templates[kind] = false`).
    var templateKind: String? {
        switch self {
        case .docs: return "doc"
        case .sheets: return "sheet"
        case .slides: return "slides"
        case .design: return "design"
        case .ops: return "ops"
        case .dashboard: return "dashboard"
        case .motion: return "motion"
        case .page: return "page"
        default: return nil
        }
    }

    /// GALLERY_TILES `beta`.
    var isBeta: Bool { ![.docs, .sheets, .slides, .page].contains(self) }

    /// The session's standing instructions — the web's `artifactNoteBody`
    /// for an unbound binding, plus OWN_TOOLS_NOTE
    /// (src/lib/artifact-session/artifact-session.ts). The kind's mode contract
    /// comes first, as the web's `artifactSessionMetadata` puts it; a session
    /// bound to an open editor uses `editorSystemPrompt` instead, which has none.
    func systemPrompt(title: String) -> String {
        let n = noun.lowercased()
        let note: String
        switch self {
        case .docs, .sheets, .slides, .design, .ops, .dashboard, .motion, .page:
            note = "## Artifact session\n\nThis session makes one \(n), titled \"\(title)\". Make it with `artifact_create` (kind \"\(studioKind)\"); it opens in the pane beside this chat as soon as it is saved. For every later change, call `artifact_read` then `artifact_update` on that same artifact instead of creating another one, unless the user asks for a new one."
        case .image:
            note = "## Artifact session\n\nThis session makes images, starting with \"\(title)\". Make each one with the media_generate tool; the result lands in the session's Outputs and opens beside this chat. Check the rendered PNG it returns and fix it before you call the image done."
        case .video:
            note = "## Artifact session\n\nThis session makes videos, starting with \"\(title)\". Make each one with the media_generate tool (kind \"video\"); the MP4 lands in the session's Outputs and opens beside this chat. Check the contact sheet it returns and fix the scene before you call the video done."
        case .build:
            note = "## Artifact session\n\nThis session makes one build, titled \"\(title)\". Deliver it as one HTML artifact the user can open beside this chat, and make it look finished: web libraries load in the preview (Tailwind or other CSS/JS from a CDN, web fonts, icon sets, https images). Then look at it: call pane_artifact with action \"call\", tool \"preview_output\" to get a picture of the rendered page (describe lists the other tools), compare it with what the user asked for, and deliver a corrected version when something is off. Check the phone width too (width 390) when the site should work on phones. Before you call it done, run artifact_check on the saved artifact: it pictures phone, tablet and laptop widths, lists script errors and sideways scroll, and compares with the user's picture when you pass reference \"attachment\"."
        case .research:
            note = "## Artifact session\n\nThis session makes one \(n), titled \"\(title)\". Deliver it as an artifact the user can open beside this chat."
        }
        return [modeContract, note, Self.ownToolsNote].compactMap { $0 }.joined(separator: "\n\n")
    }

    /// The contract for `modeId`, word for word from the web's
    /// `AGENT_MODE_CONTRACTS` + `buildModeSystemPrompt`
    /// (src/lib/agents/agent-mode-contracts.ts).
    private var modeContract: String? {
        let brief: String
        let artifact: String
        let capabilities: String
        switch modeId {
        case "research":
            brief = "You are executing Deep Research mode. Search multiple authoritative sources, distinguish evidence from inference, attach citations to claims, reconcile conflicting sources, and create a structured research-report artifact. Do not answer from memory alone."
            artifact = "research-report"
            capabilities = "web_search, browser, file_write"
        case "website":
            brief = "You are executing Build mode. Produce a runnable website or app artifact with complete HTML/CSS/JS or framework files, responsive behavior, accessible interaction, and no placeholder sections. Validate the artifact structure before completion."
            artifact = "website"
            capabilities = "artifact_create, file_write"
        case "docs":
            brief = "You are executing Docs mode. Produce an editable structured document artifact—not only chat markdown—with headings, semantic sections, consistent styles, and the requested professional format. Validate completeness and exportability."
            artifact = "document"
            capabilities = "document_create, file_write"
        case "data":
            brief = "You are executing Sheets mode. Produce an editable spreadsheet artifact with typed cells, formulas where appropriate, source-data preservation, and charts or analysis requested by the user. Validate formulas, ranges, and missing-data behavior."
            artifact = "spreadsheet"
            capabilities = "spreadsheet_create, spreadsheet_formula, file_write"
        case "slides":
            brief = "You are executing Slides mode. Produce an editable presentation artifact with a coherent narrative, slide-specific layouts, visual hierarchy, and speaker-ready content. Validate slide count, overflow, and required sections."
            artifact = "presentation"
            capabilities = "presentation_create, file_write"
        case "image":
            brief = "You are executing Image mode. Make the image with the media_generate tool (kind \"image\"; its description explains the lanes and how to write a native image as svg, html or canvas code), check the rendered PNG it returns, and fix it if needed. Match the requested subject, composition, dimensions, and constraints. A textual image description is not completion."
            artifact = "image"
            capabilities = "media_generate"
        case "video":
            brief = "You are executing Video mode. Make the video with the media_generate tool (kind \"video\"; its description explains how to write a native video as a canvas scene with an optional score), check the contact sheet it returns, and fix the scene if needed. Return a playable video artifact with the requested duration and format. A storyboard, EDL, or prose description alone is not completion."
            artifact = "video"
            capabilities = "media_generate"
        default:
            return nil
        }
        return "<!--allternit-mode-contract-->\n\(brief)\n\(Self.completionLaw)\n\nMODE: \(modeId ?? "")\nREQUIRED ARTIFACT: \(artifact)\nREQUIRED CAPABILITIES: \(capabilities)\n<!--/allternit-mode-contract-->"
    }

    private static let completionLaw = "MODE EXECUTION LAW:\n- Do not claim completion from prose alone.\n- Produce the required artifact and use the required capabilities.\n- If a required capability or provider is unavailable, return MODE_EXECUTION_BLOCKED with the missing capability. Never simulate a successful artifact.\n- Preserve the user's requested format and validate the deliverable before reporting success.\n- End successful work with MODE_EXECUTION_COMPLETE and a concise artifact manifest."

    static let ownToolsNote = "The session tools named here (pane_artifact, media_generate, pane_browser) may be listed with a prefix, such as mcp__allternit__pane_artifact; they are the same tools. Do this session's work with them, in Allternit. Don't create or edit documents, images or files through outside connectors or services (claude.ai Docs, Google Drive, Notion, email and the like), and don't write a local file instead of changing what the user has open, unless the user explicitly asks for that."
}

/// A started studio session, handed to `ArtifactStudioView`.
struct ArtifactStudioSession: Identifiable, Hashable, Sendable {
    let sessionId: String
    let kind: ArtifactStudioKind
    let title: String
    let startedAt: Date
    /// The blank Docs/Sheets/Slides record the session's editor is bound to.
    var artifactId: String? = nil
    /// The Design canvas the session's editor is bound to.
    var designProjectId: String? = nil
    var id: String { sessionId }

    /// The editor the web opens with this session (hasEditor), if any.
    var editorRoute: ArtifactWebRoute? {
        if let artifactId, let product = kind.officeProduct {
            return .office(product: product, artifactId: artifactId)
        }
        if let designProjectId { return .design(projectId: designProjectId, title: title) }
        // A Dashboard, Page or the like the session already made, bound on the web.
        if let artifactId { return .window(id: artifactId) }
        return nil
    }
}

/// Starts studio sessions and remembers each one's standing instructions,
/// which ride on every turn as the agent-chat body's `systemPrompt`
/// (v1_routes.rs `client_system_prompt`) — the phone has no session-level
/// prompt store, and the web sends its prompt per turn too. Persisted so a
/// studio thread reopened later keeps working on the same artifact.
@MainActor
final class ArtifactStudioLauncher: ObservableObject {
    static let shared = ArtifactStudioLauncher()

    @Published private(set) var launching: ArtifactStudioKind?

    private static let defaultsKey = "artifactStudio.sessions.v1"
    private struct Stored: Codable {
        let kind: String
        let title: String
        let prompt: String
        let startedAt: Date
        var artifactId: String? = nil
        var designProjectId: String? = nil
    }
    @Published private var stored: [String: Stored]
    /// Session ids already looked up and found not to be editor sessions.
    private var knownPlain: Set<String> = []

    private init() {
        if let data = UserDefaults.standard.data(forKey: Self.defaultsKey),
           let decoded = try? JSONDecoder().decode([String: Stored].self, from: data) {
            stored = decoded
        } else {
            stored = [:]
        }
    }

    /// The standing instructions for a studio session; nil for any other.
    func systemPrompt(for sessionId: String?) -> String? {
        guard let sessionId, let s = stored[sessionId] else { return nil }
        // Built fresh, so a session started on an older app gets today's prompt.
        return ArtifactStudioKind(rawValue: s.kind)?.systemPrompt(title: s.title) ?? s.prompt
    }

    /// The studio session record for a session id, if it is one.
    func session(for sessionId: String) -> ArtifactStudioSession? {
        guard let s = stored[sessionId], let kind = ArtifactStudioKind(rawValue: s.kind) else { return nil }
        return ArtifactStudioSession(sessionId: sessionId, kind: kind, title: s.title, startedAt: s.startedAt,
                                     artifactId: s.artifactId, designProjectId: s.designProjectId)
    }

    /// Sessions with an editor (Docs, Sheets, Slides, Design), newest first.
    /// Their records aren't in the Artifacts v2 library, so this is how the
    /// person gets back to one.
    var editorSessions: [ArtifactStudioSession] {
        stored.keys.compactMap { session(for: $0) }
            .filter { $0.kind.officeProduct != nil && $0.artifactId != nil || $0.designProjectId != nil }
            .sorted { $0.startedAt > $1.startedAt }
    }

    /// Creates the agent session (`POST /api/v1/agent-sessions`, Cowork
    /// surface, agent mode, the kind's mode contract) and records its prompt.
    /// One launch at a time, like the web's `launchArtifactType`.
    func launch(_ kind: ArtifactStudioKind) async throws -> ArtifactStudioSession {
        guard launching == nil else { throw CancellationError() }
        launching = kind
        defer { launching = nil }
        let title = kind.newTitle
        // Docs/Sheets/Slides start on a blank record and Design on a new canvas,
        // as `startArtifactSession` does. If the record can't be made the session
        // still opens, unbound, so the person isn't stranded.
        var artifactId: String?
        if let product = kind.officeProduct {
            artifactId = try? await OfficeArtifactClient.shared.createBlank(title: title, product: product).id
        }
        let designProjectId = kind == .design ? UUID().uuidString.lowercased() : nil
        let session = try await AgentChatClient().createSession(
            name: title,
            originSurface: "cowork",
            sessionMode: "agent",
            agentModeId: kind.modeId
        )
        let started = ArtifactStudioSession(sessionId: session.id, kind: kind, title: title, startedAt: Date(),
                                            artifactId: artifactId, designProjectId: designProjectId)
        stored[session.id] = Stored(kind: kind.rawValue, title: title, prompt: kind.systemPrompt(title: title), startedAt: started.startedAt,
                                    artifactId: artifactId, designProjectId: designProjectId)
        // Keep the newest 200 so the map can't grow without bound.
        if stored.count > 200 {
            for key in stored.sorted(by: { $0.value.startedAt < $1.value.startedAt }).prefix(stored.count - 200).map(\.key) {
                stored.removeValue(forKey: key)
            }
        }
        persist()
        // The binding rides on the session, as the web's does, so Recents on
        // another install or on the web opens the editor too. A failure only
        // costs that.
        try? await APIClient.shared.patch(
            path: "agent-sessions/\(Self.escape(session.id))",
            body: SessionBindingPatch(metadata: .init(artifactSession: .init(
                kind: kind.rawValue, title: title, artifactId: artifactId, designProjectId: designProjectId
            )))
        )
        return started
    }

    private func persist() {
        if let data = try? JSONEncoder().encode(stored) {
            UserDefaults.standard.set(data, forKey: Self.defaultsKey)
        }
    }

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))) ?? id
    }

    // MARK: Sessions from another install

    private struct SessionBinding: Codable {
        let kind: String
        let title: String?
        let artifactId: String?
        let designProjectId: String?
    }

    private struct SessionBindingPatch: Encodable {
        struct Metadata: Encodable { let artifactSession: SessionBinding }
        let metadata: Metadata
    }

    private struct SessionRow: Decodable {
        struct Metadata: Decodable {
            let artifactSession: SessionBinding?
            let agentModeId: String?
            let originSurface: String?
        }
        let name: String?
        let createdAt: String?
        let metadata: Metadata?

        enum CodingKeys: String, CodingKey {
            case name, metadata
            case createdAt = "created_at"
        }
    }

    private struct RecordList: Decodable {
        struct Record: Decodable {
            let id: String
            let title: String?
            let tags: [String]?
            let createdAt: String?

            enum CodingKeys: String, CodingKey {
                case id, title, tags, createdAt
                case createdSnake = "created_at"
            }

            init(from decoder: Decoder) throws {
                let c = try decoder.container(keyedBy: CodingKeys.self)
                id = try c.decode(String.self, forKey: .id)
                title = try? c.decodeIfPresent(String.self, forKey: .title)
                tags = try? c.decodeIfPresent([String].self, forKey: .tags)
                createdAt = (try? c.decodeIfPresent(String.self, forKey: .createdAt))
                    ?? (try? c.decodeIfPresent(String.self, forKey: .createdSnake))
            }
        }
        let artifacts: [Record]
    }

    /// What a Recents row opens as.
    enum SessionLookup {
        case editor(ArtifactStudioSession)
        case chat
        /// It made a file here, but nothing says which one any more.
        case unlinked(noun: String)
    }

    /// Whether opening this session needs a look at the server: the list row
    /// names a binding (the web's sessions are on the chat surface), or it's on
    /// the surface this app's are made on. A plain chat opens at once.
    func needsLookup(id: String) -> Bool {
        guard stored[id] == nil, !knownPlain.contains(id) else { return false }
        guard let row = ShellSessionsStore.shared.sessions.first(where: { $0.id == id }) else { return true }
        if row.artifactSessionKind != nil { return true }
        guard let surface = row.originSurface else { return true }
        return surface == "cowork" || surface.isEmpty
    }

    /// "Open as chat": not asked again this launch.
    func treatAsChat(id: String) {
        knownPlain.insert(id)
    }

    /// The editor behind a session from another install: the binding on the
    /// session, else (an older app saved none) the one record it could only be.
    /// The records don't name their session, so that means the kind, the time
    /// the session started and exactly one fit: the default title identifies the
    /// kind, and so does the mode of a renamed session.
    func lookUpSession(id: String) async -> SessionLookup {
        if let known = session(for: id) { return .editor(known) }
        guard !knownPlain.contains(id),
              let row: SessionRow = try? await APIClient.shared.get(path: "agent-sessions/\(Self.escape(id))") else { return .chat }
        let started = libraryParseDate(row.createdAt) ?? Date()
        let titled = Self.editorKinds.first { $0.newTitle == row.name }
        let moded = row.metadata?.originSurface == "cowork" ? Self.editorKind(mode: row.metadata?.agentModeId) : nil
        var binding = row.metadata?.artifactSession
        if binding == nil, let kind = titled ?? moded {
            binding = await findRecord(kind: kind, started: started, window: titled != nil ? 300 : 120)
        }
        // Any kind with a binding opens its session. Kinds with no file yet (an
        // Image or Page that hasn't made anything) show their pane until it does.
        guard let binding, let kind = ArtifactStudioKind(rawValue: binding.kind) else {
            // "data" and "website" also start dashboards and pages, which have no file to lose.
            if let lost = titled ?? (moded == .docs || moded == .slides ? moded : nil) { return .unlinked(noun: lost.noun.lowercased()) }
            knownPlain.insert(id)
            return .chat
        }
        let title = binding.title ?? row.name ?? kind.newTitle
        stored[id] = Stored(kind: kind.rawValue, title: title, prompt: kind.systemPrompt(title: title), startedAt: started,
                            artifactId: binding.artifactId, designProjectId: binding.designProjectId)
        persist()
        return session(for: id).map(SessionLookup.editor) ?? .chat
    }

    private static let editorKinds: [ArtifactStudioKind] = [.docs, .sheets, .slides, .design]

    private static func editorKind(mode: String?) -> ArtifactStudioKind? {
        switch mode {
        case "docs": return .docs
        case "data": return .sheets
        case "slides": return .slides
        case "website": return .design
        default: return nil
        }
    }

    private func findRecord(kind: ArtifactStudioKind, started: Date, window: TimeInterval) async -> SessionBinding? {
        let tagPrefix = kind == .design ? "design:" : "office:\(kind.rawValue)"
        let path = kind == .design ? "artifacts?type=design" : "artifacts"
        guard let list: RecordList = try? await APIClient.shared.get(path: path) else { return nil }
        let fits = list.artifacts.filter { record in
            guard record.tags?.contains(where: { $0.hasPrefix(tagPrefix) }) == true,
                  let made = libraryParseDate(record.createdAt) else { return false }
            return abs(made.timeIntervalSince(started)) < window
        }
        guard fits.count == 1, let record = fits.first else { return nil }
        if kind == .design {
            let project = record.tags?.first { $0.hasPrefix(tagPrefix) }.map { String($0.dropFirst(tagPrefix.count)) }
            return SessionBinding(kind: kind.rawValue, title: record.title, artifactId: nil, designProjectId: project)
        }
        return SessionBinding(kind: kind.rawValue, title: record.title, artifactId: record.id, designProjectId: nil)
    }

    // MARK: Delete

    /// Removes an editor session: the chat (the web's deleteSession), and its
    /// record, which the server only deletes for its owner. Nothing changes here
    /// until both calls are through, so a failure leaves the row to retry.
    func delete(_ session: ArtifactStudioSession) async throws {
        func ignoringGone(_ call: () async throws -> Void) async throws {
            do {
                try await call()
            } catch let error as APIError where error.httpStatus == 404 {
                return
            }
        }
        var recordId = session.artifactId
        if recordId == nil, let project = session.designProjectId {
            let list: RecordList = try await APIClient.shared.get(path: "artifacts?type=design")
            recordId = list.artifacts.first { $0.tags?.contains("design:\(project)") == true }?.id
        }
        if let recordId {
            try await ignoringGone { try await APIClient.shared.delete(path: "artifacts/\(Self.escape(recordId))") }
        }
        // An older row can hold an id the session API refuses, with no session behind it.
        if session.sessionId.hasPrefix("ses") {
            try await ignoringGone { try await APIClient.shared.delete(path: "agent-sessions/\(Self.escape(session.sessionId))") }
        }
        stored.removeValue(forKey: session.sessionId)
        persist()
    }
}
