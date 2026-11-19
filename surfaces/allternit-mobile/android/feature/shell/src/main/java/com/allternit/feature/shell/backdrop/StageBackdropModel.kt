package com.allternit.feature.shell.backdrop

import android.content.Context
import android.content.SharedPreferences
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.graphics.Color

/**
 * The bot-screen backgrounds (iOS StageBackdropModel.swift, allternit-ai stage-backdrop-model.ts).
 * Two sets share one preference: Classic (thirteen soft looks drawn in the screen's colours) and
 * ASCII (twenty looks drawn in characters).
 */
internal class BackdropOption(val id: String, val name: String, val note: String)

internal enum class BackdropFamily(val label: String) {
    Ascii("ASCII"), Classic("Classic")
}

/** Where a background can show. Code mode is off until the person turns it on. */
internal enum class BackdropSurface(val raw: String, val label: String) {
    Bot("bot", "Bot chats"),
    Group("group", "Group chats"),
    Project("project", "Bot project homes"),
    Code("code", "Code mode");

    val defaultOn: Boolean get() = this != Code
}

internal enum class BackdropMotion(val raw: String) {
    Still("still"), Slow("slow"), Normal("normal");

    companion object {
        fun from(raw: String?): BackdropMotion? = entries.firstOrNull { it.raw == raw }
    }
}

internal enum class AsciiCharSize(val raw: String) {
    Small("small"), Medium("medium"), Large("large");

    companion object {
        fun from(raw: String?): AsciiCharSize? = entries.firstOrNull { it.raw == raw }
    }
}

internal object StageBackdrops {
    val classic: List<BackdropOption> = listOf(
        BackdropOption("aurora", "Aurora", "Soft colour clouds drift behind frosted glass"),
        BackdropOption("mesh", "Glass mesh", "A slow mesh gradient with fine grain"),
        BackdropOption("constellation", "Constellation", "Drifting points that link up, like the bot graph"),
        BackdropOption("spotlight", "Spotlight", "A soft light follows whoever is speaking"),
        BackdropOption("horizon", "Horizon", "Slow tinted waves under a glass composer"),
        BackdropOption("silk", "Silk", "Wide ribbons of colour sway slowly across the screen"),
        BackdropOption("bokeh", "Bokeh", "Soft out-of-focus lights float upward"),
        BackdropOption("grid", "Grid glow", "A faint dot grid with a slow pulse of light passing through"),
        BackdropOption("lava", "Lava", "Blobs merge and split, like a lava lamp"),
        BackdropOption("prism", "Prism", "A conic gradient turns very slowly, like light through glass"),
        BackdropOption("fireflies", "Fireflies", "Small points drift and blink, warm in the dark"),
        BackdropOption("dusk", "Dusk", "A sky gradient that shifts slowly, your colour glowing low"),
        BackdropOption("plain", "Plain", "Today's flat colour, for those who want it still"),
    )

    val ascii: List<BackdropOption> = AsciiScenes.all.map { BackdropOption(it.id, it.name, it.note) }

    fun list(family: BackdropFamily): List<BackdropOption> = if (family == BackdropFamily.Ascii) ascii else classic

    const val DEFAULT_VARIANT = "aurora"

    private val ids: Set<String> by lazy { (classic + ascii).map { it.id }.toSet() }

    fun isVariant(value: String?): Boolean = value != null && value in ids
    fun isAscii(id: String): Boolean = id.startsWith("ascii-")
    fun family(id: String): BackdropFamily = if (isAscii(id)) BackdropFamily.Ascii else BackdropFamily.Classic
    fun name(id: String): String = (classic + ascii).firstOrNull { it.id == id }?.name ?: "Plain"

    /** Sample colours for previews (Settings tiles, the quick switch). */
    val sampleColors = listOf("#F2A93B", "#0F766E", "#60A5FA", "#A78BFA", "#E07A5F")

    /**
     * The colours a backdrop draws with: one colour gets a lighter partner so the clouds still read
     * as two tones; several (a group) are used as given. [accent] is the fallback with none.
     */
    fun palette(colors: List<String?>, accent: Color): List<Color> {
        val list = colors.filterNotNull()
        if (list.isEmpty()) return listOf(accent, accent.copy(alpha = accent.alpha * 0.7f), accent)
        if (list.size == 1) {
            val light = AsciiHSL.fromHex(list[0])
            light.l += (1 - light.l) * 0.3
            val c = Color(parseHexInt(list[0]))
            return listOf(c, Color(light.toColorInt()), c)
        }
        return list.map { Color(parseHexInt(it)) }
    }
}

/** A screen that keeps its own look: one bot, one group chat or one Code workspace. */
internal class BackdropScope private constructor(val raw: String) {
    override fun equals(other: Any?): Boolean = other is BackdropScope && other.raw == raw
    override fun hashCode(): Int = raw.hashCode()

    companion object {
        fun bot(id: String) = BackdropScope("bot:$id")
        fun group(id: String) = BackdropScope("group:$id")

        /** A project home keeps its own look (iOS StageBackdrops scope for `.project`). */
        fun project(id: String) = BackdropScope("project:$id")

        /** Code mode keeps one look per workspace. */
        fun code(workspaceId: String?) = BackdropScope("code:${workspaceId?.takeIf { it.isNotEmpty() } ?: "default"}")
    }
}

/**
 * The background preferences, kept like the web's appearance settings, live across every reader.
 * Backed by the SharedPreferences file "allternit_shell" with the iOS keys (no prefix).
 */
