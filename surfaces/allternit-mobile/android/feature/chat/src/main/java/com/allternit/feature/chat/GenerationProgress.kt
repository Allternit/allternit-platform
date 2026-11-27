package com.allternit.feature.chat

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlin.math.hypot
import kotlinx.serialization.json.JsonObject

enum class GenerationLane { Picture, Motion }

enum class GenerationStage { Queued, Drawing, Rendering, Encoding, Saving, Downloading }

/**
 * Live progress of one picture or motion generation: the shape every surface mirrors (gizzi
 * `PaneRender.ProgressDetail`, platform #2802; iOS `GenerationProgressDetail`). Read from a running
 * `media_generate` call's `metadata.progress_detail`.
 */
data class GenerationProgressDetail(
    val lane: GenerationLane,
    val stage: GenerationStage,
    /** What is happening, in plain words ("Frame 12 of 180"). */
    val label: String,
    val frame: Int? = null,
    val frames: Int? = null,
    /** 0 to 1, of the current stage. */
    val fraction: Double? = null,
    /** A thumbnail (`data:image` URL, at most 320 px wide) or an API path. */
    val preview: String? = null,
    val bytes: Long? = null,
    val totalBytes: Long? = null,
    val etaSeconds: Double? = null,
) {
    /** Blur (dp) of a picture preview: heavy early, none once it is saving. */
    val previewBlur: Int
        get() {
            if (stage == GenerationStage.Saving || stage == GenerationStage.Downloading) return 0
            val f = (fraction ?: 0.35).coerceIn(0.0, 1.0)
            return Math.round(24 * (1 - f)).toInt()
        }

    /** A real picture to show instead of the pulse. */
    val hasPreview: Boolean get() = preview != null && stage != GenerationStage.Queued && stage != GenerationStage.Drawing

    companion object {
        /** `metadata.progress_detail`; null when absent or malformed (a bad lane or stage is dropped, never guessed). */
        fun read(metadata: JsonObject?): GenerationProgressDetail? {
            val d = metadata?.obj("progress_detail") ?: return null
            val lane = when (d.str("lane")) { "picture" -> GenerationLane.Picture; "motion" -> GenerationLane.Motion; else -> return null }
            val stage = when (d.str("stage")) {
                "queued" -> GenerationStage.Queued; "drawing" -> GenerationStage.Drawing; "rendering" -> GenerationStage.Rendering
                "encoding" -> GenerationStage.Encoding; "saving" -> GenerationStage.Saving; "downloading" -> GenerationStage.Downloading
                else -> return null
            }
            return GenerationProgressDetail(
                lane = lane, stage = stage, label = d.str("label").orEmpty(),
                frame = d.long("frame")?.toInt(), frames = d.long("frames")?.toInt(), fraction = d.double("fraction"),
                preview = d.str("preview")?.takeIf { it.isNotEmpty() },
                bytes = d.long("bytes"), totalBytes = d.long("total_bytes"), etaSeconds = d.double("eta_s"),
            )
        }

        val openingLabel = mapOf(GenerationLane.Picture to "Generating image", GenerationLane.Motion to "Rendering video")

        fun stageWords(stage: GenerationStage) = when (stage) {
            GenerationStage.Queued -> "Waiting in line"
            GenerationStage.Drawing -> "Drawing"
            GenerationStage.Rendering -> "Rendering frames"
            GenerationStage.Encoding -> "Encoding the video"
            GenerationStage.Saving -> "Saving"
            GenerationStage.Downloading -> "Downloading"
        }

        /** The words for the stage, from the runtime's label when it sent one. */
        fun stageLabel(detail: GenerationProgressDetail?, lane: GenerationLane): String {
            if (detail == null) return openingLabel[lane] ?: "Generating"
            return detail.label.trim().ifEmpty { stageWords(detail.stage) }
        }

        /** The main line: "Frame N of M" while motion frames arrive, else the stage label. */
        fun mainLine(detail: GenerationProgressDetail?, lane: GenerationLane): String {
            val frames = detail?.frames
            val frame = detail?.frame
            if (lane == GenerationLane.Motion && detail != null && detail.stage == GenerationStage.Rendering && frames != null && frame != null) {
                return "Frame ${minOf(frame, frames)} of $frames"
            }
            return stageLabel(detail, lane)
        }

        /** 0..1 for the bar; null when nothing is known (no bar). */
        fun barFraction(detail: GenerationProgressDetail?): Double? {
            val d = detail ?: return null
            d.fraction?.let { return it.coerceIn(0.0, 1.0) }
            val frames = d.frames
            val frame = d.frame
            if (d.stage == GenerationStage.Rendering && frames != null && frames > 0 && frame != null) return (frame.toDouble() / frames).coerceIn(0.0, 1.0)
            return null
        }

        /** Which motion chip is active: -1 before frames start, 0 render, 1 encode, 2 save. */
        fun motionStageIndex(stage: GenerationStage?) = when (stage) {
            GenerationStage.Rendering -> 0
            GenerationStage.Encoding -> 1
            GenerationStage.Saving, GenerationStage.Downloading -> 2
            else -> -1
        }

        /** Picture rows of the history card: 0 queued, 1 drawing, 2 refining, 3 saving. */
        fun pictureStepIndex(stage: GenerationStage?) = when (stage) {
            null, GenerationStage.Queued -> 0
            GenerationStage.Drawing -> 1
            GenerationStage.Rendering -> 2
            else -> 3
        }

        /** True for the `media_generate` tool (any spelling: media_generate, mediaGenerate, media-generate). */
        fun isMediaGenerate(toolName: String) = toolName.lowercase().filter { it.isLetterOrDigit() } == "mediagenerate"

        /** The lane: what the app reported, else the call's own `kind`. */
        fun lane(call: ToolCallRecord, reported: GenerationProgressDetail?): GenerationLane =
            reported?.lane ?: if (call.input?.str("kind")?.lowercase() == "video") GenerationLane.Motion else GenerationLane.Picture

        /** The final picture's shape from the call's width and height, when it gave them (clamped to 1:3..3:1). */
        fun aspect(call: ToolCallRecord): Float? {
            val w = call.input?.double("width") ?: return null
            val h = call.input?.double("height") ?: return null
            if (w <= 0 || h <= 0) return null
            return (w / h).coerceIn(1.0 / 3.0, 3.0).toFloat()
        }
    }
}

