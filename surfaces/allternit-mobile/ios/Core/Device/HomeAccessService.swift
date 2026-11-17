import Foundation
import HomeKit

/// Home for bots (`device.home.list`, `device.home.set`, `device.home.scene`;
/// contract §3.1 `phone.home.*`): the person's homes, rooms, accessories and
/// scenes through HomeKit, Apple's public Home framework.
///
/// Approval is not decided here. Locks, garage doors, alarms and cameras are
/// approved on the person's computer before the request reaches the phone
/// (contract §8, phone-route.ts `approvalClass`), the same path as calendar
/// writes. This type reports each accessory's `kind` and each scene's
/// `security` so that check knows what it is approving.
///
/// HomeKit needs the `com.apple.developer.homekit` entitlement, which the
/// owner enables on the App ID (plan gate G2). Until then the
/// `AllternitHomeKitEnabled` Info.plist key is false and Home reports itself
/// unavailable instead of asking for access it can't use.
@MainActor
final class HomeAccessService: NSObject {
    static let shared = HomeAccessService()

    /// Kinds a caller may filter `device.home.list` by (contract §3.1).
    nonisolated static let listKinds: Set<String> = ["light", "switch", "lock", "thermostat", "garage", "sensor", "scene"]
    /// Kinds whose changes always need approval (phone-route.ts SECURITY_HOME_KINDS).
    nonisolated static let securityKinds: Set<String> = ["lock", "garage", "security", "alarm", "camera"]

    private nonisolated static let accessKey = "deviceAccess.home.authorization"
    /// HomeKit loads homes in the background; a call waits at most this long.
    private static let readyTimeout: TimeInterval = 10
    /// Read-back after a write.
    private static let readTimeout: TimeInterval = 5

    private var manager: HMHomeManager?
    private var ready = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    /// The build carries the HomeKit entitlement (gate G2 done).
    nonisolated static var isBuiltIn: Bool {
        (Bundle.main.object(forInfoDictionaryKey: "AllternitHomeKitEnabled") as? Bool) == true
    }

    /// What iOS last said about Home access. HomeKit only reports it from a
    /// live home manager, and making one asks the person, so the last answer
    /// is kept and refreshed whenever a manager is running.
    nonisolated static var cachedAccess: DeviceSystemAccess {
        guard isBuiltIn else { return .restricted }
        switch UserDefaults.standard.string(forKey: accessKey) {
        case "granted": return .granted
        case "denied": return .denied
        case "restricted": return .restricted
        default: return .notDetermined
        }
    }

    // MARK: - Access

    /// Starts HomeKit (iOS asks the person the first time) and waits for it.
    func requestAccess() async -> Bool {
        guard Self.isBuiltIn else { return false }
        await start(timeout: 120)
        return Self.cachedAccess == .granted
    }

