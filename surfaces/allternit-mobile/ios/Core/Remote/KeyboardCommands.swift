import AllternitBTHID
import Foundation

// Phone Use route `bt-apple` (docs/contracts/CONTRACT-phone-use-2026-11-16.md
// §1, §5, §9.2): this iPhone or iPad, with keyboard mode on, is the Bluetooth
// keyboard and mouse ("Allternit Keyboard") of the person's MAIN iPhone.
// Desktop sends `hid.*` requests over this device's app-phone control socket
// (`{id, method, params}` → `{id, result}` or `{id, error:{code,message}}`),
// the same methods and error codes as the Android sender (`bt-android`,
// HidCommands.kt), so Desktop's lanes and the node's allow-list
// (cmd/allternit-api phone_app.rs RPC_METHODS) treat both alike.
//
// This file is the pure part (no CoreBluetooth): the state answer and the
// commands, over a `KeyboardLink` (KeyboardModeService on the device, a fake
// in tests). Input only ever goes out over Bluetooth to the connected host;
// nothing here can touch this device's own screen.
//
// Not here on purpose: starting or stopping the keyboard. That is the
// person's Start / Stop in Settings > Device Access (a user-started mode with
// a visible state), so `hid.pair.status` only reports it.

/// Keyboard mode as the person set it.
enum KeyboardMode: Equatable, Sendable {
    case off
    case starting
    case on
    /// Start failed (Bluetooth off, no permission, iOS refused the service…).
    case failed(BTHIDError)
}

/// Whether this device can act as a keyboard and mouse now. Wire values match
/// Android's `HidAvailability` where they mean the same thing.
enum KeyboardAvailability: String, Sendable {
    /// Published and advertising (or connected): a host can pair or send.
    case ready
    /// Keyboard mode is not started on this device.
    case off
    case starting
    case bluetoothOff = "bluetooth_off"
    case noPermission = "no_permission"
    /// No Bluetooth Low Energy on this device.
    case unsupported
    /// iOS refused the keyboard service (the undocumented 128-bit HID id).
    case refused
    case failed
}

/// The Bluetooth side as the commands see it (contract §6 `status()`).
struct KeyboardLinkState: Equatable, Sendable {
    var availability: KeyboardAvailability
    /// A host (the main iPhone) is connected and subscribed to input.
    var connected: Bool
    var advertising: Bool
    /// Why it can't send, for every availability but `ready`.
    var message: String?

    static let notPairedMessage = "Your iPhone is not connected to the Allternit Keyboard on your other device."
    static let offMessage = "Turn on Be a keyboard for another iPhone in the Allternit app on your other iPhone or iPad (Settings > Device Access)."
    static let refusedMessage = "Not available on this iPhone or iPad: iOS refused the keyboard service; an Apple update may have changed this."

    /// Mode plus the peripheral's status → availability (state mapping, contract §5).
    static func from(mode: KeyboardMode, status: HIDPeripheralStatus) -> KeyboardLinkState {
        func unavailable(_ a: KeyboardAvailability, _ message: String) -> KeyboardLinkState {
            KeyboardLinkState(availability: a, connected: false, advertising: false, message: message)
        }
        func fromError(_ e: BTHIDError) -> KeyboardLinkState {
            switch e.code {
            case "refused": return unavailable(.refused, refusedMessage)
            case "bluetooth_off": return unavailable(.bluetoothOff, "Turn on Bluetooth on your other iPhone or iPad.")
            case "unauthorized": return unavailable(.noPermission, "Allow Bluetooth for Allternit on your other iPhone or iPad (Settings > Privacy & Security > Bluetooth).")
            case "unsupported": return unavailable(.unsupported, "Your other device has no Bluetooth Low Energy.")
            default: return unavailable(.failed, e.message)
            }
        }
        switch mode {
        case .off:
            return unavailable(.off, offMessage)
        case .starting:
            return unavailable(.starting, "The Allternit Keyboard on your other device is starting.")
        case .failed(let e):
            return fromError(e)
        case .on:
            if status.refused { return unavailable(.refused, refusedMessage) }
            if status.authorization == .denied || status.authorization == .restricted {
                return fromError(BTHIDError("unauthorized", ""))
            }
            if let e = radioError(status.radio), status.radio != .notStarted {
                return fromError(e)
            }
            // On, but the keyboard is not published right now (Bluetooth came back and it is being published again).
            guard status.servicesReady else { return unavailable(.starting, "The Allternit Keyboard on your other device is starting.") }
            return KeyboardLinkState(availability: .ready, connected: status.connected, advertising: status.advertising, message: nil)
        }
    }

