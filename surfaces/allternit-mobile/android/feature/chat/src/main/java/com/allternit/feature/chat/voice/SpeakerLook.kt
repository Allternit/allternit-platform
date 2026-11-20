package com.allternit.feature.chat.voice

import android.content.Context
import android.content.SharedPreferences
import android.provider.Settings
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext

/**
 * Live mode's speaker look (iOS Core/Voice/SpeakerLook.swift, allternit-ai speaker-style.ts): what moves around
 * the speaker's face in voice mode, its colour, and the caption size. Settings › Voice holds the default; each bot
 * can keep its own ("Same as Settings" follows the default). Kept on this phone, like iOS and the web.
 *
 * Android draws Ring (the VoiceRingKit port in VoiceRing.kt) and our own Blocks, ASCII, Glow and Still. Orbit, Sash,
 * Signal, Swell and Pixel keep their keys so a value saved for them round-trips, but they are not offered in the
 * picker until they are ported; a saved one draws as Ring meanwhile ([drawn]).
 */
enum class SpeakerStyle(val raw: String, val title: String, val note: String) {
    Ring("ring", "Ring", "A ring of light that bends with the voice"),
    Orbit("orbit", "Orbit", "Particles on tilted orbits; a voice pulls them in"),
    Sash("sash", "Sash", "A band of dotted lanes; a voice deepens the wave"),
    Signal("signal", "Signal", "A wireframe sphere that a voice bends like a skin"),
    Swell("swell", "Swell", "A halftone sphere of drifting clouds"),
    Pixel("pixel", "Pixel", "A ball of square pixels, lit in rings"),
    Blocks("blocks", "Blocks", "A-block squares pulse around the face"),
    Ascii("ascii", "ASCII", "A ring of characters, like the ASCII backgrounds"),
    Glow("glow", "Glow", "Only a soft halo in the colour"),
    Still("still", "Still", "No effect; the avatar lifts a little while it talks");

    /** The Motion set (voice-ring) or ours (Allternit), as the web and iOS group them. */
    val isMotion: Boolean get() = this in MOTION

    /** Whether this phone draws the style faithfully yet. */
    val isRendered: Boolean get() = this !in NOT_PORTED

    /** What actually draws: an unported Motion style falls back to Ring. */
    val drawn: SpeakerStyle get() = if (isRendered) this else Ring

    companion object {
        private val MOTION = setOf(Ring, Orbit, Sash, Signal, Swell, Pixel)
        private val NOT_PORTED = setOf(Orbit, Sash, Signal, Swell, Pixel)
        fun of(raw: String?): SpeakerStyle? = entries.firstOrNull { it.raw == raw }
        /** The tiles the picker offers for one set. */
        fun offered(motion: Boolean): List<SpeakerStyle> = entries.filter { it.isMotion == motion && it.isRendered }
    }
}

enum class RingPalette(val raw: String, val title: String) {
    Spectrum("spectrum", "Spectrum"),
    Bot("bot", "Bot's colour"),
    Ember("ember", "Ember"),
    Ocean("ocean", "Ocean"),
    Aurora("aurora", "Aurora"),
    Rose("rose", "Rose"),
    Gold("gold", "Gold"),
    Graphite("graphite", "Graphite");

    /** The palette's colours, or null for the ring's own spectrum (iOS RingPalette.colors(bot:)). */
    fun colors(bot: Color?): List<Color>? = when (this) {
        Spectrum -> null
        // A bot without a colour yet (or no bot at all) keeps the ring's own spectrum.
        Bot -> bot?.let { listOf(it, it.copy(alpha = it.alpha * 0.75f), it) }
        Ember -> listOf(Color(0xFFFF6A3D), Color(0xFFFFB347), Color(0xFFD9433B))
        Ocean -> listOf(Color(0xFF2F80ED), Color(0xFF56CCF2), Color(0xFF1B4DB1))
        Aurora -> listOf(Color(0xFF34D399), Color(0xFF22D3EE), Color(0xFFA78BFA))
        Rose -> listOf(Color(0xFFF472B6), Color(0xFFFB7185), Color(0xFFC084FC))
        Gold -> listOf(Color(0xFFD4A93C), Color(0xFFF5DFA0), Color(0xFF9C7A22))
        Graphite -> listOf(Color(0xFF3A3B3F), Color(0xFF8C8E94), Color(0xFF1E1F22))
    }

    companion object {
        fun of(raw: String?): RingPalette? = entries.firstOrNull { it.raw == raw }
        val spectrumColors: List<Color> = listOf(0xFFFF5F6D, 0xFFFFC371, 0xFF47E891, 0xFF3FA9F5, 0xFFA86BFF, 0xFFFF5F6D).map { Color(it) }
        /** Muted and stopped calls. */
        val idle: List<Color> = listOf(Color(0xFF9A9A95), Color(0xFFC9C9C4), Color(0xFF7C7C78))
    }
}

