import SwiftUI

/// Sheet root for the Code tab's file browser: resolves a `FileClient`
/// against the paired instance (same resolution `CodeThreadChatView` uses
/// for the terminal, via `InstanceConnection`) and shows the project root's
/// directory listing once connected.
///
/// `.navigationDestination(for: FileNode.self)` is declared exactly once
/// here at the `NavigationStack` root — `FileBrowserDirectoryView` pushes
/// nested levels via `NavigationLink(value:)` rather than each level
/// re-declaring its own destination, which is the pattern SwiftUI expects
/// for recursive/self-similar navigation.
struct FileBrowserView: View {
    @ObservedObject var instanceStore: InstanceStore

    @State private var client: FileClient?
    @State private var isResolving = true

    var body: some View {
        NavigationStack {
            Group {
                if let client {
                    // `client` is captured non-optional here, so every push
                    // this destination handles is guaranteed to reuse the
                    // same resolved connection.
                    FileBrowserDirectoryView(client: client, path: nil, title: "Files")
                        .navigationDestination(for: FileNode.self) { node in
                            FileBrowserDirectoryView(client: client, path: node.path, title: node.name)
                        }
                } else if isResolving {
                    ProgressView("Connecting…")
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    noInstanceView
                }
            }
            .background(Color("BgPrimary"))
        }
        .task {
            await resolve()
        }
    }

    private var noInstanceView: some View {
        FriendlyStateView(
            style: .error,
            icon: "folder",
            title: "Connect a computer or Allternit Cloud",
            message: "Open Allternit Desktop on your computer, or wake your cloud computer, then retry.",
            actionTitle: "Retry",
            action: { Task { await resolve() } }
        )
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .navigationTitle("Files")
        .navigationBarTitleDisplayMode(.inline)
    }

    @MainActor
    private func resolve() async {
        isResolving = true
        await instanceStore.refreshIfNeeded()
        if let baseURL = await InstanceConnection.resolveBaseURL(from: instanceStore) {
            client = FileClient(baseURL: baseURL)
        }
        isResolving = false
    }
}
