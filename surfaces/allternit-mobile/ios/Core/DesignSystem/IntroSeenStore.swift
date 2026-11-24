import Foundation

/// The first-visit intros (mobile refresh contract, D3/D4/D5). One flag per
/// intro in the account (`GET/PUT /api/v1/me/flags`, ISO time), with a local
/// copy so an intro never repeats on this device, even offline or while the
/// route still rejects the new keys. A write that fails is kept and retried
/// on a later check. `botsIntroSeenAt` is shared with "Meet your bots"
/// (`BotsIntroStore`) and keeps that store's local key.
enum IntroKey: String, CaseIterable, Sendable {
    case home = "introHomeSeenAt"
    case code = "introCodeSeenAt"
    case aci = "introAciSeenAt"
    case projects = "introProjectsSeenAt"
    case artifacts = "introArtifactsSeenAt"
    case automation = "introAutomationSeenAt"
    case customize = "introCustomizeSeenAt"
    case freeweights = "introFreeweightsSeenAt"
    case bots = "botsIntroSeenAt"

    /// Verbatim copy from the contract.
    var copy: (eyebrow: String, title: String, line: String) {
        switch self {
        case .projects:
            return ("PROJECTS", "Keep each job in one place.",
                    "A project keeps its chats, tasks and board together. See what's working and what is waiting on you.")
        case .artifacts:
            return ("ARTIFACTS", "Make real files, not just replies.",
                    "Docs, sheets, slides and pages you can edit, share and export.")
        case .automation:
            return ("AUTOMATION TASKS", "Work that runs without you.",
                    "Set a goal, a routine on a schedule, or a loop. It runs on your computer and reports back.")
        case .customize:
            return ("CUSTOMIZE", "Teach the agent new tricks.",
                    "Add skills, connect your tools and install plugins. Every chat can use them.")
        case .freeweights:
            return ("FREEWEIGHTS", "Open models you can keep.",
                    "Download one to this iPhone, or pull it to your computer. Every license is checked.")
        case .home:
            return ("HOME", "Start anything here.",
                    "Chat, plan or hand off work. Change the brain whenever you want.")
        case .bots:
            return ("BOTS", "Your team keeps working when you leave.",
                    "Tap a bot to talk to it, or describe a job and the team splits it up.")
        case .code:
            return ("GIZZI CODE", "Describe it. Gizzi builds it.",
                    "It runs on your computer or Allternit Cloud, and you steer it from your phone.")
        case .aci:
            return ("ACI", "The agent uses the web for you.",
                    "Give it a site and a goal. Watch it work, and take over any time.")
        }
    }

    /// Bots keeps the key `BotsIntroStore` already writes (D4).
    fileprivate var localKey: String {
        self == .bots ? "allternit-bots-intro-seen" : "allternit-intro-seen-\(rawValue)"
    }
}

@MainActor
final class IntroSeenStore: ObservableObject {
    static let shared = IntroSeenStore()

    private static let pendingKey = "allternit-intro-seen-pending"
    private let defaults = UserDefaults.standard
    private var checked = Set<IntroKey>()

    private init() {}

    /// Seen on this device (cache only).
    func seenLocally(_ key: IntroKey) -> Bool {
        defaults.string(forKey: key.localKey) != nil
    }

    /// True when the intro should show now. Local copy first; otherwise asks
    /// the account (3 s cap, like the web) and fails closed to "show" only
    /// when the account answered "not seen" or could not be reached in time.
    func shouldShow(_ key: IntroKey) async -> Bool {
        if seenLocally(key) {
            await flushPending()
            return false
        }
        guard AuthManager.shared.isSignedIn else { return false }
        let k = key.rawValue
        let seen = await withTaskGroup(of: Bool?.self) { group -> Bool? in
            group.addTask {
                do { return try await AccountFlagsClient.shared.string(k) != nil } catch { return nil }
            }
            group.addTask {
                try? await Task.sleep(nanoseconds: 3_000_000_000)
                return nil
            }
            let first = await group.next() ?? nil
            group.cancelAll()
            return first
        }
        if seen == true {
            defaults.set(ISO8601DateFormatter().string(from: Date()), forKey: key.localKey)
            return false
        }
        await flushPending()
        return true
    }

    /// Seen here at once; on the account when reachable, retried later if not.
    func markSeen(_ key: IntroKey) {
        let at = ISO8601DateFormatter().string(from: Date())
        defaults.set(at, forKey: key.localKey)
        addPending(key, at: at)
        Task { await flushPending() }
    }

    // MARK: Retry queue

    private var pending: [String: String] {
        get { defaults.dictionary(forKey: Self.pendingKey) as? [String: String] ?? [:] }
        set { defaults.set(newValue, forKey: Self.pendingKey) }
    }

    private func addPending(_ key: IntroKey, at: String) {
        var map = pending
        map[key.rawValue] = at
        pending = map
    }

    /// Sends every write that has not reached the account yet. A failure
    /// (offline, or a 400 until the route accepts the new keys) keeps the
    /// entry for the next check.
    func flushPending() async {
        guard AuthManager.shared.isSignedIn else { return }
        for (key, at) in pending {
            do {
                try await AccountFlagsClient.shared.set(key, to: at)
                var map = pending
                map.removeValue(forKey: key)
                pending = map
            } catch {
                // Keep it; try again on a later launch.
            }
        }
    }
}
