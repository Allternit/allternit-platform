package com.allternit.feature.shell.firstrun

import android.provider.Settings
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.layout.layout
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.ThinkingOrb
import com.allternit.core.design.ThinkingOrbSize
import com.allternit.core.design.ThinkingOrbState
import com.allternit.feature.shell.R
import kotlin.math.max
import kotlin.math.min

/** The setup palette (iOS SetupPalette): white and neutral fills, coral accents, graphite dark. */
internal data class SetupColors(
    val ink: Color, val sub: Color, val mut: Color, val line: Color, val lineDash: Color,
    val fill: Color, val fill2: Color, val bg: Color, val card: Color, val cardGlass: Color,
    val okBg: Color, val okFg: Color, val warnBg: Color, val warnFg: Color, val errBg: Color, val errFg: Color,
    val field: Color, val edge: Color,
) {
    val coral = Color(0xFFD97757)
    val ok = Color(0xFF22A06B)
    val coralHalo = Color(0xFFD97757).copy(alpha = 0.16f)
}

private val LightSetup = SetupColors(
    ink = Color(0xFF141413), sub = Color(0xFF5E5E66), mut = Color(0xFF8A8A93), line = Color(0xFFE4E4E7),
    lineDash = Color(0xFFD4D4D8), fill = Color(0xFFF4F4F5), fill2 = Color(0xFFEDEDEF), bg = Color.White,
    card = Color.White, cardGlass = Color.White.copy(alpha = 0.92f), okBg = Color(0xFFE8F5EE), okFg = Color(0xFF17663F),
    warnBg = Color(0xFFFDF0EB), warnFg = Color(0xFF9A4426), errBg = Color(0xFFFDECEC), errFg = Color(0xFFA12A2A),
    field = Color(0xFF141413).copy(alpha = 0.05f), edge = Color(0xFFD4D4D8),
)

private val DarkSetup = SetupColors(
    ink = Color(0xFFF0EEE6), sub = Color(0xFFA1A1AA), mut = Color(0xFF7C7C86), line = Color(0xFF2E2E33),
    lineDash = Color(0xFF3A3A3F), fill = Color(0xFF1F1F22), fill2 = Color(0xFF29292D), bg = Color(0xFF141413),
    card = Color(0xFF1A1A1A), cardGlass = Color(0xFF1A1A1A).copy(alpha = 0.92f), okBg = Color(0xFF22A06B).copy(alpha = 0.16f),
    okFg = Color(0xFF5FD39B), warnBg = Color(0xFFD97757).copy(alpha = 0.16f), warnFg = Color(0xFFF0A285),
    errBg = Color(0xFFDC4646).copy(alpha = 0.16f), errFg = Color(0xFFF19A9A),
    field = Color(0xFFF0EEE6).copy(alpha = 0.06f), edge = Color(0xFF3A3A3F),
)

internal val setupColors: SetupColors
    @Composable get() = if (AllternitTheme.colors.bgPrimary.luminance() < 0.5f) DarkSetup else LightSetup

internal fun mono(size: TextUnit, weight: FontWeight = FontWeight.Medium) =
    TextStyle(fontFamily = FontFamily.Monospace, fontSize = size, fontWeight = weight)

/** System animator scale 0 = reduce motion (the same check ThinkingOrb uses). */
@Composable
internal fun reducedMotion(): Boolean {
    val context = LocalContext.current
    return remember(context) {
        Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
    }
}

/** A looping 0..[period] seconds clock (holds 0 under reduce motion). */
@Composable
internal fun loopClock(period: Float, label: String): Float {
    if (reducedMotion()) return 0f
    val t by rememberInfiniteTransition(label = label).animateFloat(
        0f, period, infiniteRepeatable(tween((period * 1000).toInt(), easing = LinearEasing), RepeatMode.Restart), label = "t",
    )
    return t
}

