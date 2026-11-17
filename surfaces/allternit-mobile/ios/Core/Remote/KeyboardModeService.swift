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

    /// Started (or starting) by the person.
    var isRunning: Bool { Self.running(mode) }

    private static func running(_ mode: KeyboardMode) -> Bool {
        switch mode {
        case .on, .starting: return true
        case .off, .failed: return false
        }
    }

    /// For the app's `state` frame: keyboard mode on, and an iPhone connected to it.
    var stateSnapshot: [String: Bool] { ["on": isRunning, "connected": isRunning && status.connected] }

    var linkState: KeyboardLinkState { .from(mode: mode, status: status) }

    // MARK: - Start / Stop (the person's)

    func start() {
        guard !isRunning else { return }
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
        try await commands.handle(method: method, params: params)
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
