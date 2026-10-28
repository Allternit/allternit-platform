import SwiftUI

/// Sheet listing every file the agent changed across this session
/// (`GET v1/session/:id/diff`), each row tapping through to
/// `FileDiffDetailView`. Resolves a `SessionDiffClient` against the paired
/// instance the same way `FileBrowserView` resolves `FileClient`.
struct SessionDiffListView: View {
    let sessionId: String
    @ObservedObject var instanceStore: InstanceStore

    @State private var client: SessionDiffClient?
    @State private var diffs: [FileDiff] = []
    @State private var isResolving = true
    @State private var isLoading = false
    @State private var loadError: String?

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("Changes")
                .navigationBarTitleDisplayMode(.inline)
                .background(Color("BgPrimary"))
        }
        .task {
            await resolveAndLoad()
        }
    }

    @ViewBuilder
    private var content: some View {
        if isResolving {
            ProgressView("Connecting…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if client == nil {
            noInstanceView
        } else if isLoading && diffs.isEmpty && loadError == nil {
            ProgressView()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let loadError, diffs.isEmpty {
            FriendlyStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "Couldn't load changes",
                message: FriendlyErrorMessage.from(loadError),
                actionTitle: "Retry",
                action: { Task { await load() } }
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if diffs.isEmpty {
            // Covers both "nothing changed yet" and "not summarized yet" —
            // pull-to-refresh is the mitigation for the latter.
            ScrollView {
                FriendlyStateView(
                    style: .empty,
                    icon: "plus.forwardslash.minus",
                    title: "No changes yet",
                    message: "When this session edits files, they'll appear here."
                )
                .frame(maxWidth: .infinity)
            }
            .refreshable { await load() }
        } else {
            List(diffs) { diff in
                NavigationLink {
                    FileDiffDetailView(diff: diff)
                } label: {
                    row(for: diff)
                }
            }
            .listStyle(.plain)
            .refreshable { await load() }
        }
    }

    private func row(for diff: FileDiff) -> some View {
        HStack(spacing: 10) {
            Image(systemName: "doc.text")
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 20)
            Text(diff.file)
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer()
            HStack(spacing: 8) {
                if diff.additions > 0 {
                    Text("+\(diff.additions)")
                        .foregroundColor(Theme.statusSuccess)
                }
                if diff.deletions > 0 {
                    Text("-\(diff.deletions)")
                        .foregroundColor(Theme.statusError)
                }
            }
            .font(.system(.caption, design: .monospaced).weight(.semibold))
        }
        .padding(.vertical, 4)
    }

    private var noInstanceView: some View {
        FriendlyStateView(
            style: .error,
            icon: "plus.forwardslash.minus",
            title: "Connect a computer or Allternit Cloud",
            message: "Open Allternit Desktop on your computer, or wake your cloud computer, then retry.",
            actionTitle: "Retry",
            action: { Task { await resolveAndLoad() } }
        )
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    @MainActor
    private func resolveAndLoad() async {
        isResolving = true
        await instanceStore.refreshIfNeeded()
        if let baseURL = await InstanceConnection.resolveBaseURL(from: instanceStore) {
            client = SessionDiffClient(baseURL: baseURL)
        }
        isResolving = false
        await load()
    }

    @MainActor
    private func load() async {
        guard let client else { return }
        isLoading = true
        loadError = nil
        do {
            diffs = try await client.diff(sessionID: sessionId)
        } catch is CancellationError {
            // View disappeared mid-flight.
        } catch {
            loadError = error.localizedDescription
        }
        isLoading = false
    }
}
