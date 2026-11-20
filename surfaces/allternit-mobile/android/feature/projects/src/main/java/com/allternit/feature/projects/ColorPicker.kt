package com.allternit.feature.projects

import androidx.compose.foundation.BorderStroke
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
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.FilterLabel
import com.allternit.feature.artifacts.LibraryField
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.PillRow

/*
 * The colour picker the iOS folder look and pad sheets get from SwiftUI's ColorPicker
 * (supportsOpacity: false). Android has no system picker, so this is the same sheet: a form row
 * with the colour well, and a sheet with Grid (swatches), Spectrum (hue + saturation/brightness)
 * and a hex field, with the chosen colour previewed at the top.
 */

private val gridColors = listOf(
    // 12 hues × 3 shades, then greys: close to the iOS grid's palette.
    "#5E1F1A", "#8C2D1F", "#C8451F", "#E5484D", "#F76B15", "#E0A526", "#F5D90A", "#99D52A", "#30A46C", "#0F7F57", "#12A594", "#2FA7A0",
    "#B33A2E", "#D2453A", "#D97757", "#F2996E", "#FFB224", "#FFD866", "#C2E05C", "#7BB08B", "#4CC38A", "#69A8C8", "#397AD5", "#1F4FA8",
    "#2F54EB", "#3E63DD", "#6E56CF", "#9B6BD6", "#B08BC0", "#D6409F", "#E93D82", "#F4A6C6", "#F4F1EA", "#E1E3E6", "#D3D6DB", "#ffffff",
    "#000000", "#1F1E1D", "#23252B", "#3A3D45", "#5B6773", "#777777", "#9BA1A6", "#C1C8CD",
)

private fun hsv(color: Color): FloatArray = FloatArray(3).also { android.graphics.Color.colorToHSV(color.toArgb(), it) }
private fun fromHsv(h: Float, s: Float, v: Float): Color = Color(android.graphics.Color.HSVToColor(floatArrayOf(h, s, v)))

/** A form row: the label and a round colour well; tapping opens the picker (iOS ColorPicker row). */
@Composable
fun ColorWellRow(label: String, hex: String?, testTag: String, onPick: (String) -> Unit) {
    val c = AllternitTheme.colors
    var open by remember { mutableStateOf(false) }
    Row(
        Modifier.fillMaxWidth().heightIn(min = 44.dp).clip(RoundedCornerShape(10.dp)).testTag(testTag)
            .clickable(onClickLabel = "Choose $label") { open = true }.padding(horizontal = 4.dp)
            .semantics(mergeDescendants = true) { stateDescription = hex ?: "Default" },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, color = c.textPrimary, fontSize = 16.sp, modifier = Modifier.weight(1f))
        Box(
            Modifier.size(28.dp).clip(CircleShape)
                .background(Brush.sweepGradient(listOf(Color.Red, Color.Yellow, Color.Green, Color.Cyan, Color.Blue, Color.Magenta, Color.Red)))
                .padding(3.dp).clip(CircleShape).background(c.bgPrimary).padding(2.dp).clip(CircleShape)
                .background(hex?.let { hexColor(it) } ?: c.fillNeutral),
        )
    }
    if (open) ColorPickerSheet(label, hex ?: "#397ad5", onDone = { onPick(it); open = false }, onDismiss = { open = false })
}

private enum class PickerTab(val label: String) { Grid("Grid"), Spectrum("Spectrum") }

