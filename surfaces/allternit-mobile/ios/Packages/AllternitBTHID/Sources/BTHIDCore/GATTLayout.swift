// The GATT database a HID over GATT device exposes (HOGP 1.0 §3, HIDS 1.0,
// DIS 1.1, BAS 1.0), as plain data so it is tested without CoreBluetooth.
//
// Every UUID is written in its full 128-bit Bluetooth Base UUID form
// (0000xxxx-0000-1000-8000-00805F9B34FB). For the HID service this is the
// undocumented part: CoreBluetooth refuses 0x1812 in its 16-bit form, and a
// central reads the 128-bit form as the same service. See Package.swift.

/// The full 128-bit form of a 16-bit Bluetooth SIG assigned number.
public func bluetoothBaseUUID(_ short: UInt16) -> String {
    let hex = String(short, radix: 16, uppercase: true)
    return "0000" + String(repeating: "0", count: 4 - hex.count) + hex + "-0000-1000-8000-00805F9B34FB"
}

/// Assigned numbers used here (Bluetooth SIG Assigned Numbers).
public enum GATTNumber {
    public static let hidService: UInt16 = 0x1812
    public static let deviceInformation: UInt16 = 0x180A
    public static let battery: UInt16 = 0x180F
    public static let hidInformation: UInt16 = 0x2A4A
    public static let reportMap: UInt16 = 0x2A4B
    public static let hidControlPoint: UInt16 = 0x2A4C
    public static let report: UInt16 = 0x2A4D
    public static let protocolMode: UInt16 = 0x2A4E
    public static let pnpID: UInt16 = 0x2A50
    public static let batteryLevel: UInt16 = 0x2A19
    public static let reportReference: UInt16 = 0x2908
}

/// What one characteristic is for; the wrapper keys its objects by this.
public enum GATTRole: String, Sendable, CaseIterable {
    case hidInformation, reportMap, protocolMode, controlPoint
    case keyboardInput, mouseInput, keyboardOutput
    case pnpID, batteryLevel
}

public struct CharacteristicSpec: Equatable, Sendable {
    public let role: GATTRole
    public let uuid: String
    public let read: Bool
    public let write: Bool
    public let writeWithoutResponse: Bool
    public let notify: Bool
    /// Reads and writes need an encrypted (bonded) link.
    public let encrypted: Bool
    /// Report Reference descriptor value [report ID, report type], for Report characteristics.
    public let reportReference: [UInt8]?
}

public struct ServiceSpec: Equatable, Sendable {
    public let uuid: String
    public let name: String
    /// Without it the device is not a keyboard and mouse; the others are best effort.
    public let required: Bool
    public let characteristics: [CharacteristicSpec]
}

public enum GATTLayout {
    /// HIDS 1.0 §3.5: bcdHID 1.11 (little-endian), country code 0, flags = normally connectable.
    public static let hidInformation: [UInt8] = [0x11, 0x01, 0x00, 0x02]
    /// HIDS 1.0 §3.4: 0 boot protocol, 1 report protocol (the default).
    public static let reportProtocol: UInt8 = 1
    /// DIS PnP ID: vendor ID source 0x01 (Bluetooth SIG), vendor 0xFFFF (the SIG's
    /// value for "no company ID assigned"), product 0x0001, version 1.0.0. We have
    /// no USB-IF vendor ID; no other company's ID is used.
    public static let pnpID: [UInt8] = [0x01, 0xFF, 0xFF, 0x01, 0x00, 0x00, 0x01]
    public static let batteryPercent: UInt8 = 100

    public static func reportReference(id: UInt8, type: HIDReportType) -> [UInt8] { [id, type.rawValue] }

    private static func ch(
        _ role: GATTRole, _ number: UInt16, read: Bool = true, write: Bool = false, writeWithoutResponse: Bool = false,
        notify: Bool = false, encrypted: Bool, reportReference: [UInt8]? = nil
    ) -> CharacteristicSpec {
        CharacteristicSpec(
            role: role, uuid: bluetoothBaseUUID(number), read: read, write: write, writeWithoutResponse: writeWithoutResponse,
            notify: notify, encrypted: encrypted, reportReference: reportReference
        )
    }

