package com.allternit.feature.shell.firstrun

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.util.LruCache
import android.widget.VideoView
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import com.allternit.core.design.ThinkingOrb
import com.allternit.core.design.EmptyArtKind
import com.allternit.core.design.InlineEmpty
import com.allternit.core.design.ThinkingOrbSize
import com.allternit.core.design.ThinkingOrbState
import com.allternit.feature.shell.R
import kotlin.math.cos
import kotlin.math.sin
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import okhttp3.OkHttpClient
import okhttp3.Request

// The pictures of "Meet your bots" (iOS MeetYourBotsVisuals.swift, MeetBotCatalog tiles).

internal fun hexColor(hex: String?, fallback: Color = Color(0xFF141413)): Color =
    runCatching { Color(android.graphics.Color.parseColor(hex ?: "")) }.getOrDefault(fallback)

/** The soft card fill behind the visuals (iOS MeetPalette.soft). */
internal val meetSoft: Color @Composable get() = if (setupColors.bg == Color.White) Color(0xFFFAFAFB) else Color(0xFF18181A)

/** Frame 0 (the idle pose) of a web sprite sheet, cached (192×208 frames, iOS SpriteAvatar still). */
internal object SpriteCache {
    private val http by lazy { OkHttpClient() }
    private val cache = LruCache<String, ImageBitmap>(48)

    suspend fun frame(url: String): ImageBitmap? {
        cache.get(url)?.let { return it }
        return withContext(Dispatchers.IO) {
            runCatching {
                http.newCall(Request.Builder().url(url).build()).execute().use { r ->
                    if (!r.isSuccessful) return@use null
                    val bytes = r.body?.bytes() ?: return@use null
                    val sheet = BitmapFactory.decodeByteArray(bytes, 0, bytes.size) ?: return@use null
                    val fw = minOf(192, sheet.width)
                    val fh = minOf(208, sheet.height)
                    Bitmap.createBitmap(sheet, 0, 0, fw, fh).asImageBitmap()
                }
            }.getOrNull()?.also { cache.put(url, it) }
        }
    }
}

@Composable
private fun Sprite(url: String, size: Dp) {
    val bitmap by produceState<ImageBitmap?>(null, url) { value = SpriteCache.frame(url) }
    bitmap?.let { Image(it, contentDescription = null, contentScale = ContentScale.Fit, modifier = Modifier.size(size)) }
}

/** A template's tile: its colour with its look's sprite, else its initial (iOS MeetTemplateTile). */
@Composable
internal fun MeetTemplateTile(color: String, look: String?, label: String, size: Dp = 40.dp) {
    val shape = RoundedCornerShape(size * 0.28f)
    Box(Modifier.size(size).clip(shape).background(hexColor(color), shape).clearAndSetSemantics {}, contentAlignment = Alignment.Center) {
        if (look != null) Sprite(MeetBotCatalog.spriteUrl(look), size)
        else Text(label.take(1).uppercase(), fontSize = (size.value * 0.4f).sp, fontWeight = FontWeight.Bold, color = Color.White)
    }
}

/** A look's tile: the A:// mark, Gizzi or a cast sprite (iOS MeetLookTile). */
@Composable
internal fun MeetLookTile(id: String, catalog: MeetBotCatalog, size: Dp = 40.dp, live: Boolean = false) {
    val shape = RoundedCornerShape(size * 0.28f)
    val field = if (id == "a") "#141413" else catalog.looks.firstOrNull { it.id == id }?.field ?: "#141413"
    Box(Modifier.size(size).clip(shape).background(hexColor(field), shape).clearAndSetSemantics {}, contentAlignment = Alignment.Center) {
        when (id) {
            "a" -> MarkOrb(size * 0.52f, live)
            "gizzi" -> SetupGizzi(size * 0.78f)
            else -> Sprite(MeetBotCatalog.spriteUrl(id), size)
        }
    }
}

/** The orb in white on the ink tile. */
@Composable
private fun MarkOrb(size: Dp, live: Boolean) {
    Box(Modifier.size(size), contentAlignment = Alignment.Center) {
        // ThinkingOrb inks by theme; on the ink tile it is drawn in a dark scope so it reads white.
        com.allternit.core.design.AllternitTheme(themeMode = com.allternit.core.design.ThemeMode.Dark) {
            ThinkingOrb(state = if (live) ThinkingOrbState.Working else ThinkingOrbState.Connecting, size = ThinkingOrbSize.Inline,
                paused = !live, modifier = Modifier.size(size))
        }
    }
}

