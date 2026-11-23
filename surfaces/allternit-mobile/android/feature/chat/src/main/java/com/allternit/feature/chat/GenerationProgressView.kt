package com.allternit.feature.chat

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.provider.Settings
import android.util.Base64
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.material3.Icon
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Popup
import androidx.compose.ui.window.PopupProperties
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR
import kotlin.math.floor
import kotlin.math.hypot
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** How much of the generation a card shows. */
enum class GenerationProgressMode {
    /** The frame itself: the newest turn, and the sheet a tap opens. */
    Full,
    /** The compact card an older turn keeps. */
    History,
}

/** System animator scale 0 = reduce motion (the same check ThinkingOrb uses). */
@Composable
internal fun rememberReducedMotion(): Boolean {
    val context = LocalContext.current
    return remember(context) {
        runCatching { Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }.getOrDefault(false)
    }
}

/** Decoded previews, so a card does not decode a 320 px JPEG on every redraw. */
private object PreviewCache {
    private val cache = object : android.util.LruCache<Int, Bitmap>(16) {}

    /** Previews arrive as `data:image/...` URLs; an API path or http URL is not fetched here. */
    fun bitmap(preview: String): Bitmap? {
        if (!preview.startsWith("data:image/")) return null
        val key = preview.hashCode() * 31 + preview.length
        cache.get(key)?.let { return it }
        val bytes = runCatching { Base64.decode(preview.substringAfter(','), Base64.DEFAULT) }.getOrNull() ?: return null
        val bmp = BitmapFactory.decodeByteArray(bytes, 0, bytes.size) ?: return null
        cache.put(key, bmp)
        return bmp
    }
}

private val PulseTint = Color(0xFF26A0BE)
private val TNUM = TextStyle(fontFeatureSettings = "tnum")

/**
 * The looping Allternit pulse, filling its parent (give it a size). Decorative: the caller owns the status
 * text. Reduced motion draws one still frame. Ink comes from the theme, so it reads in light and graphite dark.
 */
@Composable
internal fun BlockPulse(modifier: Modifier = Modifier, active: Boolean = true) {
    val reduce = rememberReducedMotion()
    var elapsed by remember { mutableFloatStateOf(0f) }
    if (!reduce && active) {
        LaunchedEffect(Unit) {
            val t0 = androidx.compose.runtime.withFrameNanos { it }
            while (true) androidx.compose.runtime.withFrameNanos { elapsed = ((it - t0) / 1e9).toFloat() }
        }
    }
    val ink = AllternitTheme.colors.textPrimary
    val strength = BlockPulseMath.DEFAULT_STRENGTH
    val period = BlockPulseMath.DEFAULT_PERIOD
    Canvas(modifier) {
        if (!active || size.width <= 0f || size.height <= 0f) return@Canvas
        val w = size.width.toDouble()
        val h = size.height.toDouble()
        val pitch = BlockPulseMath.cell(w / density, h / density) * density
        val gap = BlockPulseMath.gap(pitch / density) * density
        val ox = w / 2
        val oy = h / 2
        val reach = BlockPulseMath.reach(ox, oy, w, h)
        val spread = BlockPulseMath.spread(reach)
        val half = (pitch - gap) / 2
        // The resting grid: every square faintly there, so the waves read as light passing over a field.
        var cy = 0
        while (cy * pitch < h) {
            var cx = 0
            while (cx * pitch < w) {
                val px = cx * pitch + pitch / 2
                val py = cy * pitch + pitch / 2
                drawRoundRect(ink.copy(alpha = (strength * BlockPulseMath.RESTING_SHARE).toFloat()), Offset((px - half).toFloat(), (py - half).toFloat()), Size((half * 2).toFloat(), (half * 2).toFloat()), CornerRadius((pitch / 6).toFloat()))
                cx++
            }
            cy++
        }
        val since = if (reduce) BlockPulseMath.stillSince(reach, spread) else elapsed - floor(elapsed / period) * period
        for (s in doubleArrayOf(since, since + period)) {
            if (s < 0 || s > BlockPulseMath.duration(reach, spread)) continue
            val yFrom = floor((oy - reach) / pitch).toInt(); val yTo = kotlin.math.ceil((oy + reach) / pitch).toInt()
            val xFrom = floor((ox - reach) / pitch).toInt(); val xTo = kotlin.math.ceil((ox + reach) / pitch).toInt()
            for (gy in yFrom..yTo) for (gx in xFrom..xTo) {
                val px = gx * pitch + pitch / 2
                val py = gy * pitch + pitch / 2
                val d = hypot(px - ox, py - oy)
                if (d > reach) continue
                val t = BlockPulseMath.tile(s, d, spread) ?: continue
                val centre = d < pitch * 0.8
                val color = if (centre) PulseTint.copy(alpha = (BlockPulseMath.TINT_ALPHA * t.alpha).toFloat()) else ink.copy(alpha = (strength * t.alpha).toFloat())
                val hs = ((pitch - gap) / 2) * t.scale
                drawRoundRect(color, Offset((px - hs).toFloat(), (py - hs).toFloat()), Size((hs * 2).toFloat(), (hs * 2).toFloat()), CornerRadius((pitch / 6 * t.scale).toFloat()))
            }
        }
    }
}

