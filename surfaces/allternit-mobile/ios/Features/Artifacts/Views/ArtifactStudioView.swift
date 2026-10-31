import SwiftUI

/// A studio session on the phone: the chat, and — one tap away, as the
/// PWA's phone layout does with its full-screen pane — the kind's real editor.
/// Docs, Sheets, Slides and Design open on their editor straight away, as the
/// web does (`takePhoneEditorOpen`). The other kinds show the kind's empty
/// state until the agent makes something; the pane then opens the newest v2
/// artifact whose `origin.session_id` is this session in the web's artifact
/// window. The pane refreshes while it is open.
struct ArtifactStudioView: View {
    let studio: ArtifactStudioSession

    @Environment(\.dismiss) private var dismiss
    @StateObject private var viewModel = ChatViewModel()
    @State private var showingPane: Bool
    @State private var results: [ArtifactV2Summary] = []
    @State private var paneError: String?

    init(studio: ArtifactStudioSession) {
        self.studio = studio
        _showingPane = State(initialValue: studio.editorRoute != nil)
    }

    /// What the pane shows: the agent's newest result, else the session's editor.
    private var paneRoute: ArtifactWebRoute? {
        if let newest = results.first { return .window(id: newest.id) }
        return studio.editorRoute
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().background(Color("BorderSubtle"))
            ZStack {
                ChatContentView(sessionId: studio.sessionId, viewModel: viewModel)
                    .opacity(showingPane ? 0 : 1)
                    .allowsHitTesting(!showingPane)
                if showingPane {
                    pane
                        .ignoresSafeArea(edges: .bottom)
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .task(id: studio.sessionId) {
            // Refresh the pane's results every few seconds while the studio
            // is open, so a result appears as soon as the agent saves it.
            while !Task.isCancelled {
                await refreshResults()
                try? await Task.sleep(for: .seconds(showingPane ? 4 : 8))
            }
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Button { dismiss() } label: {
                Image(systemName: "xmark")
                    .font(.body.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 44, height: 44)
            }
            .accessibilityLabel("Close")

            VStack(alignment: .leading, spacing: 1) {
                Text(studio.title)
                    .font(.system(size: 15, weight: .semibold, design: .serif))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                    .minimumScaleFactor(0.85)
                Text(studio.kind.label)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            Spacer(minLength: 4)
            Picker("View", selection: Binding(
                get: { showingPane },
                set: { value in withAnimation(.spring(response: 0.32, dampingFraction: 0.9)) { showingPane = value } }
            )) {
                Text("Chat").tag(false)
                Text(paneLabel).tag(true)
            }
            .pickerStyle(.segmented)
            .fixedSize()
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(Color("BgPrimary"))
    }

    /// Short enough not to truncate beside the title ("Spreadsheet" did).
    private var paneLabel: String {
        let name = studio.editorRoute != nil ? "Edit" : studio.kind.noun
        return results.isEmpty ? name : "\(name) · \(results.count)"
    }

    @ViewBuilder
    private var pane: some View {
        Group {
            if let route = paneRoute {
                ArtifactLiveWebView(
                    route: route,
                    onClose: { showChat() },
                    onChat: { text in
                        showChat()
                        if !text.isEmpty { viewModel.sendMessage(text) }
                    }
                )
                .id(route.paneKey)
            } else {
                VStack(spacing: 14) {
                    Image(studio.kind.iconAsset)
                        .resizable()
                        .frame(width: 40, height: 40)
                    Text(studio.kind.emptyTitle)
                        .font(.system(.title3, design: .serif))
                        .foregroundColor(Color("TextPrimary"))
                        .multilineTextAlignment(.center)
                    Text(paneError.map(FriendlyErrorMessage.from) ?? studio.kind.studioHint)
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                        .multilineTextAlignment(.center)
                    Button("Describe it in the chat") { showChat() }
                        .font(.subheadline.weight(.semibold))
                        .padding(.top, 4)
                }
                .padding(32)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .background(Color("BgPrimary"))
    }

    private func showChat() {
        withAnimation(.spring(response: 0.32, dampingFraction: 0.9)) { showingPane = false }
    }

    private func refreshResults() async {
        do {
            let page = try await ArtifactsV2Client.shared.list(ArtifactV2ListQuery(scope: "mine", limit: 50))
            results = page.items.filter { $0.origin?.sessionId == studio.sessionId }
            paneError = nil
        } catch is CancellationError {
            return
        } catch {
            paneError = error.localizedDescription
        }
    }
}
