// The HID report descriptor ("Report Map" characteristic, HIDS 1.0 §3.6),
// written from the HID 1.11 spec and the HID Usage Tables (Generic Desktop
// 0x01, Keyboard 0x07, LEDs 0x08, Button 0x09).
//
// Byte for byte the same descriptor the Allternit Android app registers for
// `bt-android` (HidReports.DESCRIPTOR in
// surfaces/allternit-mobile/android/.../phonelink/core/HidReports.kt), so the
// Desktop core (phone-hid-core.ts) treats both senders the same:
//   report 1  boot-style keyboard: modifier byte, reserved byte, 6 key slots;
//             LED output (5 bits + 3 padding)
//   report 2  relative mouse: 3 buttons + 5 padding, X, Y, wheel (-127..127)
//
// Over GATT the report ID is not part of a report's value: each Report
// characteristic carries its ID in its Report Reference descriptor
// (HIDS 1.0 §3.7.2). The IDs stay in the map so the host can match them.

public enum HIDReportID {
    public static let keyboard: UInt8 = 1
    public static let mouse: UInt8 = 2
}

/// Report Reference "report type" values (HIDS 1.0 §3.7.2.2).
public enum HIDReportType: UInt8, Sendable {
    case input = 1
    case output = 2
    case feature = 3
}

public enum HIDReportMap {
    /// Highest key usage the keyboard collection declares (Keyboard Application, 0x65).
    public static let maxKeyUsage: UInt8 = 0x65
    /// Six keys at once, as on a boot keyboard.
    public static let maxKeys = 6

    public static let bytes: [UInt8] = [
        // ── Keyboard, report ID 1 ──
        0x05, 0x01, //  Usage Page (Generic Desktop)
        0x09, 0x06, //  Usage (Keyboard)
        0xA1, 0x01, //  Collection (Application)
        0x85, HIDReportID.keyboard, //    Report ID (1)
        0x05, 0x07, //    Usage Page (Keyboard/Keypad)
        0x19, 0xE0, //    Usage Minimum (Left Control)
        0x29, 0xE7, //    Usage Maximum (Right GUI)
        0x15, 0x00, //    Logical Minimum (0)
        0x25, 0x01, //    Logical Maximum (1)
        0x75, 0x01, //    Report Size (1)
        0x95, 0x08, //    Report Count (8)
        0x81, 0x02, //    Input (Data, Variable, Absolute): modifier byte
        0x95, 0x01, //    Report Count (1)
        0x75, 0x08, //    Report Size (8)
        0x81, 0x01, //    Input (Constant): reserved byte
        0x95, 0x05, //    Report Count (5)
        0x75, 0x01, //    Report Size (1)
        0x05, 0x08, //    Usage Page (LEDs)
        0x19, 0x01, //    Usage Minimum (Num Lock)
        0x29, 0x05, //    Usage Maximum (Kana)
        0x91, 0x02, //    Output (Data, Variable, Absolute): LEDs
        0x95, 0x01, //    Report Count (1)
        0x75, 0x03, //    Report Size (3)
        0x91, 0x01, //    Output (Constant): padding
        0x95, UInt8(maxKeys), //    Report Count (6)
        0x75, 0x08, //    Report Size (8)
        0x15, 0x00, //    Logical Minimum (0)
        0x25, maxKeyUsage, //    Logical Maximum (101)
        0x05, 0x07, //    Usage Page (Keyboard/Keypad)
        0x19, 0x00, //    Usage Minimum (0)
        0x29, maxKeyUsage, //    Usage Maximum (101)
        0x81, 0x00, //    Input (Data, Array): key array
        0xC0, //  End Collection
        // ── Mouse, report ID 2 ──
        0x05, 0x01, //  Usage Page (Generic Desktop)
        0x09, 0x02, //  Usage (Mouse)
        0xA1, 0x01, //  Collection (Application)
        0x85, HIDReportID.mouse, //    Report ID (2)
        0x09, 0x01, //    Usage (Pointer)
        0xA1, 0x00, //    Collection (Physical)
        0x05, 0x09, //      Usage Page (Button)
        0x19, 0x01, //      Usage Minimum (Button 1)
        0x29, 0x03, //      Usage Maximum (Button 3)
        0x15, 0x00, //      Logical Minimum (0)
        0x25, 0x01, //      Logical Maximum (1)
        0x95, 0x03, //      Report Count (3)
        0x75, 0x01, //      Report Size (1)
        0x81, 0x02, //      Input (Data, Variable, Absolute): buttons
        0x95, 0x01, //      Report Count (1)
        0x75, 0x05, //      Report Size (5)
        0x81, 0x01, //      Input (Constant): padding
        0x05, 0x01, //      Usage Page (Generic Desktop)
        0x09, 0x30, //      Usage (X)
        0x09, 0x31, //      Usage (Y)
        0x09, 0x38, //      Usage (Wheel)
        0x15, 0x81, //      Logical Minimum (-127)
        0x25, 0x7F, //      Logical Maximum (127)
        0x75, 0x08, //      Report Size (8)
        0x95, 0x03, //      Report Count (3)
        0x81, 0x06, //      Input (Data, Variable, Relative)
        0xC0, //    End Collection
        0xC0, //  End Collection
    ]
}