/**
 * What a picture or a motion clip is doing while it is made, one composable for both lanes (the Android side of the
 * web `GenerationProgress`, ai #1224; iOS `GenerationProgressView`). It opens on the block pulse at the final aspect,
 * with the stage and the elapsed seconds along the foot, and changes only when the app reports something real: a
 * picture's preview blurs up in the same box, then "Saving"; a motion clip's pulse gives way to the latest frame,
 * "Frame N of M", a bar from the fraction and the stage chips Render, Encode, Save. Reduced motion: the pulse's
 * still frame and no blur transition.
 */
@Composable
internal fun GenerationProgress(
    callId: String,
    lane: GenerationLane,
    modifier: Modifier = Modifier,
    mode: GenerationProgressMode = GenerationProgressMode.Full,
    aspect: Float? = null,
    store: GenerationProgressStore = GenerationProgressStore.shared,
) {
    val entry = store.entry(callId)
    val detail = entry?.detail
    var now by remember { mutableStateOf(System.currentTimeMillis()) }
    LaunchedEffect(callId) { while (true) { delay(1000); now = System.currentTimeMillis() } }
    val elapsed = entry?.let { ((now - it.startedAtMs) / 1000).coerceAtLeast(0) } ?: 0L
    val main = GenerationProgressDetail.mainLine(detail, lane)
    val fraction = GenerationProgressDetail.barFraction(detail)
    val speak = fraction?.let { "$main, ${Math.round(it * 100)} percent" } ?: main
    Box(
        modifier.semantics(mergeDescendants = true) {
            contentDescription = if (lane == GenerationLane.Picture) "Making a picture" else "Making a clip"
            stateDescription = speak
        },
    ) {
        when {
            mode == GenerationProgressMode.Full -> FullFrame(detail, lane, main, elapsed, fraction, aspect ?: if (lane == GenerationLane.Picture) 1f else 16f / 9f)
            lane == GenerationLane.Picture -> HistoryPicture(detail)
            else -> HistoryMotion(detail, entry?.thumbs.orEmpty(), main)
        }
    }
}

