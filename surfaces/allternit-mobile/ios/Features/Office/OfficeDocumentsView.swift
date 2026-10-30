import SwiftUI

/// Office documents — the account's Docs, Sheets, Slides and PDFs from
/// Artifacts v2 (`GET /api/v2/artifacts?kind=…`, one request per kind,
/// merged newest first). Each opens `V2ArtifactView`; Office records
/// imported from the old sectioned store also offer the interactive editor.
struct OfficeDocumentsView: View {
    private static let kinds = ["doc", "sheet", "slides", "pdf"]

    @State private var artifacts: [ArtifactV2Summary] = []
    @State private var error: String?
    @State private var isLoading = true
    @State private var isSignPDFPresented = false
    @State private var openArtifact: ArtifactV2Summary?

    var body: some View {
        NavigationStack {
            Group {
                if isLoading {
                    ProgressView("Loading documents…")
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else if let error {
                    FriendlyStateView(
                        style: .offline,
                        icon: "wifi.slash",
                        title: "Couldn't load documents",
                        message: FriendlyErrorMessage.from(error),
                        actionTitle: "Retry",
                        action: { Task { await loadArtifacts() } }
                    )
                } else if artifacts.isEmpty {
                    FriendlyStateView(
                        style: .empty,
                        icon: "doc",
                        title: "No documents yet",
                        message: "Docs, sheets, decks and PDFs you make or are shared appear here."
                    )
                } else {
                    ScrollView {
                        LazyVStack(spacing: 8) {
                            ForEach(artifacts) { summary in
                                Button { openArtifact = summary } label: {
                                    ArtifactLibraryRow(summary: summary)
                                }
                                .buttonStyle(.plain)
                            }
                        }
                        .padding(16)
                    }
                    .refreshable { await loadArtifacts() }
                }
            }
            .navigationTitle("Office documents")
            .navigationBarTitleDisplayMode(.inline)
            .background(Color("BgPrimary").ignoresSafeArea())
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button(action: { isSignPDFPresented = true }) {
                        Label("Sign PDF", systemImage: "signature")
                    }
                }
            }
            .sheet(isPresented: $isSignPDFPresented) {
                PDFSignView(sourceURL: nil)
            }
            .fullScreenCover(item: $openArtifact) { summary in
                V2ArtifactView(artifactId: summary.id, initialTitle: summary.title) {
                    artifacts.removeAll { $0.id == summary.id }
                }
            }
            .task {
                await loadArtifacts()
            }
        }
    }

    private func loadArtifacts() async {
        do {
            let pages = try await withThrowingTaskGroup(of: [ArtifactV2Summary].self) { group in
                for kind in Self.kinds {
                    group.addTask {
                        try await ArtifactsV2Client.shared.list(ArtifactV2ListQuery(scope: "all", kind: kind, limit: 50)).items
                    }
                }
                var all: [ArtifactV2Summary] = []
                for try await items in group { all.append(contentsOf: items) }
                return all
            }
            artifacts = pages.sorted { ($0.updatedDate ?? .distantPast) > ($1.updatedDate ?? .distantPast) }
            error = nil
        } catch is CancellationError {
            return
        } catch {
            self.error = error.localizedDescription
        }
        isLoading = false
    }
}
