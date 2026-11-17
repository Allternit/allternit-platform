package com.allternit.phonelink.core

import org.json.JSONObject

// Phone Use route `bt-android` (docs/contracts/CONTRACT-phone-use-2026-11-16.md §9.2):
// the `hid.*` requests Allternit Desktop sends over the app-phone control socket
// (same SecureEnvelope and pairing as input.events), turned into HID reports for
// the iPhone this phone is connected to as a Bluetooth keyboard and mouse. On
// this socket each request is `{id, method, params}` and the answer is
// `{id, result}` or `{id, error:{code,message}}` (AppPhoneLink.reply).
// Pure Kotlin: the Bluetooth side is the HidLink below (BluetoothRemote on the
// phone, a fake in tests).

/** Whether this phone can act as a keyboard and mouse right now. */
enum class HidAvailability(val wire: String) {
    /** Registered with Android's HID device profile: an iPhone can pair or connect. */
    READY("ready"),
    /** The person has not turned on "Control my iPhone by Bluetooth". */
    OFF("off"),
    /** Bluetooth is switched off. */
    BLUETOOTH_OFF("bluetooth_off"),
    /** Android 12+: Nearby devices permission not granted. */
    NO_PERMISSION("no_permission"),
    /** Android before 9, or a phone/OEM build without the HID device profile. */
    UNSUPPORTED("unsupported"),
}

/** The Bluetooth side as the bot sees it (contract §6 status()). */
data class HidState(
    val availability: HidAvailability,
    /** An iPhone is chosen and bonded with this phone. */
    val paired: Boolean,
    /** That iPhone is connected to this phone's keyboard and mouse now. */
    val connected: Boolean,
    val name: String? = null,
) {
    /** Contract §5: the needs-you code for this state, when the person can fix it on the iPhone. */
    fun needsYou(): String? = if (availability == HidAvailability.READY && !connected) NEEDS_BT_NOT_PAIRED else null

    /** Why this phone cannot send at all (no contract code: Desktop reports the route unavailable), or null. */
    fun unavailableReason(): String? = when (availability) {
        HidAvailability.READY -> null
        HidAvailability.OFF -> "Turn on Control my iPhone by Bluetooth in the Allternit app on your Android phone."
        HidAvailability.BLUETOOTH_OFF -> "Turn on Bluetooth on your Android phone."
        HidAvailability.NO_PERMISSION -> "Allow Nearby devices for Allternit on your Android phone, then turn on Control my iPhone by Bluetooth."
        HidAvailability.UNSUPPORTED -> NOT_AVAILABLE_MESSAGE
    }

    /** The `hid.pair.status` reply. */
    fun toJson(): JSONObject = JSONObject()
        .put("paired", paired)
        .put("connected", connected)
        .put("available", availability == HidAvailability.READY)
        .put("availability", availability.wire)
        .apply {
            name?.let { put("name", it) }
            needsYou()?.let { put("needs_you", it) }
            unavailableReason()?.let { put("message", it) }
        }

    companion object {
        const val NEEDS_BT_NOT_PAIRED = "BT_NOT_PAIRED"
        const val NOT_AVAILABLE_MESSAGE = "Bluetooth keyboard and mouse is not available on this phone: Android does not offer it on this model."
    }
}

/** The Bluetooth HID link (BluetoothRemote on the phone). */
interface HidLink {
    fun state(): HidState
    /** One input report to the connected iPhone. False when nothing is connected or Android refused it. */
    fun send(reportId: Int, data: ByteArray): Boolean
}

/**
 * The `hid.*` methods. Holds what is pressed (modifiers, up to six keys, mouse
 * buttons) so every report carries the whole state, as a real keyboard does.
 * Calls arrive on AppPhoneLink's one ordered thread; [releaseAll] may come from
 * another thread (the control socket dropping, the service stopping).
 */