@Composable
fun ColorPickerSheet(title: String, initial: String, onDone: (String) -> Unit, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val start = remember { hsv(hexColor(initial)) }
    var hue by remember { mutableFloatStateOf(start[0]) }
    var sat by remember { mutableFloatStateOf(start[1]) }
    var value by remember { mutableFloatStateOf(start[2]) }
    var tab by remember { mutableStateOf(PickerTab.Grid) }
    val current = fromHsv(hue, sat, value)
    fun set(color: Color) { val f = hsv(color); hue = f[0]; sat = f[1]; value = f[2] }
    LibrarySheet(
        title, "colorPickerSheet", onDismiss = onDismiss,
        leading = LibrarySheetAction("Cancel", testTag = "colorPickerCancel", onClick = onDismiss),
        trailing = LibrarySheetAction("Done", bold = true, testTag = "colorPickerDone") { onDone(current.hex()) },
    ) {
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Box(Modifier.fillMaxWidth().height(44.dp).clip(RoundedCornerShape(12.dp)).background(current).border(1.dp, c.borderSubtle, RoundedCornerShape(12.dp)).semantics { contentDescription = "Chosen colour ${current.hex()}" })
            PillRow(PickerTab.entries, tab, { it.label }, { "colorPicker${it.name}" }) { tab = it }
            when (tab) {
                PickerTab.Grid -> Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    gridColors.chunked(12).forEach { row ->
                        Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                            row.forEach { hx ->
                                val col = hexColor(hx)
                                val on = col.hex().equals(current.hex(), true)
                                Box(
                                    Modifier.weight(1f).aspectRatio(1f).clip(RoundedCornerShape(4.dp)).background(col)
                                        .border(BorderStroke(if (on) 2.dp else 0.5.dp, if (on) c.textPrimary else c.borderSubtle), RoundedCornerShape(4.dp))
                                        .testTag("colorGrid$hx").clickable { set(col) }.semantics { contentDescription = "Colour $hx" },
                                )
                            }
                            repeat(12 - row.size) { Box(Modifier.weight(1f)) }
                        }
                    }
                }
                PickerTab.Spectrum -> Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    // Saturation (x) and brightness (y) for the current hue.
                    Canvas(
                        Modifier.fillMaxWidth().aspectRatio(1.6f).clip(RoundedCornerShape(12.dp)).testTag("colorSpectrum")
                            .pointerInput(Unit) {
                                fun pick(o: Offset) { sat = (o.x / size.width).coerceIn(0f, 1f); value = 1f - (o.y / size.height).coerceIn(0f, 1f) }
                                detectTapGestures { pick(it) }
                            }
                            .pointerInput(Unit) {
                                detectDragGestures { change, _ ->
                                    sat = (change.position.x / size.width).coerceIn(0f, 1f)
                                    value = 1f - (change.position.y / size.height).coerceIn(0f, 1f)
                                }
                            }
                            .semantics { contentDescription = "Saturation and brightness" },
                    ) {
                        drawRect(Brush.horizontalGradient(listOf(Color.White, fromHsv(hue, 1f, 1f))))
                        drawRect(Brush.verticalGradient(listOf(Color.Transparent, Color.Black)))
                        val p = Offset(sat * size.width, (1f - value) * size.height)
                        drawCircle(Color.White, 11.dp.toPx(), p, style = Stroke(3.dp.toPx()))
                        drawCircle(Color.Black.copy(alpha = 0.3f), 12.5.dp.toPx(), p, style = Stroke(1.dp.toPx()))
                    }
                    Canvas(
                        Modifier.fillMaxWidth().height(28.dp).clip(RoundedCornerShape(14.dp)).testTag("colorHue")
                            .pointerInput(Unit) { detectTapGestures { hue = (it.x / size.width).coerceIn(0f, 1f) * 360f } }
                            .pointerInput(Unit) { detectDragGestures { change, _ -> hue = (change.position.x / size.width).coerceIn(0f, 1f) * 360f } }
                            .semantics { contentDescription = "Hue"; stateDescription = "${hue.toInt()} degrees" },
                    ) {
                        drawRoundRect(Brush.horizontalGradient((0..6).map { fromHsv(it * 60f, 1f, 1f) }), cornerRadius = CornerRadius(size.height / 2))
                        drawCircle(Color.White, size.height / 2 - 2.dp.toPx(), Offset(hue / 360f * size.width, size.height / 2), style = Stroke(3.dp.toPx()))
                    }
                }
            }
            FilterLabel("Hex")
            var hexText by remember(current) { mutableStateOf(current.hex()) }
            LibraryField("", hexText, "#397ad5", "colorHex") { t ->
                hexText = t
                val h = t.trim().removePrefix("#")
                if (h.length == 6 && h.all { it.isDigit() || it.lowercaseChar() in 'a'..'f' }) set(hexColor("#$h"))
            }
        }
    }
}
