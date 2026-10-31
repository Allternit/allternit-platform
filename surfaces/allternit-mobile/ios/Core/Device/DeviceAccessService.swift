import Contacts
import EventKit
import Foundation

/// Answers the bot's `device.*` requests on the control socket (see
/// cmd/allternit-api phone_app.rs). Runs only while the app is in the
/// foreground, because that is the only time the socket is open.
///
/// Every call is checked here, whatever the node already checked: the type's
/// switch must be on (`device_access_off`) and iOS must have granted access
/// (`permission_denied`). Errors go back as `AppPhoneRPCError` codes the
/// Desktop turns into messages the bot can explain.
@MainActor
enum DeviceAccessService {
    static func handle(method: String, params: [String: Any]) async throws -> Any {
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
