// A Bluetooth LE keyboard and mouse (HID over GATT) on CBPeripheralManager.
//
//   start()  publish the HID, Device Information and Battery services (layout
//            in BTHIDCore/GATTLayout.swift) and advertise under `localName`
//   stop()   stop advertising and remove the services (the host disconnects)
//   send()   input reports to the subscribed host, with flow control: when
//            updateValue says the transmit queue is full, wait for
//            peripheralManagerIsReady(toUpdateSubscribers:) and send again
//
// Connected = a central subscribed to an input report. A host subscribes
// only after it has read the Report Map, which needs an encrypted link, so a
// subscriber is a bonded host. CoreBluetooth gives a peripheral no name for
// its central, only an identifier.
//
// Every CoreBluetooth call and every piece of state lives on one serial
// queue (the manager's delegate queue). Public methods are safe from any
// thread except that queue; `onChange` is called on it and must not call
// back into this object synchronously.
//
// UNDOCUMENTED (see Package.swift): the HID service is registered under its
// 128-bit Base UUID form. If CoreBluetooth refuses it (an error from
// didAdd, or an Objective-C exception while building), status.refused is set
// and start() throws BTHIDError code "refused".
//
// NOT CHECKED ON HARDWARE (plan check P2): pairing, the pointer showing with
// AssistiveTouch, reconnects after sleep, and whether a read request's
// characteristic is the same object we published (role(of:) assumes it is).

import BTHIDObjC
import CoreBluetooth
import Foundation
@_exported import BTHIDCore

public final class HIDPeripheral: NSObject, CBPeripheralManagerDelegate, @unchecked Sendable {
    public let localName: String
    /// Called on the internal queue whenever the status changes.
    public var onChange: (@Sendable (HIDPeripheralStatus) -> Void)?

    private let queue = DispatchQueue(label: "com.allternit.bthid.peripheral")
    // ── queue-only state ──
    private var manager: CBPeripheralManager?
    private var characteristics: [GATTRole: CBMutableCharacteristic] = [:]
    private var subscribed: [GATTRole: Set<UUID>] = [:]
    private var values: [GATTRole: [UInt8]] = HIDPeripheral.initialValues
    private var status = HIDPeripheralStatus()
    private var stateWaiters: [UUID: () -> Void] = [:]
    private var addWaiter: ((Error?) -> Void)?
    private var advertiseWaiter: ((Error?) -> Void)?
    private var readyWaiters: [() -> Void] = []

    private static let initialValues: [GATTRole: [UInt8]] = [
        .protocolMode: [GATTLayout.reportProtocol],
        .keyboardInput: [UInt8](repeating: 0, count: HIDReports.keyboardSize),
        .mouseInput: [UInt8](repeating: 0, count: HIDReports.mouseSize),
        .keyboardOutput: [0],
    ]

    public init(localName: String = "Allternit Keyboard") {
        self.localName = localName
        super.init()
    }

    // ── status ──────────────────────────────────────────────────────────────

    /// Bluetooth permission for this app, read without asking for it.
    public static func authorization() -> BluetoothAuthorization {
        switch CBManager.authorization {
        case .allowedAlways: return .allowed
        case .denied: return .denied
        case .restricted: return .restricted
        case .notDetermined: return .notDetermined
        @unknown default: return .notDetermined
        }
    }

    /// Never creates the manager, so it never shows the permission prompt.
    public func currentStatus() -> HIDPeripheralStatus {
        queue.sync {
            var s = status
            s.authorization = Self.authorization()
            s.radio = radioState()
            return s
        }
    }

    // ── start / stop ────────────────────────────────────────────────────────

