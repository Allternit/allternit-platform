package com.allternit.feature.bots

import android.graphics.Paint
import android.graphics.Typeface
import android.provider.Settings
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutLinearInEasing
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.keyframes
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.foundation.background
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.PolishGlass
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** What the bot is doing, per the bot phone motion kit (KIT.md). SuitUp/Talking/Listening wear the headset. */
enum class BotPresenceState { Idle, SuitUp, Talking, Listening, SuitDown }

/** Where the bot looks: -1 left, 0 centre, +1 right (a new message arrived on that side). Gizzi avatars only. */
typealias BotLookAt = Float

/** True when the system animator duration scale is 0 (Android's reduce-motion switch). */
@Composable
internal fun rememberReduceMotion(): Boolean {
    val ctx = LocalContext.current
    return remember(ctx) {
        runCatching { Settings.Global.getFloat(ctx.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }.getOrDefault(false)
    }
}

// Headset v2 (HEADSET-V2.md, web HeadsetLab GizziWithHeadset / HeadsetBadge): spring(520, 30) and (300, 16) as Compose damping ratios.
private fun ratio(damping: Float, stiffness: Float) = damping / (2f * kotlin.math.sqrt(stiffness))
private val DropSpring = spring<Float>(dampingRatio = ratio(30f, 520f), stiffness = 520f)
private val BoomSpring = spring<Float>(dampingRatio = ratio(16f, 300f), stiffness = 300f)
private val BadgeSpring = spring<Float>(dampingRatio = ratio(26f, 520f), stiffness = 520f)

private val Tan = Color(0xFFD4B08C)
private val Orange = Color(0xFFD97757)
private val Ink = Color(0xFF111318)
private val Cushion = Color(0xFFB8946E)
private val HsLight = Color(0xFF2B2E35)
private val HsHiLight = Color(0xFF4A4F59)
private val HsDark = Color(0xFF5C626D)
private val HsHiDark = Color(0xFF9AA1AD)

/**
 * One wrapper around [BotAvatar] for the bot phone: idle bob, talk tilt, headset worn.
 * Gizzi wears the headset drawn inside its own art (same graphicsLayer as the body); every other bot gets a corner badge.
 * Presentation only. With animator scale 0 nothing loops and the headset appears and disappears at once.
 */
@Composable
fun BotPresence(
    bot: BotRecord,
    size: Dp = 40.dp,
    presence: BotPresenceState = BotPresenceState.Idle,
    lookAt: BotLookAt = 0f,
    live: Boolean = false,
    working: Boolean = false,
    modifier: Modifier = Modifier,
) {
    val reduce = rememberReduceMotion()
    val headset = presence == BotPresenceState.SuitUp || presence == BotPresenceState.Talking || presence == BotPresenceState.Listening
    val talking = presence == BotPresenceState.Talking
    val isGizzi = remember(bot) { !bot.isPrimaryTemplateBot && AvatarSource.resolve(bot) == AvatarSource.Gizzi }
    val box = if (isGizzi) maxOf(size, 34.dp) else size
    val dark = PolishGlass.isDark()

    val inf = rememberInfiniteTransition(label = "botPresence")
    val idleBob by inf.animateFloat(0f, -1.6f, infiniteRepeatable(tween(1200, easing = FastOutSlowInEasing), RepeatMode.Reverse), label = "bob")
    val tilt by inf.animateFloat(0f, 1f, infiniteRepeatable(keyframes { durationMillis = 800; 0f at 0; -1.5f at 200; 1f at 400; 0f at 800 }), label = "tilt")
    val talkY by inf.animateFloat(0f, 1f, infiniteRepeatable(keyframes { durationMillis = 800; 0f at 0; -1.2f at 400; 0f at 800 }), label = "talkY")
    val mouth by inf.animateFloat(1f, 1f, infiniteRepeatable(keyframes { durationMillis = 420; 1f at 0; 0.6f at 105; 1.1f at 210; 1f at 420 }), label = "mouth")
    val micPulse by inf.animateFloat(1f, 1f, infiniteRepeatable(keyframes { durationMillis = 600; 1f at 0; 1.25f at 300; 1f at 600 }), label = "mic")

    // drop: 0 = off the head, 1 = on (band + cups). boom: boom-arm rotation, -95 = flipped up, 0 = at the mouth.
    val drop = remember { Animatable(if (headset) 1f else 0f) }
    val boom = remember { Animatable(if (headset) 0f else -95f) }
    val squash = remember { Animatable(0f) }
    var lookUp by remember { mutableStateOf(false) }
    LaunchedEffect(headset, reduce) {
        if (reduce) {
            drop.snapTo(if (headset) 1f else 0f); boom.snapTo(if (headset) 0f else -95f); squash.snapTo(0f); lookUp = false
            return@LaunchedEffect
        }
        if (headset) {
            // Mid-call open (already on): no put-on motion.
            if (drop.value >= 1f && boom.value >= 0f) return@LaunchedEffect
            launch { lookUp = true; delay(420); lookUp = false }
            launch { delay(260); squash.snapTo(1f); delay(90); squash.snapTo(0f) }
            launch { delay(300); boom.animateTo(0f, BoomSpring) }
            drop.animateTo(1f, DropSpring)
        } else {
            launch { boom.animateTo(-95f, tween(160, easing = FastOutLinearInEasing)) }
            delay(120)
            drop.animateTo(0f, tween(280, easing = FastOutLinearInEasing))
        }
    }

    val moving = !reduce
    val bodyY = if (!moving) 0f else if (talking && headset) talkY else idleBob
    val bodyRot = if (moving && talking && headset) tilt else 0f

    Box(modifier.size(box)) {
        if (isGizzi) {
            val look = if (moving) lookAt.coerceIn(-1f, 1f) * 3f else 0f
            val hs = if (dark) HsDark else HsLight
            val hsHi = if (dark) HsHiDark else HsHiLight
            val live = AllternitStatic.StatusSuccess
            Canvas(
                Modifier.size(box).graphicsLayer {
                    // One transform for body + headset: they bob, tilt and squash together (pivot bottom centre).
                    val u = this.size.width / 96f
                    transformOrigin = TransformOrigin(0.5f, 1f)
                    translationY = bodyY * u
                    rotationZ = bodyRot
                    scaleX = 1f + 0.03f * squash.value
                    scaleY = 1f - 0.05f * squash.value
                    clip = false
                },
            ) {
                drawGizzi(
                    u = this.size.width / 96f, drop = drop.value, boomRot = boom.value,
                    lookX = look, lookY = if (lookUp && moving) -2f else 0f,
                    mouthScale = if (moving && talking && headset) mouth else 1f,
                    micScale = if (moving && talking && headset) micPulse else 1f,
                    talking = talking && headset, hs = hs, hsHi = hsHi, live = live,
                )
            }
        } else {
            Box(Modifier.size(box).graphicsLayer {
                translationY = bodyY * (box.value / 96f) * density
                rotationZ = bodyRot
                clip = false
            }) {
                BotAvatar(bot, size, live = live || headset, working = working)
            }
            HeadsetBadge(box, headset, talking, moving, dark)
        }
    }
}

/** Corner badge for bots whose art we don't draw (sprite, picture, orb, mark): 38% of the avatar, bottom-right, ring in the surface colour. */
@Composable
private fun HeadsetBadge(avatar: Dp, wearing: Boolean, talking: Boolean, moving: Boolean, dark: Boolean) {
    val pop = remember { Animatable(if (wearing) 1f else 0f) }
    LaunchedEffect(wearing, moving) {
        if (!moving) pop.snapTo(if (wearing) 1f else 0f) else pop.animateTo(if (wearing) 1f else 0f, BadgeSpring)
    }
    if (pop.value <= 0.001f && !wearing) return
    val inf = rememberInfiniteTransition(label = "badgeDot")
    val dot by inf.animateFloat(1f, 1f, infiniteRepeatable(keyframes { durationMillis = 600; 1f at 0; 1.4f at 300; 1f at 600 }), label = "dot")
    val bg = if (dark) HsDark else HsLight
    val ringColor = AllternitTheme.colors.bgPrimary
    val dotColor = if (talking) AllternitStatic.StatusSuccess else Color(0xFF9AA0AA)
    val dotScale = if (moving && talking) dot else 1f
    val side = avatar * 0.38f
    Box(Modifier.size(avatar), contentAlignment = Alignment.BottomEnd) {
        Box(
            Modifier.offset(2.dp, 2.dp).size(side)
                .graphicsLayer { val s = 0.4f + 0.6f * pop.value; scaleX = s; scaleY = s; alpha = pop.value.coerceIn(0f, 1f) }
                .clip(CircleShape).border(2.dp, ringColor, CircleShape).background(bg),
            contentAlignment = Alignment.Center,
        ) {
            Canvas(Modifier.size(side * 0.62f)) {
                val k = this.size.width / 24f
                val white = Color.White
                drawPath(Path().apply { moveTo(4f * k, 14f * k); lineTo(4f * k, 12f * k); arcTo(androidx.compose.ui.geometry.Rect(4f * k, 4f * k, 20f * k, 20f * k), 180f, 180f, false); lineTo(20f * k, 14f * k) }, white, style = Stroke(2.4f * k, cap = StrokeCap.Round))
                drawRoundRect(white, Offset(3f * k, 13f * k), Size(4f * k, 7f * k), CornerRadius(1.5f * k))
                drawRoundRect(white, Offset(17f * k, 13f * k), Size(4f * k, 7f * k), CornerRadius(1.5f * k))
                drawCircle(dotColor, 2f * k * dotScale, Offset(12f * k, 21f * k))
            }
        }
    }
}

private fun DrawScope.rr(c: Color, x: Float, y: Float, w: Float, h: Float, r: Float, u: Float) =
    drawRoundRect(c, Offset(x * u, y * u), Size(w * u, h * u), CornerRadius(r * u))

private fun poly(u: Float, vararg pts: Float): Path = Path().apply {
    moveTo(pts[0] * u, pts[1] * u)
    var i = 2
    while (i < pts.size) { lineTo(pts[i] * u, pts[i + 1] * u); i += 2 }
    close()
}

/**
 * Gizzi (96-unit box, from gizzi-mascot.svg as redrawn in web HeadsetLab) with the headset as part of the character.
 * Layer order: cups behind the arms, band above the antennae with the beacon redrawn on top, mic boom on top.
 */
private fun DrawScope.drawGizzi(
    u: Float, drop: Float, boomRot: Float, lookX: Float, lookY: Float, mouthScale: Float, micScale: Float,
    talking: Boolean, hs: Color, hsHi: Color, live: Color,
) {
    val on = drop.coerceIn(0f, 1f)
    val dy = -20f * (1f - drop) * u
    drawOval(Color(0x24090B0E), Offset(27f * u, 82f * u), Size(42f * u, 8f * u))
    rr(Orange, 44f, 8f, 8f, 5f, 2f, u)
    rr(Tan, 24f, 15f, 17f, 7f, 3f, u); rr(Tan, 55f, 15f, 17f, 7f, 3f, u)
    drawPath(poly(u, 25f, 23f, 71f, 23f, 78f, 30f, 78f, 56f, 72f, 63f, 72f, 69f, 64f, 76f, 32f, 76f, 24f, 69f, 24f, 63f, 18f, 56f, 18f, 30f), Tan)
    // Cups: behind the arms. scaleX 1.22 -> 1 about their own centre.
    if (on > 0.001f) {
        val sx = 1f + 0.22f * (1f - drop)
        translate(0f, dy) {
            scale(sx, 1f, Offset(48f * u, 34f * u)) {
                rr(hs.copy(alpha = on), 11f, 26f, 10f, 16f, 3f, u); rr(Cushion.copy(alpha = on), 19f, 27f, 3f, 14f, 1f, u)
                rr(hs.copy(alpha = on), 75f, 26f, 10f, 16f, 3f, u); rr(Cushion.copy(alpha = on), 74f, 27f, 3f, 14f, 1f, u)
            }
        }
    }
    drawPath(poly(u, 18f, 40f, 14f, 40f, 14f, 44f, 10f, 44f, 10f, 48f, 14f, 48f, 14f, 52f, 18f, 52f, 18f, 56f, 22f, 56f, 22f, 40f), Tan)
    drawPath(poly(u, 78f, 40f, 82f, 40f, 82f, 44f, 86f, 44f, 86f, 48f, 82f, 48f, 82f, 52f, 78f, 52f, 78f, 56f, 74f, 56f, 74f, 40f), Tan)
    rr(Color(0x29111318), 24f, 29f, 48f, 31f, 9f, u)
    // Eyes (glance up while the headset lands)
    translate(lookX * u, lookY * u) { rr(Ink, 33f, 36f, 8f, 10f, 2f, u); rr(Ink, 59f, 36f, 8f, 10f, 2f, u) }
    drawPath(Path().apply {
        moveTo(44.5f * u, 52f * u); lineTo(48f * u, 42f * u); lineTo(51.5f * u, 52f * u); lineTo(49.6f * u, 52f * u)
        lineTo(48.8f * u, 49.5f * u); lineTo(47.2f * u, 49.5f * u); lineTo(46.4f * u, 52f * u); close()
    }, Orange)
    // "://" mouth, scaleY pulse about its own centre
    scale(1f, mouthScale, Offset(48f * u, 54f * u)) {
        drawIntoCanvas { c ->
            val p = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                color = android.graphics.Color.rgb(0xD9, 0x77, 0x57); textSize = 8.2f * u; textAlign = Paint.Align.CENTER
                typeface = Typeface.create(Typeface.MONOSPACE, Typeface.BOLD)
            }
            c.nativeCanvas.drawText("://", 48f * u, 57.5f * u, p)
        }
    }
    for (x in floatArrayOf(24f, 36f, 52f, 64f)) drawRect(Tan, Offset(x * u, 74f * u), Size(8f * u, 12f * u))
    if (on > 0.001f) {
        // Band: stepped pixel arch above the antennae, beacon redrawn on top.
        translate(0f, dy) {
            rr(hs.copy(alpha = on), 28f, 9f, 40f, 5f, 2f, u); rr(hsHi.copy(alpha = on), 30f, 9f, 36f, 1.4f, 0.7f, u)
            rr(hs.copy(alpha = on), 20f, 12f, 10f, 5f, 2f, u); rr(hs.copy(alpha = on), 66f, 12f, 10f, 5f, 2f, u)
            rr(hs.copy(alpha = on), 14f, 16f, 7f, 11f, 2f, u); rr(hs.copy(alpha = on), 75f, 16f, 7f, 11f, 2f, u)
            rr(Orange.copy(alpha = on), 44f, 8f, 8f, 5f, 2f, u)
        }
        // Mic boom: hinged at the left cup, swings from flipped-up (-95) down to the mouth.
        translate(0f, dy) {
            rotate(boomRot, Offset(19f * u, 41f * u)) {
                val tip = if (talking) live else hs
                rr(hs.copy(alpha = on), 17f, 40f, 4f, 14f, 1.5f, u)
                rr(hs.copy(alpha = on), 18f, 52f, 17f, 3.5f, 1.5f, u)
                scale(micScale, micScale, Offset(36.5f * u, 53.75f * u)) { rr(tip.copy(alpha = on), 33f, 50.5f, 7f, 6.5f, 2.5f, u) }
            }
        }
    }
}
