// The sentences a radio state turns into (shown to the person as-is by the
// Desktop settings row and route health).

#if os(macOS)
private let device = "this Mac"
private let Device = "This Mac"
#else
private let device = "this device"
private let Device = "This device"
#endif

/// Nil when the radio is on; otherwise why the keyboard can't start.
public func radioError(_ state: RadioState?) -> BTHIDError? {
    switch state {
    case .on: return nil
    case .off: return BTHIDError("bluetooth_off", "Turn on Bluetooth on \(device).")
    case .unauthorized:
        #if os(macOS)
        return BTHIDError("unauthorized", "Allow Allternit to use Bluetooth in System Settings > Privacy & Security > Bluetooth.")
        #else
        return BTHIDError("unauthorized", "Allow Allternit to use Bluetooth in Settings > Privacy & Security > Bluetooth.")
        #endif
    case .unsupported: return BTHIDError("unsupported", "\(Device) has no Bluetooth Low Energy.")
    case .resetting, .unknown, .notStarted, .none: return BTHIDError("bluetooth_unknown", "Bluetooth on \(device) did not answer. Try again in a moment.")
    }
}
