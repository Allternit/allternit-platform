package com.allternit.feature.artifacts

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.ui.Alignment
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.clickable
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
import kotlinx.serialization.json.JsonNull
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
internal const val SHEET_FORMAT = "application/vnd.allternit.sheet+json"
internal const val SLIDES_FORMAT = "application/vnd.allternit.slides+json"
private const val MARK_KEYS = "bold,italic,underline,strike,code,color,highlight"

/** One editable (or locked) doc block. [path]: [i] block, [i, j] list item, [i, r, c] table cell. */
internal class DocBlockEdit(val path: List<Int>, val role: String, val level: Int, val number: Int, val numbered: Boolean, val original: String, val editable: Boolean) {
    var text by mutableStateOf(original)
}

/** iOS simpleSlides: title, bullets (one per line) and body / content. */
internal class SlideEdit(val original: JsonObject) {
    val origTitle = original.str("title") ?: ""
    val origBullets = (original["bullets"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }.orEmpty().joinToString("\n")
    val origNotes = original.str("body") ?: original.str("content") ?: ""
    var title by mutableStateOf(origTitle)
    var bullets by mutableStateOf(origBullets)
    var notes by mutableStateOf(origNotes)
    val changed get() = title != origTitle || bullets != origBullets || notes != origNotes
}

/** iOS blockSlides: one `text` block of a block deck, in reading order (y, then x). */
internal class SlideTextEdit(val slide: Int, val blockIndex: Int, val original: String) {
    var text by mutableStateOf(original)
}

/** iOS DocTab: a tab of a tabbed doc; [model] null = can't be edited here. */
internal class DocTabEdit(val id: String, title: String, val originalTitle: String, val originalBody: String, val model: EditModel?) {
    var title by mutableStateOf(title)
    val hasChanges get() = title != originalTitle || (model?.let(::hasChanges) ?: false)
}

internal sealed interface EditModel {
    class Markdown(val original: String, val field: Boolean) : EditModel { var text by mutableStateOf(original) }
    class Blocks(val blocks: List<DocBlockEdit>) : EditModel
    class Tabs(val tabs: SnapshotStateList<DocTabEdit>, val originalIds: List<String>) : EditModel
    /** [workbook]: the body has `sheets[0].cells`; otherwise it becomes one on save. */
    class Sheet(val original: List<List<String>>, val rows: SnapshotStateList<SnapshotStateList<String>>, val workbook: Boolean) : EditModel
    class Slides(val slides: List<SlideEdit>) : EditModel
    class BlockSlides(val texts: List<SlideTextEdit>, val slideCount: Int) : EditModel
    data class ReadOnly(val reason: String) : EditModel
}

private fun runsText(el: JsonElement?): String =
    (el as? JsonArray)?.joinToString("") { (it as? JsonObject).str("text") ?: "" } ?: ""

private fun cellText(e: JsonElement?): String = when (e) {
    null, JsonNull -> ""
    is JsonPrimitive -> e.contentOrNull ?: ""
    else -> e.toString()
}

/** A run list for edited text: marks survive when every run shared them (iOS runs(for:replacing:)). */
private fun runsFor(text: String, old: JsonElement?): JsonArray {
    val runs = (old as? JsonArray)?.mapNotNull { it as? JsonObject }.orEmpty()
    val keys = MARK_KEYS.split(",")
    val shared = runs.firstOrNull()?.filterKeys { it in keys }?.takeIf { first -> runs.all { r -> r.filterKeys { it in keys } == first } }.orEmpty()
    return JsonArray(listOf(JsonObject(shared + ("text" to JsonPrimitive(text)))))
}

/** iOS editableCellText: a cell with exactly one paragraph or heading block. */
private fun editableCellText(blocks: JsonArray): String? {
    val only = blocks.singleOrNull() as? JsonObject ?: return null
    return if (only.str("type") == "paragraph" || only.str("type") == "heading") runsText(only["content"]) else null
}

/** doc-tabs.ts `formatOf`. */
internal fun tabFormat(body: String) = if (body.trimStart().startsWith("{")) DOC_FORMAT else "text/markdown"

private fun parse(body: String): JsonElement? = runCatching { ApiJson.default.parseToJsonElement(body) }.getOrNull()

/** iOS docModel. */
private fun docModel(body: String, format: String, allowTabs: Boolean = true): EditModel? {
    if (format == "text/markdown" || format == "text/plain") return EditModel.Markdown(body, field = false)
    val root = parse(body) as? JsonObject ?: return if (format.startsWith("text/")) EditModel.Markdown(body, false) else null
    if (allowTabs) {
        val tabs = (root["tabs"] as? JsonArray)?.mapNotNull { it as? JsonObject }?.takeIf { it.isNotEmpty() }
        if (tabs != null) {
            val list = mutableStateListOf<DocTabEdit>()
            tabs.forEachIndexed { i, t ->
                val tb = t.str("body") ?: t.str("markdown") ?: t["doc"]?.toString() ?: ""
                val title = t.str("title") ?: "Tab ${i + 1}"
                list += DocTabEdit(t.str("id") ?: "t${i + 1}", title, title, tb, docModel(tb, tabFormat(tb), allowTabs = false))
            }
            return EditModel.Tabs(list, list.map { it.id })
        }
    } else if (root["tabs"] != null) return null
    val blocks = root["blocks"] as? JsonArray ?: return root.str("markdown")?.let { EditModel.Markdown(it, field = true) }
    val out = mutableListOf<DocBlockEdit>()
    blocks.forEachIndexed { i, raw ->
        val b = raw as? JsonObject
        when (b.str("type")) {
            "paragraph" -> out += DocBlockEdit(listOf(i), "Text", 0, 0, false, runsText(b?.get("content")), true)
            "heading" -> out += DocBlockEdit(listOf(i), "Heading", b.num("level")?.toInt() ?: 1, 0, false, runsText(b?.get("content")), true)
            "list" -> {
                val numbered = b.str("style") == "numbered"
                (b?.get("items") as? JsonArray)?.forEachIndexed { j, itRaw ->
                    val item = itRaw as? JsonObject
                    val t = item.str("type")
                    out += if (t == "paragraph" || t == "heading") DocBlockEdit(listOf(i, j), "List item", 0, j + 1, numbered, runsText(item?.get("content")), true)
                    else DocBlockEdit(listOf(i, j), "Nested item", 0, 0, false, "", false)
                }
            }
            "table" -> {
                var cells = 0
                (b?.get("rows") as? JsonArray)?.forEachIndexed { r, rowRaw ->
                    ((rowRaw as? JsonObject)?.get("cells") as? JsonArray)?.forEachIndexed { cc, cellRaw ->
                        val cellBlocks = ((cellRaw as? JsonObject)?.get("blocks") as? JsonArray) ?: JsonArray(emptyList())
                        val editable = if (cellRaw is JsonObject) editableCellText(cellBlocks) else null
                        val text = editable ?: cellBlocks.mapNotNull { (it as? JsonObject)?.let(::blockMarkdown) }.joinToString(" ")
                        out += DocBlockEdit(listOf(i, r, cc), "Row ${r + 1}, column ${cc + 1}", 0, 0, false, text, editable != null)
                        cells++
                    }
                }
                if (cells == 0) out += DocBlockEdit(listOf(i), "Table", 0, 0, false, "", false)
            }
            "image" -> out += DocBlockEdit(listOf(i), "Image description", 0, 0, false, cellText(b?.get("alt")), true)
            "divider" -> out += DocBlockEdit(listOf(i), "Divider", 0, 0, false, "", false)
            else -> out += DocBlockEdit(listOf(i), "Block", 0, 0, false, "", false)
        }
    }
    return EditModel.Blocks(out)
}

/** iOS sheetModel: `sheets[0].cells["r:c"]` (formula shown with `=`), else the decoded grid. */
private fun sheetModel(body: String): EditModel? {
    val root = parse(body) as? JsonObject
    val cells = ((root?.get("sheets") as? JsonArray)?.firstOrNull() as? JsonObject)?.get("cells") as? JsonObject
    val grid: List<List<String>>
    val workbook: Boolean
    if (cells != null) {
        var maxR = -1; var maxC = -1
        val parsed = cells.mapNotNull { (k, v) ->
            val p = k.split(":"); val r = p.getOrNull(0)?.toIntOrNull(); val c = p.getOrNull(1)?.toIntOrNull()
            if (r == null || c == null) null else {
                maxR = maxOf(maxR, r); maxC = maxOf(maxC, c)
                val o = v as? JsonObject
                val f = o.str("formula")?.takeIf { it.isNotEmpty() }
                Triple(r, c, f?.let { if (it.startsWith("=")) it else "=$it" } ?: cellText(o?.get("value") ?: v.takeIf { it is JsonPrimitive }))
            }
        }
        val g = MutableList(maxR + 1) { MutableList(maxC + 1) { "" } }
        parsed.forEach { (r, c, t) -> g[r][c] = t }
        grid = g; workbook = true
    } else {
        grid = decodeSheet(body) ?: return null; workbook = false
    }
    val width = grid.maxOfOrNull { it.size } ?: 0
    val padded = grid.map { r -> r + List(width - r.size) { "" } }
    val rows = mutableStateListOf<SnapshotStateList<String>>()
    padded.forEach { r -> rows += mutableStateListOf<String>().also { it.addAll(r) } }
    return EditModel.Sheet(padded, rows, workbook)
}

/** iOS slides models: a block deck (any slide with `blocks`) or simple slides. */
private fun slidesModel(body: String): EditModel? {
    val root = parse(body)
    val slides = ((root as? JsonArray) ?: (root as? JsonObject)?.get("slides") as? JsonArray)?.mapNotNull { it as? JsonObject } ?: return null
    if (slides.any { it["blocks"] != null }) {
        val texts = slides.flatMapIndexed { i, s ->
            (s["blocks"] as? JsonArray).orEmpty().mapIndexedNotNull { j, b -> (b as? JsonObject)?.takeIf { it.str("type") == "text" }?.let { j to it } }
                .sortedWith(compareBy({ it.second.num("y") ?: 0.0 }, { it.second.num("x") ?: 0.0 }))
                .map { (j, b) -> SlideTextEdit(i, j, b.str("text") ?: "") }
        }
        return EditModel.BlockSlides(texts, slides.size)
    }
    return EditModel.Slides(slides.map { SlideEdit(it) })
}

internal fun editModel(a: ArtifactV2): EditModel = when (a.kind) {
    "doc" -> docModel(a.body, a.bodyFormat) ?: EditModel.ReadOnly("This doc can't be edited here")
    "sheet" -> sheetModel(a.body) ?: EditModel.ReadOnly("This sheet can't be edited here")
    "slides" -> slidesModel(a.body) ?: EditModel.ReadOnly("This deck can't be edited here")
    else -> EditModel.ReadOnly("This ${ArtifactKinds.noun(a.kind).lowercase()} can't be edited here")
}

internal fun hasChanges(m: EditModel): Boolean = when (m) {
    is EditModel.Markdown -> m.text != m.original
    is EditModel.Blocks -> m.blocks.any { it.text != it.original }
    is EditModel.Tabs -> m.tabs.map { it.id } != m.originalIds || m.tabs.any { it.hasChanges }
    is EditModel.Sheet -> m.rows.map { it.toList() } != m.original
    is EditModel.Slides -> m.slides.any { it.changed }
    is EditModel.BlockSlides -> m.texts.any { it.text != it.original }
    is EditModel.ReadOnly -> false
}

/** iOS addingFirstTab: the plain doc becomes Tab 1 (edits kept) and an empty Tab 2 follows. */
internal fun addingFirstTab(m: EditModel, originalBody: String): EditModel.Tabs? {
    if (m !is EditModel.Markdown && m !is EditModel.Blocks) return null
    val list = mutableStateListOf(DocTabEdit("t1", "Tab 1", "Tab 1", originalBody, m))
    list += newTab(list)
    return EditModel.Tabs(list, emptyList())
}

/** iOS DocTab.added: "Tab N", empty markdown body, the first unused `t<n>` id. */
internal fun newTab(tabs: List<DocTabEdit>): DocTabEdit {
    var n = 1
    while (tabs.any { it.id == "t$n" }) n++
    return DocTabEdit("t$n", "Tab ${tabs.size + 1}", "", "", EditModel.Markdown("", field = false))
}

private fun blockMarkdown(b: JsonObject): String = when (b.str("type")) {
    "paragraph" -> runsText(b["content"])
    "heading" -> "#".repeat((b.num("level")?.toInt() ?: 1).coerceIn(1, 6)) + " " + runsText(b["content"])
    "list" -> (b["items"] as? JsonArray).orEmpty().mapIndexed { j, it ->
        (if (b.str("style") == "numbered") "${j + 1}. " else "- ") + runsText((it as? JsonObject)?.get("content"))
    }.joinToString("\n")
    "divider" -> "---"
    "image" -> "![${b.str("alt") ?: ""}](${b.str("src") ?: ""})"
    "table" -> (b["rows"] as? JsonArray).orEmpty().joinToString("\n") { row ->
        "| " + ((row as? JsonObject)?.get("cells") as? JsonArray).orEmpty().joinToString(" | ") { cell ->
            ((cell as? JsonObject)?.get("blocks") as? JsonArray).orEmpty().mapNotNull { (it as? JsonObject)?.let(::blockMarkdown) }.joinToString(" ")
        } + " |"
    }
    else -> ""
}

private fun JsonArray.replaceAt(i: Int, v: JsonElement) = JsonArray(toMutableList().also { it[i] = v })
private fun JsonObject.with(k: String, v: JsonElement) = JsonObject(this + (k to v))

/** iOS cell(from:): empty → removed, `=…` → formula, a number → value, else a text value. */
private fun cellJson(text: String): JsonObject? {
    if (text.isEmpty()) return null
    if (text.startsWith("=")) return JsonObject(mapOf("formula" to JsonPrimitive(text)))
    text.toDoubleOrNull()?.let { d ->
        return JsonObject(mapOf("value" to if (d % 1.0 == 0.0 && kotlin.math.abs(d) < 1e15) JsonPrimitive(d.toLong()) else JsonPrimitive(d)))
    }
    return JsonObject(mapOf("value" to JsonPrimitive(text)))
}

/** iOS ArtifactBodyEditing.serialize. Null when the body can't be rebuilt. */
internal fun serializeModel(m: EditModel, kind: String, originalBody: String, originalFormat: String, title: String): Pair<String, String>? = when (m) {
    is EditModel.Markdown -> if (!m.field) m.text to (if (originalFormat.startsWith("text/")) originalFormat else "text/markdown")
    else (parse(originalBody) as? JsonObject)?.let { it.with("markdown", JsonPrimitive(m.text)).toString() to originalFormat }
    is EditModel.Blocks -> (parse(originalBody) as? JsonObject)?.let { root ->
        var blocks = root["blocks"] as? JsonArray ?: return@let null
        m.blocks.filter { it.editable && it.text != it.original }.forEach { e ->
            val block = blocks.getOrNull(e.path[0]) as? JsonObject ?: return@forEach
            blocks = when {
                block.str("type") == "image" -> {
                    val alt = e.text.trim()
                    blocks.replaceAt(e.path[0], if (alt.isEmpty()) JsonObject(block - "alt") else block.with("alt", JsonPrimitive(alt)))
                }
                e.path.size == 1 -> blocks.replaceAt(e.path[0], block.with("content", runsFor(e.text, block["content"])))
                e.path.size == 3 -> {
                    val rows = block["rows"] as? JsonArray ?: return@forEach
                    val row = rows.getOrNull(e.path[1]) as? JsonObject ?: return@forEach
                    val cells = row["cells"] as? JsonArray ?: return@forEach
                    val cell = cells.getOrNull(e.path[2]) as? JsonObject ?: return@forEach
                    val cb = (cell["blocks"] as? JsonArray) ?: JsonArray(emptyList())
                    val first = cb.firstOrNull() as? JsonObject
                    val newBlocks = if (first != null) cb.replaceAt(0, first.with("content", runsFor(e.text, first["content"])))
                    else JsonArray(listOf(JsonObject(mapOf("type" to JsonPrimitive("paragraph"), "content" to JsonArray(listOf(JsonObject(mapOf("text" to JsonPrimitive(e.text)))))))))
                    blocks.replaceAt(e.path[0], block.with("rows", rows.replaceAt(e.path[1], row.with("cells", cells.replaceAt(e.path[2], cell.with("blocks", newBlocks))))))
                }
                else -> {
                    val items = block["items"] as? JsonArray ?: return@forEach
                    val item = items.getOrNull(e.path[1]) as? JsonObject ?: return@forEach
                    blocks.replaceAt(e.path[0], block.with("items", items.replaceAt(e.path[1], item.with("content", runsFor(e.text, item["content"])))))
                }
            }
        }
        var out = root.with("blocks", blocks).with("markdown", JsonPrimitive(blocks.mapNotNull { (it as? JsonObject)?.let(::blockMarkdown) }.filter { it.isNotEmpty() }.joinToString("\n\n")))
        if (root["title"] == null) out = out.with("title", JsonPrimitive(title))
        out.toString() to DOC_FORMAT
    }
    is EditModel.Tabs -> {
        // iOS serializeTabs: one tab collapses to its own body; several become {title, tabs, markdown}.
        val written = m.tabs.map { t ->
            val body = if (t.model != null && hasChanges(t.model)) serializeModel(t.model, "doc", t.originalBody, tabFormat(t.originalBody), t.title)?.first ?: t.originalBody else t.originalBody
            Triple(t.id, t.title, body)
        }
        if (written.size == 1) written[0].third.let { it to tabFormat(it) }
        else {
            val md = written.joinToString("\n\n") { (_, tt, b) -> "# $tt\n\n${docMarkdown(b, tabFormat(b)) ?: b}" }
            JsonObject(mapOf(
                "title" to JsonPrimitive(title),
                "tabs" to JsonArray(written.map { (id, tt, b) -> JsonObject(mapOf("id" to JsonPrimitive(id), "title" to JsonPrimitive(tt), "body" to JsonPrimitive(b))) }),
                "markdown" to JsonPrimitive(md),
            )).toString() to DOC_FORMAT
        }
    }
    is EditModel.Sheet -> {
        // iOS serializeSheet: only changed cells are rewritten, then `rows` as a matrix.
        val root = (parse(originalBody) as? JsonObject).takeIf { m.workbook } ?: JsonObject(mapOf("title" to JsonPrimitive(title)))
        val sheets = (root["sheets"] as? JsonArray) ?: JsonArray(listOf(JsonObject(emptyMap())))
        val sheet0 = sheets.firstOrNull() as? JsonObject ?: JsonObject(emptyMap())
        val cells = (sheet0["cells"] as? JsonObject)?.toMutableMap() ?: mutableMapOf()
        m.rows.forEachIndexed { r, row ->
            row.forEachIndexed { c, text ->
                val before = m.original.getOrNull(r)?.getOrNull(c) ?: ""
                if (text != before || !m.workbook) { val j = cellJson(text); if (j == null) cells.remove("$r:$c") else cells["$r:$c"] = j }
            }
        }
        val matrix = JsonArray(m.rows.mapIndexed { r, row ->
            JsonArray(row.indices.map { c ->
                val o = cells["$r:$c"] as? JsonObject
                o?.get("value") ?: o.str("formula")?.let { JsonPrimitive(if (it.startsWith("=")) it else "=$it") } ?: JsonPrimitive("")
            })
        })
        root.with("sheets", sheets.replaceAt(0, sheet0.with("cells", JsonObject(cells)))).with("rows", matrix).toString() to SHEET_FORMAT
    }
    is EditModel.BlockSlides -> parse(originalBody)?.let { root ->
        val isArray = root is JsonArray
        val slides = ((root as? JsonArray) ?: (root as JsonObject)["slides"] as? JsonArray)?.toMutableList() ?: return@let null
        m.texts.groupBy { it.slide }.forEach { (i, texts) ->
            var s = slides.getOrNull(i) as? JsonObject ?: return@forEach
            var blocks = s["blocks"] as? JsonArray ?: return@forEach
            texts.filter { it.text != it.original }.forEach { t -> (blocks.getOrNull(t.blockIndex) as? JsonObject)?.let { blocks = blocks.replaceAt(t.blockIndex, it.with("text", JsonPrimitive(t.text))) } }
            val ordered = texts.map { it.text }
            s = s.with("blocks", blocks)
            ordered.firstOrNull()?.let { s = s.with("title", JsonPrimitive(it)) }
            s = s.with("body", JsonPrimitive(ordered.drop(1).filter { it.isNotBlank() }.joinToString("\n")))
            slides[i] = s
        }
        (if (isArray) JsonArray(slides) else (root as JsonObject).with("slides", JsonArray(slides))).toString() to SLIDES_FORMAT
    }
    is EditModel.Slides -> parse(originalBody)?.let { root ->
        val isArray = root is JsonArray
        val slides = JsonArray(m.slides.map { s ->
            if (!s.changed) s.original else {
                var o = s.original.with("title", JsonPrimitive(s.title))
                val bullets = s.bullets.lines().map { it.trim() }.filter { it.isNotEmpty() }
                if (s.original["bullets"] != null || bullets.isNotEmpty()) o = o.with("bullets", JsonArray(bullets.map { JsonPrimitive(it) }))
                o = o.with(if (s.original["content"] != null && s.original["body"] == null) "content" else "body", JsonPrimitive(s.notes))
                o
            }
        })
        (if (isArray) slides else (root as JsonObject).with("slides", slides)).toString() to originalFormat
    }
    is EditModel.ReadOnly -> null
}

internal fun serialize(m: EditModel, a: ArtifactV2): Pair<String, String>? = serializeModel(m, a.kind, a.body, a.bodyFormat, a.title)

@Composable
fun V2ArtifactEditorScreen(a: ArtifactV2, onDone: (saved: ArtifactV2?) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var model by remember(a.id, a.version) { mutableStateOf(editModel(a)) }
    var activeTab by remember(a.id, a.version) { mutableStateOf(0) }
    var renaming by remember { mutableStateOf<DocTabEdit?>(null) }
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
        // iOS DocTabBar: the tab strip for a tabbed doc, or "Add tab" on a plain doc.
        when (val m = model) {
            is EditModel.Tabs -> DocTabBar(
                tabs = m.tabs, active = activeTab.coerceIn(0, (m.tabs.size - 1).coerceAtLeast(0)),
                onSelect = { i -> if (i == activeTab) renaming = m.tabs[i] else activeTab = i },
                onDelete = { i -> if (m.tabs.size > 1) { m.tabs.removeAt(i); activeTab = activeTab.coerceAtMost(m.tabs.size - 1) } },
                onAdd = { m.tabs += newTab(m.tabs); activeTab = m.tabs.size - 1; renaming = m.tabs.last() },
                addLabel = "Tab",
            )
            is EditModel.Markdown, is EditModel.Blocks -> if (a.kind == "doc" && !(m is EditModel.Markdown && m.field)) {
                DocTabBar(emptyList(), 0, {}, {}, onAdd = {
                    addingFirstTab(m, a.body)?.let { t -> model = t; activeTab = 1; renaming = t.tabs.last() }
                }, addLabel = "Add tab")
            }
            else -> Unit
        }
        Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            when (val m = model) {
                is EditModel.Tabs -> {
                    val tab = m.tabs.getOrNull(activeTab)
                    val inner = tab?.model
                    if (tab != null && inner == null) {
                        Text("This tab can't be edited here", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.Medium, modifier = Modifier.padding(top = 24.dp))
                        Text("Edit \u201C${tab.title}\u201D on the web or Desktop. Your edits to the other tabs still save.", color = c.textSecondary, fontSize = 14.sp)
                    } else if (inner != null) ModelEditor(inner)
                }
                else -> ModelEditor(m)
            }
        }
    }
    renaming?.let { tab ->
        var name by remember(tab) { mutableStateOf(tab.title) }
        AlertDialog(
            onDismissRequest = { renaming = null },
            title = { Text("Tab name") },
            text = { LibraryField("", name, "Tab name", "docTabName") { name = it } },
            confirmButton = { TextButton(onClick = { name.trim().takeIf { it.isNotEmpty() }?.let { tab.title = it }; renaming = null }) { Text("Rename") } },
            dismissButton = { TextButton(onClick = { renaming = null }) { Text("Cancel") } },
        )
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

/** The editor for one model (a doc, a tab's doc, a sheet or a deck). */
@Composable
private fun ModelEditor(model: EditModel) {
    val c = AllternitTheme.colors
    when (model) {
        is EditModel.Markdown -> EditorField(model.text, "Document text", "v2EditorMarkdown", minHeight = 360, mono = false) { model.text = it }
        is EditModel.Blocks -> model.blocks.forEach { b ->
            if (b.editable) {
                val label = if (b.role == "List item" && b.numbered) "List item ${b.number}" else b.role
                EditorField(b.text, label, "v2EditorBlock${b.path.joinToString(".")}", minHeight = 44, mono = false, big = b.role == "Heading") { b.text = it }
            } else {
                Text(
                    "${b.role} \u00B7 Locked", color = c.textTertiary, fontSize = 13.sp,
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
        is EditModel.BlockSlides -> (0 until model.slideCount).forEach { i ->
            val texts = model.texts.filter { it.slide == i }
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).border(1.dp, c.borderSubtle, RoundedCornerShape(14.dp)).padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("${i + 1} / ${model.slideCount}", color = c.textTertiary, fontSize = 11.sp, fontFamily = FontFamily.Monospace)
                if (texts.isEmpty()) Text("No text on this slide", color = c.textTertiary, fontSize = 14.sp)
                texts.forEachIndexed { k, t -> EditorField(t.text, if (k == 0) "Title" else "Text", "v2EditorSlideText$i.${t.blockIndex}", 44, false, big = k == 0) { t.text = it } }
            }
        }
        is EditModel.Tabs -> Unit
        is EditModel.ReadOnly -> Text(model.reason, color = c.textSecondary, fontSize = 15.sp, modifier = Modifier.padding(top = 24.dp))
    }
}

/** iOS DocTabBar: tab pills (tap the selected one to rename, ✕ to delete when 2+), then Add tab / Tab (`docAddTab`). */
@Composable
private fun DocTabBar(tabs: List<DocTabEdit>, active: Int, onSelect: (Int) -> Unit, onDelete: (Int) -> Unit, onAdd: () -> Unit, addLabel: String) {
    val c = AllternitTheme.colors
    Row(
        Modifier.fillMaxWidth().background(c.bgSecondary).horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically,
    ) {
        tabs.forEachIndexed { i, t ->
            val on = i == active
            Row(
                Modifier.clip(CircleShape).background(if (on) c.bgPrimary else androidx.compose.ui.graphics.Color.Transparent)
                    .border(if (on) 1.dp else 0.dp, if (on) c.borderSubtle else androidx.compose.ui.graphics.Color.Transparent, CircleShape)
                    .testTag("docTab${t.id}").clickable(onClickLabel = if (on) "Rename this tab" else null) { onSelect(i) }
                    .padding(start = 12.dp, end = if (on && tabs.size > 1) 2.dp else 12.dp, top = 7.dp, bottom = 7.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (t.model == null) Text("\uD83D\uDD12 ", fontSize = 11.sp)
                Text(t.title, color = if (on) c.textPrimary else c.textSecondary, fontSize = 14.sp, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal)
                if (t.hasChanges) Box(Modifier.padding(start = 4.dp).size(6.dp).clip(CircleShape).background(c.accentPrimary))
                if (on && tabs.size > 1) Text(
                    "\u2715", color = c.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.size(28.dp).clip(CircleShape).clickable(onClickLabel = "Delete tab ${t.title}") { onDelete(i) }.padding(7.dp),
                )
            }
        }
        Text(
            "+ $addLabel", color = c.textSecondary, fontSize = 14.sp,
            modifier = Modifier.clip(CircleShape).testTag("docAddTab").clickable(onClick = onAdd).padding(horizontal = 10.dp, vertical = 7.dp),
        )
    }
    Box(Modifier.fillMaxWidth().height(0.5.dp).background(c.borderSubtle))
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
