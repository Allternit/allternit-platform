import FileProvider
import Foundation
import AllternitFilesKit

/// Thumbnails in Files for files that are not on this iPhone yet (plan
/// step 20). Compiled into the AllternitFileProvider extension: for a
/// replicated File Provider the system asks the extension itself
/// (`NSFileProviderThumbnailing`), never a separate Quick Look thumbnail
/// extension, about items it holds no bytes for. Files already downloaded
/// get Quick Look's own thumbnails.
///
/// The computer makes a 256 px PNG while it is awake (step 16,
/// `GET /items/:id/preview?kind=thumbnail`). Fetching one never wakes it:
/// an asleep computer, no preview yet or an unknown item all mean "no
/// thumbnail", and Files keeps the type icon. The system caches each
/// thumbnail until the item's content version changes.
///
/// Memory: at most `concurrent` previews in flight, each capped at 1 MiB,
/// handed to the system and dropped at once, so the extension stays far
/// under its ~20 MB limit (spike 5) while Files scrolls a big folder.
extension FileProviderExtension: NSFileProviderThumbnailing {
    private static let concurrent = 3
    private static let maxThumbnailBytes = 1_048_576

    func fetchThumbnails(for itemIdentifiers: [NSFileProviderItemIdentifier], requestedSize size: CGSize,
                         perThumbnailCompletionHandler: @escaping (NSFileProviderItemIdentifier, Data?, Error?) -> Void,
                         completionHandler: @escaping (Error?) -> Void) -> Progress {
        let context = context
        let progress = Progress(totalUnitCount: Int64(itemIdentifiers.count))
        let task = Task {
            // Which items are worth a request (folders, roots and items the
            // computer said have no thumbnail are answered "none" at once).
            var work: [(NSFileProviderItemIdentifier, Bool)] = []
            for identifier in itemIdentifiers {
                var mayHave = false
                if identifier != .rootContainer && identifier != .trashContainer && identifier != .workingSet {
                    mayHave = await context.catalog.mayHaveThumbnail(identifier.rawValue)
                }
                work.append((identifier, mayHave))
            }
            await withTaskGroup(of: Void.self) { group in
                var index = 0
                var running = 0
                while index < work.count || running > 0 {
                    while running < Self.concurrent, index < work.count, !Task.isCancelled {
                        let (identifier, mayHave) = work[index]
                        index += 1
                        running += 1
                        group.addTask {
                            var data: Data?
                            if mayHave, !Task.isCancelled {
                                data = try? await context.client.thumbnail(itemId: identifier.rawValue,
                                                                           maxBytes: Self.maxThumbnailBytes)
                            }
                            perThumbnailCompletionHandler(identifier, data, nil)
                        }
                    }
                    guard running > 0 else { break }
                    _ = await group.next()
                    running -= 1
                    progress.completedUnitCount += 1
                }
            }
            if Task.isCancelled {
                completionHandler(NSError(domain: NSCocoaErrorDomain, code: NSUserCancelledError))
            } else {
                completionHandler(nil)
            }
        }
        progress.cancellationHandler = { task.cancel() }
        return progress
    }
}
