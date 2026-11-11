import SwiftUI
import StoreKit

/// Decides when to ask for an App Store rating. Pure and injectable so the
/// rules are unit-tested: 5+ successful answers on 2+ distinct days, once per
/// app version, 120+ days between asks, never while streaming, never in
/// DEBUG / sample-workspace / XCTest runs.
struct ReviewPromptPolicy {
    static let minAnswers = 5
    static let minDistinctDays = 2
    static let minDaysBetweenAsks = 120

    private enum Key {
        static let answers = "review-prompt.answerCount"
        static let days = "review-prompt.answerDays"
        static let lastAskedAt = "review-prompt.lastAskedAt"
        static let lastAskedVersion = "review-prompt.lastAskedVersion"
    }

    let defaults: UserDefaults
    let now: () -> Date
    let appVersion: String
    /// True when this process must never ask (DEBUG, screenshots, tests).
    let isBlocked: Bool
    private let calendar = Calendar.current

    init(
        defaults: UserDefaults = .standard,
        now: @escaping () -> Date = Date.init,
        appVersion: String = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0",
        isBlocked: Bool = ReviewPromptPolicy.processIsBlocked
    ) {
        self.defaults = defaults
        self.now = now
        self.appVersion = appVersion
        self.isBlocked = isBlocked
    }

    static var processIsBlocked: Bool {
        #if DEBUG
        return true
        #else
        return launchArgumentEnabled(LaunchArgument.sampleWorkspace)
            || ProcessInfo.processInfo.environment["XCTestConfigurationFilePath"] != nil
        #endif
    }

    private var dayStamp: String {
        let c = calendar.dateComponents([.year, .month, .day], from: now())
        return String(format: "%04d-%02d-%02d", c.year ?? 0, c.month ?? 0, c.day ?? 0)
    }

    func recordSuccessfulAnswer() {
        defaults.set(defaults.integer(forKey: Key.answers) + 1, forKey: Key.answers)
        var days = defaults.stringArray(forKey: Key.days) ?? []
        let today = dayStamp
        if !days.contains(today) {
            days.append(today)
            defaults.set(Array(days.suffix(30)), forKey: Key.days)
        }
    }

    func shouldAsk(isStreaming: Bool) -> Bool {
        guard !isStreaming, !isBlocked else { return false }
        guard defaults.integer(forKey: Key.answers) >= Self.minAnswers else { return false }
        guard (defaults.stringArray(forKey: Key.days) ?? []).count >= Self.minDistinctDays else { return false }
        if defaults.string(forKey: Key.lastAskedVersion) == appVersion { return false }
        if let last = defaults.object(forKey: Key.lastAskedAt) as? Date {
            let days = calendar.dateComponents([.day], from: last, to: now()).day ?? 0
            if days < Self.minDaysBetweenAsks { return false }
        }
        return true
    }

    func markAsked() {
        defaults.set(now(), forKey: Key.lastAskedAt)
        defaults.set(appVersion, forKey: Key.lastAskedVersion)
    }
}

/// Observable wrapper the chat view model and the root modifier share.
@MainActor
final class ReviewPromptStore: ObservableObject {
    static let shared = ReviewPromptStore()

    /// Bumps after each successful answer so the root modifier can react.
    @Published private(set) var answerTick = 0
    private let policy: ReviewPromptPolicy
    private var streamingOwners = Set<ObjectIdentifier>()
    var isAnyStreaming: Bool { !streamingOwners.isEmpty }

    /// Chat view models report their streaming flag so a prompt never lands mid-stream.
    func streamingChanged(owner: AnyObject, streaming: Bool) {
        if streaming { streamingOwners.insert(ObjectIdentifier(owner)) }
        else { streamingOwners.remove(ObjectIdentifier(owner)) }
    }

    init(policy: ReviewPromptPolicy = ReviewPromptPolicy()) {
        self.policy = policy
    }

    func recordSuccessfulAnswer() {
        policy.recordSuccessfulAnswer()
        answerTick += 1
    }

    func shouldAsk(isStreaming: Bool) -> Bool {
        policy.shouldAsk(isStreaming: isStreaming)
    }

    func markAsked() {
        policy.markAsked()
    }
}

/// Root modifier: two seconds after a successful answer, asks for a rating
/// if the app is active, nothing is streaming and the policy allows it.
struct ReviewPromptModifier: ViewModifier {
    @Environment(\.requestReview) private var requestReview
    @Environment(\.scenePhase) private var scenePhase
    @ObservedObject private var store = ReviewPromptStore.shared

    func body(content: Content) -> some View {
        content.task(id: store.answerTick) {
            guard store.answerTick > 0 else { return }
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            guard !Task.isCancelled, scenePhase == .active else { return }
            guard store.shouldAsk(isStreaming: store.isAnyStreaming) else { return }
            requestReview()
            store.markAsked()
        }
    }
}

extension View {
    func reviewPrompt() -> some View { modifier(ReviewPromptModifier()) }
}
