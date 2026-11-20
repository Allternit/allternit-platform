package com.allternit.feature.chat.voice

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.dp
import com.allternit.core.design.AllternitTheme
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.min
import kotlin.math.roundToInt
import kotlin.math.sin

/** iOS VoiceSpeakerView.Phase. */
enum class SpeakerPhase { Connecting, Idle, Listening, Thinking, Speaking }

/** Voice mode's turn state as the speaker reads it. */
fun TurnState.speakerPhase(connecting: Boolean = false): SpeakerPhase = if (connecting) SpeakerPhase.Connecting else when (this) {
    TurnState.Idle -> SpeakerPhase.Idle
    TurnState.Listening -> SpeakerPhase.Listening
    TurnState.Thinking -> SpeakerPhase.Thinking
    TurnState.Speaking -> SpeakerPhase.Speaking
}

private const val SCALE = 1.78f

/**
 * Live mode's speaker (iOS VoiceSpeakerView, allternit-ai VoiceSpeaker.tsx): the face at full size with its
 * speaker look around it.
 *
 * - Ring: the VoiceRingKit port (VoiceRing.kt), drawn once over the face with a soft cut-out around the head and
 *   chest, so its lower arc passes in front of the body. A palette tints it (greyscale times the palette's first
 *   colour); Spectrum keeps the ring's own colours.
 * - Blocks, ASCII, Glow: our own looks, drawn behind the face.
 * - Still: the face alone; it lifts a little while the bot talks.
 *
 * Levels: [level] (the mic) while you talk; while the reply speaks Android has no output meter yet, so the look
 * follows iOS's gentle speaking motion. Reduce motion (system or app) and [still] hold everything; the screen's
 * status line says the state in words. [compact] lays out at the face's size (the look overflows), for the quick
 * bar and the settings tiles.
 */
