package com.allternit.phonelink.core

// Phone Use route `bt-android` (docs/contracts/CONTRACT-phone-use-2026-11-16.md §6, §9.2):
// the Bluetooth HID reports this phone sends to the person's own iPhone when it
// acts as a keyboard and mouse. Our own code (contract §10: no darwin-bt-remote,
// android-bt-remote or clak source was opened). Pure Kotlin, no Android classes,
// so the encoding is unit-tested on the JVM.

/** One combined HID device: a boot-style keyboard (report 1) and a relative mouse with wheel (report 2). */
object HidReports {
    const val KEYBOARD_ID = 1
    const val MOUSE_ID = 2

    /** Payload sizes, without the report ID (BluetoothHidDevice.sendReport takes the ID separately). */
    const val KEYBOARD_SIZE = 8
    const val MOUSE_SIZE = 4

    /** A mouse report carries one signed byte per axis. */
    const val MAX_STEP = 127

    /** Highest key usage the keyboard collection declares (usage page 0x07, boot keyboard range). */
    const val MAX_KEY_USAGE = 0x65

    /** Six keys at once, as on a boot keyboard. */
    const val MAX_KEYS = 6

    /** Mouse buttons: bit 0 left, bit 1 right. */
    const val BUTTON_LEFT = 1
    const val BUTTON_RIGHT = 2

    /**
     * Corner reset (contract §6): moves of (-127, -127) adding up past the
     * core's CORNER_REACH (20 000 mouse units, phone-hid-core.ts), so the
     * pointer ends pinned in the top-left corner wherever it started.
     */
    const val CORNER_STEPS = 158

    /** The HID report descriptor registered with BluetoothHidDevice. */
    val DESCRIPTOR: ByteArray = bytes(
        // ── Keyboard, report ID 1: modifiers, reserved, 6 keys; LED output ──
        0x05, 0x01, //  Usage Page (Generic Desktop)
        0x09, 0x06, //  Usage (Keyboard)
        0xA1, 0x01, //  Collection (Application)
        0x85, KEYBOARD_ID, //    Report ID (1)
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
        0x95, MAX_KEYS, //    Report Count (6)
        0x75, 0x08, //    Report Size (8)
        0x15, 0x00, //    Logical Minimum (0)
        0x25, MAX_KEY_USAGE, //    Logical Maximum (101)
        0x05, 0x07, //    Usage Page (Keyboard/Keypad)
        0x19, 0x00, //    Usage Minimum (0)
        0x29, MAX_KEY_USAGE, //    Usage Maximum (101)
        0x81, 0x00, //    Input (Data, Array): key array
        0xC0, //  End Collection
        // ── Mouse, report ID 2: 3 buttons, X, Y, wheel (relative, -127..127) ──
        0x05, 0x01, //  Usage Page (Generic Desktop)
        0x09, 0x02, //  Usage (Mouse)
        0xA1, 0x01, //  Collection (Application)
        0x85, MOUSE_ID, //    Report ID (2)
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
    )

    fun clamp(v: Int): Int = v.coerceIn(-MAX_STEP, MAX_STEP)

    /** Keyboard report payload: [modifiers, reserved, key1..key6]. Extra keys past six are dropped. */
    fun keyboard(modifiers: Int, keys: Collection<Int>): ByteArray {
        val out = ByteArray(KEYBOARD_SIZE)
        out[0] = (modifiers and 0xFF).toByte()
        keys.take(MAX_KEYS).forEachIndexed { i, k -> out[2 + i] = (k and 0xFF).toByte() }
        return out
    }

    /** Mouse report payload: [buttons, dx, dy, wheel], each axis clamped to -127..127. */
    fun mouse(buttons: Int, dx: Int, dy: Int, wheel: Int): ByteArray = byteArrayOf(
        (buttons and 0x07).toByte(),
        clamp(dx).toByte(),
        clamp(dy).toByte(),
        clamp(wheel).toByte(),
    )

    /** A signed total as steps within ±127 that add up to it exactly. Zero is no steps. */
    fun steps(total: Int): List<Int> {
        val out = ArrayList<Int>()
        var left = total
        while (left != 0) {
            val s = clamp(left)
            out.add(s)
            left -= s
        }
        return out
    }

    /** Contract §6: scroll lines, positive = down. A HID wheel counts up as positive. */
    fun wheelFor(lines: Int): Int = -lines

    /**
     * One printable ASCII character on a US layout → (usage, modifiers), or
     * null when the layout cannot send it. Same table as phone-hid-keys.ts
     * charStroke (the iPhone turns key positions into text with its own
     * hardware-keyboard layout; a non-US layout types other characters).
     */
    fun charStroke(c: Char): Pair<Int, Int>? {
        when (c) {
            in 'a'..'z' -> return (0x04 + (c - 'a')) to 0
            in 'A'..'Z' -> return (0x04 + (c - 'A')) to SHIFT
            in '1'..'9' -> return (0x1E + (c - '1')) to 0
            '0' -> return 0x27 to 0
        }
        val sym = US_SYMBOLS[c] ?: return null
        return sym.first to (if (sym.second) SHIFT else 0)
    }

    /** Modifier bits (contract §9.2 hid.key: 1 ctrl, 2 shift, 4 alt, 8 cmd = the left-hand keys of the HID modifier byte). */
    const val CTRL = 1
    const val SHIFT = 2
    const val ALT = 4
    const val CMD = 8

    private val US_SYMBOLS: Map<Char, Pair<Int, Boolean>> = mapOf(
        ' ' to (0x2C to false),
        '-' to (0x2D to false), '_' to (0x2D to true),
        '=' to (0x2E to false), '+' to (0x2E to true),
        '[' to (0x2F to false), '{' to (0x2F to true),
        ']' to (0x30 to false), '}' to (0x30 to true),
        '\\' to (0x31 to false), '|' to (0x31 to true),
        ';' to (0x33 to false), ':' to (0x33 to true),
        '\'' to (0x34 to false), '"' to (0x34 to true),
        '`' to (0x35 to false), '~' to (0x35 to true),
        ',' to (0x36 to false), '<' to (0x36 to true),
        '.' to (0x37 to false), '>' to (0x37 to true),
        '/' to (0x38 to false), '?' to (0x38 to true),
        '!' to (0x1E to true), '@' to (0x1F to true), '#' to (0x20 to true), '$' to (0x21 to true),
        '%' to (0x22 to true), '^' to (0x23 to true), '&' to (0x24 to true), '*' to (0x25 to true),
        '(' to (0x26 to true), ')' to (0x27 to true),
    )

    private fun bytes(vararg v: Int): ByteArray = ByteArray(v.size) { v[it].toByte() }
}
