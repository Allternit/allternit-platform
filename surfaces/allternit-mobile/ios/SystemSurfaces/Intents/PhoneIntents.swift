import AppIntents
import Foundation

// Phone and Messages wave 2, step I4p: three Shortcuts actions that drive a
// linked phone. Contract: surfaces/docs/api/phone-holds.mdx (task route with
// `source: "intent"`, refusals as 403 with a plain `message`). Not among the
// ten App Shortcuts in AllternitShortcuts.swift on purpose.

// MARK: - Value types (no app types)

struct PhoneInfo: Sendable, Equatable, Hashable {
    var serial: String
    var name: String
    var online: Bool
}

struct PhoneRoutineInfo: Sendable, Equatable, Hashable {
    var name: String
    var detail: String?
}

/// What starting a phone task or routine came back with.
enum PhoneStartOutcome: Sendable, Equatable {
    case started(runId: String, phone: PhoneInfo)
    /// 403 `{status:"refused", message}`: shown as is.
    case refused(String)
}

struct PhoneRunStatusInfo: Sendable, Equatable {
    var runId: String
    var goal: String
    /// running · waiting_approval · done · denied · approval_timeout · phone_locked · failed · max_steps · timeout
    var status: String
    var phoneName: String
}

/// Phone answers that are not the generic gate's.
enum PhoneIntentFailure: Error, Sendable, Equatable, LocalizedError, CustomLocalizedStringResourceConvertible {
    case noPhone
    case phoneOffline(String)
    case refused(String)
    case quota
    case noRoutines

    var dialog: String {
        switch self {
        case .noPhone: return "No phone is linked yet. Open Allternit and link one under Phone Link."
        case .phoneOffline(let name): return "\(name) is offline. Open Allternit Desktop and check the phone, then try again."
        case .refused(let message): return message
        case .quota: return "Your phone minutes are used up this month. Open Allternit to see your usage."
        case .noRoutines: return "You have no saved phone routines yet. Record one in Phone Link first."
        }
    }

    var errorDescription: String? { dialog }
    var localizedStringResource: LocalizedStringResource { LocalizedStringResource(stringLiteral: dialog) }
}

/// Plain sentences for each result, shared by the intents and the docs.
enum PhoneIntentText {
    static func started(goal: String, phone: PhoneInfo) -> String {
        "Started on \(phone.name): \(goal). Allternit asks you before it pays, sends or deletes anything."
    }

    static func routineStarted(_ name: String, phone: PhoneInfo) -> String {
        "Started \(name) on \(phone.name)."
    }

    static func status(_ run: PhoneRunStatusInfo?) -> String {
        guard let run else { return "No phone task has run yet." }
        let goal = run.goal.isEmpty ? "Your last phone task" : "\"\(run.goal)\""
        switch run.status {
        case "running": return "\(goal) is running on \(run.phoneName)."
        // Review only: the whole item (with the screen) is read in the app.
        case "waiting_approval": return "\(goal) is waiting for your OK on \(run.phoneName). Open Allternit to read the whole item."
        case "done": return "\(goal) is done."
        case "denied": return "\(goal) stopped because you denied a step. Nothing was done on your phone."
        case "approval_timeout": return "\(goal) ended because nobody answered in time. Nothing was done."
        case "phone_locked": return "\(goal) stopped because \(run.phoneName) was locked. Unlock it, then run it again."
        case "refused": return "\(goal) did not start."
        default: return "\(goal) did not finish."
        }
    }
}

/// Runs `body` behind the gate. A `PhoneIntentFailure` is carried out of the
/// gate as a value, because the gate turns every other error into an
/// `IntentFailure` and would lose the refusal's own sentence.
enum PhoneIntentRun {
    static func text(_ body: @escaping @Sendable (any IntentServicing) async throws -> String) async throws -> String {
        let result: Result<String, PhoneIntentFailure> = try await IntentGate.run { svc in
            do { return .success(try await body(svc)) } catch let f as PhoneIntentFailure { return .failure(f) }
        }
        return try result.get()
    }
}

/// Picks the phone: the named one, else the only online one, else the first.
enum PhonePicker {
    static func pick(_ phones: [PhoneInfo], named name: String?) throws -> PhoneInfo {
        guard !phones.isEmpty else { throw PhoneIntentFailure.noPhone }
        if let name = name?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty {
            guard let hit = phones.first(where: { $0.name.localizedCaseInsensitiveCompare(name) == .orderedSame })
                ?? phones.first(where: { $0.name.localizedCaseInsensitiveContains(name) }) else {
                throw IntentFailure.notFound("a phone named \(name)")
            }
            guard hit.online else { throw PhoneIntentFailure.phoneOffline(hit.name) }
            return hit
        }
        if let online = phones.first(where: \.online) { return online }
        throw PhoneIntentFailure.phoneOffline(phones[0].name)
    }
}

// MARK: - Entities

struct PhoneEntity: AppEntity, Sendable {
    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Phone"
    static let defaultQuery = PhoneQuery()

    var id: String
    var name: String
    var online: Bool

    init(_ info: PhoneInfo) { id = info.serial; name = info.name; online = info.online }

    var displayRepresentation: DisplayRepresentation {
        DisplayRepresentation(title: "\(name)", subtitle: online ? "Online" : "Offline")
    }
}