    private func start(timeout: TimeInterval) async {
        if manager == nil {
            let m = HMHomeManager()
            m.delegate = self
            manager = m
        }
        guard !ready else { return }
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            waiters.append(continuation)
            Task { [weak self] in
                try? await Task.sleep(for: .seconds(timeout))
                self?.markReady()
            }
        }
    }

    private func markReady() {
        ready = true
        let waiting = waiters
        waiters = []
        for w in waiting { w.resume() }
    }

    private func record(_ manager: HMHomeManager) {
        let status = manager.authorizationStatus
        let value: String?
        if status.contains(.authorized) { value = "granted" }
        else if status.contains(.restricted) { value = "restricted" }
        else if status.contains(.determined) { value = "denied" }
        else { value = nil }
        let before = UserDefaults.standard.string(forKey: Self.accessKey)
        if let value { UserDefaults.standard.set(value, forKey: Self.accessKey) }
        if before != value { DeviceAccessStore.shared.refresh() }
    }

    /// A running, allowed HomeKit, or the error to send back.
    private func readyManager() async throws -> HMHomeManager {
        guard Self.isBuiltIn else {
            throw AppPhoneRPCError(code: "not_supported", message: "Home needs the next Allternit iPhone app update.")
        }
        await start(timeout: Self.readyTimeout)
        guard let manager, Self.cachedAccess == .granted else {
            throw AppPhoneRPCError(code: "permission_denied", message: "iOS access to Home is not allowed for Allternit.")
        }
        return manager
    }

    // MARK: - List

    func list(kind: String?) async throws -> [String: Any] {
        if let kind, !Self.listKinds.contains(kind) {
            return PhoneUseEnvelope.failure("bad_params", "kind must be one of: \(Self.listKinds.sorted().joined(separator: ", ")).")
        }
        let manager = try await readyManager()
        var devices: [[String: Any]] = []
        var scenes: [[String: Any]] = []
        for home in manager.homes {
            if kind != "scene" {
                for accessory in home.accessories {
                    let k = Self.kind(of: accessory)
                    if let kind, kind != k { continue }
                    devices.append(row(accessory, kind: k, home: home))
                }
            }
            if kind == nil || kind == "scene" {
                for set in home.actionSets {
                    scenes.append([
                        "id": set.uniqueIdentifier.uuidString,
                        "name": set.name,
                        "home": home.name,
                        "security": Self.touchesSecurity(set),
                    ])
                }
            }
        }
        return PhoneUseEnvelope.ok(["devices": devices, "scenes": scenes])
    }

    /// State comes from HomeKit's last known values (no read per accessory:
    /// an unreachable one would hold the whole list). `set` reads back live.
    private func row(_ accessory: HMAccessory, kind: String, home: HMHome) -> [String: Any] {
        [
            "id": accessory.uniqueIdentifier.uuidString,
            "name": accessory.name,
            "room": accessory.room?.name ?? "",
            "home": home.name,
            "kind": kind,
            "reachable": accessory.isReachable,
            "state": Self.state(of: accessory, keys: StateKey.allCases),
        ]
    }

    // MARK: - Set

    func set(deviceId: String, state: [String: Any]) async throws -> [String: Any] {
        guard !state.isEmpty else { return PhoneUseEnvelope.failure("bad_params", "state needs at least one value, like {\"on\": true}.") }
        let manager = try await readyManager()
        guard let accessory = Self.accessory(deviceId, in: manager) else {
            return PhoneUseEnvelope.failure("unknown_device", "No Home accessory has that id. Call phone.home.list first.")
        }
        var writes: [(HMCharacteristic, NSNumber)] = []
        var keys: [StateKey] = []
        for (name, value) in state.sorted(by: { $0.key < $1.key }) {
            guard let key = StateKey(rawValue: name), let type = key.writeType else {
                return PhoneUseEnvelope.failure("bad_params", "\(name) can't be set. Use: \(StateKey.writable.map(\.rawValue).joined(separator: ", ")).")
            }
            guard let characteristic = Self.characteristic(type, on: accessory) else {
                return PhoneUseEnvelope.failure("not_supported", "\(accessory.name) has no \(name).")
            }
            guard let number = key.encode(value) else {
                return PhoneUseEnvelope.failure("bad_params", "\(name) has a value \(accessory.name) can't take.")
            }
            writes.append((characteristic, number))
            keys.append(key)
        }
        guard accessory.isReachable else {
            return PhoneUseEnvelope.failure("unreachable", "\(accessory.name) is not responding. Check it in the Home app.")
        }
        do {
            for (characteristic, number) in writes { try await Self.write(characteristic, number) }
        } catch {
            return PhoneUseEnvelope.failure("home_failed", error.localizedDescription)
        }
        let readBack = keys.compactMap { Self.characteristic($0.readType, on: accessory) }
        await Self.read(readBack, timeout: Self.readTimeout)
        let now = Self.state(of: accessory, keys: keys)
        let verified = keys.allSatisfy { key in key.matches(requested: state[key.rawValue], now: now[key.rawValue]) }
        return PhoneUseEnvelope.ok(
            ["id": deviceId, "name": accessory.name, "kind": Self.kind(of: accessory), "state": now],
            verified: verified)
    }

    // MARK: - Scene

    func runScene(id: String) async throws -> [String: Any] {
        let manager = try await readyManager()
        for home in manager.homes {
            guard let set = home.actionSets.first(where: { $0.uniqueIdentifier.uuidString.caseInsensitiveCompare(id) == .orderedSame }) else { continue }
            do {
                try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                    home.executeActionSet(set) { error in
                        if let error { continuation.resume(throwing: error) } else { continuation.resume() }
                    }
                }
            } catch {
                return PhoneUseEnvelope.failure("home_failed", error.localizedDescription)
            }
            return PhoneUseEnvelope.ok(["id": id, "name": set.name, "ran": true])
        }
        return PhoneUseEnvelope.failure("unknown_scene", "No Home scene has that id. Call phone.home.list first.")
    }

    // MARK: - HomeKit helpers

    private static func accessory(_ id: String, in manager: HMHomeManager) -> HMAccessory? {
        for home in manager.homes {
            if let a = home.accessories.first(where: { $0.uniqueIdentifier.uuidString.caseInsensitiveCompare(id) == .orderedSame }) {
                return a
            }
        }
        return nil
    }

    private static func characteristic(_ type: String, on accessory: HMAccessory) -> HMCharacteristic? {
        for service in accessory.services {
            if let c = service.characteristics.first(where: { $0.characteristicType == type }) { return c }
        }
        return nil
    }

    private static func write(_ characteristic: HMCharacteristic, _ value: NSNumber) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            characteristic.writeValue(value) { error in
                if let error { continuation.resume(throwing: error) } else { continuation.resume() }
            }
        }
    }

    /// Reads all of `characteristics`, giving up after `timeout`.
    private static func read(_ characteristics: [HMCharacteristic], timeout: TimeInterval) async {
        guard !characteristics.isEmpty else { return }
        let batch = ReadBatch(count: characteristics.count)
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            batch.continuation = continuation
            for c in characteristics {
                c.readValue { _ in DispatchQueue.main.async { batch.one() } }
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + timeout) { batch.finish() }
        }
    }

    /// Security first, from the services: a lock on a bridge whose category
    /// says "Other" must still be a lock, so its changes stay approved.
    static func kind(of accessory: HMAccessory) -> String {
        let services = Set(accessory.services.map(\.serviceType))
        if services.contains(HMServiceTypeLockMechanism) { return "lock" }
        if services.contains(HMServiceTypeGarageDoorOpener) { return "garage" }
        if services.contains(HMServiceTypeSecuritySystem) { return "security" }
        if services.contains(HMServiceTypeCameraRTPStreamManagement) || services.contains(HMServiceTypeDoorbell) { return "camera" }
        switch accessory.category.categoryType {
        case HMAccessoryCategoryTypeLightbulb: return "light"
        case HMAccessoryCategoryTypeSwitch, HMAccessoryCategoryTypeOutlet, HMAccessoryCategoryTypeProgrammableSwitch: return "switch"
        case HMAccessoryCategoryTypeThermostat: return "thermostat"
        case HMAccessoryCategoryTypeSensor: return "sensor"
        case HMAccessoryCategoryTypeFan: return "fan"
        case HMAccessoryCategoryTypeWindowCovering: return "blind"
        default: break
        }
        if services.contains(HMServiceTypeLightbulb) { return "light" }
        if services.contains(HMServiceTypeSwitch) || services.contains(HMServiceTypeOutlet) { return "switch" }
        if services.contains(HMServiceTypeThermostat) { return "thermostat" }
        return "other"
    }

    /// A scene that writes to a lock, garage door, alarm or camera.
    static func touchesSecurity(_ set: HMActionSet) -> Bool {
        for action in set.actions {
            guard let write = action as? HMCharacteristicWriteAction<NSCopying> else { continue }
            let type = write.characteristic.characteristicType
            if type == HMCharacteristicTypeTargetLockMechanismState || type == HMCharacteristicTypeTargetDoorState
                || type == HMCharacteristicTypeTargetSecuritySystemState { return true }
            if let accessory = write.characteristic.service?.accessory, securityKinds.contains(kind(of: accessory)) { return true }
        }
        return false
    }

    static func state(of accessory: HMAccessory, keys: [StateKey]) -> [String: Any] {
        var out: [String: Any] = [:]
        for key in keys {
            guard let c = characteristic(key.readType, on: accessory), let raw = c.value,
                  let value = key.decode(raw) else { continue }
            out[key.rawValue] = value
        }
        return out
    }

    // MARK: - State keys

    /// The `state` object's keys and the HomeKit characteristic behind each.
    enum StateKey: String, CaseIterable, Sendable {
        case on, brightness, hue, saturation
        case colorTemperature = "color_temperature"
        case locked, door
        case targetTemperature = "target_temperature"
        case currentTemperature = "current_temperature"
        case position, active, armed

        static var writable: [StateKey] { allCases.filter { $0.writeType != nil } }

        var writeType: String? {
            switch self {
            case .on: return HMCharacteristicTypePowerState
            case .brightness: return HMCharacteristicTypeBrightness
            case .hue: return HMCharacteristicTypeHue
            case .saturation: return HMCharacteristicTypeSaturation
            case .colorTemperature: return HMCharacteristicTypeColorTemperature
            case .locked: return HMCharacteristicTypeTargetLockMechanismState
            case .door: return HMCharacteristicTypeTargetDoorState
            case .targetTemperature: return HMCharacteristicTypeTargetTemperature
            case .currentTemperature: return nil
            case .position: return HMCharacteristicTypeTargetPosition
            case .active: return HMCharacteristicTypeActive
            case .armed: return HMCharacteristicTypeTargetSecuritySystemState
            }
        }

        /// What is read back: the current state where HomeKit has one.
        var readType: String {
            switch self {
            case .locked: return HMCharacteristicTypeCurrentLockMechanismState
            case .door: return HMCharacteristicTypeCurrentDoorState
            case .currentTemperature: return HMCharacteristicTypeCurrentTemperature
            case .position: return HMCharacteristicTypeCurrentPosition
            case .armed: return HMCharacteristicTypeCurrentSecuritySystemState
            default: return writeType ?? HMCharacteristicTypePowerState
            }
        }

        private static let armModes = ["stay", "away", "night", "off"]

        /// A JSON value to the characteristic's number, or nil when it doesn't fit.
        func encode(_ value: Any) -> NSNumber? {
            switch self {
            case .on, .locked, .active:
                guard let b = value as? Bool else { return nil }
                return NSNumber(value: b ? 1 : 0)
            case .door:
                guard let s = value as? String else { return nil }
                switch s { case "open": return 0; case "closed": return 1; default: return nil }
            case .armed:
                guard let s = value as? String, let i = Self.armModes.firstIndex(of: s) else { return nil }
                return NSNumber(value: i)
            case .brightness, .saturation, .position:
                guard let d = value as? Double, (0...100).contains(d) else { return nil }
                return NSNumber(value: Int(d.rounded()))
            case .hue:
                guard let d = value as? Double, (0...360).contains(d) else { return nil }
                return NSNumber(value: Float(d))
            case .colorTemperature:
                guard let d = value as? Double, (50...500).contains(d) else { return nil }
                return NSNumber(value: Int(d.rounded()))
            case .targetTemperature:
                guard let d = value as? Double, (10...38).contains(d) else { return nil }
                return NSNumber(value: Float(d))
            case .currentTemperature:
                return nil
            }
        }

        /// The characteristic's value as the `state` JSON value.
        func decode(_ raw: Any) -> Any? {
            guard let n = raw as? NSNumber else { return nil }
            switch self {
            case .on, .active: return n.intValue != 0
            case .locked:
                // Current lock state: 0 unsecured, 1 secured, 2 jammed, 3 unknown.
                switch n.intValue { case 0: return false; case 1: return true; case 2: return "jammed"; default: return NSNull() }
            case .door:
                switch n.intValue { case 0: return "open"; case 1: return "closed"; case 2: return "opening"; case 3: return "closing"; default: return "stopped" }
            case .armed:
                let i = n.intValue
                return i < Self.armModes.count ? Self.armModes[i] : "triggered"
            case .hue, .targetTemperature, .currentTemperature:
                return (n.doubleValue * 10).rounded() / 10
            case .brightness, .saturation, .position, .colorTemperature:
                return n.intValue
            }
        }

        /// The read-back shows what was asked for.
        /// Requested values arrive from JSON as NSNumber; read-back values
        /// are Swift Bool / Int / Double / String (`decode`).
        func matches(requested: Any?, now: Any?) -> Bool {
            guard let requested, let now else { return false }
            switch self {
            case .on, .active, .locked:
                guard let a = requested as? Bool, let b = now as? Bool else { return false }
                return a == b
            case .door, .armed:
                guard let a = requested as? String, let b = now as? String else { return false }
                return a == b
            default:
                guard let a = (requested as? NSNumber)?.doubleValue, let b = (now as? NSNumber)?.doubleValue else { return false }
                return abs(a - b) <= 1
            }
        }
    }
}

extension HomeAccessService: @preconcurrency HMHomeManagerDelegate {
    func homeManagerDidUpdateHomes(_ manager: HMHomeManager) {
        record(manager)
        markReady()
    }

    func homeManager(_ manager: HMHomeManager, didUpdate status: HMHomeManagerAuthorizationStatus) {
        record(manager)
        // Answered "Don't Allow": no homes will come; stop waiting.
        if status.contains(.determined), !status.contains(.authorized) { markReady() }
    }
}

/// Counts finished reads on the main queue; resumes once, at the last read
/// or the timeout, whichever is first.
private final class ReadBatch: @unchecked Sendable {
    private var left: Int
    private var done = false
    var continuation: CheckedContinuation<Void, Never>?

    init(count: Int) { left = count }

    func one() {
        left -= 1
        if left <= 0 { finish() }
    }

    func finish() {
        guard !done else { return }
        done = true
        continuation?.resume()
        continuation = nil
    }
}
