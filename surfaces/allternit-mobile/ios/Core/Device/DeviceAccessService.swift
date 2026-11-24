import Contacts
import CoreLocation
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
/// Calendar delete and reminder update are writes: like create, the Desktop
/// asks the person before it sends them (phone-app-store.ts DEVICE_WRITES).
/// `device.location.get` is approximate unless `precision` is `precise` and
/// the person allows full accuracy when iOS asks.
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
        case "device.calendar.delete": return try deleteEvent(params)
        case "device.reminders.list": return try await listReminders(params)
        case "device.reminders.create": return try createReminder(params)
        case "device.reminders.update": return try updateReminder(params)
        case "device.location.get": return try await getLocation(params)
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

    private static func deleteEvent(_ p: [String: Any]) throws -> Any {
        guard let id = itemId(p) else { throw bad("id is required.") }
        let store = EKEventStore()
        guard let event = store.event(withIdentifier: id) else {
            throw AppPhoneRPCError(code: "not_found", message: "No event with that id on this iPhone.")
        }
        let title = event.title ?? ""
        do { try store.remove(event, span: .thisEvent, commit: true) } catch {
            throw AppPhoneRPCError(code: "device_failed", message: error.localizedDescription)
        }
        return ["deleted": true, "id": id, "title": title]
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

    /// Only the fields sent change. A null field counts as not sent.
    private static func updateReminder(_ p: [String: Any]) throws -> Any {
        guard let id = itemId(p) else { throw bad("id is required.") }
        let title = field(p, "title"), due = field(p, "due"), completed = field(p, "completed"), notes = field(p, "notes")
        guard title != nil || due != nil || completed != nil || notes != nil else {
            throw bad("Send title, due, completed or notes.")
        }
        let store = EKEventStore()
        guard let reminder = store.calendarItem(withIdentifier: id) as? EKReminder else {
            throw AppPhoneRPCError(code: "not_found", message: "No reminder with that id on this iPhone.")
        }
        if let title {
            guard let text = (title as? String)?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty else {
                throw bad("title must be non-empty text.")
            }
            reminder.title = String(text.prefix(200))
        }
        if let notes {
            guard let text = notes as? String else { throw bad("notes must be text.") }
            reminder.notes = String(text.prefix(2000))
        }
        if let due {
            guard let date = date(due) else { throw bad("due must be an ISO 8601 time.") }
            reminder.dueDateComponents = Calendar.current.dateComponents(
                [.year, .month, .day, .hour, .minute], from: date)
        }
        if let completed {
            guard let done = completed as? Bool else { throw bad("completed must be true or false.") }
            reminder.isCompleted = done
        }
        do { try store.save(reminder, commit: true) } catch {
            throw AppPhoneRPCError(code: "device_failed", message: error.localizedDescription)
        }
        var row: [String: Any] = [
            "updated": true,
            "id": reminder.calendarItemIdentifier,
            "title": reminder.title ?? "",
            "completed": reminder.isCompleted,
            "list": reminder.calendar?.title ?? "",
        ]
        if let d = reminder.dueDateComponents?.date { row["due"] = iso(d) }
        return row
    }

    // MARK: - Location

    /// Approximate (the default): reduced accuracy, rounded to about 1 km,
    /// with city and region. Precise: full accuracy only when the person has
    /// it on or allows it for now when iOS asks; otherwise the answer stays
    /// approximate and says so in `precision`.
    private static func getLocation(_ p: [String: Any]) async throws -> Any {
        let wantsPrecise: Bool
        switch p["precision"] {
        case nil, is NSNull: wantsPrecise = false
        case let s as String where s == "approximate" || s == "precise": wantsPrecise = s == "precise"
        default: throw bad("precision must be approximate or precise.")
        }
        let locator = DeviceLocator.shared
        let precise = wantsPrecise ? await locator.allowFullAccuracy() : false
        guard let fix = await locator.currentFix(precise: precise) else {
            throw AppPhoneRPCError(code: "device_failed", message: "This iPhone could not find its location just now.")
        }
        var lat = fix.lat, lon = fix.lon, accuracy = fix.accuracy
        if !precise {
            // Two decimals is about 1.1 km of latitude.
            lat = (lat * 100).rounded() / 100
            lon = (lon * 100).rounded() / 100
            accuracy = max(accuracy, 1000)
        }
        var row: [String: Any] = [
            "lat": lat,
            "lon": lon,
            "accuracy_m": Int(accuracy.rounded()),
            "precision": precise ? "precise" : "approximate",
        ]
        let place = await locator.place(lat: lat, lon: lon)
        if let city = place.city { row["city"] = city }
        if let region = place.region { row["region"] = region }
        return row
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
                // Counts stay for Desktops from before `action_list`.
                "actions": app.intents.count,
                "web_actions": app.web.count,
                // Every action this app's shortcut can run (contract §7.2), so phone.actions.list
                // is full and phone.actions.run can check the id. Widget-only intents are not in the catalog.
                "action_list": app.intents.map { i in
                    ["id": i.id, "title": i.title, "kind": i.kind, "params": i.params, "opens_app": i.opensApp, "source": "app_intent"] as [String: Any]
                } + app.web.map { w in
                    ["id": w.action, "title": w.action, "kind": w.kind, "params": w.params, "confidence": w.confidence, "source": "web_research"] as [String: Any]
                },
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
                    "action_list": catalog.actions.actions.map { a in
                        ["id": a.id, "title": a.title, "kind": a.kind, "params": a.params] as [String: Any]
                    },
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

    private nonisolated static func itemId(_ p: [String: Any]) -> String? {
        guard let id = (p["id"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
              !id.isEmpty, id.count <= 200 else { return nil }
        return id
    }

    /// A field that was sent, with JSON null treated as not sent.
    private nonisolated static func field(_ p: [String: Any], _ key: String) -> Any? {
        guard let value = p[key], !(value is NSNull) else { return nil }
        return value
    }

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

/// One-shot when-in-use location for `device.location.get`. Created on the
/// main thread, so CoreLocation calls the delegate there.
@MainActor
final class DeviceLocator: NSObject {
    static let shared = DeviceLocator()

    struct Fix: Sendable { let lat: Double, lon: Double, accuracy: Double }

    /// The purpose key in Info.plist NSLocationTemporaryUsageDescriptionDictionary.
    private static let purposeKey = "PreciseAnswer"
    private static let fixTimeout: UInt64 = 15_000_000_000

    private let manager = CLLocationManager()
    private let geocoder = CLGeocoder()
    private var authWaiters: [CheckedContinuation<Void, Never>] = []
    private var fixWaiters: [UUID: CheckedContinuation<Fix?, Never>] = [:]

    override init() {
        super.init()
        manager.delegate = self
    }

    private var hasWhenInUse: Bool {
        manager.authorizationStatus == .authorizedWhenInUse || manager.authorizationStatus == .authorizedAlways
    }

    /// Asks iOS for when-in-use access the first time; true when granted.
    func requestWhenInUse() async -> Bool {
        if manager.authorizationStatus == .notDetermined {
            await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
                authWaiters.append(c)
                manager.requestWhenInUseAuthorization()
            }
        }
        return hasWhenInUse
    }

    /// True when full accuracy is on, or the person allows it for now. iOS
    /// asks each time the app has only approximate location.
    func allowFullAccuracy() async -> Bool {
        if manager.accuracyAuthorization == .fullAccuracy { return true }
        await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
            manager.requestTemporaryFullAccuracyAuthorization(withPurposeKey: Self.purposeKey) { @Sendable _ in c.resume() }
        }
        return manager.accuracyAuthorization == .fullAccuracy
    }

    func currentFix(precise: Bool) async -> Fix? {
        guard hasWhenInUse else { return nil }
        let token = UUID()
        return await withCheckedContinuation { (c: CheckedContinuation<Fix?, Never>) in
            fixWaiters[token] = c
            manager.desiredAccuracy = precise ? kCLLocationAccuracyBest : kCLLocationAccuracyReduced
            manager.requestLocation()
            Task { @MainActor [weak self] in
                try? await Task.sleep(nanoseconds: Self.fixTimeout)
                self?.fixWaiters.removeValue(forKey: token)?.resume(returning: nil)
            }
        }
    }

    /// City and region for a point; empty when the lookup fails.
    func place(lat: Double, lon: Double) async -> (city: String?, region: String?) {
        let names: [String?] = await withCheckedContinuation { (c: CheckedContinuation<[String?], Never>) in
            geocoder.reverseGeocodeLocation(CLLocation(latitude: lat, longitude: lon)) { @Sendable marks, _ in
                let mark = marks?.first
                c.resume(returning: [mark?.locality, mark?.administrativeArea ?? mark?.country])
            }
        }
        return (names[0], names[1])
    }

    fileprivate func authorizationChanged() {
        guard manager.authorizationStatus != .notDetermined else { return }
        let waiters = authWaiters
        authWaiters.removeAll()
        waiters.forEach { $0.resume() }
    }

    fileprivate func deliver(_ fix: Fix?) {
        let waiters = fixWaiters
        fixWaiters.removeAll()
        waiters.values.forEach { $0.resume(returning: fix) }
    }
}

extension DeviceLocator: CLLocationManagerDelegate {
    nonisolated func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
        MainActor.assumeIsolated { self.authorizationChanged() }
    }

    nonisolated func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        guard let last = locations.last else { return }
        let fix = Fix(lat: last.coordinate.latitude, lon: last.coordinate.longitude, accuracy: last.horizontalAccuracy)
        MainActor.assumeIsolated { self.deliver(fix) }
    }

    nonisolated func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {
        // "Unknown for now" is not final: keep waiting until the timeout.
        if (error as? CLError)?.code == .locationUnknown { return }
        MainActor.assumeIsolated { self.deliver(nil) }
    }
}
