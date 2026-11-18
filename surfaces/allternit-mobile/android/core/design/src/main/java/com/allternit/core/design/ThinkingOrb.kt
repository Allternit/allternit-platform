package com.allternit.core.design

import android.provider.Settings
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp

/** Port of iOS `ThinkingOrbState`: the dot-matrix animations shown while the agent works. */
enum class ThinkingOrbState(val accessibilityLabel: String) {
    Working("Working"), Searching("Searching"), Solving("Solving"), Listening("Listening"),
    Connecting("Connecting"), Weaving("Weaving"), Composing("Composing"), Breathing("Thinking"),
    Shaping("Shaping");

    companion object {
        /** Maps a wire-protocol tool name to the state that best represents it (iOS `forToolName`). */
        fun forToolName(name: String): ThinkingOrbState = when (name) {
            "WebSearch", "Grep", "Glob", "NotebookQuery" -> Searching
            "Edit", "Write" -> Composing
            "PlanWrite" -> Weaving
            "TodoWrite" -> Solving
            "Task", "Agent" -> Connecting
            "Thinking" -> Breathing
            else -> Working
        }
    }
}

/** The two tuned presets from iOS: 64 (avatar scale) and 20 (inline-text scale), in dp. */
enum class ThinkingOrbSize(val dimension: Float, internal val dotCount: Int, internal val dotRadius: Float) {
    Avatar(64f, 28, 1.6f),
    Inline(20f, 14, 0.9f),
}

private const val STILL_TIME = 0.35 // iOS draws t = 0.35 when paused or reduce-motion
private const val LOOP_SECONDS = 3600 // an hour of linear time stands in for iOS's unbounded clock

/**
 * Monochrome dot-matrix thinking indicator (iOS `ThinkingOrb`). Time comes from an infinite
 * transition scaled by [speed]; it holds still when [paused] or when the system animator
 * scale is 0 (reduce motion). Ink is black 78% on light, white 85% on dark.
 */
@Composable
fun ThinkingOrb(
    modifier: Modifier = Modifier,
    state: ThinkingOrbState = ThinkingOrbState.Working,
    size: ThinkingOrbSize = ThinkingOrbSize.Avatar,
    speed: Float = 1f,
    paused: Boolean = false,
    label: String? = null,
) {
    val context = LocalContext.current
    val reduceMotion = remember(context) {
        Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
    }
    val still = paused || reduceMotion
    val clock by rememberInfiniteTransition(label = "thinking-orb").animateFloat(
        initialValue = 0f,
        targetValue = LOOP_SECONDS.toFloat(),
        animationSpec = infiniteRepeatable(tween(LOOP_SECONDS * 1000, easing = LinearEasing), RepeatMode.Restart),
        label = "t",
    )
    val dark = AllternitTheme.colors.bgPrimary.luminance() < 0.5f
    val ink = if (dark) Color.White.copy(alpha = 0.85f) else Color.Black.copy(alpha = 0.78f)
    val desc = label ?: state.accessibilityLabel
    Canvas(modifier.size(size.dimension.dp).semantics { contentDescription = desc }) {
        val t = if (still) STILL_TIME else clock.toDouble() * speed
        drawOrb(state, size, t, ink)
    }
}
