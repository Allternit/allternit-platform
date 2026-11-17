import Foundation
import UIKit

/// Errors thrown by BrainStore flows (provisioning/capture), surfaced to the
/// onboarding page and capture sheet as `error.localizedDescription`.
enum BrainStoreError: LocalizedError {
    case noBrain
    case emptyNote
    case missingCredentials

    var errorDescription: String? {
        switch self {
        case .noBrain: return "No brain exists yet — create one from onboarding."
        case .emptyNote: return "Write something first."
        case .missingCredentials: return "The brain's git token is missing; re-create the brain."
        }
    }
}

/// Second-brain state (Track D, phase D3): owns the local clone at
/// <Documents>/brain, provisioning (D3-R1), capture + offline push queue
/// (D3-R2), and retry-on-foreground.
///
/// Persistence:
/// - brain id + clone url — UserDefaults (non-secret support/debugging
///   metadata and future flows).
/// - git token — KEYCHAIN (KeychainHelper). The `allternit_git_` token is
///   shown once at mint time, so it MUST be persisted for capture+push to
///   work across launches; it's a raw bearer secret (the D2 backend stores
///   only its sha256 hash), so the plaintext UserDefaults plist is not
///   acceptable for it.
/// - pending-push flag — UserDefaults; the whole offline queue. A capture
///   commits FIRST (local-first: the page is durable even offline), then
///   pushes best-effort; on failure the flag stays set and the next
///   foregrounding retries. Push is idempotent (re-sends the same commits)
///   and pages are never re-created, so retries can't duplicate.
@MainActor
final class BrainStore: ObservableObject {
    static let shared = BrainStore()

    /// Local repo exists on disk (the clone target). False when the user
    /// skipped brain creation — the capture row stays hidden.
    @Published private(set) var hasBrain: Bool
    /// At least one committed page hasn't reached the remote yet.
    @Published var pendingPush: Bool {
        didSet { defaults.set(pendingPush, forKey: Keys.pendingPush) }
    }
    /// Onboarding "Create my brain" spinner state.
    @Published private(set) var isProvisioning = false
    /// Last git/API failure, for UI surfaces that want it.
    @Published private(set) var lastError: String? = nil

    private let defaults: UserDefaults
    private let git = BrainGitClient()
    private var foregroundObserver: NSObjectProtocol?
    private var isPushing = false

    private enum Keys {
        static let brainID = "allternit-brain-id"
        static let cloneURL = "allternit-brain-clone-url"
        static let pendingPush = "allternit-brain-pending-push"
        /// Keychain account for the `allternit_git_` token (KeychainHelper).
        static let gitTokenAccount = "brain-git-token"
    }

    /// The local clone: <Documents>/brain.
    var repoURL: URL {
        FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("brain")
    }

    private var credentials: (username: String, token: String)? {
        guard let token = KeychainHelper.load(Keys.gitTokenAccount), !token.isEmpty else { return nil }
        return (username: "allternit", token: token)
    }

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        let repoURL = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("brain")
        self.hasBrain = FileManager.default.fileExists(atPath: repoURL.path)
        self.pendingPush = defaults.bool(forKey: Keys.pendingPush)

