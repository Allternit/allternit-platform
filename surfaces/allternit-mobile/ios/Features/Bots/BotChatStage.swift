import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

/// A bot chat's live stage, phone layout (allternit-ai src/components/bots/
/// stage/live/BotLiveStage.tsx): the chat opens on the bot itself, big and
/// alive above its conversation, with what needs you, what it suggests, its
/// steps while it works, "Since you were last here", a montage of what it
/// made, the next routine, and "Hand it a file". Swiping up (or sending) flies the bot up into the
/// thread top's avatar and the conversation takes the screen; a tap on that
/// avatar brings it back (BotChatSessionView owns the docked state).
struct BotChatStage: View {
    let bot: AgentRecord
    /// What the bot is doing now, under it ("Working…", "Needs you").
    let statusLine: String?
    let working: Bool
    /// Threads waiting on you.
    let needs: [BotThread]
    /// Threads the bot proposed (Go ahead / Not now).
    let suggestions: [BotThread]
    let onOpen: (BotThread) -> Void
    let onApprove: (BotThread) -> Void
    let onDecline: (BotThread) -> Void
    /// Sends the hand-off prompt with its files (lib/attachments/handoff.ts).
    let onHandoff: (String, [StagedAttachment]) -> Void
    /// The swipe up that docks the bot.
    let onDock: () -> Void
    /// The session on screen; nil is the bot's main chat.
    var sessionId: String? = nil
    /// This turn's tool calls as steps, used when the thread keeps no checklist.
    var liveSteps: [ThreadTodo] = []
    var onStop: (() -> Void)? = nil
    var onTakeOver: (() -> Void)? = nil

    @ObservedObject private var colors = BotColorStore.shared
    @ObservedObject private var store = BotModeStore.shared
    @StateObject private var live = BotStageLiveStore()
    @State private var openArtifact: ArtifactV2Summary? = nil
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var floating = false
    @State private var files: [StagedAttachment] = []
    @State private var isImporting = false
    @State private var photoItems: [PhotosPickerItem] = []
    @State private var pickError: String? = nil

    /// Phone: clamp(min(h × 0.22, w × 0.44), 104, 190).
    private var faceSize: CGFloat {
        let bounds = UIScreen.main.bounds
        return min(max(min(bounds.height * 0.22 * 0.82, bounds.width * 0.44), 104), 190)
    }

    private var accent: Color { colors.hex(for: bot).map { Color(hex: $0) } ?? Color("AccentPrimary") }

