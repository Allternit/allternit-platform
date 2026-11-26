package com.allternit.core.design

import android.content.Context
import android.provider.Settings
import androidx.annotation.DrawableRes
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.Easing
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.LinearOutSlowInEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.GraphicsLayerScope
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/**
 * The 17 one-shot glyph motions of the approved Settings look (web `icons.part.jsx` MOTION map).
 * Each is a whole-glyph transform, plus a halo ring (Halo) or two sparks (Breathe).
 */
enum class GlyphMotion(val durationMs: Int) {
    Spin(620), Bob(440), Rise(480), Click(420), Swipe(440), Ring(620), Halo(900), Breathe(900), Pop(360),
    Spark(560), Spread(440), Glow(560), Step(500), Snap(460), Beat(560), Buzz(380), Drift(620),
}

private val ByDrawable: Map<String, GlyphMotion> = mapOf(
    "ph_user_circle" to GlyphMotion.Bob, "ph_gear" to GlyphMotion.Spin, "ph_user" to GlyphMotion.Bob,
    "ph_buildings" to GlyphMotion.Rise, "ph_lock" to GlyphMotion.Click, "ph_credit_card" to GlyphMotion.Swipe,
    "ph_bell" to GlyphMotion.Ring, "ph_microphone" to GlyphMotion.Halo, "ph_brain" to GlyphMotion.Breathe,
    "ph_chat_text" to GlyphMotion.Pop, "ph_chat_circle_text" to GlyphMotion.Pop, "ph_cpu" to GlyphMotion.Beat,
    "ph_graph" to GlyphMotion.Spark, "ph_code" to GlyphMotion.Spread, "ph_devices" to GlyphMotion.Bob,
    "ph_desktop" to GlyphMotion.Glow, "ph_paw_print" to GlyphMotion.Step, "ph_puzzle_piece" to GlyphMotion.Snap,
    "ph_pulse" to GlyphMotion.Beat, "ph_shield_check" to GlyphMotion.Pop, "ph_vibrate" to GlyphMotion.Buzz,
    "ph_wind" to GlyphMotion.Drift, "ph_palette" to GlyphMotion.Step, "ph_sliders_horizontal" to GlyphMotion.Swipe,
    "ph_sign_out" to GlyphMotion.Swipe,
)

/** Motion for a `ph_*` drawable; anything not in the approved set gets the quiet [GlyphMotion.Bob] (default D). */
fun glyphMotionFor(context: Context, @DrawableRes icon: Int): GlyphMotion {
    val name = runCatching { context.resources.getResourceEntryName(icon) }.getOrNull()
    return ByDrawable[name] ?: GlyphMotion.Bob
}

/** True when the system animator scale is 0 (Developer options / battery saver / accessibility "remove animations"). */
fun systemAnimationsOff(context: Context): Boolean =
    runCatching { Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }.getOrDefault(false)

private val Overshoot = CubicBezierEasing(0.3f, 1.4f, 0.5f, 1f)
private val OvershootBig = CubicBezierEasing(0.3f, 1.5f, 0.5f, 1f)
private val OvershootHuge = CubicBezierEasing(0.3f, 1.6f, 0.5f, 1f)
private val Spin90 = CubicBezierEasing(0.3f, 1.3f, 0.5f, 1f)

/** Progress 0..1 of a segment that starts at [startMs] and lasts [dur], run through [e]. */
private fun seg(ms: Float, dur: Float, e: Easing = LinearEasing, startMs: Float = 0f): Float =
    e.transform(((ms - startMs) / dur).coerceIn(0f, 1f))

/** Linear interpolation through keyframes at fractions [ts] with values [vs]. */
private fun kf(p: Float, ts: FloatArray, vs: FloatArray): Float {
    if (p <= ts[0]) return vs[0]
    for (i in 1 until ts.size) {
        if (p <= ts[i]) {
            val f = (p - ts[i - 1]) / (ts[i] - ts[i - 1])
            return vs[i - 1] + (vs[i] - vs[i - 1]) * f
        }
    }
    return vs[vs.size - 1]
}

private fun fa(vararg v: Float) = v