/** A bot already on the account: its look's sprite, else its colour and initial. */
@Composable
internal fun MeetBotTile(bot: MeetRosterBot, size: Dp = 40.dp) {
    MeetTemplateTile(bot.color ?: "#E4E4E7", bot.look, bot.displayName, size)
}

// Start: the bots network (iOS MeetNetworkView)

private data class Say(val from: Double, val to: Double, val name: String, val text: String, val x: Float, val bottom: Float)

private val SAYS = listOf(
    Say(0.9, 2.6, "Claude", "A new client brief came in. Dots, can you draft the plan?", 12f, 76f),
    Say(2.7, 4.4, "Dots", "Plan drafted. Grok, can you check the numbers?", 208f, 76f),
    Say(4.5, 6.2, "Grok", "Two numbers were off. Fixed. Muse, send it for sign-off?", 208f, 241.5f),
    Say(6.3, 8.4, "Muse", "Sent to you on WhatsApp for approval.", 12f, 237.7f),
)

/**
 * Bots hand a job to each other (web TeamNetwork): the web's own animation, recorded as a 16 s
 * loop (`res/raw/meet_team_network.mp4`, the iOS bundle's file), with the speech bubbles drawn on
 * top. Reduce motion shows the still frame and no bubbles.
 */
@Composable
internal fun MeetNetworkView(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val reduce = reducedMotion()
    var position by remember { mutableLongStateOf(0L) }
    var player by remember { mutableStateOf<VideoView?>(null) }
    LaunchedEffect(player) {
        val p = player ?: return@LaunchedEffect
        while (true) { position = p.currentPosition.toLong(); delay(100) }
    }
    BoxWithConstraints(
        modifier.fillMaxSize().background(Color(0xFFFAFAFB)).clipToBounds().semantics {
            contentDescription = "Bots handing a job to each other: Claude, Dots, Grok and Muse pass work along, then more bots join the team."
        },
    ) {
        if (reduce) {
            Image(painterResource(R.drawable.meet_team_network_still), null, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
        } else {
            AndroidView(
                factory = { ctx ->
                    VideoView(ctx).apply {
                        setVideoURI(Uri.parse("android.resource://${context.packageName}/${R.raw.meet_team_network}"))
                        setOnPreparedListener { mp -> mp.isLooping = true; mp.setVolume(0f, 0f); start() }
                        player = this
                    }
                },
                onRelease = { it.stopPlayback() },
                modifier = Modifier.fillMaxSize(),
            )
            val w = maxWidth.value
            val h = maxHeight.value
            val scale = maxOf(w / 440f, h / 340f)
            val ox = (w - 440f * scale) / 2
            val oy = (h - 340f * scale) / 2
            val t = (position % 16_000L) / 1000.0
            SAYS.firstOrNull { t >= it.from && t <= it.to }?.let { say ->
                val bw = minOf(220f, w - 24f)
                val x = minOf(w - bw - 12f, maxOf(12f, ox + say.x * scale))
                val bottom = maxOf(76f, oy + say.bottom * scale)
                Column(
                    Modifier.offset(x.dp, (bottom - 56f).coerceAtLeast(4f).dp).width(bw.dp)
                        .background(Color.White, RoundedCornerShape(16.dp)).border(1.dp, Color.Black.copy(alpha = 0.1f), RoundedCornerShape(16.dp))
                        .padding(horizontal = 12.dp, vertical = 8.dp),
                ) {
                    Text(say.name, fontSize = 11.sp, color = Color(0xFF8A8A93))
                    Text(say.text, fontSize = 12.5.sp, color = Color(0xFF141413))
                }
            }
        }
    }
}

// Team: the grid (iOS MeetTeamGrid, tap to take a bot off; iOS also drags to arrange)

private val SLOTS = listOf(
    0.5f to 0.17f, 0.82f to 0.32f, 0.82f to 0.7f, 0.5f to 0.85f, 0.18f to 0.7f, 0.18f to 0.32f,
    0.68f to 0.12f, 0.93f to 0.5f, 0.68f to 0.9f, 0.32f to 0.9f, 0.07f to 0.5f, 0.32f to 0.12f,
)

private fun spot(index: Int): Pair<Float, Float> {
    if (index < SLOTS.size) return SLOTS[index]
    val n = (index - SLOTS.size).toDouble()
    val angle = n * 2.399963
    val r = 0.22 + 0.2 * ((n % 5) / 5)
    return (0.5 + cos(angle) * r * 1.3).toFloat().coerceIn(0.07f, 0.93f) to (0.5 + sin(angle) * r).toFloat().coerceIn(0.1f, 0.9f)
}

@Composable
internal fun MeetTeamGrid(
    members: List<String>,
    catalog: MeetBotCatalog,
    center: (@Composable () -> Unit)?,
    centerName: String,
    label: String,
    locked: Set<String>,
    onRemove: (String) -> Unit,
    size: Dp = 52.dp,
) {
    val c = setupColors
    BoxWithConstraints(Modifier.fillMaxSize().background(meetSoft)) {
        SetupBlockField()
        val w = maxWidth.value
        val h = maxHeight.value
        Canvas(Modifier.fillMaxSize()) {
            val mid = Offset(this.size.width / 2, this.size.height / 2)
            members.indices.forEach { i ->
                val (sx, sy) = spot(i)
                drawLine(c.ink.copy(alpha = 0.3f), mid, Offset(sx * this.size.width, sy * this.size.height), strokeWidth = 1.6.dp.toPx(),
                    cap = StrokeCap.Round, pathEffect = PathEffect.dashPathEffect(floatArrayOf(0.1f, 7.dp.toPx())))
            }
        }
        Box(Modifier.centerAt(w / 2, h / 2 + 8f)) {
            Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(4.dp)) {
                if (center != null) center() else Box(
                    Modifier.size(size * 1.3f).nodeBorder(false, 16.dp), contentAlignment = Alignment.Center,
                ) { Text("Main\nbot", fontSize = 11.sp, color = c.mut, textAlign = TextAlign.Center) }
                Text(if (center == null) " " else centerName, fontSize = 11.5.sp, fontWeight = FontWeight.SemiBold, color = c.ink, maxLines = 1)
            }
        }
        members.forEachIndexed { i, id ->
            val t = catalog.template(id) ?: return@forEachIndexed
            val (sx, sy) = spot(i)
            val isLocked = id in locked
            Box(Modifier.centerAt(sx * w, sy * h + 8f)) {
                Column(
                    Modifier.clickable(enabled = !isLocked, onClickLabel = "Take ${t.name} off the team") { onRemove(id) }
                        .popIn(i * 30),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    MeetTemplateTile(t.color, t.look, t.name, size)
                    Text(t.name, fontSize = 10.5.sp, color = c.sub, maxLines = 1, modifier = Modifier.widthIn(max = 84.dp))
                }
            }
        }
        Text(label, fontSize = 11.5.sp, color = c.mut, modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = 10.dp))
    }
}