    var body: some View {
        VStack(spacing: 10) {
            face
            Text(statusLine ?? " ")
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(1)
                .accessibilityAddTraits(.updatesFrequently)
            cards
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
        .padding(.bottom, 4)
        .frame(maxWidth: .infinity)
        .background {
            // The phone stage's wash: the bot's colour round where it stands.
            RadialGradient(colors: [accent.opacity(0.18), accent.opacity(0.06), .clear],
                           center: UnitPoint(x: 0.5, y: 0.22), startRadius: 0, endRadius: 320)
                .allowsHitTesting(false)
        }
        .contentShape(Rectangle())
        // A swipe up on the stage takes the bot into the thread top.
        .gesture(DragGesture(minimumDistance: 16).onEnded { value in
            if value.translation.height < -40 { onDock() }
        })
        .onAppear {
            guard !reduceMotion else { return }
            withAnimation(.easeInOut(duration: 1.7).repeatForever(autoreverses: true)) { floating = true }
        }
        .fileImporter(isPresented: $isImporting, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
            importFiles(result)
        }
        .onChange(of: photoItems) { _, items in
            guard !items.isEmpty else { return }
            Task { await importPhotos(items) }
        }
        .onAppear { live.open(botId: bot.id) }
        .task(id: sessionKey) { await live.load(botId: bot.id, sessions: botSessions) }
        .fullScreenCover(item: $openArtifact) { artifact in
            V2ArtifactView(artifactId: artifact.id, initialTitle: artifact.title)
        }
    }

    // MARK: Live

    private var botThreads: [BotThread] { store.threads(for: bot.id) }

    /// The thread on screen: the one on this session, else the bot's main chat.
    private var currentThread: BotThread? {
        let main = store.canonicalChatIds[bot.id]
        let session = sessionId ?? main
        let isMainChat = sessionId == nil || sessionId == main
        return botThreads.first { session != nil && $0.currentSessionId == session }
            ?? (isMainChat ? botThreads.first(where: \.isMainChat) : nil)
    }

    /// Every session of this bot → its thread's title (nil for the one on screen).
    private var botSessions: [String: String?] {
        var sessions: [String: String?] = [:]
        for thread in botThreads {
            if let id = thread.currentSessionId { sessions[id] = thread.id == currentThread?.id ? .some(nil) : .some(thread.title) }
        }
        if let id = sessionId ?? store.canonicalChatIds[bot.id] { sessions[id] = .some(nil) }
        return sessions
    }

    private var sessionKey: String { botSessions.keys.sorted().joined(separator: ",") }

    /// The checklist; with none, this turn's tool calls while it works.
    private var steps: [ThreadTodo] {
        let listed = BotStageLiveModel.visibleSteps(currentThread?.todo)
        return listed.isEmpty && working ? liveSteps : listed
    }

    // MARK: Face

    private var face: some View {
        ZStack {
            Circle()
                .fill(RadialGradient(colors: [accent.opacity(0.2), .clear], center: .center, startRadius: 0, endRadius: faceSize * 0.46))
                .blur(radius: 8)
                .padding(faceSize * 0.04)
            Ellipse()
                .fill(RadialGradient(colors: [accent.opacity(0.4), .clear], center: .center, startRadius: 0, endRadius: faceSize * 0.22))
                .frame(width: faceSize * 0.44, height: faceSize * 0.05)
                .scaleEffect(x: floating ? 0.86 : 1, y: 1)
                .opacity(floating ? 0.32 : 0.5)
                .frame(maxHeight: .infinity, alignment: .bottom)
                .padding(.bottom, faceSize * 0.02)
            BotAvatarView(bot: bot, size: faceSize, live: true)
                .offset(y: floating ? -9 : 0)
                .scaleEffect(working && !reduceMotion ? 1.03 : 1)
        }
        .frame(width: faceSize, height: faceSize)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(bot.botDisplayName)
    }

    // MARK: Cards

    @ViewBuilder
    private var cards: some View {
        if !needs.isEmpty {
            StageCard(label: "Needs you", accent: accent) {
                ForEach(needs.prefix(3)) { thread in
                    stageRow(thread.title, thread.statusLine ?? "Waiting on you") { onOpen(thread) }
                }
            }
        }
        if !suggestions.isEmpty {
            StageCard(label: suggestions.count == 1 ? "\(bot.botDisplayName) suggests" : "\(bot.botDisplayName) suggests \(suggestions.count) things", accent: accent) {
                ForEach(suggestions.prefix(2)) { thread in
                    VStack(alignment: .leading, spacing: 8) {
                        stageRow(thread.title, thread.statusLine) { onOpen(thread) }
                        HStack(spacing: 6) {
                            StageChip(title: "Go ahead", primary: true) { onApprove(thread) }
                            StageChip(title: "Not now") { onDecline(thread) }
                        }
                    }
                }
            }
        }
        if !files.isEmpty {
            StageCard(label: "Before I start", accent: accent) {
                ForEach(files) { file in
                    HStack(spacing: 8) {
                        Image(systemName: file.fileIcon).foregroundColor(Color("TextSecondary"))
                        Text(file.filename).font(.system(size: 13)).lineLimit(1).truncationMode(.middle)
                        Spacer(minLength: 0)
                        Button { files.removeAll { $0.id == file.id } } label: {
                            Image(systemName: "xmark").font(.system(size: 11, weight: .semibold))
                                .foregroundColor(Color("TextTertiary"))
                                .frame(width: 28, height: 28)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Remove \(file.filename)")
                    }
                    .padding(.leading, 10)
                    .padding(.vertical, 2)
                    .background(RoundedRectangle(cornerRadius: 10).fill(Color("FillNeutral")))
                }
                HStack(spacing: 6) {
                    StageChip(title: "Plan it first") { handOff(.plan) }
                    StageChip(title: "Just do it", primary: true) { handOff(.doIt) }
                    StageChip(title: "Ask me questions") { handOff(.ask) }
                }
                .padding(.top, 4)
            }
        }
        if !steps.isEmpty {
            StageStepsCard(steps: steps, working: working, accent: accent, onStop: onStop, onTakeOver: onTakeOver)
        }
        if let briefing = BotStageLiveModel.briefing(threads: botThreads, routines: live.routines,
                                                     since: live.seenAt, currentThreadId: currentThread?.id) {
            StageBriefingCard(briefing: briefing, onOpen: onOpen)
        }
        let frames = BotStageLiveModel.montage(saved: live.saved, todo: currentThread?.todo)
        if !frames.isEmpty {
            StageMontageCard(frames: frames, working: working) { openArtifact = $0 }
        }
        if let next = BotStageLiveModel.nextUp(live.routines) {
            StageNextUpCard(text: next)
        }
        if files.isEmpty {
            Menu {
                Button { isImporting = true } label: { Label("Choose files", systemImage: "doc.badge.plus") }
                PhotosPicker(selection: $photoItems, maxSelectionCount: 6, matching: .images) {
                    Label("Choose photos", systemImage: "photo")
                }
            } label: {
                Label("Hand \(bot.botDisplayName) a file", systemImage: "paperclip")
                    .font(.system(size: 12.5, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.horizontal, 13)
                    .frame(minHeight: 32)
                    .background(Capsule().fill(Color("BgPanel")))
                    .overlay(Capsule().stroke(Color("BorderSubtle")))
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        if let pickError {
            Text(pickError).font(.caption).foregroundColor(Theme.statusError)
        }
    }

    private func stageRow(_ title: String, _ detail: String?, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 1) {
                Text(title).font(.system(size: 13.5, weight: .semibold)).foregroundColor(Color("TextPrimary")).lineLimit(1)
                if let detail {
                    Text(detail).font(.system(size: 12)).foregroundColor(Color("TextSecondary")).lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: Hand-off

    private enum Choice { case plan, doIt, ask }

    private func handOff(_ choice: Choice) {
        guard !files.isEmpty else { return }
        let names = files.map(\.filename)
        let what = names.count == 1 ? names[0] : "these \(names.count) files"
        let text: String
        switch choice {
        case .plan: text = "Plan how you'll handle \(what) before doing anything. Lay out the steps and wait for my go-ahead."
        case .ask: text = "Before you start on \(what), ask me the questions you need answered."
        case .doIt: text = "Work on \(what)."
        }
        onHandoff(text, files)
        files = []
    }

    private func importFiles(_ result: Result<[URL], Error>) {
        switch result {
        case .failure(let error):
            pickError = error.localizedDescription
        case .success(let urls):
            pickError = nil
            for url in urls {
                let scoped = url.startAccessingSecurityScopedResource()
                defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                guard let data = try? Data(contentsOf: url) else {
                    pickError = "Couldn't read \(url.lastPathComponent)."
                    continue
                }
                let type = UTType(filenameExtension: url.pathExtension)
                files.append(StagedAttachment(
                    thumbnail: type?.conforms(to: .image) == true ? UIImage(data: data) : nil,
                    data: data,
                    filename: url.lastPathComponent,
                    mediaType: type?.preferredMIMEType ?? "application/octet-stream"
                ))
            }
        }
    }

    private func importPhotos(_ items: [PhotosPickerItem]) async {
        for (index, item) in items.enumerated() {
            guard let data = try? await item.loadTransferable(type: Data.self) else { continue }
            let type = item.supportedContentTypes.first ?? .jpeg
            files.append(StagedAttachment(
                thumbnail: UIImage(data: data),
                data: data,
                filename: "photo-\(index + 1).\(type.preferredFilenameExtension ?? "jpg")",
                mediaType: type.preferredMIMEType ?? "image/jpeg"
            ))
        }
        photoItems = []
    }
}

/// A stage card (bot-live-stage__card): 18pt corners, hairline, the
/// elevated surface; accent cards take the bot's colour (phone).
struct StageCard<Content: View>: View {
    let label: String
    var accent: Color? = nil
    @ViewBuilder let content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(label.uppercased())
                .font(.system(size: 10.5, weight: .semibold))
                .tracking(0.7)
                .foregroundColor(accent ?? Color("TextTertiary"))
            content()
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill(Color("BgPanel")))
        .background(RoundedRectangle(cornerRadius: 18, style: .continuous).fill((accent ?? .clear).opacity(0.06)))
        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous)
            .stroke(accent.map { $0.opacity(0.35) } ?? Color("BorderSubtle")))
        .shadow(color: .black.opacity(0.06), radius: 11, y: 6)
    }
}

/// A stage chip (bot-live-stage__chip): pill, hairline; primary is the
/// near-black button.
struct StageChip: View {
    let title: String
    var primary: Bool = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 12.5, weight: .medium))
                .foregroundColor(primary ? Color("BgPrimary") : Color("TextPrimary"))
                .padding(.horizontal, 13)
                .frame(minHeight: 32)
                .background(Capsule().fill(primary ? Color("TextPrimary") : Color("BgPanel")))
                .overlay(Capsule().stroke(primary ? Color("TextPrimary") : Color("BorderSubtle")))
        }
        .buttonStyle(.plain)
    }
}