    /// Contract §5: the needs-you code when the person can fix it on the main iPhone.
    var needsYou: String? { availability == .ready && !connected ? "BT_NOT_PAIRED" : nil }

    /// The `hid.pair.status` answer. A peripheral is never told its host's
    /// name, so there is no `name`; paired = connected (a host subscribes
    /// only after bonding).
    var json: [String: Any] {
        var out: [String: Any] = [
            "paired": connected,
            "connected": connected,
            "available": availability == .ready,
            "availability": availability.rawValue,
            "advertising": advertising,
            "sender": "bt-apple",
        ]
        if let needsYou { out["needs_you"] = needsYou }
        if let message { out["message"] = message }
        return out
    }
}

/// The Bluetooth HID link (KeyboardModeService on the device).
@MainActor
protocol KeyboardLink: AnyObject {
    var linkState: KeyboardLinkState { get }
    /// Sends reports in order to the connected host, each waiting at most
    /// `timeout` seconds for room. Throws `BTHIDError`.
    func send(_ outputs: [HIDOutput], timeout: Double) async throws
}

/// The `hid.*` methods. Holds what is pressed so every report carries the
/// whole state (HIDInputState). Commands run one at a time, in arrival order.
@MainActor
final class KeyboardCommands {
    nonisolated static let methods: Set<String> = [
        "hid.pair.status", "hid.pointer.move", "hid.pointer.click", "hid.pointer.reset",
        "hid.key", "hid.type", "hid.scroll",
    ]
    static let maxText = 500
    static let maxMove = 32_767
    static let maxScrollLines = 1_000

    private unowned let link: KeyboardLink
    private(set) var input = HIDInputState()
    private var busy = false
    private var waiting: [CheckedContinuation<Void, Never>] = []

    init(link: KeyboardLink) {
        self.link = link
    }

    nonisolated static func handles(_ method: String) -> Bool { method.hasPrefix("hid.") }

    /// Runs one method; waits for the one before it to finish first.
    func handle(method: String, params: [String: Any]) async throws -> [String: Any] {
        await acquire()
        defer { release() }
        return try await run(method: method, params: params)
    }

    /// Lets go of every key and button (contract §9.2: the lane dropped or the
    /// app went to the background). Never throws; sends only when something
    /// is held and a host is connected.
    func releaseAll() async {
        await acquire()
        defer { release() }
        let held = input.modifiers != 0 || !input.keys.isEmpty || input.buttons != 0
        let reports = input.releaseAll()
        guard held, link.linkState.connected else { return }
        try? await link.send(reports, timeout: 0.5)
    }