enum class CaptionSize(val raw: String, val title: String, val sp: Float) {
    Small("S", "Small", 15f), Medium("M", "Medium", 17f), Large("L", "Large", 21f);

    companion object {
        fun of(raw: String?): CaptionSize? = entries.firstOrNull { it.raw == raw }
    }
}

/**
 * Who voice mode is talking with, when it is a bot (iOS passes AgentRecord). [face] draws the bot's avatar at the
 * given size in dp; [colorHex] is the bot's accent ("Bot's colour"), null when it has none.
 */
class VoiceBot(
    val id: String,
    val name: String,
    val colorHex: String?,
    val face: @Composable (sizeDp: Float) -> Unit,
) {
    val color: Color? get() = parseHexColor(colorHex)
}

internal fun parseHexColor(hex: String?): Color? {
    val h = hex?.trim()?.takeIf { it.isNotEmpty() } ?: return null
    return runCatching { Color(android.graphics.Color.parseColor(if (h.startsWith("#")) h else "#$h")) }.getOrNull()
}

/**
 * The defaults and each bot's own picks (iOS SpeakerLookStore), in the voice prefs file under the iOS keys.
 * Compose state, so voice mode, the quick bar and every settings section follow a change at once.
 */
object SpeakerLookStore {
    private const val FILE = "allternit_voice"
    private const val STYLE = "voice.speakerStyle"
    private const val PALETTE = "voice.ringPalette"
    private const val CAPTION = "voice.captionSize"

    private var prefs: SharedPreferences? = null

    var style by mutableStateOf(SpeakerStyle.Ring)
        private set
    var palette by mutableStateOf(RingPalette.Bot)
        private set
    var captionSize by mutableStateOf(CaptionSize.Medium)
        private set
    /** Bumps when a bot's own pick changes, so readers recompose. */
    var botRevision by mutableIntStateOf(0)
        private set

    fun load(context: Context) {
        if (prefs != null) return
        val p = context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE)
        prefs = p
        style = SpeakerStyle.of(p.getString(STYLE, null)) ?: SpeakerStyle.Ring
        palette = RingPalette.of(p.getString(PALETTE, null)) ?: RingPalette.Bot
        captionSize = CaptionSize.of(p.getString(CAPTION, null)) ?: CaptionSize.Medium
    }

    fun updateStyle(value: SpeakerStyle) { style = value; prefs?.edit()?.putString(STYLE, value.raw)?.apply() }
    fun updatePalette(value: RingPalette) { palette = value; prefs?.edit()?.putString(PALETTE, value.raw)?.apply() }
    fun updateCaptionSize(value: CaptionSize) { captionSize = value; prefs?.edit()?.putString(CAPTION, value.raw)?.apply() }

    private fun botKey(base: String, botId: String) = "$base.for.bot:$botId"

    /** The bot's own style, or null while it follows Settings. */
    fun ownStyle(botId: String): SpeakerStyle? {
        botRevision // read, so callers recompose on a change
        return SpeakerStyle.of(prefs?.getString(botKey(STYLE, botId), null))
    }

    fun ownPalette(botId: String): RingPalette? {
        botRevision
        return RingPalette.of(prefs?.getString(botKey(PALETTE, botId), null))
    }

    fun setOwnStyle(botId: String, value: SpeakerStyle?) {
        prefs?.edit()?.apply { if (value == null) remove(botKey(STYLE, botId)) else putString(botKey(STYLE, botId), value.raw) }?.apply()
        botRevision++
    }

    fun setOwnPalette(botId: String, value: RingPalette?) {
        prefs?.edit()?.apply { if (value == null) remove(botKey(PALETTE, botId)) else putString(botKey(PALETTE, botId), value.raw) }?.apply()
        botRevision++
    }

    /** What live mode uses for this bot: its own pick, else Settings. */
    fun look(botId: String?): Pair<SpeakerStyle, RingPalette> {
        if (botId == null) return style to palette
        return (ownStyle(botId) ?: style) to (ownPalette(botId) ?: palette)
    }
}

/**
 * iOS @ReducedMotion: the system's animator scale at 0 (Remove animations) or the app's own Settings › Reduce
 * motion switch (SettingsPrefs "allternit.reduceMotion" in "allternit_settings").
 */
@Composable
internal fun rememberReduceMotion(): Boolean {
    val context = LocalContext.current
    return remember(context) {
        val system = Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
        val app = context.applicationContext.getSharedPreferences("allternit_settings", Context.MODE_PRIVATE).getBoolean("allternit.reduceMotion", false)
        system || app
    }
}
