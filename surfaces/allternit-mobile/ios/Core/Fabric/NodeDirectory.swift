import Foundation
import SwiftUI

/// Main-actor directory of Fabric peers reachable through the currently
/// resolved instance connection. Wires `InstanceConnection.resolve()` into
/// the capability-native peer discovery endpoints (`/v1/fabric/peers`).
@MainActor
final class NodeDirectory: ObservableObject {
    static let shared = NodeDirectory()

    /// Resolved base URL for the current connection target. nil when no
    /// instance or static fallback is available.
    @Published private(set) var baseURL: URL?

    /// Last known peer list, newest-first by discovery order.
    @Published private(set) var peers: [NodeIdentity] = []

    /// Human-readable name of the node the directory is attached to.
    @Published private(set) var attachedName: String?

    /// Last refresh error, for diagnostics.
    @Published private(set) var lastError: String?

    private var client: SessionWorkerClient? {
        guard let baseURL else { return nil }
        return SessionWorkerClient(
            baseURL: baseURL,
            tokenProvider: { try await AuthManager.shared.getToken() }
        )
    }

    /// Refreshes the directory: resolves the connection target, then fetches
    /// `/v1/fabric/peers` from it.
    func refresh() async {
        guard let resolved = await InstanceConnection.resolve() else {
            baseURL = nil
            attachedName = nil
            peers = []
            lastError = "No reachable instance"
            return
        }
        baseURL = resolved.baseURL
        attachedName = resolved.instanceName
        do {
            peers = try await client?.fetchPeers() ?? []
            lastError = nil
        } catch is CancellationError {
            // View/task disappeared mid-flight — keep stale state.
        } catch {
            lastError = error.localizedDescription
        }
    }

    /// Fetches the local peer identity from the resolved connection target.
    func localPeer() async -> NodeIdentity? {
        guard await refreshNeeded() else { return nil }
        return (try? await client?.fetchLocalPeer()) ?? nil
    }

    /// Peer matching `nodeId`, nil if not currently known.
    func peer(for nodeId: String) -> NodeIdentity? {
        peers.first { $0.nodeId == nodeId }
    }

    /// Peers advertising at least one capability matching the given filter.
    func peers(capability name: String) -> [NodeIdentity] {
        peers.filter { node in
            node.capabilities.contains { $0.name == name || $0.id == name }
        }
    }

    /// Peers advertising a capability for the given resource.
    func peers(resource: String) -> [NodeIdentity] {
        peers.filter { node in
            node.capabilities.contains { $0.resource == resource }
        }
    }

    /// True when the directory has no usable base URL or peer list yet.
    var isEmpty: Bool { baseURL == nil || peers.isEmpty }

    private func refreshNeeded() async -> Bool {
        if baseURL == nil {
            await refresh()
        }
        return baseURL != nil
    }
}

// MARK: - SwiftUI task helper

extension View {
    /// Refreshes a `NodeDirectory` when the view appears and on every
    /// subsequent appearance, cancelling automatically when the view leaves
    /// the hierarchy.
    func refreshNodeDirectory(_ directory: NodeDirectory) -> some View {
        task {
            await directory.refresh()
        }
    }
}