/** The quiet grid of rounded squares behind the graph (22dp cells, breathing). */
@Composable
internal fun SetupBlockField(modifier: Modifier = Modifier) {
    val c = setupColors
    val reduce = reducedMotion()
    val breathe by rememberInfiniteTransition(label = "field").animateFloat(
        0.75f, 1f, infiniteRepeatable(tween(3000), RepeatMode.Reverse), label = "a",
    )
    Canvas(modifier.fillMaxSize().graphicsLayer { alpha = if (reduce) 1f else breathe }) {
        val cell = 22.dp.toPx()
        val sq = 18.dp.toPx()
        val r = CornerRadius(3.2.dp.toPx())
        var y = 0f
        while (y < size.height) {
            var x = 0f
            while (x < size.width) {
                drawRoundRect(c.field, Offset(x + 2.dp.toPx(), y + 2.dp.toPx()), Size(sq, sq), r)
                x += cell
            }
            y += cell
        }
    }
}

/**
 * Gizzi (iOS SetupGizzi). iOS tilts it on Where and bobs it on Style; every step Android asks
 * uses the focused emotion, which is still.
 */
@Composable
internal fun SetupGizzi(size: Dp) {
    Image(painter = painterResource(R.drawable.gizzi_mascot), contentDescription = "Gizzi", modifier = Modifier.size(size))
}

/** The 30-block progress (iOS SetupProgressBlocks). */
@Composable
internal fun SetupProgressBlocks(step: SetupStep, order: List<SetupStep>, modifier: Modifier = Modifier) {
    val c = setupColors
    val blocks = progressBlocks(step, order)
    Row(modifier.fillMaxWidth().height(8.dp).semantics {
        contentDescription = "Step ${order.indexOf(step) + 1} of ${order.size}"
    }, horizontalArrangement = Arrangement.spacedBy(3.dp)) {
        blocks.forEach { b ->
            Box(
                Modifier.weight(1f).fillMaxSize().background(
                    when (b) { ProgressBlock.On -> c.ink; ProgressBlock.Lead -> c.coral; ProgressBlock.Off -> c.fill2 },
                    RoundedCornerShape(2.dp),
                ),
            )
        }
    }
}

/** Places a child centred on ([x], [y]) dp inside a top-left-aligned fixed canvas. */
internal fun Modifier.centerAt(x: Float, y: Float): Modifier = layout { measurable, constraints ->
    val p = measurable.measure(constraints.copy(minWidth = 0, minHeight = 0))
    layout(constraints.maxWidth, constraints.maxHeight) {
        p.place((x.dp.roundToPx() - p.width / 2), (y.dp.roundToPx() - p.height / 2))
    }
}

/** Fixed W×H dp content scaled to fit and centred (iOS SetupFitBox, the web FitBox). */
@Composable
internal fun SetupFitBox(width: Float, height: Float, modifier: Modifier = Modifier, maxScale: Float = 1.15f, content: @Composable () -> Unit) {
    BoxWithConstraints(modifier, contentAlignment = Alignment.Center) {
        val scale = max(0.01f, min(min(maxWidth.value / width, maxHeight.value / height), maxScale))
        Box(Modifier.requiredSize(width.dp, height.dp).graphicsLayer { scaleX = scale; scaleY = scale }) { content() }
    }
}

/** Pop-in with a light spring, after [delayMs] (iOS SetupPopIn). */
@Composable
internal fun Modifier.popIn(delayMs: Int): Modifier {
    val reduce = reducedMotion()
    val p = remember { Animatable(if (reduce) 1f else 0f) }
    LaunchedEffect(Unit) {
        if (!reduce) p.animateTo(1f, tween(500, delayMs, CubicBezierEasing(0.2f, 0.9f, 0.3f, 1.3f)))
    }
    return graphicsLayer {
        val v = p.value
        alpha = v.coerceIn(0f, 1f)
        scaleX = 0.4f + 0.6f * v
        scaleY = 0.4f + 0.6f * v
    }
}

/** Draws an edge from [from] to [to] with the coral dashed flow; [phase] scrolls the dashes. */
internal fun DrawScope.setupEdge(from: Offset, to: Offset, edge: Color, coral: Color, phase: Float) {
    drawLine(edge, from, to, strokeWidth = 1.2.dp.toPx())
    drawLine(coral.copy(alpha = 0.55f), from, to, strokeWidth = 1.4.dp.toPx(),
        pathEffect = PathEffect.dashPathEffect(floatArrayOf(2.dp.toPx(), 10.dp.toPx()), phase))
}