/**
 * Live progress of media generations, keyed by the `media_generate` tool call id. The chat list recycles
 * cards of older turns, so what a card shows (stage, start time, the last motion thumbnails) lives here
 * and survives a remount. The press-and-hold peek and the running strip read the same store.
 */
class GenerationProgressStore {
    data class Entry(val detail: GenerationProgressDetail, val startedAtMs: Long, val thumbs: List<String>)

    var entries by mutableStateOf<Map<String, Entry>>(emptyMap())
        private set
    /** Oldest first. */
    var order by mutableStateOf<List<String>>(emptyList())
        private set

    fun entry(id: String): Entry? = entries[id]

    /**
     * How a running picture card ended up looking, so the finished image can replace it the right way: true = a halftone
     * preview was shown (crossfade), false = only the dot field (cells bloom). Not state: read once at completion.
     */
    private val revealStyles = object : LinkedHashMap<String, Boolean>() {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, Boolean>?) = size > MAX_ENTRIES
    }

    /** A picture card drew [halftone] (or just the field) for [id]; a halftone, once shown, stays true. */
    fun noteShown(id: String, halftone: Boolean) { synchronized(revealStyles) { revealStyles[id] = (revealStyles[id] == true) || halftone } }

    /** Returns and forgets how [id]'s card looked: null if it was never drawn running (a finished chat reloading), so no reveal replays. */
    fun takeRevealStyle(id: String): Boolean? = synchronized(revealStyles) { revealStyles.remove(id) }

    fun set(id: String, detail: GenerationProgressDetail, nowMs: Long = System.currentTimeMillis()) {
        val prev = entries[id]
        var thumbs = prev?.thumbs.orEmpty()
        val preview = detail.preview
        if (detail.lane == GenerationLane.Motion && preview != null && thumbs.lastOrNull() != preview) {
            thumbs = (thumbs + preview).takeLast(MAX_THUMBS)
        }
        val next = Entry(detail, prev?.startedAtMs ?: nowMs, thumbs)
        if (prev == next) return
        val map = entries + (id to next)
        var ids = if (id in order) order else order + id
        val dropped = mutableListOf<String>()
        while (ids.size > MAX_ENTRIES) { dropped += ids.first(); ids = ids.drop(1) }
        entries = map - dropped.toSet()
        order = ids
    }

    fun clear(id: String) {
        if (id !in entries) return
        entries = entries - id
        order = order - id
    }

    /**
     * Keeps the store in step with one message's tool calls: a running `media_generate` call of a live reply
     * with a report is recorded, any other call is cleared (done, failed, stopped, or the reply ended).
     */
    fun sync(calls: List<ToolCallRecord>, replyIsLive: Boolean, nowMs: Long = System.currentTimeMillis()) {
        for (call in calls) {
            if (!GenerationProgressDetail.isMediaGenerate(call.toolName)) continue
            if (replyIsLive && call.state == ToolState.Running) {
                GenerationProgressDetail.read(call.metadata)?.let { set(call.id, it, nowMs) }
            } else {
                clear(call.id)
            }
        }
    }

    companion object {
        /** Motion keeps this many thumbnails (the history card shows them). */
        const val MAX_THUMBS = 4
        /** Entries kept at once; the oldest goes first. */
        const val MAX_ENTRIES = 24
        /** One store for the app: the chat, the strip and the sheet read the same one. */
        val shared = GenerationProgressStore()
    }
}