@Composable
fun VoiceSpeakerView(
    phase: SpeakerPhase,
    muted: Boolean,
    style: SpeakerStyle,
    palette: RingPalette,
    botColor: Color?,
    sizeDp: Float,
    modifier: Modifier = Modifier,
    compact: Boolean = false,
    still: Boolean = false,
    level: () -> Float = { 0f },
    face: @Composable () -> Unit,
) {
    val reduce = rememberReduceMotion()
    val paused = still || reduce
    var nanos by remember { mutableLongStateOf(0L) }
    LaunchedEffect(paused) {
        if (!paused) while (true) withFrameNanos { nanos = it }
    }
    val engine = remember { RingEngine() }
    val drawn = style.drawn
    val outer = (sizeDp * SCALE).roundToInt().toFloat()
    val colors = if (muted) RingPalette.idle else palette.colors(botColor)
    val light = colors?.firstOrNull() ?: botColor ?: Color(0xFF8C8E94)
    val faceDy = if (drawn == SpeakerStyle.Ring && !compact) (sizeDp * 0.15f).roundToInt().toFloat() else 0f
    val ringState = when {
        muted -> RingState.Idle
        phase == SpeakerPhase.Speaking -> RingState.Speaking
        phase == SpeakerPhase.Listening -> RingState.Listening
        phase == SpeakerPhase.Thinking || phase == SpeakerPhase.Connecting -> RingState.Processing
        else -> RingState.Idle
    }
    val dark = colors != null || AllternitTheme.colors.bgPrimary.luminance() < 0.5f

    fun time(): Double = if (paused) 0.0 else nanos / 1e9
    fun levelAt(t: Double): Double {
        if (paused || muted) return 0.0
        return when (phase) {
            SpeakerPhase.Listening -> level().toDouble().coerceIn(0.0, 1.0)
            SpeakerPhase.Speaking -> {
                // The phone's speech voice has no audio to measure: a speaking motion, not a measurement (iOS).
                val syl = maxOf(0.0, sin(t * 11) * 0.5 + sin(t * 3.1) * 0.35 + 0.25)
                min(1.0, syl * (if (sin(t * 0.9) > -0.6) 0.8 else 0.1))
            }
            else -> 0.0
        }
    }

    val textMeasurer = rememberTextMeasurer()
    val density = LocalDensity.current
    val asciiGlyphs: List<TextLayoutResult>? = if (drawn == SpeakerStyle.Ascii) {
        val fontSize = with(density) { (outer * 0.05f).dp.toSp() }
        remember(fontSize) {
            ASCII_CHARS.map { textMeasurer.measure(it.toString(), TextStyle(fontSize = fontSize, fontWeight = FontWeight.SemiBold, fontFamily = FontFamily.Monospace)) }
        }
    } else null

    Box(
        modifier.size((if (compact) sizeDp else outer).dp).clearAndSetSemantics {},
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.requiredSize(outer.dp), contentAlignment = Alignment.Center) {
            if (drawn == SpeakerStyle.Blocks || drawn == SpeakerStyle.Ascii || drawn == SpeakerStyle.Glow) {
                val ownColors = colors ?: RingPalette.spectrumColors
                Canvas(Modifier.fillMaxSize()) {
                    val t = time()
                    drawOwnLook(drawn, levelAt(t), ringState == RingState.Processing, ownColors, t, asciiGlyphs)
                }
            }
            // The face, its floor glow and its halo (iOS draws a coloured shadow; a radial glow reads the same).
            Box(
                Modifier.size(sizeDp.dp).offset(y = faceDy.dp).drawBehind {
                    val t = time()
                    val lv = levelAt(t).toFloat()
                    val faceR = this.size.minDimension / 2
                    val glowR = faceR + ((if (compact) 2f else 6f) + lv * (if (compact) 6f else 16f)).dp.toPx()
                    val a = if (muted) 0.15f else 0.6f
                    drawCircle(
                        Brush.radialGradient(0f to light.copy(alpha = a), (faceR / glowR) * 0.92f to light.copy(alpha = a * 0.6f), 1f to light.copy(alpha = 0f), center = center, radius = glowR),
                        radius = glowR,
                    )
                    if (!compact) {
                        // the soft floor light under the face
                        val w = this.size.width * 0.56f * (1 + lv * 0.15f)
                        val h = this.size.width * 0.09f
                        val cy = this.size.height - this.size.width * 0.04f - h / 2
                        scale(1f, h / w, pivot = Offset(center.x, cy)) {
                            drawCircle(Brush.radialGradient(0f to light.copy(alpha = 0.45f), 1f to light.copy(alpha = 0f), center = Offset(center.x, cy), radius = w / 2), radius = w / 2, center = Offset(center.x, cy))
                        }
                    }
                },
                contentAlignment = Alignment.Center,
            ) {
                Box(
                    Modifier.fillMaxSize().graphicsLayer {
                        val t = time()
                        val lv = levelAt(t).toFloat()
                        val k = 1 + lv * 0.035f
                        scaleX = k; scaleY = k
                        transformOrigin = TransformOrigin(0.5f, 0.85f)
                        val lift = drawn == SpeakerStyle.Still && phase == SpeakerPhase.Speaking && !paused
                        translationY = if (lift) -4.dp.toPx() * abs(sin(t * 3.5)).toFloat() else 0f
                        alpha = if (muted) 0.7f else 1f
                    },
                    contentAlignment = Alignment.Center,
                ) { face() }
            }
            if (drawn == SpeakerStyle.Ring) {
                val tint = colors?.firstOrNull()
                Canvas(Modifier.fillMaxSize().graphicsLayer { compositingStrategy = CompositingStrategy.Offscreen }) {
                    val t = time()
                    val frame = engine.frame(nanos, outer.toDouble(), ringState, levelAt(t), paused)
                    paintRing(frame, dark, tint)
                    if (!compact) cutOutFace(sizeDp.dp.toPx(), faceDy.dp.toPx())
                }
            }
        }
    }
}

