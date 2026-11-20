package com.allternit.feature.projects

import android.graphics.BitmapFactory
import android.net.Uri
import android.util.Base64
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.FilterLabel
import com.allternit.feature.artifacts.LibraryField
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.PillRow
import java.io.ByteArrayOutputStream
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonPrimitive

/** `#rrggbb` → Color (fallback the default folder blue). */
internal fun hexColor(hex: String?, fallback: Color = Color(0xFF397AD5)): Color {
    val h = hex?.trim()?.removePrefix("#") ?: return fallback
    return runCatching { Color(("ff" + h.take(6)).toLong(16)) }.getOrDefault(fallback)
}

internal fun Color.hex(): String = String.format("#%06x", toArgb() and 0xFFFFFF)

/** iOS shade: lighten (+) or darken (-) by a fraction. */
private fun Color.shade(f: Float): Color = if (f >= 0) Color(red + (1 - red) * f, green + (1 - green) * f, blue + (1 - blue) * f, alpha)
else Color(red * (1 + f), green * (1 + f), blue * (1 + f), alpha)

private fun Color.isLight(): Boolean = (red * 299 + green * 587 + blue * 114) * 255 / 1000 > 165

/** The 24 cover symbols (iOS FolderLookSheet), drawn as glyphs on Android. */
internal val folderSymbols = linkedMapOf(
    "sparkle" to "✦", "code" to "</>", "briefcase" to "▤", "robot" to "⌂", "star" to "★", "heart" to "♥",
    "lightning" to "ϟ", "leaf" to "❧", "globe" to "◎", "music" to "♫", "camera" to "◉", "book" to "▥",
    "game" to "◈", "palette" to "◐", "terminal" to ">_", "diamond" to "◆", "rocket" to "↗", "shield" to "⛨",
    "target" to "◎", "chart" to "▁▃▅", "gear" to "⚙", "lightbulb" to "☀", "cube" to "▣", "folder" to "▭",
)

/**
 * iOS FolderCoverView: a 250×304 viewbox scaled to [width]. Back tab with its rotated label, the
 * back panel, up to three papers, the front cover with its pattern, frame and edge, the art
 * (title / symbol / text / image) and the mode chip. [open] lifts the papers and tilts the cover.
 */
