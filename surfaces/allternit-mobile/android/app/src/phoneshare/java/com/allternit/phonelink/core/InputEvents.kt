package com.allternit.phonelink.core

import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min

/**
 * Coordinate spaces for viewer input. Viewer x/y are in the streamed VIDEO's
 * pixels (the last CONFIG width/height); gestures go to the screen in real
 * pixels. Step 4's capture code supplies this from the current projection.
 */
data class InputGeometry(val videoWidth: Int, val videoHeight: Int, val screenWidth: Int, val screenHeight: Int)

/** How many events were carried out and how many had no Android equivalent here. */
data class InputResult(val handled: Int, val ignored: Int)

/**
 * `input.events` (control RPC, full-control viewer only): validates the batch
 * the relay forwarded and drives [DeviceOps]. Event shapes match
 * PhoneInputEvent in the Desktop (electron-api.d.ts):
 *
 *   touch  {action: down|move|up, pointerId, x, y, pressure?}
 *   key    {action: down|up, keyCode, metaState?}
 *   text   {text}
 *   scroll {x, y, hScroll?, vScroll?}
 *   back | home | app_switch | power
 *
 * AccessibilityService can only dispatch whole gestures, so a touch sequence
 * is collected (down, move..., up) and played on `up`: a short still press is a
 * tap, a long still press is a press, anything that travelled is a swipe.
 * Not thread-safe: call from one serial executor so events stay in order.
 */
