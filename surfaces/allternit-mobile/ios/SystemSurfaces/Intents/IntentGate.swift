import AppIntents
import Foundation

/// Returned (as `OpensIntent`) by any intent whose gate said `opensApp`.
struct OpenAllternitIntent: AppIntent {
    static let title: LocalizedStringResource = "Open Allternit"
    static let openAppWhenRun: Bool = true
    init() {}
    func perform() async throws -> some IntentResult { .result() }
}

extension IntentFailure: LocalizedError, CustomLocalizedStringResourceConvertible {
    var errorDescription: String? { dialog }
    var localizedStringResource: LocalizedStringResource { LocalizedStringResource(stringLiteral: dialog) }
}

/// The one gate every intent calls first.
///
///     // inside perform():
///     let bot = try await IntentGate.run { svc in
///         try await svc.ask(botId: bot.id, text: question, wait: 20)
///     }
///     return .result(value: bot.answer ?? "Still working", dialog: ...)
///
/// Signed out -> `.signedOut`; no computer (when `needsComputer`) ->
/// `.noComputer`; a sleeping computer gets one wake and retry (up to 15 s),
/// then `.offline`. In an extension (no service registered) it throws
/// `.appRequired`. Catch `IntentFailure` and, when `opensApp`, return
/// `.result(opensIntent: OpenAllternitIntent())`; otherwise rethrow (the
/// error is its own dialog).
enum IntentGate {
    static let wakeTimeout: TimeInterval = 15

    static func run<T: Sendable>(
        needsComputer: Bool = true,
        _ body: @Sendable (any IntentServicing) async throws -> T
    ) async throws -> T {
        try check(needsComputer: needsComputer)
        guard let svc = IntentServiceRegistry.current else { throw IntentFailure.appRequired }
        do {
            return try await body(svc)
        } catch {
            guard svc.isSleepingError(error) else { throw svc.classify(error) }
            guard await svc.wakeComputer(timeout: wakeTimeout) else { throw IntentFailure.offline }
            do { return try await body(svc) } catch { throw svc.classify(error) }
        }
    }

    /// Gate checks alone, for intents that only read the snapshot.
    static func check(needsComputer: Bool) throws {
        let svc = IntentServiceRegistry.current
        let signedIn = svc?.isSignedIn() ?? AppGroupStore.signedIn
        guard signedIn else { throw IntentFailure.signedOut }
        let paired = svc?.hasComputer() ?? AppGroupStore.hasComputer
        if needsComputer && !paired { throw IntentFailure.noComputer }
    }
}
