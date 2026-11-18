import AllternitBTHID
import Foundation
import UIKit

/// Keyboard mode (Phone Use route `bt-apple`, plan step 13): this iPhone or
/// iPad shows up as a Bluetooth keyboard and mouse called "Allternit
/// Keyboard". The person pairs it in their MAIN iPhone's Settings >
/// Bluetooth; Desktop then moves the pointer, clicks and types there through
/// `hid.*` requests on this device's app-phone control socket
/// (KeyboardCommands), and watches the result on the main iPhone's screen
/// broadcast. Every send is still approved on the person's computer before
/// the request reaches this device.
///
/// - Only the person starts it (Start in Settings > Device Access) and it
///   shows a banner with Stop while it runs (KeyboardModeBanner). A request
///   from Desktop can never start it.
/// - It controls another device only: input goes out over Bluetooth to the
///   connected host. Nothing here touches this device's own screen.
/// - `bluetooth-peripheral` background mode keeps the keyboard published and
///   the main iPhone connected when this device locks or the person switches
///   apps. Requests arrive only while the Allternit app is open (the control
///   socket closes in the background), so the screen is kept awake while
///   keyboard mode runs in the foreground.
/// - Held keys and buttons are let go when the control socket drops, the app
///   goes to the background, the host disconnects, or the person stops it
///   (contract §9.2).
///
/// UNDOCUMENTED (accepted by Eoj 2026-11-17): CoreBluetooth accepts the HID
/// service only under its 128-bit Base UUID form (AllternitBTHID). If iOS
/// refuses it, the mode says "Not available on this iPhone or iPad" and the
/// assistant uses another route. NOT CHECKED ON HARDWARE (plan check P5).
/// The iPhone this device is the keyboard for. CoreBluetooth gives a
/// peripheral only a per-app id for its host, never a name, so the person
/// picks it once in Settings > Device Access from the account's iPhones.
struct KeyboardTarget: Codable, Equatable, Sendable, Identifiable {
    /// `app:<phoneId>`, as the phone listing names it.
    let serial: String
    let name: String
    var id: String { serial }

    /// The pick to keep, given the account's other iPhones: the saved one
    /// (with its current name) while it is still listed, else the only
    /// iPhone there is, else none (the person picks).
    static func resolve(saved: KeyboardTarget?, iphones: [KeyboardTarget]) -> KeyboardTarget? {
        if let saved, let listed = iphones.first(where: { $0.serial == saved.serial }) { return listed }
        return iphones.count == 1 ? iphones[0] : nil
    }
}

@MainActor
final class KeyboardModeService: ObservableObject, KeyboardLink {
    static let shared = KeyboardModeService()
    static let localName = "Allternit Keyboard"

    @Published private(set) var mode: KeyboardMode = .off {
        // The node lists this device as a keyboard (`keyboard_for`) from the app's state frame.
        didSet { if Self.running(oldValue) != isRunning { AppPhoneService.shared.keyboardModeChanged() } }
    }
    @Published private(set) var status = HIDPeripheralStatus() {
        didSet { if oldValue.connected != status.connected { AppPhoneService.shared.keyboardModeChanged() } }
    }
    /// The iPhone this is the keyboard for (the person's pick, kept across launches).
    @Published private(set) var target: KeyboardTarget? = KeyboardModeService.savedTarget() {
        didSet { if oldValue?.serial != target?.serial { AppPhoneService.shared.keyboardModeChanged() } }
    }
    nonisolated private static let targetKey = "allternit.keyboardMode.target"

    /// Created on first Start, so the Bluetooth permission prompt appears only then.
    private var peripheral: HIDPeripheral?
    private lazy var commands = KeyboardCommands(link: self)
    private var startTask: Task<Void, Never>?
    /// A publish (start) is in flight.
    private var publishing = false
    private var isForeground = true
    /// We turned the idle timer off (and so must turn it back on).
    private var holdsScreenAwake = false

    private init() {}

    /// Keyboard mode ships in this build (Info.plist `AllternitKeyboardModeEnabled`).
    /// False for the App Store build: the Settings section, the banner and the
    /// Bluetooth peripheral stay off, and `hid.*` answers `unavailable`. The one
    /// place the key is read.
    nonisolated static var isBuiltIn: Bool {
        (Bundle.main.object(forInfoDictionaryKey: "AllternitKeyboardModeEnabled") as? Bool) == true
    }

    /// Started (or starting) by the person.
    var isRunning: Bool { Self.running(mode) }

    private static func running(_ mode: KeyboardMode) -> Bool {
        switch mode {
        case .on, .starting: return true
        case .off, .failed: return false
        }
    }

