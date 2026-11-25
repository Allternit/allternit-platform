package com.allternit.feature.shell

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import kotlinx.coroutines.delay

private val StopRed = Color(0xFFD64545)
private val WaveColors = listOf(Color(0xFF6F7FF0), Color(0xFFC96442), Color(0xFFFF8A3D))

/**
 * Polish 6.7: the "Listening" panel under the composer while dictating (clip: Cancel / stop / Keep
 * mine). No rewrite model: [level] is the live mic level (Dictation.micLevel, 0..1). The waveform is
 * a thin multi-colour line of the last ~48 samples; the border glows while speech is heard.
 * Cancel discards what this dictation added, Stop keeps it and offers Undo, Keep mine keeps it.
 */
@Composable
internal fun DictationPanel(level: Float, onCancel: () -> Unit, onStop: () -> Unit, onKeepMine: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val reduced = remember(context) {
        android.provider.Settings.Global.getFloat(context.contentResolver, android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
    }
    val latest by rememberUpdatedState(level)
    val samples = remember { mutableStateListOf<Float>().apply { repeat(SAMPLES) { add(0f) } } }
    LaunchedEffect(Unit) {
        while (true) {
            delay(60)
            samples.removeAt(0)
            samples.add(latest.coerceIn(0f, 1f))
        }
    }
    val heard = level > 0.12f
    val glow by animateFloatAsState(if (heard) 1f else 0f, if (reduced) tween(0) else tween(180), label = "dictationGlow")
    val shape = RoundedCornerShape(14.dp)
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 8.dp, vertical = 8.dp)
            .clip(shape)
            .background(c.fillNeutral)
            .border(1.dp, if (glow > 0f) Brush.horizontalGradient(WaveColors.map { it.copy(alpha = 0.25f + 0.65f * glow) }) else Brush.horizontalGradient(listOf(c.borderSubtle, c.borderSubtle)), shape)
            .padding(horizontal = 12.dp, vertical = 8.dp)
            .testTag("dictationPanel")
            .semantics(mergeDescendants = false) { contentDescription = "Listening" },
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text("Listening", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.Medium)
        Canvas(Modifier.fillMaxWidth().height(28.dp).testTag("dictationWave")) {
            val mid = size.height / 2f
            val step = size.width / (SAMPLES - 1)
            val brush = Brush.horizontalGradient(WaveColors)
            // Thin line: each sample is a short vertical stroke, so silence reads as a flat hairline.
            for (i in 0 until SAMPLES) {
                val h = (1.5.dp.toPx() + samples[i] * (size.height - 2.dp.toPx()))
                val x = i * step
                drawLine(brush, Offset(x, mid - h / 2f), Offset(x, mid + h / 2f), strokeWidth = 1.5.dp.toPx(), cap = StrokeCap.Round)
            }
        }
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            DictationTextButton("Cancel", "dictationCancel", onCancel)
            Spacer(Modifier.weight(1f))
            Box(
                modifier = Modifier
                    .size(44.dp)
                    .clip(CircleShape)
                    .testTag("dictationStop")
                    .clickable(role = Role.Button, onClickLabel = "Stop dictation", onClick = onStop)
                    .semantics { contentDescription = "Stop dictation" },
                contentAlignment = Alignment.Center,
            ) { Box(Modifier.size(28.dp).clip(CircleShape).background(StopRed), contentAlignment = Alignment.Center) { Box(Modifier.size(10.dp).clip(RoundedCornerShape(2.dp)).background(Color.White)) } }
            Spacer(Modifier.weight(1f))
            DictationTextButton("Keep mine", "dictationKeepMine", onKeepMine)
        }
    }
}

private const val SAMPLES = 48

@Composable
private fun DictationTextButton(label: String, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Text(
        label,
        color = c.textPrimary,
        fontSize = 14.sp,
        fontWeight = FontWeight.Medium,
        modifier = Modifier
            .heightIn(min = 44.dp)
            .clip(RoundedCornerShape(10.dp))
            .testTag(tag)
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 12.dp),
    )
}

/** The pill that floats over the composer right after dictation stops; one tap restores the text before it. */
@Composable
internal fun UndoDictationPill(onUndo: () -> Unit) {
    val c = AllternitTheme.colors
    Text(
        "Undo dictation",
        color = c.textPrimary,
        fontSize = 13.sp,
        fontWeight = FontWeight.Medium,
        modifier = Modifier
            .heightIn(min = 44.dp)
            .padding(vertical = 4.dp)
            .clip(RoundedCornerShape(50))
            .background(c.fillNeutral)
            .border(1.dp, c.borderSubtle, RoundedCornerShape(50))
            .testTag("dictationUndo")
            .clickable(role = Role.Button, onClick = onUndo)
            .padding(horizontal = 14.dp, vertical = 10.dp),
    )
}