    /// Powers up (asking for Bluetooth permission the first time), publishes the services and advertises.
    public func start() async throws {
        let radio = await wait(seconds: 8) { [self] (done: @escaping (RadioState) -> Void) in
            self.ensureManager()
            let now = self.radioState()
            if now != .unknown, now != .resetting { return done(now) }
            let id = UUID()
            self.stateWaiters[id] = { [weak self] in
                guard let self else { return }
                let s = self.radioState()
                guard s != .unknown, s != .resetting else { return }
                self.stateWaiters[id] = nil
                done(s)
            }
        }
        if let e = radioError(radio) { throw record(e) }

        if !queue.sync(execute: { status.servicesReady }) {
            for spec in GATTLayout.services { try await publish(spec) }
        }

        let advertising = queue.sync { manager?.isAdvertising ?? false }
        if !advertising {
            let result: Error?? = await wait(seconds: 5) { (done: @escaping (Error?) -> Void) in
                guard let m = self.manager else { return done(BTHIDError("bluetooth_unknown", "Bluetooth stopped.")) }
                self.advertiseWaiter = done
                m.startAdvertising([
                    CBAdvertisementDataLocalNameKey: self.localName,
                    CBAdvertisementDataServiceUUIDsKey: [CBUUID(string: GATTLayout.hid.uuid)],
                ])
            }
            switch result {
            case .none: throw record(BTHIDError("timeout", "Bluetooth did not start advertising in time."))
            case .some(.some(let e)): throw record(BTHIDError("advertise_failed", "Bluetooth could not advertise the keyboard: \(e.localizedDescription)"))
            case .some(.none): break
            }
        }
        queue.sync {
            status.advertising = true
            status.lastError = nil
            notify()
        }
    }

    /// Stops advertising and removes the services; a connected host drops the link.
    public func stop() {
        queue.sync {
            manager?.stopAdvertising()
            manager?.removeAllServices()
            clearPublished()
            status.refused = false
            notify()
        }
    }

    private func publish(_ spec: ServiceSpec) async throws {
        var built: (CBMutableService, [GATTRole: CBMutableCharacteristic])?
        if let raised = BTHIDCatchException({ built = Self.build(spec) }) {
            if spec.required { throw record(.refused(raised), refused: true) }
            return
        }
        guard let built else { return }
        let (service, chars) = built
        let result: Error?? = await wait(seconds: 5) { (done: @escaping (Error?) -> Void) in
            guard let m = self.manager else { return done(BTHIDError("bluetooth_unknown", "Bluetooth stopped.")) }
            self.addWaiter = done
            if let raised = BTHIDCatchException({ m.add(service) }) {
                self.addWaiter = nil
                done(BTHIDError("refused", raised))
            }
        }
        switch result {
        case .none:
            if spec.required { throw record(BTHIDError("timeout", "Bluetooth did not publish the keyboard service in time.")) }
        case .some(.some(let e)):
            guard spec.required else { return }
            // Only a refusal while the radio is on means the system said no to the service itself.
            let radio = queue.sync { radioState() }
            if radio != .on, let re = radioError(radio) { throw record(re) }
            let detail = (e as? BTHIDError)?.message ?? e.localizedDescription
            throw record(.refused(detail), refused: true)
        case .some(.none):
            queue.sync {
                characteristics.merge(chars) { _, new in new }
                if spec.required { status.servicesReady = true } else { status.extraServices.append(spec.name) }
                notify()
            }
        }
    }

    private static func build(_ spec: ServiceSpec) -> (CBMutableService, [GATTRole: CBMutableCharacteristic]) {
        let service = CBMutableService(type: CBUUID(string: spec.uuid), primary: true)
        var byRole: [GATTRole: CBMutableCharacteristic] = [:]
        var list: [CBMutableCharacteristic] = []
        for c in spec.characteristics {
            var props: CBCharacteristicProperties = []
            if c.read { props.insert(.read) }
            if c.write { props.insert(.write) }
            if c.writeWithoutResponse { props.insert(.writeWithoutResponse) }
            if c.notify { props.insert(.notify) }
            var perms: CBAttributePermissions = []
            if c.read { perms.insert(c.encrypted ? .readEncryptionRequired : .readable) }
            if c.write || c.writeWithoutResponse { perms.insert(c.encrypted ? .writeEncryptionRequired : .writeable) }
            // value nil: every read is answered in didReceiveRead, so encryption applies to all of them.
            let ch = CBMutableCharacteristic(type: CBUUID(string: c.uuid), properties: props, value: nil, permissions: perms)
            if let ref = c.reportReference {
                ch.descriptors = [CBMutableDescriptor(type: CBUUID(string: bluetoothBaseUUID(GATTNumber.reportReference)), value: Data(ref))]
            }
            byRole[c.role] = ch
            list.append(ch)
        }
        service.characteristics = list
        return (service, byRole)
    }

