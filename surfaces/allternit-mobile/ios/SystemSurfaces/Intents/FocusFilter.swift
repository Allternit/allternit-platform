import AppIntents
import Foundation

/// What a Focus asks of Allternit's notifications. Runs in the app process
/// (it needs the registered service). The system calls it when a Focus turns
/// on and again with the defaults when it turns off; "every bot, approvals
/// allowed" is the same as no filter, so that case is sent as inactive.
struct AllternitFocusFilterIntent: SetFocusFilterIntent {
    static let title: LocalizedStringResource = "Set bot notifications"
    static let description = IntentDescription("Chooses which bots may notify you loudly while this Focus is on. Other bots still notify, quietly.")

    @Parameter(title: "Allowed bots", description: "Leave empty to allow every bot.", default: [])
    var allowedBots: [BotEntity]

    @Parameter(title: "Allow approvals", description: "Approval requests from any bot still notify loudly.", default: true)
    var allowApprovals: Bool

    var displayRepresentation: DisplayRepresentation {
        let who = allowedBots.isEmpty ? "All bots" : allowedBots.map(\.name).joined(separator: ", ")
        return DisplayRepresentation(title: "\(who)", subtitle: allowApprovals ? "Approvals allowed" : "Approvals muted")
    }

    static var parameterSummary: some ParameterSummary {
        Summary("Allow \(\.$allowedBots), approvals \(\.$allowApprovals)")
    }

    func perform() async throws -> some IntentResult {
        let request = FocusFilterRequest.make(allowedBotIds: allowedBots.map(\.id), allowApprovals: allowApprovals)
        guard let svc = IntentServiceRegistry.current as? any FocusFilterServicing else {
            throw IntentFailure.appRequired
        }
        try await svc.setFocusFilter(request)
        return .result()
    }
}

extension FocusFilterRequest {
    /// Empty bot list = all bots (`null`). Active only when something is
    /// actually restricted. `deviceId` is filled in by the app's service.
    static func make(allowedBotIds: [String], allowApprovals: Bool) -> FocusFilterRequest {
        let restricted = !allowedBotIds.isEmpty || !allowApprovals
        return FocusFilterRequest(deviceId: "", active: restricted,
                                  allowedBotIds: allowedBotIds.isEmpty ? nil : allowedBotIds,
                                  allowApprovals: allowApprovals)
    }
}
