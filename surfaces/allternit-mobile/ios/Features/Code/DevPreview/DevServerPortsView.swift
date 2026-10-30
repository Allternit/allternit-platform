import SwiftUI

/// Sheet listing the active terminal pty's currently-detected dev-server
/// ports. Resolves a `DevPreviewClient` against the paired instance the
/// same way `FileBrowserView`/`SessionDiffListView` resolve theirs, then
/// polls (~4s, matching `CodeThreadChatView.pollPendingPermissions()`'s
/// cadence) only while this sheet is on screen. Tapping a port mints a
/// preview token and pushes `DevServerPreviewView`.
struct DevServerPortsView: View {
    let ptyID: String
    @ObservedObject var instanceStore: InstanceStore

    @State private var client: DevPreviewClient?
    @State private var ports: [DevServerPort] = []
    @State private var isResolving = true
    @State private var loadError: String? = nil
    @State private var mintingPort: Int? = nil
    @State private var mintError: String? = nil
    @State private var previewTarget: PreviewTarget? = nil

    struct PreviewTarget: Identifiable {
        let port: Int
        let token: String
        var id: Int { port }
    }

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("Dev Servers")
                .navigationBarTitleDisplayMode(.inline)
                .background(Color("BgPrimary"))
        }
        .task {
            await resolve()
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(4))
                if Task.isCancelled { break }
                await refresh()
            }
        }
        .fullScreenCover(item: $previewTarget) { target in
            if let client {
                DevServerPreviewView(client: client, ptyID: ptyID, port: target.port, token: target.token)
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        if isResolving {
            ProgressView("Connecting…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if client == nil {
            noInstanceView
        } else if let loadError, ports.isEmpty {
            FriendlyStateView(
                style: .offline,
                icon: "wifi.slash",
                title: "Couldn't load ports",
                message: FriendlyErrorMessage.from(loadError),
                actionTitle: "Retry",
                action: { Task { await refresh() } }
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if ports.isEmpty {
            FriendlyStateView(
                style: .empty,
                icon: "globe",
                title: "No dev servers detected",
                message: "Start something like `npm run dev` in the terminal — it'll show up here."
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            List(ports) { port in
                Button {
                    Task { await openPreview(port: port.port) }
                } label: {
                    row(for: port)
                }
                .disabled(mintingPort != nil)
            }
            .listStyle(.plain)
            .overlay(alignment: .bottom) {
                if let mintError {
                    Text(mintError)
                        .font(.caption)
                        .foregroundColor(Theme.statusError)
                        .padding(8)
                        .background(Color("BgPanel"))
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                        .padding(.bottom, 12)
                }
            }
        }
    }

    private func row(for port: DevServerPort) -> some View {
        HStack(spacing: 10) {
            Image(systemName: "globe")
                .font(.subheadline)
                .foregroundColor(Color("AccentPrimary"))
                .frame(width: 20)
            VStack(alignment: .leading, spacing: 2) {
                Text(port.command ?? "Unknown process")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                Text(":\(port.port)")
                    .font(.system(.caption, design: .monospaced))
                    .foregroundColor(Color("TextSecondary"))
            }
            Spacer()
            if mintingPort == port.port {
                ProgressView()
            } else {
                Image(systemName: "chevron.right")
                    .font(.caption2)
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .padding(.vertical, 4)
    }

    private var noInstanceView: some View {
        FriendlyStateView(
            style: .error,
            icon: "globe",
            title: "Connect a computer or Allternit Cloud",
            message: "Open Allternit Desktop on your computer, or wake your cloud computer, then retry.",
            actionTitle: "Retry",
            action: { Task { await resolve() } }
        )
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    @MainActor
    private func resolve() async {
        isResolving = true
        await instanceStore.refreshIfNeeded()
        if let baseURL = await InstanceConnection.resolveBaseURL(from: instanceStore) {
            client = DevPreviewClient(baseURL: baseURL)
        }
        isResolving = false
        await refresh()
    }

    @MainActor
    private func refresh() async {
        guard let client else { return }
        do {
            ports = try await client.ports(ptyID: ptyID)
            loadError = nil
        } catch is CancellationError {
            // View disappeared mid-flight.
        } catch {
            loadError = error.localizedDescription
        }
    }

    @MainActor
    private func openPreview(port: Int) async {
        guard let client else { return }
        // The preview is a live WKWebView page; the cloud relay carries
        // REST calls, not a browser session, so it needs a direct or mesh
        // connection to the computer.
        if RuntimeRelayClient.isRelayURL(client.baseURL) {
            mintError = "Live preview needs a direct connection to your computer. Start the mesh node in Settings → Infrastructure, or open the page on the computer."
            return
        }
        mintingPort = port
        mintError = nil
        do {
            let minted = try await client.mintPreviewToken(ptyID: ptyID, port: port)
            previewTarget = PreviewTarget(port: port, token: minted.token)
        } catch {
            mintError = error.localizedDescription
        }
        mintingPort = nil
    }
}