/** Main: the leader on its field (iOS MeetMainStage). */
@Composable
internal fun MeetMainStage(field: Color, label: String, tile: @Composable () -> Unit) {
    Box(Modifier.fillMaxSize().background(field).clipToBounds().clearAndSetSemantics {}) {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { tile() }
        Row(
            Modifier.align(Alignment.BottomStart).padding(14.dp).height(26.dp).background(Color.Black.copy(alpha = 0.35f), RoundedCornerShape(8.dp))
                .padding(horizontal = 10.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Box(Modifier.size(6.dp).background(setupColors.coral, CircleShape))
            Text(label, style = mono(11.5.sp, FontWeight.SemiBold), color = Color.White)
        }
    }
}

/**
 * Routines: today, 6a to 10p, with each routine at its time (iOS MeetRoutineCalendar's Day view;
 * iOS also has Week and Month and drags to move). Tapping a routine toggles it.
 */
@Composable
internal fun MeetRoutineDay(routines: List<MeetRoutineDraft>, colorOf: (String) -> Color, onToggle: (String) -> Unit) {
    val c = setupColors
    val today = java.time.LocalDate.now().dayOfWeek.value % 7 // 0 = Sunday, like cron
    val placed = routines.mapNotNull { r -> CronTime.at(r.cron, today)?.let { r to it } }
    BoxWithConstraints(Modifier.fillMaxSize().background(meetSoft).padding(horizontal = 12.dp, vertical = 10.dp)) {
        val h = maxHeight.value
        val top = 6 * 60
        val span = 16 * 60f
        Canvas(Modifier.fillMaxSize()) {
            for (hour in 6..22 step 2) {
                val y = ((hour * 60 - top) / span) * size.height
                drawLine(c.line, Offset(36.dp.toPx(), y), Offset(size.width, y), strokeWidth = 1f)
            }
        }
        for (hour in 6..22 step 4) {
            val y = ((hour * 60 - top) / span) * h
            Text(if (hour == 12) "12p" else if (hour < 12) "${hour}a" else "${hour - 12}p", fontSize = 10.sp, color = c.mut,
                modifier = Modifier.offset(y = (y - 7).coerceAtLeast(0f).dp))
        }
        placed.forEachIndexed { i, (r, minute) ->
            val y = ((minute.coerceIn(top, top + span.toInt()) - top) / span) * (h - 24f)
            Row(
                Modifier.offset(x = (40 + (i % 2) * 8).dp, y = y.dp).fillMaxWidth(0.9f).height(22.dp)
                    .background(colorOf(r.botKey).copy(alpha = if (r.enabled) 0.18f else 0.06f), RoundedCornerShape(6.dp))
                    .border(1.dp, colorOf(r.botKey).copy(alpha = if (r.enabled) 0.6f else 0.2f), RoundedCornerShape(6.dp))
                    .clickable { onToggle(r.id) }.padding(horizontal = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("${CronTime.label(minute)} · ${r.title}", fontSize = 11.sp, color = if (r.enabled) c.ink else c.mut, maxLines = 1)
            }
        }
        if (placed.isEmpty()) {
            InlineEmpty(text = "Nothing runs today.", art = EmptyArtKind.Calendar, modifier = Modifier.align(Alignment.Center))
        }
    }
}

/** Reads the minute of day a routine's cron runs on [dow] (0 = Sunday), for simple `m h * * days` crons. */
internal object CronTime {
    fun at(cron: String, dow: Int): Int? {
        val f = cron.trim().split(Regex("\\s+"))
        if (f.size < 5) return null
        val minute = f[0].toIntOrNull() ?: 0
        val hour = f[1].split(",", "-", "/").firstOrNull()?.toIntOrNull() ?: return null
        if (!matchesDay(f[4], dow)) return null
        return hour * 60 + minute
    }

    private fun matchesDay(field: String, dow: Int): Boolean {
        if (field == "*" || field == "?") return true
        return field.split(",").any { part ->
            val range = part.split("-")
            val a = range[0].toIntOrNull()?.let { it % 7 } ?: return@any false
            val b = range.getOrNull(1)?.toIntOrNull()?.let { if (it == 7) 7 else it } ?: a
            if (range.size == 2) dow in a..b || (b == 7 && dow == 0) else dow == a
        }
    }

    fun label(minute: Int): String {
        val h = minute / 60
        val m = minute % 60
        val h12 = if (h % 12 == 0) 12 else h % 12
        return "$h12:${"%02d".format(m)} ${if (h < 12) "AM" else "PM"}"
    }
}

/** First job: who does it, and what you'll get (iOS MeetFirstJobVisual). */
@Composable
internal fun MeetFirstJobVisual(main: @Composable () -> Unit, other: (@Composable () -> Unit)?, caption: String, expect: String?) {
    val c = setupColors
    val t = loopClock(1.4f, "job")
    Column(
        Modifier.fillMaxSize().background(meetSoft).padding(20.dp),
        horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            main()
            if (other != null) {
                Box(Modifier.width(64.dp).height(10.dp), contentAlignment = Alignment.CenterStart) {
                    Canvas(Modifier.fillMaxWidth().height(2.dp)) {
                        drawLine(c.lineDash, Offset(0f, 1f), Offset(size.width, 1f), strokeWidth = 2.dp.toPx(),
                            pathEffect = PathEffect.dashPathEffect(floatArrayOf(2.dp.toPx(), 3.dp.toPx())))
                    }
                    val p = t / 1.4f
                    Box(Modifier.offset(x = (54 * p).dp).size(10.dp).background(c.coral.copy(alpha = 1 - p), CircleShape))
                }
                other()
            }
        }
        Text(caption, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, color = c.ink, textAlign = TextAlign.Center)
        if (expect != null) {
            Text(
                androidx.compose.ui.text.buildAnnotatedString {
                    pushStyle(androidx.compose.ui.text.SpanStyle(color = c.ink, fontWeight = FontWeight.SemiBold)); append("You’ll get: "); pop()
                    append(expect)
                },
                fontSize = 12.5.sp, color = c.sub,
                modifier = Modifier.widthIn(max = 340.dp).background(c.bg, RoundedCornerShape(12.dp)).border(1.dp, c.line, RoundedCornerShape(12.dp)).padding(12.dp),
            )
        }
        Text("Nothing is sent, posted or paid without your approval.", fontSize = 12.sp, color = c.mut, textAlign = TextAlign.Center)
    }
}

