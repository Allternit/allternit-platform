#if canImport(FileProvider)
import FileProvider
import Foundation

/// One File Provider domain per computer: each of the user's computers is
/// its own location in Files (iOS) and Finder (macOS, plan step 23). The
/// host app calls `sync` with the computers it lists; the extension reads
/// the computer id back from its domain.
///
/// Only domains whose identifier starts with `prefix` are touched, so a
/// host that registers other domains keeps them.
public enum FilesDomains {
    public static let prefix = "computer."

    public static func identifier(for computerId: ComputerId) -> NSFileProviderDomainIdentifier {
        NSFileProviderDomainIdentifier(prefix + computerId)
    }

    /// The computer behind a domain, nil for a domain Files did not add.
    public static func computerId(for identifier: NSFileProviderDomainIdentifier) -> ComputerId? {
        let raw = identifier.rawValue
        guard raw.hasPrefix(prefix), raw.count > prefix.count else { return nil }
        return String(raw.dropFirst(prefix.count))
    }

    /// `computerId` with only `[A-Za-z0-9_-]`, for session ids and file names.
    public static func safe(_ computerId: ComputerId) -> String {
        String(computerId.map { $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" ? $0 : "_" })
    }

    /// Makes the registered domains match `computers`. New computers get a
    /// domain, renamed ones get the new display name (`add` updates an
    /// existing domain), and domains of computers no longer listed are
    /// removed together with their local copies. Every kept domain is
    /// signalled afterwards: on macOS a domain the user enabled after it was
    /// added stays empty until it is re-added or signalled (spike 6).
    public static func sync(_ computers: [FilesSharedComputer]) async throws {
        let wanted = Dictionary(computers.map { (identifier(for: $0.id), $0) }, uniquingKeysWith: { first, _ in first })
        let existing = try await NSFileProviderManager.domains()
        for domain in existing where computerId(for: domain.identifier) != nil && wanted[domain.identifier] == nil {
            try? await NSFileProviderManager.remove(domain)
        }
        for (id, computer) in wanted {
            let domain = NSFileProviderDomain(identifier: id, displayName: computer.name)
            try await NSFileProviderManager.add(domain)
        }
        for computer in computers {
            await signalChanges(computer.id)
        }
    }

    /// Sign-out or flag off: removes every Files domain and, with it, every
    /// file the system kept for them on this device.
    public static func removeAll() async {
        guard let existing = try? await NSFileProviderManager.domains() else { return }
        for domain in existing where computerId(for: domain.identifier) != nil {
            try? await NSFileProviderManager.remove(domain)
        }
    }

    /// Asks the system to pull changes for `computerId` (it calls the
    /// working-set enumerator, which asks the computer what changed), and to
    /// retry now any upload it is holding back.
    ///
    /// Queued offline writes (plan step 15): an edit saved while the computer
    /// is asleep (no wake consent) or unreachable fails with
    /// `.serverUnreachable` (`FileProviderErrors.map`), which the system keeps
    /// as a pending upload and retries with backoff; the edit stays on this
    /// device meanwhile. Saying the error is resolved makes it retry at once.
    /// If the computer still sleeps it fails the same way and stays pending.
    /// The retry carries the edit's base version, so a file that changed on
    /// the computer meanwhile is kept twice (keep-both conflict copy).
    public static func signalChanges(_ computerId: ComputerId) async {
        guard let manager = await manager(for: computerId) else { return }
        try? await manager.signalErrorResolved(NSFileProviderError(.serverUnreachable))
        try? await manager.signalErrorResolved(NSFileProviderError(.notAuthenticated))
        try? await manager.signalEnumerator(for: .workingSet)
    }

    /// The manager of the registered domain for `computerId`, if there is one.
    public static func manager(for computerId: ComputerId) async -> NSFileProviderManager? {
        let id = identifier(for: computerId)
        guard let domains = try? await NSFileProviderManager.domains(),
              let domain = domains.first(where: { $0.identifier == id }) else { return nil }
        return NSFileProviderManager(for: domain)
    }
}
#endif