/** Two coral packets looping along an edge (one reversed); [t] in seconds. */
internal fun DrawScope.setupPackets(from: Offset, to: Offset, coral: Color, t: Float, period: Float = 2.6f, size: Dp = 7.dp) {
    fun packet(delay: Float, forward: Boolean) {
        var p = ((t - delay) % period) / period
        if (p < 0) p += 1f
        val e = p * p * (3 - 2 * p)
        val k = if (forward) e else 1 - e
        val alpha = if (p < 0.1f) p / 0.1f else if (p > 0.9f) (1 - p) / 0.1f else 1f
        val s = size.toPx()
        val at = Offset(from.x + (to.x - from.x) * k, from.y + (to.y - from.y) * k)
        drawRoundRect(coral.copy(alpha = alpha), Offset(at.x - s / 2, at.y - s / 2), Size(s, s), CornerRadius(2.dp.toPx()))
    }
    packet(0.6f, true)
    packet(1.9f, false)
}

/** The expanding coral ring behind the core (2 s loop). */
internal fun DrawScope.coreRing(center: Offset, diameter: Dp, coral: Color, t: Float, period: Float = 2f, minScale: Float = 0.6f, maxScale: Float = 1.35f, width: Dp = 1.dp) {
    val p = (t % period) / period
    val e = 1 - (1 - p) * (1 - p)
    val r = diameter.toPx() / 2 * (minScale + (maxScale - minScale) * e)
    drawCircle(coral.copy(alpha = 0.4f * (1 - e)), r, center, style = Stroke(width.toPx()))
}

/**
 * "Your workspace" (iOS SetupGraphView, connect kit layout): a 694×648 canvas with the A:// core at
 * its centre. On Your AI the brain ring shows (lit when it is the chosen one); after it, the
 * connected brain sits above A:// and the phone joins below right.
 */
@Composable
internal fun SetupGraph(state: SetupState, orb: ThinkingOrbState, orbWord: String, modifier: Modifier = Modifier) {
    val c = setupColors
    val showBrains = state.step == SetupStep.Ai
    val center = Offset(347f, 316f)
    data class Node(val id: BrainChoice, val name: String, val mono: String, val at: Offset)
    val ring = listOf(
        Node(BrainChoice.Allternit, "Allternit Cloud", "A", Offset(347f, 188f)),
        Node(BrainChoice.Claude, "Claude", "C", Offset(469f, 276f)),
        Node(BrainChoice.Chatgpt, "ChatGPT", "G", Offset(422f, 420f)),
        Node(BrainChoice.Gemini, "Gemini", "G", Offset(272f, 420f)),
        Node(BrainChoice.Local, "Local model", "L", Offset(225f, 276f)),
    )
    val otherAt = Offset(520f, 180f)
    val kitBrainAt = Offset(347f, 150f)
    val phoneAt = Offset(540f, 500f)
    val phoneIdx = state.index(SetupStep.Phone)
    val showPhone = phoneIdx >= 0 && state.index(state.step) >= phoneIdx
    val aiOutcome = state.outcomes[SetupField.Ai]
    val kitBrainLabel = if (!showBrains && aiOutcome?.status == OutcomeStatus.Connected) aiOutcome.label else null
    val phoneOutcome = state.outcomes[SetupField.Phone]
    val phoneOn = phoneOutcome?.status == OutcomeStatus.Connected
    val t = loopClock(2.6f * 10f, "graph")
    val reduce = reducedMotion()

    SetupFitBox(694f, 648f, modifier.semantics { contentDescription = "Your workspace" }) {
        Box(Modifier.fillMaxSize()) {
            Canvas(Modifier.fillMaxSize()) {
                fun p(o: Offset) = Offset(o.x.dp.toPx(), o.y.dp.toPx())
                val phase = -(t % 1f) * 12.dp.toPx()
                val from = p(center)
                if (kitBrainLabel != null) {
                    setupEdge(from, p(kitBrainAt), c.edge, c.coral, phase)
                    if (!reduce) setupPackets(from, p(kitBrainAt), c.coral, t)
                }
                if (showPhone) {
                    setupEdge(from, p(phoneAt), c.edge, c.coral, phase)
                    if (phoneOn && !reduce) setupPackets(from, p(phoneAt), c.coral, t)
                }
                if (showBrains) {
                    ring.forEach { setupEdge(from, p(it.at), c.edge, c.coral, phase) }
                    ring.firstOrNull { it.id == state.ai }?.let { if (!reduce) setupPackets(from, p(it.at), c.coral, t) }
                }
                if (!reduce) coreRing(from, 116.dp, c.coral, t)
            }
            if (showBrains) {
                ring.forEachIndexed { i, b ->
                    val lit = state.ai == b.id
                    Box(Modifier.centerAt(b.at.x, b.at.y)) {
                        BrainChip(b.mono, b.name, lit, if (lit) "Default" else null, fixedWidth = true, modifier = Modifier.popIn(100 + i * 80))
                    }
                }
                val ai = state.ai
                if (ai != null && ring.none { it.id == ai }) {
                    val mono = if (ai.isSubscription) "S" else if (ai == BrainChoice.PhoneApps) "P" else "+"
                    val name = if (ai.isSubscription) "${ai.shortName} sub" else aiOutcome?.label ?: "Other brain"
                    Box(Modifier.centerAt(otherAt.x, otherAt.y)) { BrainChip(mono, name, true, "Default", false, Modifier.popIn(0)) }
                }
            }
            if (kitBrainLabel != null) {
                Box(Modifier.centerAt(kitBrainAt.x, kitBrainAt.y)) {
                    BrainChip(kitBrainLabel.take(1), kitBrainLabel, true, "Brain", false, Modifier.popIn(0))
                }
            }
            if (showPhone) {
                Box(Modifier.centerAt(phoneAt.x, phoneAt.y)) {
                    BrainChip("P", phoneOutcome?.label ?: "Your phone", phoneOn, if (phoneOn) "Connected" else null, false, Modifier.popIn(100))
                }
            }
            Box(Modifier.centerAt(center.x, center.y)) {
                ThinkingOrb(state = orb, size = ThinkingOrbSize.Avatar, label = "A:// is $orbWord", modifier = Modifier.popIn(0))
            }
            Box(Modifier.centerAt(center.x, center.y + 52f)) {
                Text("A:// · ${orbWord.uppercase()}", style = mono(10.5.sp, FontWeight.SemiBold), color = c.mut)
            }
        }
    }
}

