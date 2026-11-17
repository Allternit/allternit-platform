import Contacts
import EventKit
import Foundation
import UIKit

/// Answers the bot's `device.*` requests on the control socket (see
/// cmd/allternit-api phone_app.rs). Runs only while the app is in the
/// foreground, because that is the only time the socket is open.
///
/// Every call is checked here, whatever the node already checked: the type's
/// switch must be on (`device_access_off`) and iOS must have granted access
/// (`permission_denied`). Errors go back as `AppPhoneRPCError` codes the
/// Desktop turns into messages the bot can explain.
///
/// Phone Use (docs/contracts/CONTRACT-phone-use-2026-11-16.md §9.1) adds
/// `device.shortcuts.run`, `device.home.list|set|scene` (behind the same two
/// gates), and two reads with no switch: `device.describe` and
/// `device.screen.read`. Those answer with the contract envelope
/// (`PhoneUseEnvelope`) as the result. Approval is never decided on the
/// phone: sends, purchases and Home security changes are approved on the
/// person's computer before the request is sent (phone-tools.ts
/// `phoneUseApproval`, the same gate as calendar and reminder writes).
@MainActor
enum DeviceAccessService {
    static func handle(method: String, params: [String: Any]) async throws -> Any {
        // Reads with no switch: what this phone can do, and the screen.
        switch method {
        case "device.describe": return describe()
        case "device.screen.read": return screenReadFromApp()
        default: break
        }
        guard let kind = kind(of: method) else {
            throw AppPhoneRPCError(code: "unknown_method", message: "Unknown method \(method)")
        }
        let store = DeviceAccessStore.shared
        guard store.enabled[kind] == true else {
            throw AppPhoneRPCError(code: "device_access_off", message: "\(kind.title) is switched off in Settings > Device Access.")
        }
        guard DeviceAccessStore.systemAccess(kind) == .granted else {
            throw AppPhoneRPCError(code: "permission_denied", message: "iOS access to \(kind.title) is not allowed for Allternit.")
        }
        switch method {
        case "device.calendar.list": return try listEvents(params)
        case "device.calendar.create": return try createEvent(params)
        case "device.reminders.list": return try await listReminders(params)
        case "device.reminders.create": return try createReminder(params)
        case "device.contacts.search": return try searchContacts(params)
        case "device.shortcuts.run": return try await runShortcut(params)
        case "device.home.list":
            let filter = params["kind"] as? String
            if params["kind"] != nil, filter == nil { throw bad("kind must be a string.") }
            return try await HomeAccessService.shared.list(kind: filter)
        case "device.home.set":
            guard let id = homeId(params, "device_id"), let state = params["state"] as? [String: Any] else {
                throw bad("device_id and state (an object) are required.")
            }
            return try await HomeAccessService.shared.set(deviceId: id, state: state)
        case "device.home.scene":
            guard let id = homeId(params, "scene_id") else { throw bad("scene_id is required.") }
            return try await HomeAccessService.shared.runScene(id: id)
        default: throw AppPhoneRPCError(code: "unknown_method", message: "Unknown method \(method)")
        }
    }

    static func kind(of method: String) -> DeviceAccessKind? {
        let parts = method.split(separator: ".")
        guard parts.count == 3, parts[0] == "device" else { return nil }
        return DeviceAccessKind(rawValue: String(parts[1]))
    }

    // MARK: - Calendar

    private static func listEvents(_ p: [String: Any]) throws -> Any {
        guard let from = date(p["from"]), let to = date(p["to"]), to > from, to.timeIntervalSince(from) <= 62 * 86_400 else {
            throw bad("from and to must be ISO 8601 times within 62 days.")
        }
        let limit = min(max(p["limit"] as? Int ?? 100, 1), 200)
        let store = EKEventStore()
        let predicate = store.predicateForEvents(withStart: from, end: to, calendars: nil)
        let events = store.events(matching: predicate).sorted { $0.startDate < $1.startDate }.prefix(limit)
        return ["events": events.map { e -> [String: Any] in
            var row: [String: Any] = [
                "id": e.eventIdentifier ?? "",
                "title": e.title ?? "",
                "start": iso(e.startDate),
                "end": iso(e.endDate),
                "allDay": e.isAllDay,
                "calendar": e.calendar?.title ?? "",
            ]
            if let location = e.location, !location.isEmpty { row["location"] = location }
            return row
        }]
    }