class InputEvents(
    private val ops: DeviceOps,
    private val geometry: () -> InputGeometry?,
    private val clock: () -> Long = { System.currentTimeMillis() },
) {
    private class Down(val x: Float, val y: Float, val at: Long)

    private val downs = HashMap<Int, Down>()

    fun handle(params: JSONObject): InputResult = apply(parse(params))

    fun apply(events: List<Event>): InputResult {
        var handled = 0
        var ignored = 0
        for (event in events) {
            val ok = try { run(event) } catch (e: RpcException) { false }
            if (ok) handled++ else ignored++
        }
        return InputResult(handled, ignored)
    }

    sealed class Event {
        data class Touch(val action: String, val pointerId: Int, val x: Double, val y: Double) : Event()
        data class Key(val action: String, val keyCode: Int) : Event()
        data class Text(val text: String) : Event()
        data class Scroll(val x: Double, val y: Double, val h: Double, val v: Double) : Event()
        data class Nav(val type: String) : Event()
    }

    private fun run(event: Event): Boolean = when (event) {
        is Event.Touch -> touch(event)
        is Event.Key -> key(event)
        is Event.Text -> text(event.text)
        is Event.Scroll -> scroll(event)
        is Event.Nav -> when (event.type) {
            "back" -> ops.globalAction(GlobalAction.BACK)
            "home" -> ops.globalAction(GlobalAction.HOME)
            "app_switch" -> ops.globalAction(GlobalAction.RECENTS)
            else -> false // power: no accessibility equivalent
        }
    }

    // ── Mapping ─────────────────────────────────────────────────────────────

    private fun toScreen(x: Double, y: Double): Pair<Float, Float> {
        val g = geometry()
        if (g == null || g.videoWidth <= 0 || g.videoHeight <= 0) return Pair(max(0.0, x).toFloat(), max(0.0, y).toFloat())
        val sx = (x * g.screenWidth / g.videoWidth).coerceIn(0.0, (g.screenWidth - 1).toDouble())
        val sy = (y * g.screenHeight / g.videoHeight).coerceIn(0.0, (g.screenHeight - 1).toDouble())
        return Pair(sx.toFloat(), sy.toFloat())
    }

    private fun screenMin(): Int = geometry()?.let { min(it.screenWidth, it.screenHeight) } ?: 720
    private fun screenHeight(): Int = geometry()?.screenHeight ?: 1600

    private fun touch(e: Event.Touch): Boolean {
        if (e.pointerId != 0) return false // no multi-touch through accessibility
        val (x, y) = toScreen(e.x, e.y)
        val now = clock()
        when (e.action) {
            "down" -> {
                downs.entries.removeAll { now - it.value.at > STALE_DOWN_MS }
                downs[0] = Down(x, y, now)
                return true
            }
            "move" -> return downs.containsKey(0)
            "up" -> {
                val d = downs.remove(0) ?: return false
                val dist = hypot((x - d.x).toDouble(), (y - d.y).toDouble())
                val held = max(0L, now - d.at)
                val slop = max(MIN_SLOP_PX, screenMin() * SLOP_FRACTION)
                return if (dist < slop) {
                    if (held < LONG_PRESS_MS) ops.tap(d.x, d.y) else ops.press(d.x, d.y, min(held, MAX_PRESS_MS))
                } else {
                    ops.swipe(d.x, d.y, x, y, held.coerceIn(MIN_SWIPE_MS, MAX_SWIPE_MS))
                }
            }
        }
        return false
    }

    /** Wheel up (positive vScroll) moves the finger down, like scrcpy's mouse wheel. */
    private fun scroll(e: Event.Scroll): Boolean {
        val (x, y) = toScreen(e.x, e.y)
        val h = screenHeight().toFloat()
        val w = (geometry()?.screenWidth ?: 720).toFloat()
        val dy = (e.v * h * SCROLL_UNIT).coerceIn(-h * MAX_SCROLL, h * MAX_SCROLL).toFloat()
        val dx = (e.h * w * SCROLL_UNIT).coerceIn(-w * MAX_SCROLL, w * MAX_SCROLL).toFloat()
        if (dx == 0f && dy == 0f) return false
        val x2 = (x - dx).coerceIn(0f, w - 1)
        val y2 = (y + dy).coerceIn(0f, h - 1)
        return ops.swipe(x, y, x2, y2, SCROLL_MS)
    }

    private fun key(e: Event.Key): Boolean {
        if (e.action != "up") return false // one action per key press
        return when (e.keyCode) {
            KEYCODE_HOME -> ops.globalAction(GlobalAction.HOME)
            KEYCODE_BACK -> ops.globalAction(GlobalAction.BACK)
            KEYCODE_APP_SWITCH -> ops.globalAction(GlobalAction.RECENTS)
            KEYCODE_ENTER -> ops.focusedInput()?.let { ops.imeEnter(it) } ?: false
            KEYCODE_DEL -> {
                val node = ops.focusedInput() ?: return false
                val current = node.text.orEmpty()
                current.isNotEmpty() && ops.setText(node, current.dropLast(1))
            }
            else -> false
        }
    }

    private fun text(text: String): Boolean {
        if (text.isEmpty()) return false
        val node = ops.focusedInput() ?: return false
        val current = node.text.orEmpty()
        return ops.setText(node, current + text) || ops.paste(node, text)
    }

    // ── Parsing ─────────────────────────────────────────────────────────────

    companion object {
        const val MAX_EVENTS = 64
        const val MAX_TEXT = 2000
        const val KEYCODE_HOME = 3
        const val KEYCODE_BACK = 4
        const val KEYCODE_ENTER = 66
        const val KEYCODE_DEL = 67
        const val KEYCODE_APP_SWITCH = 187

        private const val LONG_PRESS_MS = 500L
        private const val MAX_PRESS_MS = 3000L
        private const val MIN_SWIPE_MS = 50L
        private const val MAX_SWIPE_MS = 2000L
        private const val STALE_DOWN_MS = 15_000L
        private const val MIN_SLOP_PX = 16f
        private const val SLOP_FRACTION = 0.02f
        private const val SCROLL_UNIT = 0.15
        private const val MAX_SCROLL = 0.6
        private const val SCROLL_MS = 200L
        private val NAV = setOf("back", "home", "app_switch", "power")

        private fun bad(message: String) = RpcException(RpcException.INVALID_PARAMS, message, "invalid_params")

        /** Validates `{"events":[…]}`; throws RpcException(INVALID_PARAMS) on any bad event. */
        fun parse(params: JSONObject): List<Event> {
            val arr: JSONArray = params.optJSONArray("events") ?: throw bad("events must be an array")
            if (arr.length() == 0 || arr.length() > MAX_EVENTS) throw bad("events must hold 1..$MAX_EVENTS items")
            val out = ArrayList<Event>(arr.length())
            for (i in 0 until arr.length()) {
                val o = arr.optJSONObject(i) ?: throw bad("event $i is not an object")
                out.add(parseOne(o, i))
            }
            return out
        }

        private fun num(o: JSONObject, key: String, i: Int): Double {
            val v = (o.opt(key) as? Number)?.toDouble() ?: throw bad("event $i: $key must be a number")
            if (v.isNaN() || v.isInfinite()) throw bad("event $i: $key must be finite")
            return v
        }

        private fun optNum(o: JSONObject, key: String, i: Int): Double =
            if (o.isNull(key) || !o.has(key)) 0.0 else num(o, key, i)

        private fun parseOne(o: JSONObject, i: Int): Event {
            val type = o.opt("type") as? String ?: throw bad("event $i: type is missing")
            return when (type) {
                "touch" -> {
                    val action = o.opt("action") as? String
                    if (action != "down" && action != "move" && action != "up") throw bad("event $i: bad touch action")
                    val pid = num(o, "pointerId", i)
                    if (pid < 0 || pid != Math.floor(pid)) throw bad("event $i: bad pointerId")
                    Event.Touch(action, pid.toInt(), num(o, "x", i), num(o, "y", i))
                }
                "key" -> {
                    val action = o.opt("action") as? String
                    if (action != "down" && action != "up") throw bad("event $i: bad key action")
                    val code = num(o, "keyCode", i)
                    if (code < 0 || code != Math.floor(code)) throw bad("event $i: bad keyCode")
                    Event.Key(action, code.toInt())
                }
                "text" -> {
                    val t = o.opt("text") as? String ?: throw bad("event $i: text must be a string")
                    if (t.length > MAX_TEXT) throw bad("event $i: text too long")
                    Event.Text(t)
                }
                "scroll" -> Event.Scroll(num(o, "x", i), num(o, "y", i), optNum(o, "hScroll", i), optNum(o, "vScroll", i))
                in NAV -> Event.Nav(type)
                else -> throw bad("event $i: unknown type")
            }
        }
    }
}
