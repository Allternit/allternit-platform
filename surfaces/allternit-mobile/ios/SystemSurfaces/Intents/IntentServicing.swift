import Foundation
import os

// MARK: - Value types (Sendable, no app types)

struct BotInfo: Sendable, Equatable, Hashable {
    var id: String
    var name: String
    var status: String = ""
    var step: String = ""
}

struct ConversationInfo: Sendable, Equatable, Hashable {
    /// Thread id when the row is a bot thread, else the session id.
    var id: String
    /// The chat session behind it (what `sendUserMessage` takes).
    var sessionId: String?
    var title: String
    var botId: String?
    var lastActivityAt: String?
}

struct RoutineInfo: Sendable, Equatable, Hashable {
    var id: String
    var name: String
    var state: String = ""
    var schedule: String?
}

struct ProjectInfo: Sendable, Equatable, Hashable {
    var id: String
    var title: String
}

struct ComputerInfo: Sendable, Equatable, Hashable {
    var id: String
    var name: String
    var typeLabel: String = ""
    var isOnline: Bool = false
    var isActive: Bool = false
}

struct ModelInfo: Sendable, Equatable, Hashable {
    var id: String
    var name: String
    var provider: String
}

/// One inbox item. `fullText` and `recipient` are what Approve must show.
struct ApprovalInfo: Sendable, Equatable, Hashable {
    var id: String
    var kind: String
    var botId: String
    var botName: String
    var summary: String
    /// The whole text the bot wants sent (peer-held items), else the summary.
    var fullText: String
    var recipient: String?
    var contentHash: String?
    var sessionId: String?
    var isPeerHeld: Bool = false
    var canApprove: Bool = true
    var at: String = ""
}

// MARK: - Typed results, one per action in the suite table

/// Ask a bot.
struct AskResult: Sendable, Equatable {
    var botName: String
    /// nil with `stillWorking` true when the wait ran out.
    var answer: String?
    var stillWorking: Bool
    var conversation: ConversationInfo
}

/// Run a routine.
struct RoutineStartResult: Sendable, Equatable {
    var routine: RoutineInfo
    var started: Bool
}

/// Show what needs me. `fromSnapshot` is true when the computer was not
/// reachable and the rows come from the cloud summary (read-only, no hash).
struct ApprovalList: Sendable, Equatable {
    var items: [ApprovalInfo]
    var asOf: Date?
    var fromSnapshot: Bool
}

enum DecisionKind: String, Sendable { case approve, deny, snooze }

/// `approved|sent|changed|alreadyHandled|offline` for Approve; Deny and
/// Snooze report `denied` / `snoozed` on success and reuse the rest.
enum DecisionOutcome: String, Sendable, Equatable {
    case approved, sent, changed, alreadyHandled, offline, denied, snoozed
}

struct DecisionResult: Sendable, Equatable {
    var kind: DecisionKind
    var outcome: DecisionOutcome
    var itemId: String
    var snoozedUntil: Date?
}

/// Pause all bots / Resume all bots.
struct PausedState: Sendable, Equatable {
    var paused: Bool
    var since: Date?
    var by: String?
}

enum BotStatusFilter: String, Sendable { case all, working, needsMe }

struct BotStatusList: Sendable, Equatable {
    var bots: [BotInfo]
    var asOf: Date?
}

struct LastRoutineResult: Sendable, Equatable {
    var routineId: String
    var title: String
    var status: String
    var at: Date?
}

struct QuietHoursState: Sendable, Equatable { var enabled: Bool }
struct StoppedResult: Sendable, Equatable { var sessionId: String }
struct BrainChangeResult: Sendable, Equatable { var bot: BotInfo; var model: ModelInfo }
struct SavedResult: Sendable, Equatable { var botName: String }
struct ArtifactInfo: Sendable, Equatable { var id: String; var title: String; var mimeType: String; var fileURL: URL? }

// MARK: - Failure

/// What a gate or a service throws. Intents show `dialog`; `opensApp` means
/// the intent should return `OpenAllternitIntent` as its `OpensIntent`.
enum IntentFailure: Error, Sendable, Equatable {
    case signedOut
    case noComputer
    /// Computer asleep or unreachable, even after the one wake and retry.
    case offline
    /// No service is registered (an extension): finish in the app.
    case appRequired
    case notFound(String)
    /// The backing call does not exist yet.
    case notAvailable(String)
    case failed(String)

    var dialog: String {
        switch self {
        case .signedOut: return "Sign in to Allternit first. Open the app to sign in."
        case .noComputer: return "Pair a computer first. Open Allternit and add one under Computers."
        case .offline: return "Your computer is asleep or offline. Try again in a moment."
        case .appRequired: return "Open Allternit to finish this."
        case .notFound(let what): return "I could not find \(what)."
        case .notAvailable(let what): return "\(what) is not available yet."
        case .failed(let why): return "That did not work: \(why)"
        }
    }

    var opensApp: Bool {
        switch self {
        case .signedOut, .noComputer, .appRequired: return true
        default: return false
        }
    }
}