/// The thread composer on a bot chat's Threads and Tasks tabs: the same
/// card, tabs tray and compact deck as the chat, sending through `onSend`
/// (a new thread, or a task into this thread).
struct BotTabComposer: View {
    let placeholder: String
    let topDeck: AnyView
    let onSend: @MainActor (String) async throws -> Void

    @StateObject private var attachments = AttachmentStore()
    @State private var text = ""
    @State private var sending = false
    @State private var error: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            ComposerView(
                inputText: $text,
                attachmentStore: attachments,
                isStreaming: false,
                isSendDisabled: sending,
                hasActiveSession: true,
                deck: .compact,
                onSend: { _ in send() },
                onStop: {},
                onVoiceMode: {},
                placeholderOverride: placeholder,
                showsPermission: false,
                topDeck: topDeck,
                showsVoice: false,
                showsAttach: false
            )
            if let error {
                Text(error)
                    .font(.system(size: 12))
                    .foregroundColor(Theme.statusWarning)
                    .padding(.horizontal, 8)
            }
        }
        .padding(.horizontal, 14)
        .padding(.top, 6)
        .padding(.bottom, 8)
    }

    private func send() {
        let draft = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !draft.isEmpty, !sending else { return }
        sending = true
        error = nil
        Task {
            defer { sending = false }
            do {
                try await onSend(draft)
                text = ""
            } catch {
                self.error = "Couldn't start the thread: \(FriendlyErrorMessage.from(error.localizedDescription))"
            }
        }
    }
}
