package com.allternit.feature.shell

import android.graphics.Bitmap
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import java.io.ByteArrayOutputStream

/**
 * Sketch to attach (iOS SketchSheet): a full-screen white canvas, always light. Draw with a
 * finger in black; **Attach** renders the drawing on white, 24dp of margin around it, as a PNG
 * and stages it on the composer. iOS gets PencilKit's tool picker; Android has no system
 * equivalent, so the canvas offers one pen and **Undo**. An empty canvas just closes.
 */
@Composable
internal fun SketchSheet(onAttach: (ComposerAttachment) -> Unit, onDismiss: () -> Unit) {
    val strokes = remember { mutableStateListOf<List<Offset>>() }
    val current = remember { mutableStateListOf<Offset>() }
    val density = LocalDensity.current
    val widthPx = with(density) { 4.dp.toPx() }
    val marginPx = with(density) { 24.dp.toPx() }

    fun attach() {
        val all = strokes.flatten()
        if (all.isEmpty()) {
            onDismiss()
            return
        }
        val left = all.minOf { it.x } - marginPx
        val top = all.minOf { it.y } - marginPx
        val w = (all.maxOf { it.x } - left + marginPx).toInt().coerceAtLeast(1)
        val h = (all.maxOf { it.y } - top + marginPx).toInt().coerceAtLeast(1)
        val bitmap = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
        val canvas = android.graphics.Canvas(bitmap)
        canvas.drawColor(android.graphics.Color.WHITE)
        val paint = android.graphics.Paint(android.graphics.Paint.ANTI_ALIAS_FLAG).apply {
            color = android.graphics.Color.BLACK
            style = android.graphics.Paint.Style.STROKE
            strokeWidth = widthPx
            strokeCap = android.graphics.Paint.Cap.ROUND
            strokeJoin = android.graphics.Paint.Join.ROUND
        }
        strokes.forEach { pts ->
            val path = android.graphics.Path()
            pts.forEachIndexed { i, p -> if (i == 0) path.moveTo(p.x - left, p.y - top) else path.lineTo(p.x - left, p.y - top) }
            if (pts.size == 1) path.lineTo(pts[0].x - left + 0.1f, pts[0].y - top)
            canvas.drawPath(path, paint)
        }
        val png = ByteArrayOutputStream().use { out ->
            bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)
            out.toByteArray()
        }
        onAttach(ComposerAttachment("sketch.png", null, "photo", bytes = png, mediaType = "image/png"))
        onDismiss()
    }

    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false),
    ) {
        Box(Modifier.fillMaxSize().background(Color.White).testTag("sketchSheet")) {
            Canvas(
                Modifier
                    .fillMaxSize()
                    .semantics { contentDescription = "Sketch canvas" }
                    .pointerInput(Unit) {
                        detectDragGestures(
                            onDragStart = { start ->
                                current.clear()
                                current.add(start)
                            },
                            onDragEnd = {
                                if (current.isNotEmpty()) strokes.add(current.toList())
                                current.clear()
                            },
                            onDragCancel = { current.clear() },
                        ) { change, _ -> current.add(change.position) }
                    },
            ) {
                (strokes + listOf(current.toList())).forEach { pts ->
                    if (pts.isEmpty()) return@forEach
                    val path = Path().apply {
                        moveTo(pts[0].x, pts[0].y)
                        pts.drop(1).forEach { lineTo(it.x, it.y) }
                    }
                    drawPath(path, Color.Black, style = Stroke(width = widthPx, cap = StrokeCap.Round, join = StrokeJoin.Round))
                }
            }
            Row(
                Modifier.fillMaxWidth().statusBarsPadding().padding(horizontal = 16.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                SketchButton("Cancel", filled = false, testTag = "sketchCancel", onClick = onDismiss)
                if (strokes.isNotEmpty()) SketchButton("Undo", filled = false, testTag = "sketchUndo") { strokes.removeAt(strokes.lastIndex) }
                Spacer(Modifier.weight(1f))
                SketchButton("Attach", filled = true, testTag = "sketchAttach", onClick = ::attach)
            }
        }
    }
}

@Composable
private fun SketchButton(title: String, filled: Boolean, testTag: String, onClick: () -> Unit) {
    Text(
        title,
        color = if (filled) Color.White else Color.Black,
        fontSize = 16.sp,
        fontWeight = FontWeight.SemiBold,
        modifier = Modifier
            .clip(CircleShape)
            .background(if (filled) Color.Black else Color.Black.copy(alpha = 0.07f))
            .testTag(testTag)
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 9.dp),
    )
}
