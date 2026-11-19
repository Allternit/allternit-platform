package com.allternit.feature.artifacts

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.SnapshotStateList
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull

/*
 * The phone editor (iOS V2ArtifactEditorView + ArtifactBodyEditing). Models: markdown,
 * a doc's `markdown` field, doc blocks (paragraphs, headings and list items edit; tables,
 * images, dividers and other blocks show locked), a sheet grid with Add row / Add column, and
 * simple slides (title, bullets, notes). Save posts a new version with base_version; 409 shows
 * the iOS conflict copy. Tabbed docs and block decks open read-only with a plain note.
 */

internal const val DOC_FORMAT = "application/vnd.allternit.doc+json"
private const val MARK_KEYS = "bold,italic,underline,strike,code,color,highlight"

internal class DocBlockEdit(val path: List<Int>, val role: String, val level: Int, val number: Int, val numbered: Boolean, val original: String) {
    var text by mutableStateOf(original)
    val editable get() = role == "paragraph" || role == "heading" || role == "item"
}

internal class SlideEdit(val original: JsonObject) {
    val origTitle = original.str("title") ?: ""
    val origBullets = (original["bullets"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }.orEmpty().joinToString("\n")
    val origNotes = original.str("notes") ?: original.str("body") ?: ""
    var title by mutableStateOf(origTitle)
    var bullets by mutableStateOf(origBullets)
    var notes by mutableStateOf(origNotes)
    val changed get() = title != origTitle || bullets != origBullets || notes != origNotes
}

internal sealed interface EditModel {
    class Markdown(val original: String, val field: Boolean) : EditModel { var text by mutableStateOf(original) }
    class Blocks(val blocks: List<DocBlockEdit>) : EditModel
    class Sheet(val original: List<List<String>>, val rows: SnapshotStateList<SnapshotStateList<String>>) : EditModel
    class Slides(val slides: List<SlideEdit>) : EditModel
    data class ReadOnly(val reason: String) : EditModel
}

private fun runsText(el: JsonElement?): String =
    (el as? JsonArray)?.joinToString("") { (it as? JsonObject).str("text") ?: "" } ?: ""

/** A run list for edited text: marks survive when every run shared them (iOS runs(for:replacing:)). */
private fun runsFor(text: String, old: JsonElement?): JsonArray {
    val runs = (old as? JsonArray)?.mapNotNull { it as? JsonObject }.orEmpty()
    val keys = MARK_KEYS.split(",")
    val shared = runs.firstOrNull()?.filterKeys { it in keys }?.takeIf { first -> runs.all { r -> r.filterKeys { it in keys } == first } }.orEmpty()
    return JsonArray(listOf(JsonObject(shared + ("text" to JsonPrimitive(text)))))
}

internal fun editModel(a: ArtifactV2): EditModel {
    val body = a.body
    return when (a.kind) {
        "doc" -> {
            if (a.bodyFormat == "text/markdown" || a.bodyFormat == "text/plain") return EditModel.Markdown(body, field = false)
            val root = LibraryJson.parse(body) ?: return if (a.bodyFormat.startsWith("text/")) EditModel.Markdown(body, false) else EditModel.ReadOnly("This doc can't be edited here")
            if (root["tabs"] != null) return EditModel.ReadOnly("This doc has tabs. Edit it on the web or Desktop.")
            val blocks = root["blocks"] as? JsonArray
            if (blocks == null) {
                return root.str("markdown")?.let { EditModel.Markdown(it, field = true) } ?: EditModel.ReadOnly("This doc can't be edited here")
            }
            val out = mutableListOf<DocBlockEdit>()
            blocks.forEachIndexed { i, raw ->
                val b = raw as? JsonObject
                when (b.str("type")) {
                    "paragraph" -> out += DocBlockEdit(listOf(i), "paragraph", 0, 0, false, runsText(b?.get("content")))
                    "heading" -> out += DocBlockEdit(listOf(i), "heading", b.num("level")?.toInt() ?: 1, 0, false, runsText(b?.get("content")))
                    "list" -> {
                        val numbered = b.str("style") == "numbered"
                        (b?.get("items") as? JsonArray)?.forEachIndexed { j, itRaw ->
                            val item = itRaw as? JsonObject
                            val t = item.str("type")
                            out += if (t == "paragraph" || t == "heading") DocBlockEdit(listOf(i, j), "item", 0, j + 1, numbered, runsText(item?.get("content")))
                            else DocBlockEdit(listOf(i, j), "Nested item", 0, 0, false, "")
                        }
                    }
                    "table" -> out += DocBlockEdit(listOf(i), "Table", 0, 0, false, "")
                    "image" -> out += DocBlockEdit(listOf(i), "Image", 0, 0, false, b.str("alt") ?: "")
                    "divider" -> out += DocBlockEdit(listOf(i), "Divider", 0, 0, false, "")
                    else -> out += DocBlockEdit(listOf(i), "Block", 0, 0, false, "")
                }
            }
            EditModel.Blocks(out)
        }
        "sheet" -> {
            val rows = decodeSheet(body) ?: return EditModel.ReadOnly("This sheet can't be edited here")
            val root = runCatching { ApiJson.default.parseToJsonElement(body) }.getOrNull()
            val rowsShape = root is JsonArray || (root as? JsonObject)?.get("rows") is JsonArray
            if (!rowsShape) return EditModel.ReadOnly("This sheet can't be edited here. Open it on the web or Desktop.")
            // Rows padded to one width up front, so the grid never grows while it draws.
            val width = rows.maxOfOrNull { it.size } ?: 0
            val padded = rows.map { r -> r + List(width - r.size) { "" } }
            val grid = mutableStateListOf<SnapshotStateList<String>>()
            padded.forEach { r -> grid += mutableStateListOf<String>().also { it.addAll(r) } }
            EditModel.Sheet(padded, grid)
        }
        "slides" -> {
            val root = LibraryJson.parse(body)
            val slides = root.list("slides")
            if (slides.isEmpty() || slides.any { it["blocks"] != null }) EditModel.ReadOnly("This deck can't be edited here. Open it on the web or Desktop.")
            else EditModel.Slides(slides.map { SlideEdit(it) })
        }
        else -> EditModel.ReadOnly("This ${ArtifactKinds.noun(a.kind).lowercase()} can't be edited here")
    }
}

internal fun hasChanges(m: EditModel): Boolean = when (m) {
    is EditModel.Markdown -> m.text != m.original
    is EditModel.Blocks -> m.blocks.any { it.text != it.original }
    is EditModel.Sheet -> m.rows.map { it.toList() } != m.original
    is EditModel.Slides -> m.slides.any { it.changed }
    is EditModel.ReadOnly -> false
}

private fun blockMarkdown(b: JsonObject): String = when (b.str("type")) {
    "paragraph" -> runsText(b["content"])
    "heading" -> "#".repeat((b.num("level")?.toInt() ?: 1).coerceIn(1, 6)) + " " + runsText(b["content"])
    "list" -> (b["items"] as? JsonArray).orEmpty().mapIndexed { j, it ->
        (if (b.str("style") == "numbered") "${j + 1}. " else "- ") + runsText((it as? JsonObject)?.get("content"))
    }.joinToString("\n")
    "divider" -> "---"
    "image" -> "![${b.str("alt") ?: ""}](${b.str("src") ?: ""})"
    else -> ""
}

private fun JsonArray.replaceAt(i: Int, v: JsonElement) = JsonArray(toMutableList().also { it[i] = v })

/** iOS ArtifactBodyEditing.serialize for the models above. Null when the body can't be rebuilt. */
internal fun serialize(m: EditModel, a: ArtifactV2): Pair<String, String>? = when (m) {
    is EditModel.Markdown -> if (!m.field) {
        m.text to (if (a.bodyFormat.startsWith("text/")) a.bodyFormat else "text/markdown")
    } else {
        LibraryJson.parse(a.body)?.let { root -> JsonObject(root + ("markdown" to JsonPrimitive(m.text))).toString() to a.bodyFormat }
    }
    is EditModel.Blocks -> LibraryJson.parse(a.body)?.let { root ->
        var blocks = root["blocks"] as? JsonArray ?: return@let null
        m.blocks.filter { it.editable && it.text != it.original }.forEach { e ->
            val block = blocks.getOrNull(e.path[0]) as? JsonObject ?: return@forEach
            blocks = if (e.path.size == 1) {
                blocks.replaceAt(e.path[0], JsonObject(block + ("content" to runsFor(e.text, block["content"]))))
            } else {
                val items = block["items"] as? JsonArray ?: return@forEach
                val item = items.getOrNull(e.path[1]) as? JsonObject ?: return@forEach
                val newItems = items.replaceAt(e.path[1], JsonObject(item + ("content" to runsFor(e.text, item["content"]))))
                blocks.replaceAt(e.path[0], JsonObject(block + ("items" to newItems)))
            }
        }
        val md = blocks.mapNotNull { (it as? JsonObject)?.let(::blockMarkdown) }.filter { it.isNotEmpty() }.joinToString("\n\n")
        var out = root + ("blocks" to blocks) + ("markdown" to JsonPrimitive(md))
        if (root["title"] == null) out = out + ("title" to JsonPrimitive(a.title))
        JsonObject(out).toString() to DOC_FORMAT
    }
    is EditModel.Sheet -> {
        val rows = JsonArray(m.rows.map { r -> JsonArray(r.map { JsonPrimitive(it) }) })
        val root = runCatching { ApiJson.default.parseToJsonElement(a.body) }.getOrNull()
        val body = if (root is JsonObject) JsonObject(root + ("rows" to rows)).toString() else JsonObject(mapOf("rows" to rows)).toString()
        body to a.bodyFormat
    }
    is EditModel.Slides -> LibraryJson.parse(a.body)?.let { root ->
        val slides = JsonArray(m.slides.map { s ->
            if (!s.changed) s.original else {
                var o = s.original + ("title" to JsonPrimitive(s.title))
                if (s.bullets != s.origBullets) o = o + ("bullets" to JsonArray(s.bullets.lines().map { it.trim() }.filter { it.isNotEmpty() }.map { JsonPrimitive(it) }))
                if (s.notes != s.origNotes) o = o + ((if (s.original["notes"] != null || s.original["body"] == null) "notes" else "body") to JsonPrimitive(s.notes))
                JsonObject(o)
            }
        })
        JsonObject(root + ("slides" to slides)).toString() to a.bodyFormat
    }
    is EditModel.ReadOnly -> null
}

@Composable
fun V2ArtifactEditorScreen(a: ArtifactV2, onDone: (saved: ArtifactV2?) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val model = remember(a.id, a.version) { editModel(a) }
    var saving by remember { mutableStateOf(false) }
    var confirmDiscard by remember { mutableStateOf(false) }
    var conflict by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    fun cancel() { if (hasChanges(model)) confirmDiscard = true else onDone(null) }
    BackHandler(onBack = ::cancel)

    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().imePadding().testTag("v2ArtifactEditor")) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 6.dp)) {
            TextButton(onClick = ::cancel, modifier = Modifier.testTag("v2EditorCancel")) { Text("Cancel", color = c.textPrimary) }
            Text(a.title, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, modifier = Modifier.weight(1f).padding(top = 12.dp))
            TextButton(
                enabled = !saving && hasChanges(model),
                modifier = Modifier.testTag("v2EditorSave"),
                onClick = {
                    val out = serialize(model, a)
                    if (out == null) {
                        error = "This artifact couldn't be read."
                    } else scope.launch {
                        saving = true
                        val r = ArtifactsV2Client.saveVersion(a, out.first, out.second)
                        saving = false
                        when {
                            r is ApiResult.Ok -> onDone(r.value ?: a)
                            r is ApiResult.HttpError && r.status == 409 -> conflict = true
                            else -> error = artifactError(r)
                        }
                    }
                },
            ) { Text(if (saving) "Saving…" else "Save", fontWeight = FontWeight.SemiBold) }
        }
        Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            when (model) {
                is EditModel.Markdown -> EditorField(model.text, "Document text", "v2EditorMarkdown", minHeight = 360, mono = false) { model.text = it }
                is EditModel.Blocks -> model.blocks.forEach { b ->
                    if (b.editable) {
                        val label = when (b.role) { "heading" -> "Heading"; "item" -> if (b.numbered) "List item ${b.number}" else "List item"; else -> "Text" }
                        EditorField(b.text, label, "v2EditorBlock${b.path.joinToString(".")}", minHeight = 44, mono = false, big = b.role == "heading") { b.text = it }
                    } else {
                        Text(
                            "${b.role} · Locked", color = c.textTertiary, fontSize = 13.sp,
                            modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).padding(12.dp),
                        )
                    }
                }
                is EditModel.Sheet -> SheetEditor(model)
                is EditModel.Slides -> model.slides.forEachIndexed { i, s ->
                    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).border(1.dp, c.borderSubtle, RoundedCornerShape(14.dp)).padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text("${i + 1} / ${model.slides.size}", color = c.textTertiary, fontSize = 11.sp, fontFamily = FontFamily.Monospace)
                        EditorField(s.title, "Title", "v2EditorSlideTitle$i", 44, false, big = true) { s.title = it }
                        EditorField(s.bullets, "Bullets, one per line", "v2EditorSlideBullets$i", 88, false) { s.bullets = it }
                        EditorField(s.notes, "Notes or body", "v2EditorSlideNotes$i", 66, false) { s.notes = it }
                    }
                }
                is EditModel.ReadOnly -> Text(model.reason, color = c.textSecondary, fontSize = 15.sp, modifier = Modifier.padding(top = 24.dp))
            }
        }
    }

    if (confirmDiscard) AlertDialog(
        onDismissRequest = { confirmDiscard = false },
        title = { Text("Discard your changes?") },
        confirmButton = { TextButton(onClick = { confirmDiscard = false; onDone(null) }) { Text("Discard changes") } },
        dismissButton = { TextButton(onClick = { confirmDiscard = false }) { Text("Keep editing") } },
    )
    if (conflict) AlertDialog(
        onDismissRequest = { conflict = false },
        title = { Text("Someone saved a newer version") },
        text = { Text("Your changes aren't saved. Copy anything you need, then get the latest version and edit again.") },
        confirmButton = { TextButton(onClick = { conflict = false; onDone(null) }) { Text("Get the latest") } },
        dismissButton = { TextButton(onClick = { conflict = false }) { Text("Keep editing") } },
    )
    error?.let { msg ->
        AlertDialog(
            onDismissRequest = { error = null },
            title = { Text("Couldn't save") },
            text = { Text(msg) },
            confirmButton = { TextButton(onClick = { error = null }) { Text("OK") } },
        )
    }
}

