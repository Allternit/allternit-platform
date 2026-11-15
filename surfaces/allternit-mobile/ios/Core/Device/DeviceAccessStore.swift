import Combine
import Contacts
import EventKit
import Foundation

/// The three things a bot can reach on the phone through Device Access.
/// No Health, by design.
enum DeviceAccessKind: String, CaseIterable, Identifiable, Sendable {
    case calendar, reminders, contacts

    var id: String { rawValue }

    var title: String {
        switch self {
        case .calendar: return "Calendar"
        case .reminders: return "Reminders"
        case .contacts: return "Contacts"
        }
    }

    var systemImage: String {
        switch self {
        case .calendar: return "calendar"
        case .reminders: return "checklist"
        case .contacts: return "person.crop.circle"
        }
    }

    var detail: String {
        switch self {
        case .calendar: return "Bots can read your events and add new ones. Each new event needs your approval on your computer."
        case .reminders: return "Bots can read your reminders and add new ones. Each new reminder needs your approval on your computer."
        case .contacts: return "Bots can search your contacts by name. Read only."
        }
    }
}

/// What iOS itself says about the app's access, apart from our own switch.
enum DeviceSystemAccess: Equatable, Sendable {
    case notDetermined, granted, denied, restricted

    var isDeniedByUser: Bool { self == .denied || self == .restricted }
}

/// Per-type switches (off by default, saved on this device) plus the real iOS
/// permission state. A type reaches a bot only when the switch is on AND iOS
/// has granted access; the Desktop and the node are told exactly that.
@MainActor
final class DeviceAccessStore: ObservableObject {
    static let shared = DeviceAccessStore()

    @Published private(set) var enabled: [DeviceAccessKind: Bool] = [:]
    @Published private(set) var system: [DeviceAccessKind: DeviceSystemAccess] = [:]

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        for kind in DeviceAccessKind.allCases {
            enabled[kind] = defaults.bool(forKey: Self.key(kind))
        }
        refresh()
    }

    private static func key(_ kind: DeviceAccessKind) -> String { "deviceAccess.\(kind.rawValue)" }

    /// Re-reads the iOS permission state (call when the app becomes active:
    /// the person may have changed it in the Settings app).
    func refresh() {
        for kind in DeviceAccessKind.allCases {
            system[kind] = Self.systemAccess(kind)
        }
        // A permission revoked in iOS Settings turns our switch off too.
        for kind in DeviceAccessKind.allCases where enabled[kind] == true && system[kind] != .granted {
            store(kind, false)
        }
        AppPhoneService.shared.deviceAccessChanged()
    }

    /// True when a bot may use this type right now.
    func isAllowed(_ kind: DeviceAccessKind) -> Bool {
        enabled[kind] == true && Self.systemAccess(kind) == .granted
    }

    /// Both gates for the control socket's `state` frame.
    var allowedSnapshot: [String: Bool] {
        Dictionary(uniqueKeysWithValues: DeviceAccessKind.allCases.map { ($0.rawValue, isAllowed($0)) })
    }

    /// Turning a type on asks iOS the first time. Denied leaves it off.
    func setEnabled(_ kind: DeviceAccessKind, _ on: Bool) async {
        guard on else {
            store(kind, false)
            refresh()
            return
        }
        var state = Self.systemAccess(kind)
        if state == .notDetermined {
            _ = await Self.request(kind)
            state = Self.systemAccess(kind)
        }
        store(kind, state == .granted)
        refresh()
    }

    /// Signing out or unpairing: nothing stays switched on.
    func disableAll() {
        for kind in DeviceAccessKind.allCases { store(kind, false) }
        refresh()
    }

    private func store(_ kind: DeviceAccessKind, _ value: Bool) {
        enabled[kind] = value
        defaults.set(value, forKey: Self.key(kind))
    }

    // MARK: - iOS permission state

    nonisolated static func systemAccess(_ kind: DeviceAccessKind) -> DeviceSystemAccess {
        switch kind {
        case .calendar: return map(EKEventStore.authorizationStatus(for: .event))
        case .reminders: return map(EKEventStore.authorizationStatus(for: .reminder))
        case .contacts:
            switch CNContactStore.authorizationStatus(for: .contacts) {
            case .authorized: return .granted
            case .notDetermined: return .notDetermined
            case .restricted: return .restricted
            case .denied: return .denied
            @unknown default:
                // iOS 18 "limited" access still lets a search run.
                return CNContactStore.authorizationStatus(for: .contacts).rawValue == 4 ? .granted : .denied
            }
        }
    }

    /// Only full access reads events; write-only is not enough for list.
    private nonisolated static func map(_ status: EKAuthorizationStatus) -> DeviceSystemAccess {
        switch status {
        case .fullAccess: return .granted
        case .notDetermined: return .notDetermined
        case .restricted: return .restricted
        case .denied, .writeOnly: return .denied
        case .authorized: return .granted
        @unknown default: return .denied
        }
    }

    private nonisolated static func request(_ kind: DeviceAccessKind) async -> Bool {
        switch kind {
        case .calendar: return (try? await EKEventStore().requestFullAccessToEvents()) ?? false
        case .reminders: return (try? await EKEventStore().requestFullAccessToReminders()) ?? false
        case .contacts: return (try? await CNContactStore().requestAccess(for: .contacts)) ?? false
        }
    }
}