class HidCommands(
    private val link: HidLink,
    /** Gap between reports so the iPhone sees each one (key down, key up). */
    private val pause: (Long) -> Unit = { Thread.sleep(it) },
    private val gapMs: Long = REPORT_GAP_MS,
) {
    private var modifiers = 0
    private val keys = LinkedHashSet<Int>()
    private var buttons = 0

    fun handles(method: String): Boolean = method.startsWith("hid.")

    /** Runs one method. Throws [RpcException] with a typed code the Desktop lane maps (phone-bt-android.ts). */
    @Synchronized
    fun handle(method: String, params: JSONObject): JSONObject {
        if (method == "hid.pair.status") return link.state().toJson()
        if (method !in METHODS) throw RpcException(RpcException.METHOD_NOT_FOUND, "Unknown method $method", "unknown_method")
        ready()
        when (method) {
            "hid.pointer.move" -> {
                val dx = int(params, "dx", -HidReports.MAX_STEP..HidReports.MAX_STEP, clampOutside = true)
                val dy = int(params, "dy", -HidReports.MAX_STEP..HidReports.MAX_STEP, clampOutside = true)
                mouse(dx, dy, 0)
            }
            "hid.pointer.click" -> {
                val bit = when (params.opt("button") ?: "left") {
                    "left" -> HidReports.BUTTON_LEFT
                    "right" -> HidReports.BUTTON_RIGHT
                    else -> throw RpcException.invalid("button must be left or right")
                }
                val down = bool(params, "down", true)
                val up = bool(params, "up", true)
                if (down) { buttons = buttons or bit; mouse(0, 0, 0) }
                if (down && up) pause(gapMs)
                if (up) { buttons = buttons and bit.inv(); mouse(0, 0, 0) }
            }
            "hid.pointer.reset" -> {
                buttons = 0
                repeat(HidReports.CORNER_STEPS) { mouse(-HidReports.MAX_STEP, -HidReports.MAX_STEP, 0) }
            }
            "hid.scroll" -> {
                val lines = int(params, "lines", -MAX_SCROLL_LINES..MAX_SCROLL_LINES)
                for (step in HidReports.steps(HidReports.wheelFor(lines))) mouse(0, 0, step)
            }
            "hid.key" -> {
                val usage = int(params, "usage", 0..HidReports.MAX_KEY_USAGE)
                val mods = int(params, "modifiers", 0..0xFF)
                val down = params.opt("down") as? Boolean ?: throw RpcException.invalid("down must be true or false")
                key(usage, mods, down)
            }
            "hid.type" -> {
                val text = params.opt("text") as? String ?: throw RpcException.invalid("text is required")
                if (text.isEmpty() || text.length > MAX_TEXT) throw RpcException.invalid("text must be 1-$MAX_TEXT characters")
                val strokes = text.map { c ->
                    HidReports.charStroke(c) ?: throw RpcException.typed("bad_params", "The keyboard can't type \"$c\": only printable ASCII on a US layout can be sent.")
                }
                for ((usage, mods) in strokes) {
                    keyboard(mods, listOf(usage))
                    pause(gapMs)
                    keyboard(0, emptyList())
                    pause(gapMs)
                }
                // Back to whatever was held before the text.
                keyboard(modifiers, keys)
                return JSONObject().put("sent", strokes.size)
            }
        }
        return JSONObject()
    }

    /** Lets go of every key and button (contract §9.2: when the link drops or the app goes away). Never throws. */
    @Synchronized
    fun releaseAll() {
        val held = modifiers != 0 || keys.isNotEmpty() || buttons != 0
        modifiers = 0
        keys.clear()
        buttons = 0
        if (!held || !link.state().connected) return
        try {
            link.send(HidReports.KEYBOARD_ID, HidReports.keyboard(0, emptyList()))
            link.send(HidReports.MOUSE_ID, HidReports.mouse(0, 0, 0, 0))
        } catch (_: Exception) {}
    }

    private fun key(usage: Int, mods: Int, down: Boolean) {
        if (down) {
            if (usage != 0 && usage !in keys && keys.size >= HidReports.MAX_KEYS) {
                throw RpcException.typed("bad_params", "At most ${HidReports.MAX_KEYS} keys can be held at once.")
            }
            modifiers = modifiers or mods
            if (usage != 0) keys.add(usage)
        } else if (usage == 0 && mods == 0) {
            // contract: key(0, 0, up) lets go of everything (phone-hid-core.ts releaseAll).
            modifiers = 0
            keys.clear()
        } else {
            modifiers = modifiers and mods.inv()
            keys.remove(usage)
        }
        keyboard(modifiers, keys)
    }

    private fun ready() {
        val s = link.state()
        s.unavailableReason()?.let { throw RpcException.typed("hid_unavailable", it, JSONObject().put("availability", s.availability.wire)) }
        if (!s.connected) throw RpcException.typed("bt_not_paired", NOT_PAIRED_MESSAGE)
    }

    private fun keyboard(mods: Int, held: Collection<Int>) = send(HidReports.KEYBOARD_ID, HidReports.keyboard(mods, held))

    private fun mouse(dx: Int, dy: Int, wheel: Int) = send(HidReports.MOUSE_ID, HidReports.mouse(buttons, dx, dy, wheel))

    private fun send(id: Int, data: ByteArray) {
        if (!link.send(id, data)) {
            if (!link.state().connected) throw RpcException.typed("bt_not_paired", NOT_PAIRED_MESSAGE)
            throw RpcException.typed("hid_failed", "Android did not send the keyboard or mouse report.")
        }
    }

    private fun int(p: JSONObject, name: String, range: IntRange, clampOutside: Boolean = false): Int {
        val v = p.opt(name)
        val n = when (v) {
            is Int -> v
            is Long -> if (v in Int.MIN_VALUE..Int.MAX_VALUE) v.toInt() else null
            else -> null
        } ?: throw RpcException.invalid("$name must be a whole number")
        if (n in range) return n
        if (clampOutside) return n.coerceIn(range.first, range.last)
        throw RpcException.invalid("$name must be ${range.first}..${range.last}")
    }

    private fun bool(p: JSONObject, name: String, default: Boolean): Boolean = when (val v = p.opt(name)) {
        null -> default
        is Boolean -> v
        else -> throw RpcException.invalid("$name must be true or false")
    }

    companion object {
        const val REPORT_GAP_MS = 8L
        const val MAX_TEXT = 500
        const val MAX_SCROLL_LINES = 1000
        const val NOT_PAIRED_MESSAGE = "Your iPhone is not connected to this phone's Bluetooth keyboard and mouse."
        val METHODS = setOf("hid.pair.status", "hid.pointer.move", "hid.pointer.click", "hid.pointer.reset", "hid.key", "hid.type", "hid.scroll")
    }
}