@Composable
private fun EditorField(value: String, label: String, tag: String, minHeight: Int, mono: Boolean, big: Boolean = false, onChange: (String) -> Unit) {
    val c = AllternitTheme.colors
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(label, color = c.textTertiary, fontSize = 11.sp)
        BasicTextField(
            value = value, onValueChange = onChange,
            textStyle = TextStyle(color = c.textPrimary, fontSize = if (big) 18.sp else 15.sp, lineHeight = 22.sp, fontWeight = if (big) FontWeight.SemiBold else FontWeight.Normal, fontFamily = if (mono) FontFamily.Monospace else FontFamily.Default),
            cursorBrush = SolidColor(c.textPrimary),
            modifier = Modifier.fillMaxWidth().heightIn(min = minHeight.dp).clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).padding(12.dp).testTag(tag).semantics { contentDescription = label },
        )
    }
}

@Composable
private fun SheetEditor(m: EditModel.Sheet) {
    val c = AllternitTheme.colors
    val cols = (m.rows.maxOfOrNull { it.size } ?: 0).coerceAtLeast(1)
    Column(Modifier.horizontalScroll(rememberScrollState())) {
        m.rows.forEachIndexed { i, row ->
            Row {
                repeat(cols) { j ->
                    BasicTextField(
                        value = row.getOrElse(j) { "" }, onValueChange = { v -> if (j < row.size) row[j] = v }, singleLine = true,
                        textStyle = TextStyle(color = c.textPrimary, fontSize = 13.sp), cursorBrush = SolidColor(c.textPrimary),
                        modifier = Modifier.width(110.dp).border(0.5.dp, c.borderSubtle).padding(6.dp).testTag("v2EditorCell$i:$j")
                            .semantics { contentDescription = "${columnName(j)}${i + 1}" },
                    )
                }
            }
        }
    }
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        TextButton(onClick = { m.rows.add(mutableStateListOf<String>().also { r -> repeat(cols) { r.add("") } }) }, modifier = Modifier.testTag("v2EditorAddRow")) { Text("Add row") }
        TextButton(onClick = { if (m.rows.isEmpty()) m.rows.add(mutableStateListOf("")) else m.rows.forEach { it.add("") } }, modifier = Modifier.testTag("v2EditorAddColumn")) { Text("Add column") }
    }
}