@Composable
fun FolderCover(appearance: Appearance?, title: String, mode: ProjectMode, width: Dp = 170.dp, open: Boolean = false, flat: Boolean = false, modifier: Modifier = Modifier) {
    val a = appearance ?: Appearance.Default
    val color = hexColor(a.color)
    val paper = hexColor(a.paperColor, Color.White)
    val ink = if (color.isLight()) Color(0xFF1F1E1D) else Color.White
    val image = remember(a.image) {
        a.image?.substringAfter("base64,", "")?.takeIf { it.isNotEmpty() }?.let { b ->
            runCatching { Base64.decode(b, Base64.DEFAULT).let { BitmapFactory.decodeByteArray(it, 0, it.size)?.asImageBitmap() } }.getOrNull()
        }
    }
    Canvas(modifier.size(width, width * 304f / 250f).semantics { contentDescription = "Folder cover for $title" }) {
        val k = size.width / 250f
        fun r(x: Float, y: Float, w: Float, h: Float) = Offset(x * k, y * k) to Size(w * k, h * k)
        // Back tab + label.
        val (tabO, tabS) = r(196f, 16f, 50f, 112f)
        drawRoundRect(a.tabColor?.let { hexColor(it) } ?: color.shade(0.07f), tabO, tabS, CornerRadius(11 * k))
        if (a.tabLabel.isNotEmpty()) {
            rotate(90f, Offset(221 * k, 72 * k)) {
                drawLabel(a.tabLabel.take(8).uppercase(), 221 * k, 76 * k, 11 * k, ink, mono = true)
            }
        }
        val (backO, backS) = r(12f, 4f, 214f, 294f)
        drawRoundRect(color.shade(0.03f), backO, backS, CornerRadius(12 * k))
        // Papers.
        val lift = if (open) 14f else 0f
        for (i in 0 until a.sheets) {
            val (pO, pS) = r(16f + i * 4f, 14f + i * 4f - lift, 200f, 280f)
            drawRoundRect(paper, pO, pS, CornerRadius(8 * k))
            drawRoundRect(Color.Black.copy(alpha = 0.2f), pO, pS, CornerRadius(8 * k), style = Stroke(1f))
        }
        // Front cover.
        withTransform({ if (open) rotate(-2.5f, Offset(2 * k, 300 * k)) }) {
            val (fO, fS) = r(2f, 10f, 214f, 290f)
            val front = Path().apply { addRoundRect(androidx.compose.ui.geometry.RoundRect(fO.x, fO.y, fO.x + fS.width, fO.y + fS.height, CornerRadius(12 * k))) }
            if (a.edge == "shadow") withTransform({ translate(0f, 3 * k) }) { drawPath(front, Color.Black.copy(alpha = 0.18f)) }
            drawPath(front, color)
            clipPath(front) { drawPattern(a.pattern, ink, k) }
            when (a.frame) {
                "dotted" -> { val (o, s) = r(14f, 22f, 190f, 266f); drawRoundRect(ink.copy(alpha = 0.7f), o, s, CornerRadius(5 * k), style = Stroke(1.2f * k, pathEffect = PathEffect.dashPathEffect(floatArrayOf(1 * k, 4 * k)))) }
                "double" -> {
                    val (o, s) = r(14f, 22f, 190f, 266f); drawRoundRect(ink.copy(alpha = 0.7f), o, s, CornerRadius(5 * k), style = Stroke(1.2f * k))
                    val (o2, s2) = r(19f, 27f, 180f, 256f); drawRoundRect(ink.copy(alpha = 0.5f), o2, s2, CornerRadius(4 * k), style = Stroke(1f * k))
                }
            }
            when (a.edge) {
                "ink" -> drawPath(front, Color.Black.copy(alpha = 0.62f), style = Stroke(1.7f * k))
                "soft" -> drawPath(front, Color.Black.copy(alpha = 0.2f), style = Stroke(1f * k))
            }
            if (!flat) {
                val y = when (a.place) { "top" -> 70f; "bottom" -> 230f; else -> 150f } * k
                when (a.art) {
                    "title" -> {
                        val serif = a.font == "serif"
                        val base = if (title.length > 14) 30f else 36f
                        drawLabel(title.take(28), 109 * k, y, (if (serif) base + 4 else base) * k * 0.6f, ink, mono = a.font == "mono" || a.font == "typewriter", serif = serif, wrapWidth = 180 * k)
                    }
                    "symbol" -> drawLabel(folderSymbols[a.symbol] ?: "✦", 109 * k, y + 20 * k, 64 * k, ink)
                    "text" -> drawLabel(a.text, 109 * k, y + 14 * k, (if (a.text.length > 4) 34f else 64f) * k, ink)
                    "image" -> image?.let { img ->
                        val box = 120 * k
                        drawImage(img, dstOffset = androidx.compose.ui.unit.IntOffset((109 * k - box / 2).toInt(), (y - box / 2).toInt()), dstSize = androidx.compose.ui.unit.IntSize(box.toInt(), box.toInt()))
                    }
                }
            }
            if (a.showMode) {
                val (o, s) = r(82f, 252f, 54f, 17f)
                drawRoundRect(ink.copy(alpha = 0.16f), o, s, CornerRadius(3 * k))
                drawLabel(mode.raw.uppercase(), 109 * k, 264.5f * k, 10 * k, ink, mono = true)
            }
        }
    }
}