        // Retry the pending push on every foregrounding (same pattern as
        // MeshClient) — no reachability helper exists in the app, and the
        // queue is just the persisted flag above.
        foregroundObserver = NotificationCenter.default.addObserver(
            forName: UIApplication.willEnterForegroundNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor in self?.retryPushIfNeeded() }
        }

        // A push left pending at the last launch gets one immediate retry.
        retryPushIfNeeded()
    }

    // No deinit observer removal: BrainStore is a process-lifetime
    // singleton, and a @MainActor deinit is nonisolated — touching the
    // non-Sendable NSObjectProtocol from it is a Swift 6 error.

    // MARK: - D3-R1: provision + clone + seed

    /// One-tap onboarding creation: provision the hosted remote (POST
    /// /api/v1/brains), mint a git token (POST /api/v1/tokens/git), clone the
    /// empty remote, write the 7 canonical files, commit, push, and persist
    /// everything capture needs later. Throws on failure after cleaning up —
    /// no half-created repo is left behind, so retry starts fresh.
    func createBrain() async throws {
        guard !isProvisioning else { return }
        isProvisioning = true
        defer { isProvisioning = false }

        do {
            let client = BrainsClient()
            let provision = try await client.provisionBrain()
            var token = try await client.mintGitToken(name: "ios")

            #if DEBUG
            // `-brain-corrupt-token` (D3 live verification): mint a real
            // token, then sabotage it so the auth-failure path can be
            // exercised end to end — clone must fail and must NOT be masked
            // by the empty-remote init fallback below.
            if launchArgumentEnabled("brain-corrupt-token") {
                token = GitToken(
                    id: token.id,
                    token: "allternit_git_00000000000000000000000000000000",
                    note: token.note)
            }
            #endif

            // libgit2 clones an empty remote fine (unborn HEAD; the first
            // commitAll creates the branch). The init + origin fallback is
            // ONLY for the empty-remote quirk — an auth failure (rejected
            // token, HTTP 401) must surface as an error, not be silently
            // masked as apparent success with a stuck pendingPush later.
            do {
                try await git.clone(
                    url: provision.cloneURL, to: repoURL,
                    username: "allternit", token: token.token)
            } catch {
                if Self.isAuthFailure(error) { throw error }
                try await git.initRepoWithOrigin(dir: repoURL, remoteURL: provision.cloneURL)
            }

            for file in BrainTemplates.canonicalFiles(now: Date(), remote: provision.cloneURL) {
                let fileURL = repoURL.appendingPathComponent(file.path)
                try FileManager.default.createDirectory(
                    at: fileURL.deletingLastPathComponent(),
                    withIntermediateDirectories: true)
                try file.content.write(to: fileURL, atomically: true, encoding: .utf8)
            }
            try await git.commitAll(
                dir: repoURL, message: "Initialize second brain (iOS onboarding)")

            persist(brainID: provision.brainID, cloneURL: provision.cloneURL, token: token.token)
            hasBrain = true
            pendingPush = true
            lastError = nil
            // Best-effort: if the push itself fails, the queue flag above
            // stays set and foregrounding retries — creation still succeeded.
            await pushPending()
        } catch {
            try? FileManager.default.removeItem(at: repoURL)
            clearPersisted()
            hasBrain = false
            pendingPush = false
            lastError = error.localizedDescription
            throw error
        }
    }

    // MARK: - D3-R2: capture + offline queue

    /// Append a captured note as an ideas/ page, commit it (this is the
    /// durable step — capture succeeds offline), then push best-effort;
    /// failure leaves `pendingPush` set for the foreground retry. One commit
    /// per capture, idempotent push — the queue is only the flag.
    func capture(text: String, type: BrainCaptureType) async throws {
        guard hasBrain else { throw BrainStoreError.noBrain }
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { throw BrainStoreError.emptyNote }

        // Title = first line, capped; the page body keeps the full note.
        let firstLine = trimmed.components(separatedBy: .newlines)
            .first?.trimmingCharacters(in: .whitespaces) ?? trimmed
        let title = String(firstLine.prefix(80)).isEmpty ? "Note" : String(firstLine.prefix(80))

        let page = BrainPage.ideaPage(title: title, body: trimmed, type: type, now: Date())
        let fileURL = repoURL.appendingPathComponent(page.filename)
        try FileManager.default.createDirectory(
            at: fileURL.deletingLastPathComponent(),
            withIntermediateDirectories: true)
        try page.content.write(to: fileURL, atomically: true, encoding: .utf8)

        try await git.commitAll(
            dir: repoURL, message: "capture: \(type.rawValue) \(BrainPage.slugify(title))")

        pendingPush = true
        await pushPending()
    }

    /// Foreground hook (wired in init via willEnterForegroundNotification):
    /// recompute the queue from the persisted flag and retry the push.
    func retryPushIfNeeded() {
        guard hasBrain, pendingPush else { return }
        Task { await pushPending() }
    }

    // MARK: - Person change (phone calm I14b)

    /// The brain is one person's: on a person change (`AccountSwitchCleanup`)
    /// the local clone, its ids and the git token leave this phone. Unsent
    /// captures get one push first; if that fails the clone is moved aside to
    /// `Application Support/brain-unsent/` (never shown, never pushed) rather
    /// than deleted, so no note is destroyed. Nothing on the server changes.
    func forgetLocal() async {
        if hasBrain, pendingPush { await pushPending() }
        let fm = FileManager.default
        if fm.fileExists(atPath: repoURL.path) {
            if pendingPush, let aside = Self.unsentDirectory() {
                let stamp = Int(Date().timeIntervalSince1970)
                try? fm.moveItem(at: repoURL, to: aside.appendingPathComponent("brain-\(stamp)"))
            }
            try? fm.removeItem(at: repoURL)
        }
        clearPersisted()
        defaults.removeObject(forKey: Keys.pendingPush)
        defaults.removeObject(forKey: "allternit-brain-last-error")
        hasBrain = false
        pendingPush = false
        lastError = nil
    }

    private static func unsentDirectory() -> URL? {
        guard let support = try? FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask,
                                                         appropriateFor: nil, create: true) else { return nil }
        let dir = support.appendingPathComponent("brain-unsent", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    // MARK: - Internals

    /// Gate for the clone→init fallback: libgit2 surfaces HTTP auth
    /// rejections as "unexpected HTTP status code: 401" / credential errors —
    /// never the empty-remote quirk the fallback exists for, so those must
    /// propagate instead of silently downgrading to a local-only repo.
    private static func isAuthFailure(_ error: Error) -> Bool {
        let message = error.localizedDescription.lowercased()
        return message.contains("401")
            || message.contains("auth")
            || message.contains("credential")
    }

    private func pushPending() async {
        guard !isPushing else { return }
        guard let credentials else {
            lastError = BrainStoreError.missingCredentials.localizedDescription
            recordLastErrorForDiagnostics()
            return
        }
        isPushing = true
        defer { isPushing = false }
        do {
            try await git.push(
                dir: repoURL, username: credentials.username, token: credentials.token)
            pendingPush = false
            lastError = nil
            recordLastErrorForDiagnostics()
        } catch {
            lastError = error.localizedDescription
            recordLastErrorForDiagnostics()
        }
    }

    /// Persist lastError so support tooling (and simctl-based verification)
    /// can read push failures from the app container's plist — lastError
    /// itself is in-memory only.
    private func recordLastErrorForDiagnostics() {
        defaults.set(lastError, forKey: "allternit-brain-last-error")
    }

    private func persist(brainID: String, cloneURL: String, token: String) {
        defaults.set(brainID, forKey: Keys.brainID)
        defaults.set(cloneURL, forKey: Keys.cloneURL)
        KeychainHelper.save(token, for: Keys.gitTokenAccount)
    }

    private func clearPersisted() {
        defaults.removeObject(forKey: Keys.brainID)
        defaults.removeObject(forKey: Keys.cloneURL)
        KeychainHelper.delete(Keys.gitTokenAccount)
    }
}