struct PhoneQuery: EntityStringQuery {
    init() {}
    private func all() async -> [PhoneInfo] {
        guard let svc = IntentServiceRegistry.current, svc.isSignedIn() else { return [] }
        return (try? await svc.listPhones()) ?? []
    }
    func entities(for identifiers: [String]) async throws -> [PhoneEntity] {
        let list = await all()
        return identifiers.compactMap { id in list.first { $0.serial == id }.map(PhoneEntity.init) }
    }
    func entities(matching string: String) async throws -> [PhoneEntity] {
        await all().filter { $0.name.localizedCaseInsensitiveContains(string) }.map(PhoneEntity.init)
    }
    func suggestedEntities() async throws -> [PhoneEntity] { await all().map(PhoneEntity.init) }
}

struct PhoneRoutineEntity: AppEntity, Sendable {
    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Phone routine"
    static let defaultQuery = PhoneRoutineQuery()

    var id: String
    var detail: String?

    init(_ info: PhoneRoutineInfo) { id = info.name; detail = info.detail }

    var displayRepresentation: DisplayRepresentation {
        DisplayRepresentation(title: "\(id)", subtitle: detail.map { "\($0)" })
    }
}

struct PhoneRoutineQuery: EntityStringQuery {
    init() {}
    private func all() async -> [PhoneRoutineInfo] {
        guard let svc = IntentServiceRegistry.current, svc.isSignedIn() else { return [] }
        return (try? await svc.listPhoneRoutines()) ?? []
    }
    func entities(for identifiers: [String]) async throws -> [PhoneRoutineEntity] {
        let list = await all()
        return identifiers.map { id in list.first { $0.name == id }.map(PhoneRoutineEntity.init) ?? PhoneRoutineEntity(PhoneRoutineInfo(name: id)) }
    }
    func entities(matching string: String) async throws -> [PhoneRoutineEntity] {
        await all().filter { $0.name.localizedCaseInsensitiveContains(string) }.map(PhoneRoutineEntity.init)
    }
    func suggestedEntities() async throws -> [PhoneRoutineEntity] { await all().map(PhoneRoutineEntity.init) }
}

// MARK: - Intents

/// Signed out, no phone, phone offline and refused each answer with their own
/// sentence; signed out opens the app.
struct RunPhoneTaskIntent: AppIntent {
    static let title: LocalizedStringResource = "Run a Phone Task"
    static let description = IntentDescription(
        "Asks your bot to do something on your linked phone. It stops and asks you before paying, sending or deleting.")

    @Parameter(title: "Task")
    var goal: String
    @Parameter(title: "Phone")
    var phone: PhoneEntity?

    static var parameterSummary: some ParameterSummary {
        Summary("Run \(\.$goal) on my phone") { \.$phone }
    }

    init() {}

    func perform() async throws -> some IntentResult & ProvidesDialog {
        let goal = self.goal.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !goal.isEmpty else { throw $goal.needsValueError("What should your bot do on the phone?") }
        let name = phone?.name
        let text = try await PhoneIntentRun.text { svc in
            let picked = try PhonePicker.pick(try await svc.listPhones(), named: name)
            switch try await svc.startPhoneTask(serial: picked.serial, goal: goal) {
            case .started(_, let p): return PhoneIntentText.started(goal: goal, phone: p)
            case .refused(let message): throw PhoneIntentFailure.refused(message)
            }
        }
        return .result(dialog: IntentSpeech.dialog(text))
    }
}

struct RunPhoneRoutineIntent: AppIntent {
    static let title: LocalizedStringResource = "Run a Phone Routine"
    static let description = IntentDescription("Replays one of your saved phone routines on your linked phone.")

    @Parameter(title: "Routine")
    var routine: PhoneRoutineEntity
    @Parameter(title: "Phone")
    var phone: PhoneEntity?

    static var parameterSummary: some ParameterSummary {
        Summary("Run \(\.$routine) on my phone") { \.$phone }
    }

    init() {}

    func perform() async throws -> some IntentResult & ProvidesDialog {
        let name = routine.id
        let phoneName = phone?.name
        let text = try await PhoneIntentRun.text { svc in
            let picked = try PhonePicker.pick(try await svc.listPhones(), named: phoneName)
            switch try await svc.runPhoneRoutine(name: name, serial: picked.serial) {
            case .started(_, let p): return PhoneIntentText.routineStarted(name, phone: p)
            case .refused(let message): throw PhoneIntentFailure.refused(message)
            }
        }
        return .result(dialog: IntentSpeech.dialog(text))
    }
}

struct PhoneTaskStatusIntent: AppIntent {
    static let title: LocalizedStringResource = "Phone Task Status"
    static let description = IntentDescription("Tells you how your latest phone task is going.")

    init() {}

    func perform() async throws -> some IntentResult & ProvidesDialog {
        let text = try await PhoneIntentRun.text { svc in
            let phones = try await svc.listPhones()
            guard !phones.isEmpty else { throw PhoneIntentFailure.noPhone }
            return PhoneIntentText.status(try await svc.latestPhoneRun())
        }
        return .result(dialog: IntentSpeech.dialog(text))
    }
}