private fun DrawScope.drawLabel(text: String, cx: Float, baseline: Float, sizePx: Float, color: Color, mono: Boolean = false, serif: Boolean = false, wrapWidth: Float? = null) {
    val paint = android.graphics.Paint(android.graphics.Paint.ANTI_ALIAS_FLAG).apply {
        this.color = color.toArgb()
        textSize = sizePx
        textAlign = android.graphics.Paint.Align.CENTER
        typeface = when {
            mono -> android.graphics.Typeface.create(android.graphics.Typeface.MONOSPACE, android.graphics.Typeface.BOLD)
            serif -> android.graphics.Typeface.SERIF
            else -> android.graphics.Typeface.create(android.graphics.Typeface.SANS_SERIF, android.graphics.Typeface.BOLD)
        }
    }
    val lines = if (wrapWidth == null) listOf(text) else {
        val words = text.split(' ')
        val out = mutableListOf<String>()
        var line = ""
        for (w in words) {
            val next = if (line.isEmpty()) w else "$line $w"
            if (paint.measureText(next) > wrapWidth && line.isNotEmpty()) { out += line; line = w } else line = next
        }
        if (line.isNotEmpty()) out += line
        out.take(3)
    }
    lines.forEachIndexed { i, l -> drawContext.canvas.nativeCanvas.drawText(l, cx, baseline + i * sizePx * 1.15f, paint) }
}

private fun DrawScope.drawPattern(pattern: String, ink: Color, k: Float) {
    val line = ink.copy(alpha = 0.10f)
    when (pattern) {
        "grid" -> { var x = 0f; while (x < size.width) { drawLine(line, Offset(x, 0f), Offset(x, size.height)); x += 16 * k }; var y = 0f; while (y < size.height) { drawLine(line, Offset(0f, y), Offset(size.width, y)); y += 16 * k } }
        "linen" -> { var y = 0f; while (y < size.height) { drawLine(line, Offset(0f, y), Offset(size.width, y)); y += 6 * k } }
        "dots" -> { var y = 6 * k; while (y < size.height) { var x = 6 * k; while (x < size.width) { drawCircle(ink.copy(alpha = 0.16f), 1.2f * k, Offset(x, y)); x += 12 * k }; y += 12 * k } }
        "stripes" -> { var d = -size.height; while (d < size.width) { drawLine(line, Offset(d, size.height), Offset(d + size.height, 0f), strokeWidth = 4 * k); d += 14 * k } }
        "blueprint" -> {
            var x = 0f; while (x < size.width) { drawLine(ink.copy(alpha = if ((x / (10 * k)).toInt() % 4 == 0) 0.18f else 0.07f), Offset(x, 0f), Offset(x, size.height)); x += 10 * k }
            var y = 0f; while (y < size.height) { drawLine(ink.copy(alpha = if ((y / (10 * k)).toInt() % 4 == 0) 0.18f else 0.07f), Offset(0f, y), Offset(size.width, y)); y += 10 * k }
        }
        "circuit" -> { var y = 24 * k; while (y < size.height) { var x = 24 * k; while (x < size.width) { drawLine(line, Offset(x - 12 * k, y), Offset(x + 12 * k, y), strokeWidth = 1.5f * k); drawLine(line, Offset(x + 12 * k, y), Offset(x + 12 * k, y + 12 * k), strokeWidth = 1.5f * k); drawCircle(ink.copy(alpha = 0.16f), 2 * k, Offset(x - 12 * k, y)); x += 48 * k }; y += 48 * k } }
        "grain" -> { val rnd = java.util.Random(7); repeat(260) { drawCircle(ink.copy(alpha = 0.05f), 0.8f * k, Offset(rnd.nextFloat() * size.width, rnd.nextFloat() * size.height)) } }
    }
}

// MARK: - Folder look sheet

private data class Preset(val id: String, val name: String, val color: String?, val art: String, val font: String, val place: String, val pattern: String, val frame: String, val edge: String)

private val presets = listOf(
    Preset("classic", "Classic", null, "title", "mono", "center", "grain", "dotted", "ink"),
    Preset("blueprint", "Blueprint", "#1F4FA8", "title", "mono", "top", "blueprint", "none", "ink"),
    Preset("notebook", "Notebook", "#F4F1EA", "title", "typewriter", "top", "grid", "none", "soft"),
    Preset("editorial", "Editorial", "#D2453A", "title", "serif", "bottom", "grain", "double", "ink"),
    Preset("circuit", "Circuit", "#2FA7A0", "title", "mono", "center", "circuit", "dotted", "ink"),
    Preset("badge", "Badge", "#9B6BD6", "symbol", "sans", "center", "dots", "none", "shadow"),
)