// MARK: - Service protocol

/// Everything the intents call. Implemented in the app (`Core/IntentServices`)
/// and registered at launch. All methods throw `IntentFailure` (or an
/// underlying error the gate classifies through `classify`).
protocol IntentServicing: Sendable {
    func isSignedIn() -> Bool
    func hasComputer() -> Bool
    /// True when `error` means "computer asleep or unreachable".
    func isSleepingError(_ error: Error) -> Bool
    /// Wakes the active computer and waits up to `timeout`; true when it answers.
    func wakeComputer(timeout: TimeInterval) async -> Bool
    func classify(_ error: Error) -> IntentFailure

    // Directory (entity queries)
    func listBots() async throws -> [BotInfo]
    func listConversations(matching: String?) async throws -> [ConversationInfo]
    func listRoutines() async throws -> [RoutineInfo]
    func listProjects() async throws -> [ProjectInfo]
    func listComputers() async throws -> [ComputerInfo]
    func listModels() async throws -> [ModelInfo]

    // Talk
    func ask(botId: String, text: String, wait: TimeInterval) async throws -> AskResult
    func newChat(botId: String) async throws -> ConversationInfo
    /// The bot's pinned main chat, nil when it has none yet.
    func mainConversation(botId: String) async throws -> ConversationInfo?

    // Delegate
    func handOff(text: String, title: String?) async throws -> ProjectInfo
    func runRoutine(id: String) async throws -> RoutineStartResult
    func send(botId: String, text: String) async throws -> ConversationInfo
    func reply(conversation: ConversationInfo, text: String) async throws -> ConversationInfo
    func createRoutine(name: String, instructions: String, schedule: String?) async throws -> RoutineInfo

    // Decide
    func listApprovals() async throws -> ApprovalList
    func approve(_ item: ApprovalInfo) async throws -> DecisionResult
    func deny(_ item: ApprovalInfo) async throws -> DecisionResult
    func snooze(_ item: ApprovalInfo, minutes: Int) async throws -> DecisionResult

    // Status (cloud summary; works with the computer asleep)
    func botStatus(filter: BotStatusFilter) async throws -> BotStatusList
    func lastRoutineResult(routineId: String?) async throws -> LastRoutineResult
    func latestArtifact(botId: String) async throws -> ArtifactInfo
    func watchBot(botId: String) async throws -> BotInfo

    // Control
    func pausedState() async throws -> PausedState
    func setPaused(_ paused: Bool) async throws -> PausedState
    func stopRun(conversation: ConversationInfo) async throws -> StoppedResult
    func setQuietHours(enabled: Bool) async throws -> QuietHoursState
    func switchComputer(id: String) async throws -> ComputerInfo
    func setBrain(botId: String, modelId: String) async throws -> BrainChangeResult

    // Memory
    func remember(botId: String, text: String) async throws -> SavedResult

    // MARK: - Talk/Delegate/Decide additions

    /// Routes an `allternit://` link inside the running app. False when no
    /// screen handled it.
    func openLink(_ url: URL) async -> Bool
    /// Creates a Code session on the active computer (in `projectId` when
    /// given) and sends `text` as its first message.
    func startCodeTask(text: String, projectId: String?) async throws -> ConversationInfo
    /// Uploads one file (at most `SendFileRules.maxBytes`) and sends the bot
    /// a message carrying its link, plus `note` when given.
    func sendFile(botId: String, data: Data, filename: String, note: String?) async throws -> ConversationInfo

    // Phone (PhoneIntents.swift). Tasks start with `source: "intent"`.
    func listPhones() async throws -> [PhoneInfo]
    func startPhoneTask(serial: String, goal: String) async throws -> PhoneStartOutcome
    func listPhoneRoutines() async throws -> [PhoneRoutineInfo]
    func runPhoneRoutine(name: String, serial: String) async throws -> PhoneStartOutcome
    func latestPhoneRun() async throws -> PhoneRunStatusInfo?
}

// MARK: - Registry

/// Where the app registers its implementation. Extensions never register, so
/// `current` is nil there and queries fall back to the App Group snapshot.
enum IntentServiceRegistry {
    private static let box = OSAllocatedUnfairLock<(any IntentServicing)?>(initialState: nil)

    static var current: (any IntentServicing)? { box.withLock { $0 } }
    static func register(_ service: any IntentServicing) { box.withLock { $0 = service } }
}

// MARK: - Status/Control additions

/// A Focus filter confirm (`PUT /api/v1/notifications/focus`).
struct FocusFilterRequest: Sendable, Equatable, Codable {
    var deviceId: String
    var active: Bool
    /// nil = every bot is allowed (encoded as JSON `null`).
    var allowedBotIds: [String]?
    var allowApprovals: Bool

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(deviceId, forKey: .deviceId)
        try c.encode(active, forKey: .active)
        if let allowedBotIds { try c.encode(allowedBotIds, forKey: .allowedBotIds) }
        else { try c.encodeNil(forKey: .allowedBotIds) }
        try c.encode(allowApprovals, forKey: .allowApprovals)
    }
}