    // ── sending ─────────────────────────────────────────────────────────────

    private enum Step { case sent, retry, notConnected }

    /// Sends the reports in order. Each one waits at most `timeout` seconds for room in the transmit queue.
    public func send(_ outputs: [HIDOutput], timeout: Double = 2) async throws {
        for o in outputs { try await sendOne(o, timeout: timeout) }
    }

    private func sendOne(_ output: HIDOutput, timeout: Double) async throws {
        let (role, data): (GATTRole, Data) = {
            switch output {
            case .keyboard(let d): return (.keyboardInput, d)
            case .mouse(let d): return (.mouseInput, d)
            }
        }()
        let deadline = Date().addingTimeInterval(timeout)
        while true {
            try Task.checkCancellation()
            let left = deadline.timeIntervalSinceNow
            if left <= 0 { throw record(BTHIDError("send_timeout", "The iPhone did not take keyboard and mouse input in time.")) }
            let step = await wait(seconds: left) { (done: @escaping (Step) -> Void) in
                guard let m = self.manager, let c = self.characteristics[role], !(self.subscribed[role]?.isEmpty ?? true) else {
                    return done(.notConnected)
                }
                self.values[role] = [UInt8](data)
                if m.updateValue(data, for: c, onSubscribedCentrals: nil) { return done(.sent) }
                self.readyWaiters.append { done(.retry) }
            }
            switch step {
            case .sent: return
            case .retry, .none: continue
            case .notConnected: throw BTHIDError("not_connected", "No iPhone is connected to this Bluetooth keyboard and mouse.")
            }
        }
    }

    // ── queue helpers ───────────────────────────────────────────────────────

    /// Runs `register` on the queue and waits until it calls back, or `seconds` pass (nil). Callbacks after the first are ignored.
    private func wait<T>(seconds: Double, _ register: @escaping (_ done: @escaping (T) -> Void) -> Void) async -> T? {
        await withCheckedContinuation { (cont: CheckedContinuation<T?, Never>) in
            queue.async {
                var finished = false
                let finish: (T?) -> Void = { v in
                    if finished { return }
                    finished = true
                    cont.resume(returning: v)
                }
                register { finish($0) }
                self.queue.asyncAfter(deadline: .now() + max(0, seconds)) { finish(nil) }
            }
        }
    }

    private func record(_ e: BTHIDError, refused: Bool = false) -> BTHIDError {
        queue.sync {
            status.lastError = e
            if refused { status.refused = true }
            notify()
        }
        return e
    }

    private func ensureManager() {
        guard manager == nil else { return }
        manager = CBPeripheralManager(delegate: self, queue: queue, options: [CBPeripheralManagerOptionShowPowerAlertKey: false])
    }

    private func radioState() -> RadioState {
        guard let m = manager else { return .notStarted }
        switch m.state {
        case .poweredOn: return .on
        case .poweredOff: return .off
        case .unauthorized: return .unauthorized
        case .unsupported: return .unsupported
        case .resetting: return .resetting
        case .unknown: return .unknown
        @unknown default: return .unknown
        }
    }