/** A custom look: no preset pill is selected. */
private val NoPreset = Preset("", "", null, "", "", "", "", "", "")

/**
 * iOS FolderLookSheet: preview, presets, colours, cover art (title / symbol / text / image /
 * none), cover style (font, position, pattern, frame, edge, sheets), the tab and the mode label.
 * Save puts `{appearance}` and reports the iOS 503 / 404 copy. [extras] is the caller's
 * "Project" section (settings, pad, details, archive, delete).
 */
@Composable
fun FolderLookSheet(
    project: LibraryProject,
    initial: Appearance?,
    onSaved: (Appearance) -> Unit,
    onDismiss: () -> Unit,
    extras: (@Composable () -> Unit)? = null,
) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var draft by remember { mutableStateOf(initial ?: Appearance.Default) }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri: Uri? ->
        if (uri != null) scope.launch {
            val url = withContext(Dispatchers.IO) { encodeCoverImage(context, uri) }
            if (url == null) error = "That image could not be used. Try another one." else draft = draft.withField("image", JsonPrimitive(url)).withCover("art", JsonPrimitive("image"))
        }
    }
    fun cover(key: String, value: String) { draft = draft.withCover(key, JsonPrimitive(value)) }
    LibrarySheet(
        "Folder look", "folderLookSheet", onDismiss = onDismiss,
        leading = LibrarySheetAction("Cancel", testTag = "folderLookCancel", onClick = onDismiss),
        trailing = LibrarySheetAction(if (saving) "Saving" else "Save", enabled = !saving && !(draft.art == "image" && draft.image == null), bold = true, testTag = "folderLookSave") {
            saving = true
            error = null
            scope.launch {
                when (val r = AppearanceClient.put(project.mode, project.nativeId, draft)) {
                    AppearanceSave.Ok -> { saving = false; onSaved(draft); onDismiss() }
                    is AppearanceSave.Failed -> { saving = false; error = r.message }
                }
            }
        },
    ) {
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 28.dp).verticalScroll(rememberScrollState())) {
            Box(Modifier.fillMaxWidth().padding(vertical = 8.dp), contentAlignment = Alignment.Center) {
                FolderCover(draft, project.title, project.mode, width = 150.dp, modifier = Modifier.semantics { contentDescription = "Folder preview for ${project.title}" })
            }
            error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.padding(vertical = 6.dp)) }
            FilterLabel("Presets")
            PillRow(presets, presets.firstOrNull { it.id == draft.cover.strOrNull("preset") } ?: NoPreset, { it.name }, { "folderPreset${it.id}" }) { p ->
                var d = draft
                p.color?.let { d = d.withField("color", JsonPrimitive(it)) }
                listOf("art" to p.art, "font" to p.font, "place" to p.place, "pattern" to p.pattern, "frame" to p.frame, "edge" to p.edge).forEach { (k, v) -> d = d.withCover(k, JsonPrimitive(v)) }
                draft = d.withCover("preset", JsonPrimitive(p.id))
            }
            FilterLabel("Color")
            ColorWellRow("Folder color", draft.color, "folderColor") { draft = draft.withField("color", JsonPrimitive(it)) }
            ColorWellRow("Paper color", draft.paperColor, "folderPaper") { draft = draft.withField("paperColor", JsonPrimitive(it)) }
            FilterLabel("Cover art")
            PillRow(listOf("title", "symbol", "text", "image", "none"), draft.art, { it.replaceFirstChar { ch -> ch.uppercase() } }, { "folderArt$it" }) { cover("art", it) }
            when (draft.art) {
                "symbol" -> Column(Modifier.padding(top = 8.dp)) {
                    PillRow(folderSymbols.keys.toList(), draft.symbol, { "${folderSymbols[it]}  $it" }, { "folderSymbol$it" }) { draft = draft.withField("symbol", JsonPrimitive(it)) }
                }
                "text" -> Box(Modifier.padding(top = 8.dp)) {
                    LibraryField("", draft.text, "Text or emoji (12 characters)", "folderCoverText") { draft = draft.withField("text", JsonPrimitive(it.take(12))) }
                }
                "image" -> Row(Modifier.padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    TextButton(onClick = { pick.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) }, modifier = Modifier.testTag("folderChooseImage")) {
                        Text(if (draft.image != null) "Change image" else "Choose image")
                    }
                    if (draft.image != null) TextButton(onClick = { draft = Appearance(kotlinx.serialization.json.JsonObject(draft.raw - "image")) }) { Text("Remove image", color = AllternitStatic.StatusError) }
                }
            }
            if (draft.art == "image" && draft.image == null) Text("Choose an image to use it as cover art.", color = c.textSecondary, fontSize = 13.sp)
            FilterLabel("Font")
            PillRow(listOf("mono", "serif", "sans", "typewriter"), draft.font, { it.replaceFirstChar { ch -> ch.uppercase() } }, { "folderFont$it" }) { cover("font", it) }
            FilterLabel("Position")
            PillRow(listOf("top", "center", "bottom"), draft.place, { it.replaceFirstChar { ch -> ch.uppercase() } }, { "folderPlace$it" }) { cover("place", it) }
            FilterLabel("Pattern")
            PillRow(listOf("plain", "grain", "linen", "grid", "dots", "stripes", "blueprint", "circuit"), draft.pattern, { it.replaceFirstChar { ch -> ch.uppercase() } }, { "folderPattern$it" }) { cover("pattern", it) }
            FilterLabel("Frame")
            PillRow(listOf("none", "dotted", "double"), draft.frame, { it.replaceFirstChar { ch -> ch.uppercase() } }, { "folderFrame$it" }) { cover("frame", it) }
            FilterLabel("Edge")
            PillRow(listOf("ink", "soft", "shadow"), draft.edge, { it.replaceFirstChar { ch -> ch.uppercase() } }, { "folderEdge$it" }) { cover("edge", it) }
            FilterLabel("Sheets of paper: ${draft.sheets}")
            PillRow(listOf(0, 1, 2, 3), draft.sheets, { "$it" }, { "folderSheets$it" }) { draft = draft.withCover("sheets", JsonPrimitive(it)) }
            FilterLabel("Tab")
            ColorWellRow("Tab color", draft.tabColor, "folderTab") { cover("tabColor", it) }
            Box(Modifier.padding(top = 8.dp)) {
                LibraryField("", draft.tabLabel, "Tab label (up to 8 characters)", "folderTabLabel") { cover("tabLabel", it.take(8)) }
            }
            FilterLabel("Show mode label")
            PillRow(listOf(true, false), draft.showMode, { if (it) "On" else "Off" }, { "folderShowMode$it" }) { draft = draft.withCover("showMode", JsonPrimitive(it)) }
            extras?.let { Column(Modifier.padding(top = 18.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) { FilterLabel("Project"); it() } }
        }
    }
}

private fun kotlinx.serialization.json.JsonObject?.strOrNull(key: String): String? = (this?.get(key) as? JsonPrimitive)?.content

/** iOS: scale to 256, then 192, then 128 px PNG until the data URL is under 128,000 characters. */
private fun encodeCoverImage(context: android.content.Context, uri: Uri): String? {
    val src = context.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it) } ?: return null
    for (side in listOf(256, 192, 128)) {
        val scale = side.toFloat() / maxOf(src.width, src.height)
        val bmp = android.graphics.Bitmap.createScaledBitmap(src, maxOf(1, (src.width * scale).toInt()), maxOf(1, (src.height * scale).toInt()), true)
        val out = ByteArrayOutputStream()
        bmp.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, out)
        val url = "data:image/png;base64," + Base64.encodeToString(out.toByteArray(), Base64.NO_WRAP)
        if (url.length < 128_000) return url
    }
    return null
}
