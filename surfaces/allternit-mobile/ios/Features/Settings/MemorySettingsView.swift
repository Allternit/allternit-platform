import SwiftUI

/// Settings → Memory (pushed from SettingsView). Reads the live
/// `/api/v1/memory/*` routes (stats + documents; `/query` exists but the UI
/// searches the already-fetched document list client-side). A failed
/// request shows a plain error state with retry.
struct MemorySettingsView: View {
    @ObservedObject private var settings = SettingsStore.shared

    @State private var documents: [MemoryDocument] = []
    @State private var stats: MemoryStats? = nil
    @State private var isLoading = false
    /// Non-nil when the memory request genuinely failed — the view renders
    /// an error state with retry.
    @State private var loadError: String? = nil
    @State private var searchText = ""
    @State private var selectedDocument: MemoryDocument? = nil

    private let client = MemoryClient()

    private var filtered: [MemoryDocument] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return documents }
        return documents.filter {
            $0.title.localizedCaseInsensitiveContains(query)
                || $0.sourceType.localizedCaseInsensitiveContains(query)
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            if loadError == nil {
                searchBar
                Divider().background(Color("BorderSubtle"))
            }
            ScrollView {
                // The account memory map (every bot plus your own memory).
                ImpactMapView(botId: nil)
                    .padding(.horizontal, 20)
                    .padding(.top, 16)
                    .padding(.bottom, 8)
                content
            }
            .refreshable { await load() }
        }
        .background(Color("BgPrimary"))
        .navigationTitle("Memory")
        .navigationBarTitleDisplayMode(.inline)
        .task { await load() }
        .sheet(item: $selectedDocument) { document in
            MemoryDocumentDetailView(document: document)
        }
    }

    // MARK: - Search

    private var searchBar: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
            TextField("Search memories", text: $searchText)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            if !searchText.isEmpty {
                Button(action: { searchText = "" }) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(Color("BgSecondary"))
        .cornerRadius(10)
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        if isLoading && documents.isEmpty && loadError == nil {
            HStack {
                Spacer()
                ProgressView().padding(.top, 40)
                Spacer()
            }
        } else if let loadError, documents.isEmpty {
            FriendlyStateView(
                style: .offline,
                icon: "brain.head.profile",
                title: "Couldn't load memory",
                message: FriendlyErrorMessage.from(loadError),
                actionTitle: "Retry",
                action: { Task { await load() } }
            )
            .frame(maxWidth: .infinity)
        } else {
            VStack(alignment: .leading, spacing: 0) {
                if let loadError {
                    FriendlyInlineStateView(
                        style: .error,
                        icon: "exclamationmark.triangle",
                        title: "Refresh failed",
                        message: FriendlyErrorMessage.from(loadError),
                        actionTitle: "Retry",
                        action: { Task { await load() } }
                    )
                    .padding(.horizontal, 20)
                    .padding(.top, 12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }

                statsHeader

                // Persisted locally; the consolidation pipeline consumes this
                // flag in a later phase (no backend preference endpoint yet).
                Toggle(isOn: $settings.generateMemoryFromHistory) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Generate memory from chat history")
                            .font(.subheadline)
                            .foregroundColor(Color("TextPrimary"))
                        Text("Let Allternit learn from your chats to personalize replies.")
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
                .tint(Color("AccentPrimary"))
                .padding(.horizontal, 20)
                .padding(.vertical, 12)

                Divider().background(Color("BorderSubtle")).padding(.leading, 20)

                if filtered.isEmpty {
                    FriendlyStateView(
                        style: .empty,
                        icon: searchText.isEmpty ? "brain" : "magnifyingglass",
                        title: searchText.isEmpty ? "No memories yet" : "No matches",
                        message: searchText.isEmpty
                            ? "Memories Allternit learns from your chats will appear here."
                            : "No memories match your search."
                    )
                    .frame(maxWidth: .infinity)
                } else {
                    LazyVStack(spacing: 0) {
                        ForEach(filtered) { document in
                            documentRow(document)
                            Divider().background(Color("BorderSubtle")).padding(.leading, 68)
                        }
                    }
                }
            }
        }
    }

    private var statsHeader: some View {
        HStack(spacing: 16) {
            statPill(title: "Memories", value: stats?.memoryTotal)
            statPill(title: "Documents", value: stats == nil ? nil : documents.count)
            Spacer()
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    private func statPill(title: String, value: Int?) -> some View {
        VStack(spacing: 2) {
            Text(value.map(String.init) ?? "—")
                .font(.headline)
                .foregroundColor(Color("TextPrimary"))
            Text(title)
                .font(.caption2)
                .foregroundColor(Color("TextSecondary"))
        }
        .frame(width: 88)
        .padding(.vertical, 10)
        .background(Color("BgSecondary"))
        .cornerRadius(10)
    }

    private func documentRow(_ document: MemoryDocument) -> some View {
        Button(action: {
            let generator = UIImpactFeedbackGenerator(style: .light)
            generator.impactOccurred()
            selectedDocument = document
        }) {
            HStack(spacing: 12) {
                ZStack {
                    Circle().fill(Color("AccentPrimary").opacity(0.12))
                    Image(systemName: "doc.text")
                        .font(.subheadline)
                        .foregroundColor(Color("AccentPrimary"))
                }
                .frame(width: 40, height: 40)

                VStack(alignment: .leading, spacing: 2) {
                    Text(document.title)
                        .font(.subheadline)
                        .fontWeight(.semibold)
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text("\(document.sourceType) · \(document.chunkCount) chunk\(document.chunkCount == 1 ? "" : "s")")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }

                Spacer(minLength: 8)

                Image(systemName: "chevron.right")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundColor(Color("TextSecondary"))
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - Loading

    @MainActor
    private func load() async {
        if documents.isEmpty { isLoading = true }
        loadError = nil
        do {
            async let statsResult = client.stats()
            async let documentsResult = client.listDocuments()
            stats = try await statsResult
            documents = try await documentsResult
        } catch is CancellationError {
            // View disappeared mid-flight — keep current state.
        } catch {
            loadError = error.localizedDescription
        }
        isLoading = false
    }
}

/// Read-only detail sheet for a memory document. The document list API
/// returns metadata only (no content field), so this shows every stored
/// field verbatim.
private struct MemoryDocumentDetailView: View {
    let document: MemoryDocument
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    detailRow("Title", value: document.title)
                    detailRow("Source type", value: document.sourceType)
                    if let sourceURL = document.sourceUrl, !sourceURL.isEmpty {
                        detailRow("Source URL", value: sourceURL)
                    }
                    detailRow("Chunks", value: String(document.chunkCount))
                    detailRow("Indexed", value: document.isIndexed ? "Yes" : "No")
                    if let agentId = document.agentId, !agentId.isEmpty {
                        detailRow("Agent", value: agentId)
                    }
                    detailRow("Created", value: document.createdAt)
                    detailRow("Updated", value: document.updatedAt)
                }
                .padding(20)
            }
            .background(Color("BgPrimary"))
            .navigationTitle("Memory")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Close") { dismiss() }
                }
            }
        }
    }

    private func detailRow(_ label: String, value: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
            Text(value)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .textSelection(.enabled)
        }
    }
}