protocol FocusFilterServicing: Sendable {
    func setFocusFilter(_ request: FocusFilterRequest) async throws
}

// MARK: - Bar (Do with Allternit)

/// One app in the bar (`GET /api/v1/bar/apps`). `id` is `<kind>:<ref>`.
struct BarAppInfo: Sendable, Equatable, Hashable {
    /// `connector` | `vendor` | `artifact`.
    var kind: String
    var ref: String
    var name: String
    /// `browser` | `app` | `full`.
    var level: String
    var toolCount: Int

    var id: String { "\(kind):\(ref)" }
    /// Artifact apps are listed but cannot run yet (contract section 8).
    var isRunnable: Bool { kind != "artifact" }

    /// Splits an id made by `id`. The ref may itself hold colons.
    static func split(_ id: String) -> (kind: String, ref: String)? {
        guard let colon = id.firstIndex(of: ":") else { return nil }
        let kind = String(id[..<colon]), ref = String(id[id.index(after: colon)...])
        return kind.isEmpty || ref.isEmpty ? nil : (kind, ref)
    }
}

/// One tool of a bar app (`GET /api/v1/bar/apps/:kind/:ref/tools`).
struct BarToolInfo: Sendable, Equatable, Hashable {
    var name: String
    var title: String
    var hasUI: Bool
    var readOnly: Bool
}

/// A value for one blank, sent as typed JSON.
enum BarValue: Sendable, Equatable, Hashable {
    case text(String)
    case number(Double)
    case bool(Bool)

    /// What a person reads (and what a deep link carries).
    var display: String {
        switch self {
        case .text(let s): return s
        case .number(let n): return n.rounded() == n && abs(n) < 1e15 ? String(Int64(n)) : String(n)
        case .bool(let b): return b ? "true" : "false"
        }
    }
}

/// One blank of an intent sentence (contract `Blank`).
struct BarBlankInfo: Sendable, Equatable, Hashable {
    struct Option: Sendable, Equatable, Hashable { var value: String; var label: String }

    var key: String
    var label: String
    /// `text` | `number` | `enum` | `boolean` | anything newer.
    var type: String
    var required: Bool
    var unit: String?
    var options: [Option]
    /// `standard` | `mcp_app` (`mcp_app` needs the app's own picker).
    var pickerKind: String

    /// True when Siri or Shortcuts can ask for it as plain text.
    var isSimple: Bool {
        pickerKind == "standard" && ["text", "number", "enum", "boolean", "string", "integer"].contains(type)
    }
}

/// `POST /api/v1/bar/intent`. `sentence` is the parts joined, with each
/// blank shown as its label.
struct BarIntentInfo: Sendable, Equatable {
    var intentId: String
    var app: BarAppInfo
    var toolName: String
    var toolTitle: String
    var sentence: String
    var blanks: [BarBlankInfo]
}

/// `POST /api/v1/bar/review`.
struct BarReviewInfo: Sendable, Equatable {
    struct Line: Sendable, Equatable, Hashable { var label: String; var value: String }

    var jobId: String
    var level: String
    var needsConfirmation: Bool
    /// The server's rule text, shown as sent (contract section 4).
    var ruleText: String?
    var approverName: String?
    var approverIsYou: Bool
    var lines: [Line]
}

/// Where a run ended up when the intent stopped watching it.
enum BarRunOutcome: Sendable, Equatable {
    /// A receipt such as "Refunded €320", else the tool title.
    case completed(receipt: String)
    /// Still running when the wait ran out.
    case working
    /// A rule sent it to someone else. `approverName` nil = not known.
    case waitingApproval(approverName: String?)
    case needsInput
    case failed(String)
    case cancelled
}

/// Bar calls for the Do intent. Implemented by the app's `IntentServices`
/// (over `APIClient`) and reached as `IntentServiceRegistry.current as?
/// any BarIntentServicing`, like `FocusFilterServicing`.
///
/// Throws `IntentFailure`: `.noComputer` for `no_runtime` / 428 / no reply,
/// `.failed(text)` with plain wording for gate codes, never a raw code.
protocol BarIntentServicing: Sendable {
    /// `query` filters by app name (sent as `?q=` and applied here too).
    func listBarApps(query: String?) async throws -> [BarAppInfo]
    func listBarTools(appKind: String, appRef: String) async throws -> [BarToolInfo]
    /// `botId` nil = the caller acts as themself.
    func barIntent(appKind: String, appRef: String, tool: String, botId: String?) async throws -> BarIntentInfo
    /// A 422 `missing_fields` throws `.failed("Fill in X first.")`.
    func barReview(intentId: String, values: [String: BarValue], botId: String?) async throws -> BarReviewInfo
    /// Runs the job and watches it for up to `wait` seconds.
    func barRun(jobId: String, wait: TimeInterval) async throws -> BarRunOutcome
    func barCancel(jobId: String) async
}