@Composable
private fun FullFrame(detail: GenerationProgressDetail?, lane: GenerationLane, main: String, elapsed: Long, fraction: Double?, ratio: Float) {
    val c = AllternitTheme.colors
    val shape = RoundedCornerShape(16.dp)
    Box(
        Modifier.fillMaxWidth().aspectRatio(ratio).clip(shape).background(c.fillNeutral).border(BorderStroke(1.dp, c.borderSubtle), shape),
    ) {
        Art(detail, thumb = null, lane = lane, modifier = Modifier.fillMaxSize())
        if (lane == GenerationLane.Motion) StageChips(detail?.stage, Modifier.align(Alignment.TopStart).padding(10.dp))
        Column(Modifier.align(Alignment.BottomCenter).fillMaxWidth()) {
            Row(
                Modifier.fillMaxWidth().background(Brush.verticalGradient(0f to Color.Transparent, 0.55f to c.bgPrimary)).padding(start = 14.dp, end = 14.dp, top = 22.dp, bottom = 10.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(main, color = c.textPrimary, fontSize = 12.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                Text("${elapsed}s", color = c.textSecondary, fontSize = 12.5.sp, style = TNUM)
            }
            Bar(fraction, rounded = false)
        }
    }
}

/** The picture of the generation: the pulse until something real arrives. */
@Composable
private fun Art(detail: GenerationProgressDetail?, thumb: String?, lane: GenerationLane, modifier: Modifier) {
    val reduce = rememberReducedMotion()
    val source = thumb ?: detail?.takeIf { it.hasPreview }?.preview
    val bmp = source?.let { PreviewCache.bitmap(it) }
    if (bmp == null) { BlockPulse(modifier); return }
    val target = if (lane == GenerationLane.Picture && thumb == null) (detail?.previewBlur ?: 0).toFloat() else 0f
    val blurDp by animateFloatAsState(target, if (reduce) tween(0) else tween(300), label = "previewBlur")
    Image(
        bmp.asImageBitmap(), contentDescription = null, contentScale = ContentScale.Crop,
        // Modifier.blur works from Android 12 (API 31); older phones show the preview with its fade only.
        modifier = modifier.then(if (blurDp > 0.5f) Modifier.blur(blurDp.dp) else Modifier).scale(if (blurDp > 0.5f) 1.06f else 1f),
    )
}

@Composable
private fun Bar(fraction: Double?, rounded: Boolean) {
    if (fraction == null) return
    val c = AllternitTheme.colors
    val reduce = rememberReducedMotion()
    val f by animateFloatAsState(fraction.toFloat(), if (reduce) tween(0) else tween(300), label = "bar")
    val shape = RoundedCornerShape(if (rounded) 2.dp else 0.dp)
    Box(Modifier.fillMaxWidth().height(3.dp).clip(shape).background(c.fillNeutral)) {
        Box(Modifier.fillMaxWidth(f).height(3.dp).background(c.textPrimary))
    }
}

/** Motion stage chips, in order: Render, Encode, Save. */
@Composable
private fun StageChips(stage: GenerationStage?, modifier: Modifier) {
    val c = AllternitTheme.colors
    val at = GenerationProgressDetail.motionStageIndex(stage)
    Row(modifier, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        listOf("Render", "Encode", "Save").forEachIndexed { k, name ->
            Text(
                (if (k < at) "✓ " else "") + name,
                color = if (k == at) c.textPrimary else c.textSecondary.copy(alpha = if (k < at) 1f else 0.7f),
                fontSize = 11.5.sp,
                modifier = Modifier.clip(CircleShape).background(if (k == at) c.fillNeutral else c.bgPrimary).border(1.dp, c.borderSubtle, CircleShape).padding(horizontal = 8.dp, vertical = 2.dp),
            )
        }
    }
}

@Composable
private fun HistoryPicture(detail: GenerationProgressDetail?) {
    val c = AllternitTheme.colors
    val at = GenerationProgressDetail.pictureStepIndex(detail?.stage)
    val rows = listOf("Queued", "Drawing", "Refining", "Saving")
    val label = GenerationProgressDetail.stageLabel(detail, GenerationLane.Picture)
    val shape = RoundedCornerShape(16.dp)
    Row(
        Modifier.fillMaxWidth().clip(shape).background(c.bgPrimary).border(1.dp, c.borderSubtle, shape).padding(12.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(84.dp).clip(RoundedCornerShape(10.dp)).background(c.fillNeutral)) {
            Art(detail, null, GenerationLane.Picture, Modifier.fillMaxSize())
        }
        Column(verticalArrangement = Arrangement.spacedBy(7.dp)) {
            rows.forEachIndexed { k, name ->
                val on = k == at
                val text = if (on && k >= 2 && !detail?.label.isNullOrBlank()) label else name
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.size(14.dp), contentAlignment = Alignment.Center) {
                        when {
                            k < at -> Text("✓", color = c.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold)
                            on -> Box(Modifier.size(8.dp).clip(CircleShape).background(c.textPrimary))
                            else -> Box(Modifier.size(6.dp).clip(CircleShape).background(c.fillNeutral))
                        }
                    }
                    Text(text, color = if (on) c.textPrimary else c.textSecondary.copy(alpha = 0.7f), fontSize = 13.sp, maxLines = 1)
                }
            }
        }
    }
}

