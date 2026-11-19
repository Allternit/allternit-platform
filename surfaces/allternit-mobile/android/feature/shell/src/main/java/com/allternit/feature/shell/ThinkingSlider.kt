package com.allternit.feature.shell

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.setProgress
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.gen.ModelThinking
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import kotlin.math.roundToInt

/** The vendor's tint on the block grid (iOS ThinkingSliderView.tint). Null = the theme's text colour. */
internal fun thinkingTint(vendor: String): Color? = when (vendor) {
    "claude" -> Color(0xFFC96442)
    "google" -> Color(0xFF6F7FF0)
    "deepseek" -> Color(0xFF4D6BFE)
    "zai" -> Color(0xFF3859FF)
    else -> null
}

private val CoreHead = Color(0xFFD97757)
private const val COLS = 30
private const val ROWS = 5

/** The vendor's word for thinking ("Effort", "Reasoning"); iOS decodes a missing term as "Thinking". */
internal val ModelThinking.termOrDefault: String get() = term?.takeIf { it.isNotEmpty() } ?: "Thinking"

internal fun ModelThinking.labelFor(id: String): String = levels.firstOrNull { it.id == id }?.label ?: id

/**
 * The model's own thinking levels on Allternit's block grid (iOS ThinkingSliderView, web
 * ThinkingSlider): vendor tint, a coral head block, the vendor's word and level names. Drag or
 * tap the grid, or tap a level name. [value] is the picked level id; [onChange] gets the new
 * one. The level picked here is the effort the chat sends with (ModelStore.level).
 */
@Composable
internal fun ThinkingSlider(thinking: ModelThinking, value: String, onChange: (String) -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val context = LocalContext.current
    val tint = thinkingTint(thinking.vendor) ?: c.textPrimary
    val n = thinking.levels.size
    val index = thinking.levels.indexOfFirst { it.id == value }.coerceAtLeast(0)
    val target = if (n <= 1) 1f else maxOf(0.06f, index.toFloat() / (n - 1))
    val reduceMotion = remember(context) {
        android.provider.Settings.Global.getFloat(context.contentResolver, android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
    }
    val fill by animateFloatAsState(target, if (reduceMotion) tween(0) else tween(150), label = "thinkingFill")
    val mono = thinking.vendor == "openai"
    val current by rememberUpdatedState(value)

    fun pickAt(x: Float, width: Float) {
        if (n == 0) return
        val i = ((x / maxOf(1f, width)) * (n - 1)).roundToInt().coerceIn(0, n - 1)
        val level = thinking.levels[i]
        if (level.id != current) {
            Haptics.perform(view, Haptics.Kind.Tap)
            onChange(level.id)
        }
    }

    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(14.dp))
            .background(c.fillNeutral)
            .border(1.dp, c.borderSubtle, RoundedCornerShape(14.dp))
            .padding(10.dp)
            .testTag("thinkingSlider")
            .clearAndSetSemantics {
                contentDescription = thinking.termOrDefault
                stateDescription = thinking.labelFor(value)
                progressBarRangeInfo = ProgressBarRangeInfo(index.toFloat(), 0f..maxOf(0, n - 1).toFloat(), steps = maxOf(0, n - 2))
                setProgress { v ->
                    val i = v.roundToInt().coerceIn(0, maxOf(0, n - 1))
                    if (n > 0) onChange(thinking.levels[i].id)
                    n > 0
                }
            },
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Box(Modifier.size(7.dp).clip(RoundedCornerShape(2.dp)).background(tint))
            Text(thinking.termOrDefault, color = c.textPrimary, fontSize = 12.5.sp, fontWeight = FontWeight.SemiBold)
            Spacer(Modifier.weight(1f))
            Text(
                "${thinking.vendorName.orEmpty()} · default ${thinking.labelFor(thinking.default)}",
                color = c.textTertiary,
                fontSize = 11.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Box(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(12.dp))
                .background(c.bgPanel)
                .padding(6.dp),
        ) {
            Canvas(
                Modifier
                    .fillMaxWidth()
                    .height(30.dp)
                    .pointerInput(thinking) {
                        detectTapGestures { pickAt(it.x, size.width.toFloat()) }
                    }
                    .pointerInput(thinking) {
                        detectDragGestures(onDragStart = { pickAt(it.x, size.width.toFloat()) }) { change, _ ->
                            pickAt(change.position.x, size.width.toFloat())
                        }
                    },
            ) {
                val gap = 2.dp.toPx()
                val cellW = (size.width - gap * (COLS - 1)) / COLS
                val cellH = (size.height - gap * (ROWS - 1)) / ROWS
                val radius = CornerRadius(1.dp.toPx())
                for (col in 0 until COLS) {
                    val x = col.toFloat() / (COLS - 1)
                    val on = x <= fill + 0.001f
                    if (!on) continue
                    val head = fill - x < 1.5f / COLS
                    val color = if (head) CoreHead else tint.copy(alpha = 0.28f + 0.72f * (x / maxOf(fill, 0.01f)))
                    for (row in 0 until ROWS) {
                        drawRoundRect(
                            color = color,
                            topLeft = Offset(col * (cellW + gap), row * (cellH + gap)),
                            size = Size(cellW, cellH),
                            cornerRadius = radius,
                        )
                    }
                }
            }
        }
        Row(Modifier.fillMaxWidth()) {
            thinking.levels.forEachIndexed { i, level ->
                val on = i == index
                Text(
                    if (mono) level.id else level.label,
                    color = if (on) tint else c.textTertiary,
                    fontSize = if (n > 5) 10.5.sp else 11.5.sp,
                    fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal,
                    fontFamily = if (mono) FontFamily.Monospace else null,
                    maxLines = 1,
                    modifier = Modifier
                        .testTag("thinkingLevel${level.id}")
                        .clickable { onChange(level.id) },
                )
                if (i < n - 1) Spacer(Modifier.weight(1f))
            }
        }
        thinking.note?.takeIf { it.isNotEmpty() }?.let { Text(it, color = c.textTertiary, fontSize = 10.5.sp) }
    }
}