    private func clearPublished() {
        characteristics = [:]
        subscribed = [:]
        values = Self.initialValues
        status.servicesReady = false
        status.extraServices = []
        status.advertising = false
        status.subscribers = 0
        status.centralID = nil
        wakeReady()
    }

    private func wakeReady() {
        let ws = readyWaiters
        readyWaiters = []
        ws.forEach { $0() }
    }

    private func notify() {
        var s = status
        s.authorization = Self.authorization()
        s.radio = radioState()
        onChange?(s)
    }

    private func role(of c: CBCharacteristic) -> GATTRole? {
        characteristics.first { $0.value === c }?.key
    }

    private func recount() {
        let all = subscribed.filter { $0.key == .keyboardInput || $0.key == .mouseInput }.values.reduce(into: Set<UUID>()) { $0.formUnion($1) }
        status.subscribers = all.count
        if all.isEmpty { status.centralID = nil }
    }

    // ── CBPeripheralManagerDelegate (on the queue) ──────────────────────────

    public func peripheralManagerDidUpdateState(_ peripheral: CBPeripheralManager) {
        if peripheral.state != .poweredOn { clearPublished() }
        let ws = stateWaiters
        ws.values.forEach { $0() }
        notify()
    }

    public func peripheralManager(_ peripheral: CBPeripheralManager, didAdd service: CBService, error: Error?) {
        let w = addWaiter
        addWaiter = nil
        w?(error)
    }

    public func peripheralManagerDidStartAdvertising(_ peripheral: CBPeripheralManager, error: Error?) {
        let w = advertiseWaiter
        advertiseWaiter = nil
        w?(error)
    }

    public func peripheralManager(_ peripheral: CBPeripheralManager, central: CBCentral, didSubscribeTo characteristic: CBCharacteristic) {
        guard let role = role(of: characteristic) else { return }
        subscribed[role, default: []].insert(central.identifier)
        peripheral.setDesiredConnectionLatency(.low, for: central)
        recount()
        status.centralID = central.identifier.uuidString
        notify()
    }

    public func peripheralManager(_ peripheral: CBPeripheralManager, central: CBCentral, didUnsubscribeFrom characteristic: CBCharacteristic) {
        guard let role = role(of: characteristic) else { return }
        subscribed[role]?.remove(central.identifier)
        recount()
        wakeReady()
        notify()
    }

    public func peripheralManager(_ peripheral: CBPeripheralManager, didReceiveRead request: CBATTRequest) {
        guard let role = role(of: request.characteristic) else { return peripheral.respond(to: request, withResult: .attributeNotFound) }
        guard let value = GATTLayout.fixedValue(role) ?? values[role] else { return peripheral.respond(to: request, withResult: .readNotPermitted) }
        switch readSlice(value, offset: request.offset) {
        case .value(let v):
            request.value = Data(v)
            peripheral.respond(to: request, withResult: .success)
        case .invalidOffset:
            peripheral.respond(to: request, withResult: .invalidOffset)
        }
    }

    public func peripheralManager(_ peripheral: CBPeripheralManager, didReceiveWrite requests: [CBATTRequest]) {
        guard let first = requests.first else { return }
        for r in requests {
            let v = [UInt8](r.value ?? Data())
            switch role(of: r.characteristic) {
            case .protocolMode:
                guard v.count == 1, v[0] <= 1 else { return peripheral.respond(to: first, withResult: .invalidAttributeValueLength) }
                values[.protocolMode] = v
            case .controlPoint:
                break // Suspend / Exit Suspend (HIDS §3.8): nothing to change; we send only when asked.
            case .keyboardOutput:
                values[.keyboardOutput] = [v.first ?? 0] // LEDs (Caps Lock etc.)
            default:
                return peripheral.respond(to: first, withResult: .writeNotPermitted)
            }
        }
        peripheral.respond(to: first, withResult: .success)
    }

    public func peripheralManagerIsReady(toUpdateSubscribers peripheral: CBPeripheralManager) {
        wakeReady()
    }
}