    public static let hid = ServiceSpec(
        uuid: bluetoothBaseUUID(GATTNumber.hidService),
        name: "HID",
        required: true,
        characteristics: [
            ch(.hidInformation, GATTNumber.hidInformation, encrypted: true),
            ch(.reportMap, GATTNumber.reportMap, encrypted: true),
            ch(.protocolMode, GATTNumber.protocolMode, writeWithoutResponse: true, encrypted: true),
            ch(.controlPoint, GATTNumber.hidControlPoint, read: false, writeWithoutResponse: true, encrypted: true),
            ch(.keyboardInput, GATTNumber.report, notify: true, encrypted: true,
               reportReference: reportReference(id: HIDReportID.keyboard, type: .input)),
            ch(.mouseInput, GATTNumber.report, notify: true, encrypted: true,
               reportReference: reportReference(id: HIDReportID.mouse, type: .input)),
            ch(.keyboardOutput, GATTNumber.report, write: true, writeWithoutResponse: true, encrypted: true,
               reportReference: reportReference(id: HIDReportID.keyboard, type: .output)),
        ]
    )

    public static let deviceInformation = ServiceSpec(
        uuid: bluetoothBaseUUID(GATTNumber.deviceInformation),
        name: "Device Information",
        required: false,
        characteristics: [ch(.pnpID, GATTNumber.pnpID, encrypted: false)]
    )

    public static let battery = ServiceSpec(
        uuid: bluetoothBaseUUID(GATTNumber.battery),
        name: "Battery",
        required: false,
        characteristics: [ch(.batteryLevel, GATTNumber.batteryLevel, notify: true, encrypted: false)]
    )

    /// In the order they are added: HID first, so a refusal stops before anything else is published.
    public static let services: [ServiceSpec] = [hid, deviceInformation, battery]

    /// The value a read of a fixed characteristic returns; nil for ones whose value changes.
    public static func fixedValue(_ role: GATTRole) -> [UInt8]? {
        switch role {
        case .hidInformation: return hidInformation
        case .reportMap: return HIDReportMap.bytes
        case .pnpID: return pnpID
        case .batteryLevel: return [batteryPercent]
        default: return nil
        }
    }
}

/// ATT read results (the wrapper maps them to CBATTError).
public enum ReadOutcome: Equatable, Sendable {
    case value([UInt8])
    case invalidOffset
}

/// A long read (ATT Read Blob) asks from an offset; past the end is an error, at the end is empty.
public func readSlice(_ value: [UInt8], offset: Int) -> ReadOutcome {
    guard offset >= 0, offset <= value.count else { return .invalidOffset }
    return .value(Array(value[offset...]))
}

// ── Status, as the tool and the app report it ───────────────────────────────

public enum RadioState: String, Sendable {
    case notStarted = "not_started", unknown, resetting, unsupported, unauthorized, off, on
}

public enum BluetoothAuthorization: String, Sendable {
    case notDetermined = "not_determined", restricted, denied, allowed
}

public struct HIDPeripheralStatus: Equatable, Sendable {
    public var radio: RadioState
    public var authorization: BluetoothAuthorization
    /// The HID service is published.
    public var servicesReady: Bool
    /// Optional services published next to HID (Device Information, Battery).
    public var extraServices: [String]
    public var advertising: Bool
    /// Centrals subscribed to an input report: a connected, bonded host.
    public var subscribers: Int
    /// CoreBluetooth's id for the subscribed central (peripherals get no device name).
    public var centralID: String?
    /// The system refused the HID service (the undocumented behaviour changed).
    public var refused: Bool
    public var lastError: BTHIDError?

    public init(
        radio: RadioState = .notStarted, authorization: BluetoothAuthorization = .notDetermined, servicesReady: Bool = false,
        extraServices: [String] = [], advertising: Bool = false, subscribers: Int = 0, centralID: String? = nil,
        refused: Bool = false, lastError: BTHIDError? = nil
    ) {
        self.radio = radio
        self.authorization = authorization
        self.servicesReady = servicesReady
        self.extraServices = extraServices
        self.advertising = advertising
        self.subscribers = subscribers
        self.centralID = centralID
        self.refused = refused
        self.lastError = lastError
    }

    public var connected: Bool { subscribers > 0 }
}

/// A failure with a short code and a plain sentence.
public struct BTHIDError: Error, Equatable, Sendable {
    public let code: String
    public let message: String
    public init(_ code: String, _ message: String) {
        self.code = code
        self.message = message
    }

    #if os(macOS)
    public static let refusedMessage = "Not available: macOS refused the keyboard service; Apple may have changed this."
    #else
    public static let refusedMessage = "Not available: iOS refused the keyboard service; Apple may have changed this."
    #endif
    public static func refused(_ detail: String) -> BTHIDError {
        BTHIDError("refused", detail.isEmpty ? refusedMessage : "\(refusedMessage) (\(detail))")
    }
}