    /// For the app's `state` frame: keyboard mode on, an iPhone connected to
    /// it, and which iPhone it is for (`keyboard_for`, a serial) once picked.
    var stateSnapshot: [String: Any] {
        var out: [String: Any] = ["on": isRunning, "connected": isRunning && status.connected]
        if let target { out["keyboard_for"] = target.serial }
        return out
    }

    var linkState: KeyboardLinkState {
        var s = KeyboardLinkState.from(mode: mode, status: status)
        s.keyboardFor = target?.serial
        return s
    }

    /// The person's answer to "Which iPhone is this the keyboard for?" (nil clears it).
    func setTarget(_ new: KeyboardTarget?) {
        if let new, let data = try? JSONEncoder().encode(new) {
            UserDefaults.standard.set(data, forKey: Self.targetKey)
        } else {
            UserDefaults.standard.removeObject(forKey: Self.targetKey)
        }
        if new != target { target = new }
    }

    nonisolated private static func savedTarget() -> KeyboardTarget? {
        guard let data = UserDefaults.standard.data(forKey: targetKey) else { return nil }
        return try? JSONDecoder().decode(KeyboardTarget.self, from: data)
    }

    // MARK: - Start / Stop (the person's)

    func start() {
        guard Self.isBuiltIn, !isRunning else { return }
        mode = .starting
        publish()
    }

    func stop() {
        startTask?.cancel()
        startTask = nil
        publishing = false
        guard let p = peripheral, mode != .off else {
            mode = .off
            updateScreenAwake()
            return
        }
        Task {
            await commands.releaseAll()
            p.stop()
            mode = .off
            status = p.currentStatus()
            updateScreenAwake()
        }
    }

    // MARK: - The app-phone lane

    /// A `hid.*` request from Desktop (AppPhoneService).
    func handle(method: String, params: [String: Any]) async throws -> Any {
        guard Self.isBuiltIn else {
            throw AppPhoneRPCError(code: "unavailable", message: "Keyboard mode isn't available in this version of the Allternit app.")
        }
        return try await commands.handle(method: method, params: params)
    }

    /// The control socket closed or the app went to the background.
    func laneDropped() {
        guard peripheral != nil else { return }
        Task { await commands.releaseAll() }
    }

    func setForeground(_ foreground: Bool) {
        isForeground = foreground
        if !foreground { laneDropped() }
        updateScreenAwake()
    }

    // MARK: - KeyboardLink

    func send(_ outputs: [HIDOutput], timeout: Double) async throws {
        guard let p = peripheral else {
            throw BTHIDError("not_connected", KeyboardLinkState.notPairedMessage)
        }
        try await p.send(outputs, timeout: timeout)
    }

    // MARK: - Internals

    private func publish() {
        let p = peripheral ?? makePeripheral()
        startTask?.cancel()
        publishing = true
        startTask = Task { [weak self] in
            var failure: BTHIDError?
            do {
                try await p.start()
            } catch {
                failure = error as? BTHIDError ?? BTHIDError("failed", error.localizedDescription)
            }
            guard let self else { return p.stop() }
            if Task.isCancelled || !self.isRunning {
                // Stopped while starting: never leave a keyboard published that nobody sees.
                p.stop()
                return
            }
            self.publishing = false
            if let failure {
                // Take down whatever was published before the failure; the mode keeps the reason.
                p.stop()
                self.mode = .failed(failure)
            } else {
                self.mode = .on
            }
            self.status = p.currentStatus()
            self.updateScreenAwake()
        }
        status = p.currentStatus()
        updateScreenAwake()
    }

    private func makePeripheral() -> HIDPeripheral {
        let p = HIDPeripheral(localName: Self.localName)
        p.onChange = { status in
            Task { @MainActor in KeyboardModeService.shared.statusChanged(status) }
        }
        peripheral = p
        return p
    }

    private func statusChanged(_ new: HIDPeripheralStatus) {
        let wasConnected = status.connected
        status = new
        // The host went away: whatever was held is gone on its side too.
        if wasConnected, !new.connected { Task { await commands.releaseAll() } }
        // Bluetooth was switched off and on again while running: publish again.
        if mode == .on, new.radio == .on, !new.servicesReady, !publishing {
            mode = .starting
            publish()
        }
    }

    private func updateScreenAwake() {
        let want = isRunning && isForeground
        guard want != holdsScreenAwake else { return }
        holdsScreenAwake = want
        UIApplication.shared.isIdleTimerDisabled = want
    }
}
