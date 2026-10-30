import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

/// The live bot stage (allternit-ai src/components/bots/stage/live/
/// BotLiveStage.tsx, phone layout): the bot large and alive over its main
/// chat, what it's doing right now, and a hand-off tray — give it files,
/// photos or links, then pick Plan first / Do it / Ask me first
/// (lib/attachments/handoff.ts `handoffPrompt`). The hand-off sends
/// through the same chat stack as the conversation below it.
struct BotLiveStageView: View {
    let bot: AgentRecord

    @StateObject private var viewModel = ChatViewModel()
    @ObservedObject private var store = BotModeStore.shared
    @ObservedObject private var status = BotStatusStore.shared
    @EnvironmentObject private var navigator: BotNavigator
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    @State private var sessionId: String? = nil
    @State private var resolveError: String? = nil
    @State private var files: [StagedAttachment] = []
    @State private var links: [String] = []
    @State private var linkDraft = ""
    @State private var note = ""
    @State private var isImporting = false
    @State private var photoItems: [PhotosPickerItem] = []
    @State private var pickError: String? = nil
    @State private var isTrayOpen = false

    private enum Choice: String, CaseIterable {
        case plan, doIt, ask
        var label: String {
            switch self {
            case .plan: return "Plan first"
            case .doIt: return "Do it"
            case .ask: return "Ask me first"
            }
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            stage
            if isTrayOpen { tray }
            if let sessionId {
                ChatContentView(sessionId: sessionId, viewModel: viewModel)
            } else if let resolveError {
                FriendlyStateView(style: .error, icon: "exclamationmark.bubble", title: "Couldn't open the stage",
                                  message: resolveError, actionTitle: "Try again", action: { Task { await resolve() } })
                    .frame(maxHeight: .infinity)
            } else {
                ProgressView().frame(maxHeight: .infinity)
            }
        }
        .background(Color("BgPrimary"))
        .navigationTitle(bot.botDisplayName)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button { navigator.open(.botComputer(bot.id)) } label: { Image(systemName: "desktopcomputer") }
                    .accessibilityLabel("Open computer")
            }
        }
        .task {
            status.subscribe(botId: bot.id)
            await resolve()
        }
        .onDisappear { status.unsubscribe(botId: bot.id) }
        .fileImporter(isPresented: $isImporting, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
            importFiles(result)
        }
        .onChange(of: photoItems) { _, items in
            guard !items.isEmpty else { return }
            Task { await importPhotos(items) }
        }
    }

    // MARK: Stage

    private var stage: some View {
        let working = status.isWorking(for: bot.id) || viewModel.isStreaming
        let waiting = status.needsAttention(for: bot.id)
        return VStack(spacing: 8) {
            BotAvatarView(bot: bot, size: 96, live: true)
                .scaleEffect(working && !reduceMotion ? 1.04 : 1)
                .animation(reduceMotion ? nil : .easeInOut(duration: 1.1).repeatForever(autoreverses: true), value: working)
            Text(stageLine(working: working, waiting: waiting))
                .font(.subheadline)
                .foregroundColor(waiting ? Theme.statusWarning : Color("TextSecondary"))
                .lineLimit(2)
                .multilineTextAlignment(.center)
            Button {
                withAnimation(reduceMotion ? nil : .spring(response: 0.32, dampingFraction: 0.86)) { isTrayOpen.toggle() }
            } label: {
                Label(isTrayOpen ? "Close" : "Hand it something", systemImage: isTrayOpen ? "xmark" : "tray.and.arrow.down")
                    .font(.subheadline.weight(.medium))
                    .padding(.horizontal, 14)
                    .frame(minHeight: 36)
                    .background(Color("BgSecondary"))
                    .clipShape(Capsule())
            }
            .buttonStyle(.plain)
        }
        .padding(.vertical, 14)
        .frame(maxWidth: .infinity)
        .background(
            LinearGradient(colors: [Color("AccentPrimary").opacity(0.12), .clear], startPoint: .top, endPoint: .bottom)
        )
    }

    private func stageLine(working: Bool, waiting: Bool) -> String {
        if let thread = store.threads.first(where: { $0.currentSessionId == sessionId }), let line = thread.statusLine {
            return line
        }
        if waiting { return "Needs you" }
        if working { return "Working…" }
        return "Ready when you are"
    }

    // MARK: Tray

    private var tray: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Button { isImporting = true } label: { Label("Files", systemImage: "doc.badge.plus") }
                PhotosPicker(selection: $photoItems, maxSelectionCount: 6, matching: .images) {
                    Label("Photos", systemImage: "photo")
                }
                Spacer()
            }
            .buttonStyle(.bordered)
            HStack {
                TextField("Paste a link", text: $linkDraft)
                    .textInputAutocapitalization(.never)
                    .keyboardType(.URL)
                    .autocorrectionDisabled()
                    .onSubmit(addLinks)
                Button("Add", action: addLinks).disabled(linkDraft.isEmpty)
            }
            if !files.isEmpty || !links.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(files) { file in
                            chip(file.filename) { files.removeAll { $0.id == file.id } }
                        }
                        ForEach(links, id: \.self) { link in
                            chip(link) { links.removeAll { $0 == link } }
                        }
                    }
                }
                TextField("Anything to add? (optional)", text: $note, axis: .vertical)
                    .lineLimit(1...3)
                HStack(spacing: 8) {
                    Button(Choice.plan.label) { handOff(.plan) }
                        .buttonStyle(.bordered)
                    Button(Choice.doIt.label) { handOff(.doIt) }
                        .buttonStyle(.borderedProminent)
                    Button(Choice.ask.label) { handOff(.ask) }
                        .buttonStyle(.bordered)
                }
                .font(.subheadline)
                .disabled(sessionId == nil || viewModel.isStreaming)
            }
            if let pickError {
                Text(pickError).font(.caption).foregroundColor(Theme.statusError)
            }
        }
        .padding(14)
        .background(Color("BgSecondary"))
        .transition(.move(edge: .top).combined(with: .opacity))
    }

    private func chip(_ title: String, remove: @escaping () -> Void) -> some View {
        HStack(spacing: 4) {
            Text(title).font(.caption).lineLimit(1).frame(maxWidth: 160)
            Button(action: remove) { Image(systemName: "xmark.circle.fill") }
                .accessibilityLabel("Remove \(title)")
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(Color("BgPrimary"))
        .clipShape(Capsule())
    }

    // MARK: Actions

    private func resolve() async {
        resolveError = nil
        do {
            sessionId = try await store.mainChatSessionId(for: bot)
        } catch is CancellationError {
        } catch {
            resolveError = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    private func addLinks() {
        let found = linkDraft.split(whereSeparator: { $0.isWhitespace }).map(String.init)
            .filter { $0.lowercased().hasPrefix("http://") || $0.lowercased().hasPrefix("https://") }
        if found.isEmpty {
            pickError = "That doesn't look like a link (it should start with https://)."
            return
        }
        pickError = nil
        for link in found where !links.contains(link) { links.append(link) }
        linkDraft = ""
    }

    private func importFiles(_ result: Result<[URL], Error>) {
        switch result {
        case .failure(let error):
            pickError = error.localizedDescription
        case .success(let urls):
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
            let ext = type.preferredFilenameExtension ?? "jpg"
            files.append(StagedAttachment(
                thumbnail: UIImage(data: data),
                data: data,
                filename: "photo-\(index + 1).\(ext)",
                mediaType: type.preferredMIMEType ?? "image/jpeg"
            ))
        }
        photoItems = []
    }

    private func handOff(_ choice: Choice) {
        let names = files.map(\.filename) + links
        guard !names.isEmpty else { return }
        let what = names.count == 1 ? names[0] : "these \(names.count) items"
        let lead: String
        switch choice {
        case .plan: lead = "Plan how you'll handle \(what) before doing anything. Lay out the steps and wait for my go-ahead."
        case .ask: lead = "Before you start on \(what), ask me the questions you need answered."
        case .doIt: lead = "Work on \(what)."
        }
        var text = lead
        if !links.isEmpty { text += "\n\n" + links.joined(separator: "\n") }
        let extra = note.trimmingCharacters(in: .whitespacesAndNewlines)
        if !extra.isEmpty { text += "\n\n" + extra }
        viewModel.sendMessage(text, attachments: files, runtimeModelId: nil)
        files = []
        links = []
        note = ""
        withAnimation(reduceMotion ? nil : .easeOut(duration: 0.2)) { isTrayOpen = false }
    }
}