/**
 * The numbers of the Allternit block pulse, ported from the web `BlockPulse.tsx` (`drawBlockPulse`,
 * `pulseCell`, `PULSE_TILE_S`, `RISE`) through the iOS `BlockPulseMath`: rounded squares on a grid pop in
 * from an origin, delayed by their distance from it, then fade.
 */
object BlockPulseMath {
    /** PULSE_TILE_S: seconds one square takes to pop in and fade out. */
    const val TILE_SECONDS = 0.7
    /** RISE: the pop-in share of a square's life; the rest is the fade. */
    const val RISE = 0.35
    const val DEFAULT_SPREAD = 0.00045
    /** Seconds between waves, and the peak opacity of the squares. */
    const val DEFAULT_PERIOD = 0.9
    const val DEFAULT_STRENGTH = 0.18
    /** The centre square (within 0.8 cells of the origin) wears the tint at this opacity. */
    const val TINT_ALPHA = 0.5
    /** The resting grid is the squares' opacity times this. */
    const val RESTING_SHARE = 0.22

    data class Tile(val alpha: Double, val scale: Double)

    fun duration(reach: Double, spread: Double = DEFAULT_SPREAD) = TILE_SECONDS + reach * spread

    /** The grid pitch for a box, about a dozen squares across, never tiny or huge. */
    fun cell(width: Double, height: Double): Double = maxOf(12.0, minOf(30.0, Math.round(minOf(width, height) / 10).toDouble()))

    fun gap(pitch: Double) = maxOf(3.0, pitch * 0.27)

    /** A ring that crosses the box in about half a second, whatever its size. */
    fun spread(reach: Double) = if (reach > 0) 0.5 / reach else DEFAULT_SPREAD

    /** Reach to the farthest corner, so the whole box fills. */
    fun reach(ox: Double, oy: Double, w: Double, h: Double) = maxOf(hypot(ox, oy), hypot(w - ox, oy), hypot(ox, h - oy), hypot(w - ox, h - oy))

    /** One square's opacity share and scale at [since] seconds into a wave; null outside its life. */
    fun tile(since: Double, distance: Double, spread: Double): Tile? {
        val q = (since - distance * spread) / TILE_SECONDS
        if (q < 0 || q > 1) return null
        val a = if (q < RISE) q / RISE else 1 - (q - RISE) / (1 - RISE)
        val sc = if (q < RISE) 0.3 + 0.7 * (q / RISE) else 1 - 0.15 * ((q - RISE) / (1 - RISE))
        return Tile(a, sc)
    }

    /** The still frame reduced motion shows: mid-wave. */
    fun stillSince(reach: Double, spread: Double) = RISE * TILE_SECONDS + reach * spread * 0.5
}

/** Chat wait, design pick B: the sent message anchors at the top and the answer grows into the room below (iOS `AnswerRoom`). */
object AnswerRoom {
    /** The message to anchor when a turn starts running: the newest one the person sent; null when no turn just started. */
    fun anchorId(wasStreaming: Boolean, isStreaming: Boolean, messages: List<MessageRecord>): String? {
        if (!isStreaming || wasStreaming) return null
        return messages.lastOrNull { it.role == "user" }?.id
    }

    /** Height of the spacer under the anchored turn: what is missing to fill one view (px or dp, one unit throughout). */
    fun room(viewport: Float, turnHeight: Float, topGap: Float, rowSpacing: Float): Float =
        maxOf(0f, kotlin.math.ceil(viewport - turnHeight - topGap - rowSpacing))

    /** An anchored turn that is still running must not drag the view to the bottom. */
    fun followsBottom(anchored: Boolean, streaming: Boolean) = !(anchored && streaming)
}

/**
 * Whether a chat follows its streaming reply. Rule: only the reader's own drag releases it; text growing,
 * layout changes and our own pinning never do. It re-follows when the reader reaches the bottom, taps the
 * "New text" pill ([jumped]) or sends ([sent]). [hasNew] is true while text arrived below a released reader.
 * Immutable: hold it in Compose state and replace it with the returned value.
 */
data class TailFollow(val follows: Boolean = true, val hasNew: Boolean = false) {
    fun userScrolledUp() = copy(follows = false)
    fun reachedBottom() = TailFollow(follows = true, hasNew = false)
    fun grew() = if (follows) this else copy(hasNew = true)
    fun jumped() = TailFollow()
    fun sent() = TailFollow()
}