/** Over the face, minus the head and chest (soft-edged): the ring's lower arc crosses the body (iOS mask). */
private fun DrawScope.cutOutFace(facePx: Float, faceDyPx: Float) {
    val w = facePx * 0.92f
    val h = facePx * 0.82f
    val soft = facePx * 0.04f
    val c = Offset(center.x, center.y + faceDyPx - facePx * 0.1f)
    val r = w / 2 + soft
    scale(1f, h / w, pivot = c) {
        drawCircle(
            Brush.radialGradient(0f to Color.Black, ((w / 2 - soft) / r).coerceIn(0f, 1f) to Color.Black, 1f to Color.Transparent, center = c, radius = r),
            radius = r, center = c, blendMode = BlendMode.DstOut,
        )
    }
}

private val ASCII_CHARS = " .:-=+*#%@".toList()
private val BLOCK_ACCENT = Color(0xFFD97757)

/** iOS OwnLookCanvas: Blocks, ASCII and Glow, drawn from the same level each frame. */
private fun DrawScope.drawOwnLook(kind: SpeakerStyle, level: Double, working: Boolean, colors: List<Color>, time: Double, glyphs: List<TextLayoutResult>?) {
    val sizePx = this.size.minDimension
    val cx = this.size.width / 2; val cy = this.size.height / 2
    val R = sizePx * 0.39f
    fun col(i: Int, n: Int) = colors[(i * colors.size / maxOf(n, 1)) % colors.size]
    when (kind) {
        SpeakerStyle.Blocks -> {
            val n = 24
            val b = sizePx * 0.026f
            for (i in 0 until n) {
                val ang = i.toDouble() / n * PI * 2 - PI / 2 + (if (working) time * 0.8 else 0.0)
                val wob = 0.5 + 0.5 * sin(time * 6 + i * 1.7)
                val amp = if (working) 0.3 + 0.7 * ((sin(time * 4 - i * 0.5) + 1) / 2) else 0.25 + level * 1.6 * wob
                val r = R * 0.86f + (amp * sizePx * 0.035).toFloat()
                val s = b * (0.75f + amp.toFloat() * 0.6f)
                translate(cx + (cos(ang) * r).toFloat(), cy + (sin(ang) * r).toFloat()) {
                    rotate((ang * 180 / PI).toFloat(), pivot = Offset.Zero) {
                        drawRoundRect(
                            if (i % 8 == 0) BLOCK_ACCENT else col(i, n),
                            topLeft = Offset(-s / 2, -s / 2), size = Size(s, s), cornerRadius = CornerRadius(s * 0.18f),
                            alpha = (0.35f + min(0.65, amp).toFloat()).coerceIn(0f, 1f),
                        )
                    }
                }
            }
        }
        SpeakerStyle.Ascii -> {
            val g = glyphs ?: return
            val n = 40
            for (i in 0 until n) {
                val ang = i.toDouble() / n * PI * 2 + time * (if (working) 0.6 else 0.12)
                val e = if (working) (sin(time * 5 - i * 0.6) + 1) / 2 else min(1.0, 0.15 + level * 1.8 * (0.5 + 0.5 * sin(time * 7 + i * 2.3)))
                val r = R + (e * sizePx * 0.05).toFloat()
                val idx = maxOf(1, (e * (ASCII_CHARS.size - 1)).roundToInt()).coerceAtMost(ASCII_CHARS.size - 1)
                val layout = g[idx]
                val x = cx + (cos(ang) * r).toFloat() - layout.size.width / 2f
                val y = cy + (sin(ang) * r).toFloat() - layout.size.height / 2f
                drawText(layout, color = col(i, n), topLeft = Offset(x, y), alpha = (0.35f + e.toFloat() * 0.65f).coerceIn(0f, 1f))
            }
        }
        else -> {
            val k = if (working) 0.5 + 0.2 * sin(time * 3) else 0.35 + level * 0.9
            val base = colors.firstOrNull() ?: Color.Gray
            val radius = (R * (1.05 + k * 0.25)).toFloat()
            drawCircle(
                Brush.radialGradient(
                    0f to base.copy(alpha = 0f), 0.5f to base.copy(alpha = 0f),
                    0.72f to base.copy(alpha = (min(1.0, k) * 0.65).toFloat()), 1f to base.copy(alpha = 0f),
                    center = Offset(cx, cy), radius = radius,
                ),
                radius = radius, center = Offset(cx, cy),
            )
        }
    }
}
