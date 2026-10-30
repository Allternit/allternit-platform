import SwiftUI

/// A studio session on the phone: the chat, and — one tap away, as the
/// PWA's phone layout does with its full-screen pane — the studio pane
/// showing what the session made. The pane lists the v2 artifacts whose
/// `origin.session_id` is this session (what `artifact_create` and saved
/// chat frames record) and refreshes while it is open.
struct ArtifactStudioView: View {
    let studio: ArtifactStudioSession

    @Environment(\.dismiss) private var dismiss
    @StateObject private var viewModel = ChatViewModel()
    @State private var showingPane = false
    @State private var results: [ArtifactV2Summary] = []
    @State private var paneError: String?
    @State private var openArtifact: ArtifactV2Summary?

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
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .sheet(item: $openArtifact) { summary in
            V2ArtifactView(artifactId: summary.id, initialTitle: summary.title) {
                results.removeAll { $0.id == summary.id }
            }
        }
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
                    .font(.system(.headline, design: .serif))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text(studio.kind.label)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            Spacer()
            Picker("View", selection: Binding(
                get: { showingPane },
                set: { value in withAnimation(.spring(response: 0.32, dampingFraction: 0.9)) { showingPane = value } }
            )) {
                Text("Chat").tag(false)
                Text(results.isEmpty ? studio.kind.noun : "\(studio.kind.noun) · \(results.count)").tag(true)
            }
            .pickerStyle(.segmented)
            .frame(maxWidth: 200)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(Color("BgPrimary"))
    }

    @ViewBuilder
    private var pane: some View {
        Group {
            if results.isEmpty {
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
                    Button("Describe it in the chat") {
                        withAnimation(.spring(response: 0.32, dampingFraction: 0.9)) { showingPane = false }
                    }
                    .font(.subheadline.weight(.semibold))
                    .padding(.top, 4)
                }
                .padding(32)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(spacing: 10) {
                        ForEach(results) { summary in
                            Button { openArtifact = summary } label: {
                                ArtifactLibraryRow(summary: summary)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding(16)
                }
                .refreshable { await refreshResults() }
            }
        }
        .background(Color("BgPrimary"))
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