@Composable
private fun HistoryMotion(detail: GenerationProgressDetail?, storedThumbs: List<String>, main: String) {
    val c = AllternitTheme.colors
    val label = GenerationProgressDetail.stageLabel(detail, GenerationLane.Motion)
    val counting = main != label
    val shots = storedThumbs.ifEmpty { listOfNotNull(detail?.preview) }.takeLast(GenerationProgressStore.MAX_THUMBS)
    val bare = detail?.copy(label = "")
    val headline = if (counting) GenerationProgressDetail.stageLabel(bare, GenerationLane.Motion) else label
    val rendering = GenerationProgressDetail.motionStageIndex(detail?.stage) == 0
    val shape = RoundedCornerShape(16.dp)
    Column(
        Modifier.fillMaxWidth().clip(shape).background(c.bgPrimary).border(1.dp, c.borderSubtle, shape).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(headline, color = c.textPrimary, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            if (counting) Text(main.lowercase(), color = c.textSecondary, fontSize = 13.sp, style = TNUM)
        }
        if (shots.isEmpty()) {
            BlockPulse(Modifier.fillMaxWidth().aspectRatio(16f / 9f).clip(RoundedCornerShape(8.dp)).background(c.fillNeutral))
        } else {
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                shots.forEachIndexed { k, src ->
                    val live = k == shots.lastIndex && rendering
                    Box(
                        Modifier.weight(1f).aspectRatio(16f / 9f).clip(RoundedCornerShape(6.dp)).background(c.fillNeutral)
                            .border(if (live) 2.dp else 1.dp, if (live) c.textPrimary else c.borderSubtle, RoundedCornerShape(6.dp)),
                    ) { Art(null, src, GenerationLane.Motion, Modifier.fillMaxSize()) }
                }
            }
        }
        Bar(GenerationProgressDetail.barFraction(detail), rounded = true)
    }
}

// MARK: - Peek, sheet, strip

/** How long to hold before the peek opens (the web hover waits 200 ms; touch holds 350 ms). */
internal const val PEEK_HOLD_MS = 350L

/**
 * Press and hold shows [peek] over the content after 350 ms and releasing closes it; a tap calls [onTap]
 * (the touch version of the web card's hover peek).
 */
@Composable
internal fun PeekOnHold(
    modifier: Modifier = Modifier,
    onTap: () -> Unit,
    peek: (@Composable () -> Unit)?,
    content: @Composable () -> Unit,
) {
    val haptic = LocalHapticFeedback.current
    var peeking by remember { mutableStateOf(false) }
    Box(
        modifier.semantics { role = Role.Button; contentDescription = "Opens the full view" }
            .pointerInput(peek != null) {
                var held = false
                detectTapGestures(
                    onPress = {
                        held = false
                        if (peek != null) coroutineScope {
                            val timer = launch { delay(PEEK_HOLD_MS); held = true; peeking = true; haptic.performHapticFeedback(HapticFeedbackType.LongPress) }
                            tryAwaitRelease()
                            timer.cancel()
                            peeking = false
                        }
                    },
                    onTap = { if (!held) onTap() },
                )
            },
    ) {
        content()
        if (peeking && peek != null) {
            Popup(alignment = Alignment.TopCenter, properties = PopupProperties(focusable = false, clippingEnabled = false)) {
                Box(Modifier.width(280.dp).padding(top = 4.dp)) { peek() }
            }
        }
    }
}

