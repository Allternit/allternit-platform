#if canImport(FileProvider)
import FileProvider
import Foundation

/// Keeps files pinned on this device ("Keep offline") downloaded.
///
/// iOS has no keep-downloaded content policy for a File Provider item
/// (`downloadEagerlyAndKeepDownloaded` is macOS only), and the system may
/// evict a downloaded file when space runs low. So the host app calls
/// `materialize` when it comes to the foreground: each pinned file is read
/// once through file coordination, which makes the system fetch it again if
/// it was evicted. Already-downloaded files cost one stat.
///
/// Runs in the host app, never in the extension (a coordinated read of the
/// extension's own domain from inside the extension can deadlock).
public enum FilesPinKeeper {
    /// Reads at most `limit` pinned files per call, one at a time.
    public static func materialize(store: FilesSharedStore, limit: Int = 200) async {
        var done = 0
        for computer in store.computers {
            let pinned = store.pins
                .filter { $0.hasPrefix(computer.id + "/") }
                .map { String($0.dropFirst(computer.id.count + 1)) }
            guard !pinned.isEmpty, let manager = await FilesDomains.manager(for: computer.id) else { continue }
            for itemId in pinned {
                guard done < limit, !Task.isCancelled else { return }
                done += 1
                guard let url = try? await manager.getUserVisibleURL(for: NSFileProviderItemIdentifier(itemId)) else { continue }
                await coordinatedRead(url)
            }
        }
    }

    private static func coordinatedRead(_ url: URL) async {
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            DispatchQueue.global(qos: .utility).async {
                let coordinator = NSFileCoordinator(filePresenter: nil)
                var error: NSError?
                coordinator.coordinate(readingItemAt: url, options: [], error: &error) { _ in }
                continuation.resume()
            }
        }
    }
}
#endif