    private func run(method: String, params: [String: Any]) async throws -> [String: Any] {
        if method == "hid.pair.status" { return link.linkState.json }
        guard Self.methods.contains(method) else {
            throw AppPhoneRPCError(code: "unknown_method", message: "Unknown method \(method)")
        }
        try ready()
        switch method {
        case "hid.pointer.move":
            let dx = try int(params, "dx", -Self.maxMove...Self.maxMove)
            let dy = try int(params, "dy", -Self.maxMove...Self.maxMove)
            try await send(input.move(dx: dx, dy: dy))
        case "hid.pointer.click":
            let name = params["button"] == nil ? "left" : params["button"] as? String
            guard let name, name == "left" || name == "right", let b = MouseButton(rawValue: name) else {
                throw invalid("button must be left or right.")
            }
            let down = try flag(params, "down", true)
            let up = try flag(params, "up", true)
            guard down || up else { throw invalid("down, up or both must be true.") }
            try await send(input.button(b, down: down, up: up))
        case "hid.pointer.reset":
            // Let go of any held button first, so the sweep is not a drag.
            var out: [HIDOutput] = []
            for b in MouseButton.allCases where input.buttons & b.bit != 0 {
                out += input.button(b, down: false, up: true)
            }
            try await send(out + input.cornerReset())
        case "hid.scroll":
            let lines = try int(params, "lines", -Self.maxScrollLines...Self.maxScrollLines)
            try await send(input.scroll(lines: lines))
        case "hid.key":
            let usage = try int(params, "usage", 0...Int(HIDReportMap.maxKeyUsage))
            let mods = try params["modifiers"] == nil ? 0 : int(params, "modifiers", 0...0xFF)
            guard let n = params["down"] as? NSNumber, CFGetTypeID(n) == CFBooleanGetTypeID() else {
                throw invalid("down must be true or false.")
            }
            let reports: [HIDOutput]
            do {
                reports = try input.key(usage: usage, modifiers: mods, down: n.boolValue)
            } catch let e as HIDInputError {
                throw AppPhoneRPCError(code: "bad_params", message: e.message)
            }
            try await send(reports)
        case "hid.type":
            guard let text = params["text"] as? String else { throw invalid("text is required.") }
            guard !text.isEmpty, text.count <= Self.maxText else { throw invalid("text must be 1-\(Self.maxText) characters.") }
            if let c = USLayout.untypable(text) {
                throw AppPhoneRPCError(code: "bad_params", message: "The keyboard can't type \"\(c)\": only printable ASCII on a US layout can be sent.")
            }
            guard let reports = input.type(text) else {
                throw AppPhoneRPCError(code: "bad_params", message: "The keyboard can't type that text.")
            }
            try await send(reports)
            return ["sent": text.count]
        default:
            throw AppPhoneRPCError(code: "unknown_method", message: "Unknown method \(method)")
        }
        return [:]
    }

    private func ready() throws {
        let s = link.linkState
        if s.availability != .ready {
            throw AppPhoneRPCError(code: "hid_unavailable", message: s.message ?? KeyboardLinkState.offMessage)
        }
        if !s.connected { throw AppPhoneRPCError(code: "bt_not_paired", message: KeyboardLinkState.notPairedMessage) }
    }

    private func send(_ reports: [HIDOutput]) async throws {
        do {
            try await link.send(reports, timeout: 2)
        } catch let e as BTHIDError {
            if e.code == "not_connected" || !link.linkState.connected {
                throw AppPhoneRPCError(code: "bt_not_paired", message: KeyboardLinkState.notPairedMessage)
            }
            throw AppPhoneRPCError(code: "hid_failed", message: e.message)
        } catch is CancellationError {
            throw AppPhoneRPCError(code: "hid_failed", message: "The keyboard command was cancelled.")
        }
    }

    // ── params ──────────────────────────────────────────────────────────────

    private func invalid(_ message: String) -> AppPhoneRPCError {
        AppPhoneRPCError(code: "invalid_params", message: message)
    }

    /// Whole numbers only; booleans (NSNumber too) and fractions are refused.
    private func int(_ p: [String: Any], _ name: String, _ range: ClosedRange<Int>) throws -> Int {
        guard let n = p[name] as? NSNumber, CFGetTypeID(n) != CFBooleanGetTypeID() else {
            throw invalid("\(name) must be a whole number.")
        }
        let v = n.doubleValue
        guard v.isFinite, v.rounded() == v, range.contains(Int(exactly: v) ?? Int.max) else {
            throw invalid("\(name) must be a whole number from \(range.lowerBound) to \(range.upperBound).")
        }
        return Int(v)
    }

    private func flag(_ p: [String: Any], _ name: String, _ dflt: Bool) throws -> Bool {
        guard let v = p[name] else { return dflt }
        guard let n = v as? NSNumber, CFGetTypeID(n) == CFBooleanGetTypeID() else {
            throw invalid("\(name) must be true or false.")
        }
        return n.boolValue
    }

    // ── one command at a time ───────────────────────────────────────────────

    private func acquire() async {
        if !busy {
            busy = true
            return
        }
        await withCheckedContinuation { waiting.append($0) }
    }

    private func release() {
        if waiting.isEmpty { busy = false } else { waiting.removeFirst().resume() }
    }
}