/** The full view a tap opens, in a bottom sheet. Closes itself when the generation ends. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun GenerationSheet(callId: String, lane: GenerationLane, aspect: Float?, onDismiss: () -> Unit, store: GenerationProgressStore = GenerationProgressStore.shared) {
    val c = AllternitTheme.colors
    val ended = store.entry(callId) == null
    LaunchedEffect(ended) { if (ended) onDismiss() }
    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = c.bgPrimary) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp).padding(bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(14.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Text(if (lane == GenerationLane.Picture) "Making a picture" else "Making a clip", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
            GenerationProgress(callId, lane, mode = GenerationProgressMode.Full, aspect = aspect, store = store)
            store.entry(callId)?.detail?.takeIf { it.stage == GenerationStage.Downloading }?.let { d ->
                bytesLine(d)?.let { Text(it, color = c.textSecondary, fontSize = 13.sp, style = TNUM) }
            }
        }
    }
}

/** "3.2 of 8.0 MB, 12s left"; the total and the ETA only when known. */
internal fun bytesLine(d: GenerationProgressDetail): String? {
    val bytes = d.bytes ?: return null
    fun fmt(n: Long) = if (n >= 1_048_576) String.format(java.util.Locale.US, "%.1f MB", n / 1_048_576.0) else if (n >= 1024) "${Math.round(n / 1024.0)} KB" else "$n B"
    var line = fmt(bytes)
    d.totalBytes?.let { line += " of ${fmt(it)}" }
    d.etaSeconds?.let { line += ", ${Math.round(it)}s left" }
    return line
}

/**
 * A running `media_generate` call drawn in the chat: the progress card while it runs (full for the newest turn,
 * compact with a press-and-hold peek for older ones), the failure card in the media frame if it fails. A call that
 * finished draws nothing here; its result is the picture or clip itself.
 */
@Composable
internal fun MediaGenerateCard(
    call: ToolCallRecord,
    isNewestTurn: Boolean,
    replyIsLive: Boolean,
    onChooseModel: () -> Unit,
    onConnect: (Boolean) -> Unit,
    onRetry: () -> Unit,
    store: GenerationProgressStore = GenerationProgressStore.shared,
) {
    when {
        call.state == ToolState.Running && replyIsLive -> {
            val lane = GenerationProgressDetail.lane(call, store.entry(call.id)?.detail)
            val aspect = GenerationProgressDetail.aspect(call)
            var opened by remember { mutableStateOf(false) }
            PeekOnHold(
                onTap = { opened = true },
                peek = if (isNewestTurn) null else ({ GenerationProgress(call.id, lane, mode = GenerationProgressMode.Full, aspect = aspect, store = store) }),
            ) {
                GenerationProgress(call.id, lane, mode = if (isNewestTurn) GenerationProgressMode.Full else GenerationProgressMode.History, aspect = aspect, store = store)
            }
            if (opened) GenerationSheet(call.id, lane, aspect, onDismiss = { opened = false }, store = store)
        }
        call.state == ToolState.Failed -> {
            val lane = GenerationProgressDetail.lane(call, null)
            GenerationFailure(
                info = ChatErrorInfo.make(call.error, lane = if (lane == GenerationLane.Motion) ChatErrorInfo.Lane.Motion else ChatErrorInfo.Lane.Picture).let { if (call.browserImageResult != null) it.copy(action = null) else it },
                lane = lane, aspect = GenerationProgressDetail.aspect(call), onChooseModel = onChooseModel, onConnect = onConnect, onRetry = onRetry,
            )
        }
        call.state == ToolState.Done -> BrowserImageResult(call)
        else -> Unit
    }
}

/**
 * A picture or clip that did not get made (errors, design B): the reason in the server's words, when a usage limit
 * resets, and the way out, drawn in the media frame at the shape the result would have had.
 */