/** Applies the motion at [ms] elapsed; at `ms == durationMs` every motion is the identity. */
private fun GraphicsLayerScope.applyMotion(m: GlyphMotion, ms: Float) {
    val w = size.width
    val p = seg(ms, m.durationMs.toFloat())
    when (m) {
        GlyphMotion.Spin -> rotationZ = 90f * seg(ms, 620f, Spin90)
        GlyphMotion.Bob -> translationY = w * kf(p, fa(0f, .4f, 1f), fa(0f, -.14f, 0f))
        GlyphMotion.Rise -> { transformOrigin = TransformOrigin(0.5f, 1f); scaleY = 0.6f + 0.4f * seg(ms, 480f, OvershootBig) }
        GlyphMotion.Click -> {
            translationY = w * kf(p, fa(0f, .35f, .7f, 1f), fa(0f, .10f, -.04f, 0f))
            val s = kf(p, fa(0f, .35f, 1f), fa(1f, .92f, 1f)); scaleX = s; scaleY = s
        }
        GlyphMotion.Swipe -> {
            val e = seg(ms, 440f, Overshoot)
            translationX = -.22f * w * (1f - e); alpha = (.25f + .75f * e).coerceIn(0f, 1f)
        }
        GlyphMotion.Ring -> {
            transformOrigin = TransformOrigin(0.5f, 0.06f)
            rotationZ = kf(p, fa(0f, .15f, .35f, .55f, .75f, 1f), fa(0f, 16f, -13f, 8f, -4f, 0f))
        }
        GlyphMotion.Halo -> { val s = kf(seg(ms, 320f), fa(0f, .6f, 1f), fa(.78f, 1.14f, 1f)); scaleX = s; scaleY = s }
        GlyphMotion.Breathe -> { val s = kf(p, fa(0f, .5f, 1f), fa(1f, 1.12f, 1f)); scaleX = s; scaleY = s }
        GlyphMotion.Pop -> { val s = kf(seg(ms, 360f), fa(0f, .6f, 1f), fa(.78f, 1.14f, 1f)); scaleX = s; scaleY = s }
        GlyphMotion.Spark -> {
            val e = seg(ms, 560f, Overshoot)
            rotationZ = -28f * (1f - e); val s = .85f + .15f * e; scaleX = s; scaleY = s
        }
        GlyphMotion.Spread -> scaleX = 0.7f + 0.3f * seg(ms, 440f, OvershootHuge)
        GlyphMotion.Glow -> { val e = seg(ms, 560f, LinearOutSlowInEasing); alpha = .3f + .7f * e; translationY = .08f * w * (1f - e) }
        GlyphMotion.Step -> { translationY = w * kf(p, fa(0f, .4f, 1f), fa(0f, -.10f, 0f)); rotationZ = kf(p, fa(0f, .4f, 1f), fa(0f, -18f, 0f)) }
        GlyphMotion.Snap -> {
            val e = seg(ms, 460f, OvershootHuge)
            translationX = -.16f * w * (1f - e); translationY = -.12f * w * (1f - e); rotationZ = -12f * (1f - e)
        }
        GlyphMotion.Beat -> { val s = kf(p, fa(0f, .25f, .5f, .7f, 1f), fa(1f, 1.16f, .94f, 1.07f, 1f)); scaleX = s; scaleY = s }
        GlyphMotion.Buzz -> translationX = w * kf(p, fa(0f, .2f, .4f, .6f, .8f, 1f), fa(0f, -.08f, .08f, -.05f, .04f, 0f))
        GlyphMotion.Drift -> { val e = seg(ms, 620f, LinearOutSlowInEasing); translationX = -.18f * w * (1f - e); alpha = .35f + .65f * e }
    }
}

/**
 * A `ph_*` drawable with a one-shot motion. Plays when [playKey] changes (row press) and, with
 * [playOnAppear], once when it enters composition (a page opening). [reduced] (SettingsPrefs.reduceMotion
 * or [systemAnimationsOff]) turns all motion off.
 */
@Composable
fun AnimatedGlyph(
    @DrawableRes icon: Int,
    tint: Color,
    size: Dp,
    modifier: Modifier = Modifier,
    playKey: Int = 0,
    playOnAppear: Boolean = false,
    reduced: Boolean = false,
    motion: GlyphMotion? = null,
) {
    val context = LocalContext.current
    val m = motion ?: remember(icon) { glyphMotionFor(context, icon) }
    val end = m.durationMs.toFloat()
    val clock = remember(m) { Animatable(end) }
    LaunchedEffect(playKey, reduced, m) {
        if (reduced) { clock.snapTo(end); return@LaunchedEffect }
        if (playKey == 0 && !playOnAppear) return@LaunchedEffect
        clock.snapTo(0f)
        clock.animateTo(end, tween(m.durationMs, easing = LinearEasing))
    }
    Box(
        modifier.size(size).drawBehind {
            val ms = clock.value
            if (reduced || ms >= end) return@drawBehind
            val c = tint
            val center = Offset(this.size.width / 2f, this.size.height / 2f)
            if (m == GlyphMotion.Halo) {
                val p = seg(ms, 900f, LinearOutSlowInEasing)
                val r = (this.size.width / 2f + 3.dp.toPx()) * (0.7f + 1.1f * p)
                drawCircle(c.copy(alpha = 0.5f * (1f - p)), radius = r, center = center, style = Stroke(1.5.dp.toPx()))
            }
            if (m == GlyphMotion.Breathe) {
                // s1 top-right (120ms delay), s2 left (260ms delay); 700ms each.
                val sparks = listOf(
                    Offset(this.size.width + 2.5.dp.toPx(), -1.5.dp.toPx()) to 120f,
                    Offset(-2.5.dp.toPx(), 2.5.dp.toPx()) to 260f,
                )
                for ((pos, delay) in sparks) {
                    val p = seg(ms, 700f, LinearEasing, delay)
                    if (p <= 0f || p >= 1f) continue
                    val a = kf(p, fa(0f, .4f, 1f), fa(0f, .9f, 0f))
                    val s = kf(p, fa(0f, .4f, 1f), fa(.4f, 1f, .6f))
                    drawCircle(c.copy(alpha = a), radius = 1.5.dp.toPx() * s, center = pos.copy(y = pos.y - 4.dp.toPx() * p))
                }
            }
        },
    ) {
        Icon(
            painterResource(icon), contentDescription = null, tint = tint,
            modifier = Modifier.size(size).graphicsLayer { applyMotion(m, clock.value) },
        )
    }
}
