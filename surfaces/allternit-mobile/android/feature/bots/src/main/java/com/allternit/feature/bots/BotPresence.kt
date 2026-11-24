package com.allternit.feature.bots

import android.provider.Settings
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.keyframes
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme

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

/**
 * One wrapper around [BotAvatar] for the bot phone: idle bob, talk tilt, headset spin-in/out.
 * Presentation only. With animator scale 0 everything is static and the headset appears at once.
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

    val unit = size.value / 96f
    val inf = rememberInfiniteTransition(label = "botPresence")
    val bob by inf.animateFloat(0f, -2f, infiniteRepeatable(tween(1200, easing = FastOutSlowInEasing), RepeatMode.Reverse), label = "bob")
    val tilt by inf.animateFloat(0f, 1f, infiniteRepeatable(keyframes { durationMillis = 900; 0f at 0; -3f at 300; 2f at 600; 0f at 900 }), label = "tilt")
    val talkY by inf.animateFloat(0f, 1f, infiniteRepeatable(keyframes { durationMillis = 900; 0f at 0; -1.5f at 450; 0f at 900 }), label = "talkY")
    val mic by inf.animateFloat(1f, 1.6f, infiniteRepeatable(tween(300), RepeatMode.Reverse), label = "mic")

    // Headset progress: 0 = away, 1 = on. Spring stiffness 260, damping 14 (ratio 14 / (2 * sqrt(260))).
    val p = remember { Animatable(if (headset) 1f else 0f) }
    var entering by remember { mutableStateOf(headset) }
    LaunchedEffect(headset, reduce) {
        entering = headset
        val target = if (headset) 1f else 0f
        if (reduce) p.snapTo(target) else p.animateTo(target, spring(dampingRatio = 14f / (2f * kotlin.math.sqrt(260f)), stiffness = 260f))
    }

    val moving = !reduce
    val y = if (!moving) 0f else if (talking) talkY else bob
    val rot = if (moving && talking) tilt else 0f
    val eyeShift = if (moving && isGizzi) lookAt.coerceIn(-1f, 1f) * 3f * unit else 0f

    Box(modifier.size(size).graphicsLayer {
        translationY = y * unit * density
        rotationZ = rot
        translationX = eyeShift * density
        clip = false
    }) {
        BotAvatar(bot, size, live = live || headset, working = working)
        if (p.value > 0.001f || headset) {
            val band = AllternitTheme.colors.textPrimary
            val micColor = AllternitStatic.StatusSuccess
            val micScale = if (moving && talking) mic else 1f
            val thin = size < 36.dp
            Canvas(Modifier.size(size).alpha(p.value.coerceIn(0f, 1f))) {
                val u = this.size.width / 96f
                val hide = 1f - p.value
                translate(0f, -46f * u * hide) {
                    rotate((if (entering) -200f else 160f) * hide, Offset(48f * u, 30f * u)) {
                        scale(0.55f + 0.45f * p.value, Offset(48f * u, 30f * u)) { headsetArt(u, band, micColor, micScale, if (thin) 6f else 5f) }
                    }
                }
            }
        }
    }
}

/** The headset from web CallHeadset.tsx (96-unit box): band, two ear cups, boom arm, live-green mic tip. */
private fun androidx.compose.ui.graphics.drawscope.DrawScope.headsetArt(u: Float, color: Color, mic: Color, micScale: Float, bandStroke: Float) {
    val bandPath = Path().apply {
        moveTo(16f * u, 44f * u); cubicTo(16f * u, 2f * u, 80f * u, 2f * u, 80f * u, 44f * u)
    }
    drawPath(bandPath, color, style = Stroke(bandStroke * u, cap = StrokeCap.Round))
    drawRoundRect(color, Offset(9f * u, 36f * u), Size(11f * u, 18f * u), CornerRadius(4f * u))
    drawRoundRect(color, Offset(76f * u, 36f * u), Size(11f * u, 18f * u), CornerRadius(4f * u))
    val boom = Path().apply {
        moveTo(14f * u, 53f * u); cubicTo(14f * u, 64f * u, 24f * u, 68f * u, 34f * u, 64f * u)
    }
    drawPath(boom, color, style = Stroke(3f * u, cap = StrokeCap.Round))
    drawCircle(mic, 3.5f * u * micScale, Offset(36f * u, 63f * u), alpha = if (micScale > 1f) 0.7f else 1f)
}