@Composable
internal fun GenerationFailure(
    info: ChatErrorInfo,
    lane: GenerationLane,
    aspect: Float?,
    onChooseModel: () -> Unit,
    onConnect: (Boolean) -> Unit,
    onRetry: () -> Unit,
) {
    val c = AllternitTheme.colors
    val shape = RoundedCornerShape(16.dp)
    val ratio = aspect ?: if (lane == GenerationLane.Picture) 1f else 16f / 9f
    val errLane = if (lane == GenerationLane.Motion) ChatErrorInfo.Lane.Motion else ChatErrorInfo.Lane.Picture
    Box(
        Modifier.fillMaxWidth().aspectRatio(ratio).heightInMin().clip(shape).background(c.fillNeutral).border(1.dp, AllternitStatic.StatusWarning.copy(alpha = 0.4f), shape).padding(16.dp),
        contentAlignment = Alignment.Center,
    ) {
        ErrorBody(info, errLane, onChooseModel, onConnect, onRetry, center = true)
    }
}

private fun Modifier.heightInMin(): Modifier = this.then(Modifier.defaultMinSize(minHeight = 160.dp))

/** Title, reason and the way out of a typed error: shared by the media frame and the chat's inline error card. */
@Composable
internal fun ErrorBody(info: ChatErrorInfo, lane: ChatErrorInfo.Lane, onChooseModel: () -> Unit, onConnect: (Boolean) -> Unit, onRetry: () -> Unit, center: Boolean) {
    val c = AllternitTheme.colors
    val align = if (center) TextAlign.Center else TextAlign.Start
    Column(horizontalAlignment = if (center) Alignment.CenterHorizontally else Alignment.Start, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Icon(painterResource(DesignR.drawable.ph_warning), contentDescription = null, tint = AllternitStatic.StatusWarning, modifier = Modifier.size(18.dp))
        Text(info.title(lane), color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, textAlign = align)
        Text(info.reason, color = c.textSecondary, fontSize = 12.sp, lineHeight = 16.sp, textAlign = align)
        info.actionLabel?.let { label ->
            CapsuleButton(label, primary = true) {
                when (info.action) {
                    ChatErrorInfo.Action.Connect -> onConnect(info.isChatGPT)
                    ChatErrorInfo.Action.PickModel -> onChooseModel()
                    ChatErrorInfo.Action.Retry -> onRetry()
                    null -> Unit
                }
            }
        }
        // A usage limit also offers a plain retry once it resets.
        if (info.action == ChatErrorInfo.Action.PickModel) CapsuleButton("Try again") { onRetry() }
    }
}

/**
 * The running strip above the composer: one chip per running generation (oldest first). A tap opens the full view in a
 * sheet; press and hold peeks at it. Nothing when nothing runs.
 */
@Composable
internal fun RunningStrip(modifier: Modifier = Modifier, store: GenerationProgressStore = GenerationProgressStore.shared) {
    val ids = store.order
    if (ids.isEmpty()) return
    val c = AllternitTheme.colors
    var opened by remember { mutableStateOf<String?>(null) }
    Row(modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        ids.forEach { id ->
            val entry = store.entry(id) ?: return@forEach
            val lane = entry.detail.lane
            val main = GenerationProgressDetail.mainLine(entry.detail, lane)
            PeekOnHold(
                onTap = { opened = id },
                peek = { GenerationProgress(id, lane, mode = GenerationProgressMode.Full, store = store) },
            ) {
                Row(
                    Modifier.clip(CircleShape).background(c.fillNeutral).border(1.dp, c.borderSubtle, CircleShape).padding(horizontal = 12.dp, vertical = 6.dp),
                    horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(Modifier.size(7.dp).clip(CircleShape).background(PulseTint))
                    Text((if (lane == GenerationLane.Picture) "Picture · " else "Clip · ") + main, color = c.textPrimary, fontSize = 12.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 220.dp))
                }
            }
        }
    }
    opened?.let { id -> GenerationSheet(id, store.entry(id)?.detail?.lane ?: GenerationLane.Picture, null, onDismiss = { opened = null }, store = store) }
}