    private static func createEvent(_ p: [String: Any]) throws -> Any {
        guard let title = (p["title"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines), !title.isEmpty,
              let start = date(p["start"]) else { throw bad("title and start are required.") }
        let end = date(p["end"]) ?? start.addingTimeInterval(3600)
        guard end > start else { throw bad("end must be after start.") }
        let store = EKEventStore()
        guard let calendar = store.defaultCalendarForNewEvents else {
            throw AppPhoneRPCError(code: "device_failed", message: "No calendar on this iPhone accepts new events.")
        }
        let event = EKEvent(eventStore: store)
        event.calendar = calendar
        event.title = String(title.prefix(200))
        event.startDate = start
        event.endDate = end
        event.isAllDay = p["allDay"] as? Bool ?? false
        if let location = p["location"] as? String { event.location = String(location.prefix(200)) }
        if let notes = p["notes"] as? String { event.notes = String(notes.prefix(2000)) }
        do { try store.save(event, span: .thisEvent) } catch {
            throw AppPhoneRPCError(code: "device_failed", message: error.localizedDescription)
        }
        return ["created": true, "id": event.eventIdentifier ?? "", "calendar": calendar.title]
    }

    // MARK: - Reminders

    private static func listReminders(_ p: [String: Any]) async throws -> Any {
        let completed = p["completed"] as? Bool ?? false
        let limit = min(max(p["limit"] as? Int ?? 100, 1), 200)
        let store = EKEventStore()
        let predicate = completed
            ? store.predicateForCompletedReminders(withCompletionDateStarting: nil, ending: nil, calendars: nil)
            : store.predicateForIncompleteReminders(withDueDateStarting: nil, ending: nil, calendars: nil)
        // EKReminder is not Sendable: turn the rows into JSON data inside the callback.
        let data: Data = await withCheckedContinuation { continuation in
            store.fetchReminders(matching: predicate) { reminders in
                let rows: [[String: Any]] = (reminders ?? []).prefix(limit).map { r in
                    var row: [String: Any] = [
                        "id": r.calendarItemIdentifier,
                        "title": r.title ?? "",
                        "completed": r.isCompleted,
                        "list": r.calendar?.title ?? "",
                    ]
                    if let due = r.dueDateComponents?.date { row["due"] = iso(due) }
                    return row
                }
                let json = (try? JSONSerialization.data(withJSONObject: ["reminders": rows])) ?? Data("{\"reminders\":[]}".utf8)
                continuation.resume(returning: json)
            }
        }
        return (try? JSONSerialization.jsonObject(with: data)) ?? ["reminders": [Any]()]
    }

    private static func createReminder(_ p: [String: Any]) throws -> Any {
        guard let title = (p["title"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines), !title.isEmpty else {
            throw bad("title is required.")
        }
        let store = EKEventStore()
        guard let calendar = store.defaultCalendarForNewReminders() else {
            throw AppPhoneRPCError(code: "device_failed", message: "No reminders list on this iPhone accepts new reminders.")
        }
        let reminder = EKReminder(eventStore: store)
        reminder.calendar = calendar
        reminder.title = String(title.prefix(200))
        if let notes = p["notes"] as? String { reminder.notes = String(notes.prefix(2000)) }
        if p["due"] != nil {
            guard let due = date(p["due"]) else { throw bad("due must be an ISO 8601 time.") }
            reminder.dueDateComponents = Calendar.current.dateComponents(
                [.year, .month, .day, .hour, .minute], from: due)
        }
        do { try store.save(reminder, commit: true) } catch {
            throw AppPhoneRPCError(code: "device_failed", message: error.localizedDescription)
        }
        return ["created": true, "id": reminder.calendarItemIdentifier, "list": calendar.title]
    }

    // MARK: - Contacts

    private static func searchContacts(_ p: [String: Any]) throws -> Any {
        guard let query = (p["query"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
              !query.isEmpty, query.count <= 100 else { throw bad("query is required.") }
        let limit = min(max(p["limit"] as? Int ?? 20, 1), 50)
        let keys: [CNKeyDescriptor] = [
            CNContactFormatter.descriptorForRequiredKeys(for: .fullName),
            CNContactPhoneNumbersKey as CNKeyDescriptor,
            CNContactEmailAddressesKey as CNKeyDescriptor,
        ]
        let store = CNContactStore()
        let found: [CNContact]
        do {
            found = try store.unifiedContacts(matching: CNContact.predicateForContacts(matchingName: query), keysToFetch: keys)
        } catch {
            throw AppPhoneRPCError(code: "device_failed", message: error.localizedDescription)
        }
        return ["contacts": found.prefix(limit).map { c -> [String: Any] in
            [
                "id": c.identifier,
                "name": CNContactFormatter.string(from: c, style: .fullName) ?? "",
                "phones": c.phoneNumbers.map { $0.value.stringValue },
                "emails": c.emailAddresses.map { String($0.value) },
            ]
        }]
    }

    // MARK: - Phone Use

    private static func runShortcut(_ p: [String: Any]) async throws -> Any {
        guard let name = p["name"] as? String else { throw bad("name is required.") }
        let timeout: Int
        switch p["timeout_s"] {
        case nil: timeout = ShortcutBridge.defaultTimeout
        case let n as Int where (1...120).contains(n): timeout = n
        default: throw bad("timeout_s must be a whole number of seconds, 1-120.")
        }
        return await ShortcutBridge.shared.run(name: name, input: p["input"], timeoutSeconds: timeout)
    }

    /// The contract names the socket field (`device_id`, `scene_id`); `id`
    /// is accepted too, as the tool itself calls it.
    private static func homeId(_ p: [String: Any], _ field: String) -> String? {
        let raw = (p[field] as? String) ?? (p["id"] as? String)
        guard let id = raw?.trimmingCharacters(in: .whitespacesAndNewlines), !id.isEmpty, id.count <= 64 else { return nil }
        return id
    }

    /// `device.describe`: what this phone can do now. The app answers only
    /// while it is open, so it is in front and the phone is unlocked.
    private static func describe() -> Any {
        let store = DeviceAccessStore.shared
        let catalog = PhoneActionsCatalog.shared
        let known = Set(ShortcutBridge.shared.knownShortcuts)
        let foreground = UIApplication.shared.applicationState == .active
        let apps: [[String: Any]] = catalog.apps.map { app in
            [
                "id": app.id,
                "name": app.name,
                "category": app.category,
                "actions": app.intents.count,
                "web_actions": app.web.count,
                "shortcut": app.shortcut,
                "shortcut_known": known.contains(app.shortcut),
                "shortcut_link": app.link ?? NSNull(),
            ]
        }
        return PhoneUseEnvelope.ok([
            "foreground": foreground,
            "foreground_app": foreground ? "Allternit" : NSNull(),
            "locked": foreground ? false : NSNull(),
            "shortcuts": Array(known).sorted(),
            "home": ["available": store.isAllowed(.home)],
            "broadcast": AppPhoneSharedState.isLive,
            "access": store.allowedSnapshot,
            "catalog": [
                "built": catalog.built,
                "actions_shortcut": [
                    "name": catalog.actions.name,
                    "known": known.contains(catalog.actions.name),
                    "link": catalog.actions.link ?? NSNull(),
                    "actions": catalog.actions.actions.map(\.id),
                ] as [String: Any],
                "apps": apps,
            ] as [String: Any],
        ])
    }

    /// `device.screen.read` reaching the app: the frames live in the
    /// broadcast extension (AllternitScreen/ScreenReader.swift), which
    /// answers the same request on its own socket. Here, only say why not.
    private static func screenReadFromApp() -> Any {
        guard AppPhoneSharedState.isLive else { return PhoneUseEnvelope.needsYou(.broadcastStopped, error: "broadcast_stopped") }
        return PhoneUseEnvelope.failure("unavailable", "The screen is read from the live broadcast, not the app. Read the latest frame instead.",
                                        extra: ["fallback": "runtime_reads_frames"])
    }

    // MARK: - Helpers

    private nonisolated static func bad(_ message: String) -> AppPhoneRPCError {
        AppPhoneRPCError(code: "bad_params", message: message)
    }

    private nonisolated static func date(_ value: Any?) -> Date? {
        guard let text = value as? String, text.count <= 40 else { return nil }
        let withFraction = ISO8601DateFormatter()
        withFraction.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = withFraction.date(from: text) { return d }
        let plain = ISO8601DateFormatter()
        plain.formatOptions = [.withInternetDateTime]
        return plain.date(from: text)
    }

    private nonisolated static func iso(_ date: Date) -> String {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f.string(from: date)
    }
}