@Composable
private fun BrainChip(mono: String, name: String, lit: Boolean, tag: String?, fixedWidth: Boolean, modifier: Modifier = Modifier) {
    val c = setupColors
    val shape = RoundedCornerShape(9.dp)
    Row(
        modifier
            .then(if (fixedWidth) Modifier.width(118.dp) else Modifier.widthIn(min = 118.dp))
            .height(32.dp)
            .background(c.card, shape)
            .nodeBorder(lit, 9.dp)
            .padding(horizontal = 9.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(7.dp),
    ) {
        Box(
            Modifier.size(16.dp).background(if (lit) c.coral else c.fill2, RoundedCornerShape(4.dp)),
            contentAlignment = Alignment.Center,
        ) { Text(mono, style = mono(9.sp, FontWeight.Bold), color = if (lit) Color.White else c.sub) }
        Text(name, fontSize = 11.5.sp, fontWeight = FontWeight.SemiBold, color = if (lit) c.ink else c.sub,
            maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = if (fixedWidth) Modifier.weight(1f, fill = false) else Modifier)
        if (tag != null) {
            Spacer(Modifier.weight(1f, fill = false).width(2.dp))
            Text(tag, style = mono(9.sp), color = c.coral, maxLines = 1)
        }
    }
}

/** Ink border + coral halo when lit; dashed when available (iOS nodeBorder). */
@Composable
internal fun Modifier.nodeBorder(on: Boolean, radius: Dp): Modifier {
    val c = setupColors
    return if (on) {
        this.drawBehind {
            val pad = 2.dp.toPx()
            drawRoundRect(c.coralHalo, Offset(-pad - 1.5f, -pad - 1.5f), Size(size.width + 2 * pad + 3f, size.height + 2 * pad + 3f),
                CornerRadius((radius + 3.dp).toPx()), style = Stroke(3.dp.toPx()))
        }.border(1.dp, c.ink, RoundedCornerShape(radius))
    } else {
        this.drawBehind {
            drawRoundRect(c.lineDash, cornerRadius = CornerRadius(radius.toPx()), style = Stroke(1.dp.toPx(),
                pathEffect = PathEffect.dashPathEffect(floatArrayOf(3.dp.toPx(), 2.dp.toPx()))))
        }
    }
}