internal class StageBackdropStore private constructor(context: Context) {
    private val prefs: SharedPreferences =
        context.applicationContext.getSharedPreferences("allternit_shell", Context.MODE_PRIVATE)

    private val variantState = mutableStateOf(
        prefs.getString(VARIANT_KEY, null)?.takeIf { StageBackdrops.isVariant(it) } ?: StageBackdrops.DEFAULT_VARIANT,
    )
    private val motionState = mutableStateOf(BackdropMotion.from(prefs.getString(MOTION_KEY, null)) ?: BackdropMotion.Normal)
    private val colourState = mutableStateOf(AsciiColourMode.from(prefs.getString(COLOUR_KEY, null)) ?: AsciiColourMode.Scene)
    private val charsState = mutableStateOf(AsciiCharMode.from(prefs.getString(CHARS_KEY, null)) ?: AsciiCharMode.Design)
    private val charSizeState = mutableStateOf(AsciiCharSize.from(prefs.getString(CHAR_SIZE_KEY, null)) ?: AsciiCharSize.Medium)
    private val densityState = mutableStateOf(prefs.getFloat(DENSITY_KEY, 1f).toDouble().coerceIn(0.4, 1.6))
    private val veilState = mutableStateOf(prefs.getFloat(VEIL_KEY, 35f).toDouble().coerceIn(20.0, 70.0))
    private val surfaces = mutableStateMapOf<BackdropSurface, Boolean>().also { map ->
        for (s in BackdropSurface.entries) map[s] = prefs.getBoolean(surfaceKey(s), s.defaultOn)
    }
    private val scoped = mutableStateMapOf<String, String>()

    var variant: String
        get() = variantState.value
        set(value) {
            variantState.value = value
            prefs.edit().putString(VARIANT_KEY, value).apply()
        }

    var motion: BackdropMotion
        get() = motionState.value
        set(value) {
            motionState.value = value
            prefs.edit().putString(MOTION_KEY, value.raw).apply()
        }

    var colour: AsciiColourMode
        get() = colourState.value
        set(value) {
            colourState.value = value
            prefs.edit().putString(COLOUR_KEY, value.raw).apply()
        }

    var chars: AsciiCharMode
        get() = charsState.value
        set(value) {
            charsState.value = value
            prefs.edit().putString(CHARS_KEY, value.raw).apply()
        }

    var charSize: AsciiCharSize
        get() = charSizeState.value
        set(value) {
            charSizeState.value = value
            prefs.edit().putString(CHAR_SIZE_KEY, value.raw).apply()
        }

    /** How much is drawn, 0.4-1.6. */
    var density: Double
        get() = densityState.value
        set(value) {
            densityState.value = value
            prefs.edit().putFloat(DENSITY_KEY, value.toFloat()).apply()
        }

    /** The fade behind the conversation, 20-70 (%). Never below 20, so text always has a backing. */
    var veil: Double
        get() = veilState.value
        set(value) {
            veilState.value = value
            prefs.edit().putFloat(VEIL_KEY, value.toFloat()).apply()
        }

    fun isOn(surface: BackdropSurface): Boolean = surfaces[surface] ?: surface.defaultOn

    fun setOn(surface: BackdropSurface, on: Boolean) {
        surfaces[surface] = on
        prefs.edit().putBoolean(surfaceKey(surface), on).apply()
    }

    /** This screen's own look, or null while it follows Settings. */
    fun own(scope: BackdropScope?): String? {
        if (scope == null) return null
        scoped[scope.raw]?.let { return it.ifEmpty { null } }
        val stored = prefs.getString(scopeKey(scope), null)
        return if (StageBackdrops.isVariant(stored)) stored else null
    }

    /** null goes back to "Same as Settings". */
    fun setOwn(scope: BackdropScope, value: String?) {
        scoped[scope.raw] = value ?: ""
        val edit = prefs.edit()
        if (value != null) edit.putString(scopeKey(scope), value) else edit.remove(scopeKey(scope))
        edit.apply()
    }

    /** The background a surface shows: the screen's own look when it has one, else the Settings default, or Plain when that surface is off. */
    fun variantFor(surface: BackdropSurface, scope: BackdropScope?): String {
        own(scope)?.let { return it }
        return if (isOn(surface)) variant else "plain"
    }

    val fontScale: Double
        get() = when (charSize) {
            AsciiCharSize.Small -> 0.85
            AsciiCharSize.Medium -> 1.0
            AsciiCharSize.Large -> 1.25
        }

    companion object {
        const val VARIANT_KEY = "appearance.stageBackground"
        const val MOTION_KEY = "appearance.background.motion"
        const val COLOUR_KEY = "appearance.background.colour"
        const val CHARS_KEY = "appearance.background.chars"
        const val CHAR_SIZE_KEY = "appearance.background.charSize"
        const val DENSITY_KEY = "appearance.background.density"
        const val VEIL_KEY = "appearance.background.veil"

        fun surfaceKey(s: BackdropSurface): String = "appearance.background.on.${s.raw}"
        fun scopeKey(scope: BackdropScope): String = "appearance.stageBackground.for.${scope.raw}"

        @Volatile
        private var instance: StageBackdropStore? = null

        fun get(context: Context): StageBackdropStore =
            instance ?: synchronized(this) {
                instance ?: StageBackdropStore(context).also { instance = it }
            }
    }
}
