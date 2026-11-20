package com.allternit.feature.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.launch
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/* Memory pages: MemorySettingsView, MemoryDriveView, MemoryDriveHistoryView. */

private fun q(s: String) = java.net.URLEncoder.encode(s, "UTF-8").replace("+", "%20")

/** iOS MemorySettingsView: the history switch, what Allternit remembers (`memory/v2/facts`), documents, and the drive. */
@Composable
internal fun MemoryPage(host: SettingsHost, back: () -> Unit, push: (String) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var generate by remember { mutableStateOf(SettingsPrefs.flag(context, "generateMemory", true)) }
    val (stats, reloadStats) = rememberLoad("stats") { Net.get("memory/stats").toLoad("load memory") }
    val (facts, reloadFacts) = rememberLoad("facts") { Net.get("memory/v2/facts").toLoad("load your memories") }
    val (docs, reloadDocs) = rememberLoad("docs") { Net.get("memory/documents").toLoad("load your documents") }
    var forget by remember { mutableStateOf<Pair<String, String>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    SPage("Memory", tag = "settingsMemory", onBack = back) {
        SCard {
            SToggleRow("Generate memory from chat history", "memory_generate", generate, detail = "Let Allternit learn from your chats to personalize replies.",
                onChange = { generate = it; SettingsPrefs.setFlag(context, "generateMemory", it) })
        }
        LoadView(stats, reloadStats, "load memory") { body ->
            val o = body.obj
            val total = o.num("facts") ?: o.num("total") ?: o.num("count")
            if (total != null) SNote("${total.toLong()} ${if (total.toLong() == 1L) "memory" else "memories"} stored.")
        }
        SLabel("Memories")
        LoadView(facts, reloadFacts, "load your memories") { body ->
            val rows = body.listUnder("facts", "items", "memories")
            if (rows.isEmpty()) SNote("Nothing yet. Chat with your bots and what matters shows up here.") else SCard {
                rows.forEachIndexed { i, f ->
                    val id = f.str("id") ?: return@forEachIndexed
                    val text = f.str("text") ?: f.str("content") ?: f.str("fact") ?: id
                    if (i > 0) SDivider()
                    SRow(text, "memory_fact_$id", detail = f.str("source"), chevron = false, onClick = { forget = id to text })
                }
            }
        }
        SError(error)
        SLabel("Documents")
        LoadView(docs, reloadDocs, "load your documents") { body ->
            val rows = body.listUnder("documents", "items")
            if (rows.isEmpty()) SNote("No documents yet.") else SCard {
                rows.forEachIndexed { i, d ->
                    if (i > 0) SDivider()
                    val chunks = d.num("chunk_count") ?: d.num("chunkCount")
                    SRow(d.str("title") ?: d.str("name") ?: d.str("id") ?: "Document", "memory_doc_$i", detail = listOfNotNull(d.str("source_type") ?: d.str("sourceType"), chunks?.let { "${it.toLong()} chunk${if (it.toLong() == 1L) "" else "s"}" }).joinToString(" · ").ifBlank { null }, chevron = false)
                }
            }
        }
        SLabel("Memory drive")
        SCard {
            SRow("Memory drive", "memory_drive_row", detail = "The files your bots keep, with every change saved.", onClick = { push("memory-drive") })
        }
        forget?.let { (id, text) ->
            androidx.compose.material3.AlertDialog(
                onDismissRequest = { forget = null },
                title = { androidx.compose.material3.Text("Forget this?") },
                text = { androidx.compose.material3.Text("Allternit forgets “$text”.") },
                confirmButton = {
                    androidx.compose.material3.TextButton(onClick = {
                        forget = null
                        scope.launch {
                            val r = Net.send("DELETE", "memory/v2/facts/${LibraryNet.escape(id)}")
                            if (r is ApiResult.Ok) { error = null; reloadFacts() } else error = r.friendly("forget that memory")
                        }
                    }) { androidx.compose.material3.Text("Forget", color = Danger) }
                },
                dismissButton = { androidx.compose.material3.TextButton(onClick = { forget = null }) { androidx.compose.material3.Text("Cancel") } },
            )
        }
    }
}

/** Selected drive and file for History (shared between the drive page and the history page). */
private object DriveSelection {
    var drive by mutableStateOf<String?>(null)
    var path by mutableStateOf("")
}

private fun driveQuery(drive: String?, first: Boolean = true): String = if (drive.isNullOrBlank()) "" else (if (first) "?" else "&") + "drive=" + q(drive)

/** iOS MemoryDriveView: mounts, the file tree, open a file, reindex. */
@Composable
internal fun MemoryDrivePage(host: SettingsHost, back: () -> Unit, push: (String) -> Unit) {
    val scope = rememberCoroutineScope()
    val (mounts, reloadMounts) = rememberLoad("mounts") { Net.get("memory/drive/mounts").toLoad("load your drives") }
    val drive = DriveSelection.drive
    val (tree, reloadTree) = rememberLoad(drive) { Net.get("memory/drive/tree" + driveQuery(drive)).toLoad("load the files") }
    var open by remember { mutableStateOf<String?>(null) }
    var content by remember { mutableStateOf<String?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    SPage("Memory drive", tag = "settingsMemoryDrive", onBack = if (open != null) ({ open = null; content = null }) else back) {
        if (open != null) {
            val path = open ?: ""
            SNote(path)
            SCard { SRow("History", "drive_history", detail = "Every saved version of this file.", onClick = { DriveSelection.path = path; push("memory-history") }) }
            SNote(content ?: "Loading...")
        } else {
            LoadView(mounts, reloadMounts, "load your drives") { body ->
                val rows = body.listUnder("mounts", "drives", "items")
                if (rows.size > 1) SCard {
                    rows.forEachIndexed { i, m ->
                        val id = m.str("id") ?: m.str("name") ?: return@forEachIndexed
                        if (i > 0) SDivider()
                        SRow(m.str("name") ?: id, "drive_mount_$id", detail = m.str("path"), chevron = false, onClick = { DriveSelection.drive = id },
                            trailing = { if (DriveSelection.drive == id || (DriveSelection.drive == null && i == 0)) CheckMark() })
                    }
                }
            }
            SLabel("Files")
            LoadView(tree, reloadTree, "load the files") { body ->
                val rows = body.listUnder("files", "entries", "tree", "items").filter { (it.str("type") ?: "file") != "dir" && (it.str("type") ?: "file") != "directory" }
                if (rows.isEmpty()) SNote("No files yet.") else SCard {
                    rows.forEachIndexed { i, f ->
                        val path = f.str("path") ?: f.str("name") ?: return@forEachIndexed
                        if (i > 0) SDivider()
                        SRow(path, "drive_file_$i", detail = f.num("size")?.let { "${it.toLong()} bytes" }, onClick = {
                            open = path
                            content = null
                            scope.launch {
                                val r = Net.get("memory/drive/file?path=${q(path)}${driveQuery(DriveSelection.drive, false)}")
                                content = if (r is ApiResult.Ok) (r.value.obj.str("content") ?: r.value.obj.str("text") ?: "(empty)") else r.friendly("open the file")
                            }
                        })
                    }
                }
            }
            SLabel("Index")
            SCard {
                SRow("Reindex", "drive_reindex", detail = "Rebuild the search index for this drive.", chevron = false, onClick = {
                    scope.launch {
                        val r = Net.send("POST", "memory/drive/reindex", buildJsonObject { DriveSelection.drive?.let { put("drive", it) } })
                        message = if (r is ApiResult.Ok) "Reindex started." else r.friendly("reindex")
                    }
                })
            }
            message?.let { SNote(it) }
        }
    }
}

/** iOS MemoryDriveHistoryView: a file's versions (`memory/drive/history`), view one, and the copies (`memory/drive/copies`). */
@Composable
internal fun MemoryHistoryPage(host: SettingsHost, back: () -> Unit) {
    val scope = rememberCoroutineScope()
    val path = DriveSelection.path
    val drive = DriveSelection.drive
    val (history, reloadHistory) = rememberLoad(path) { Net.get("memory/drive/history?path=${q(path)}&limit=30${driveQuery(drive, false)}").toLoad("load the history") }
    val (copies, reloadCopies) = rememberLoad("copies") { Net.get("memory/drive/copies").toLoad("load the copies") }
    var shown by remember { mutableStateOf<String?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    SPage("History", tag = "settingsMemoryHistory", onBack = back) {
        SNote(path.ifBlank { "No file chosen. Open a file in the Memory drive first." })
        LoadView(history, reloadHistory, "load the history") { body ->
            val rows = body.listUnder("history", "revisions", "items", "commits")
            if (rows.isEmpty()) SNote("No saved versions yet.") else SCard {
                rows.forEachIndexed { i, r ->
                    val rev = r.str("revision") ?: r.str("id") ?: r.str("sha") ?: return@forEachIndexed
                    if (i > 0) SDivider()
                    SRow(r.str("message") ?: r.str("summary") ?: rev.take(8), "history_rev_$i", detail = listOfNotNull(r.str("author"), r.str("at") ?: r.str("created_at") ?: r.str("date")).joinToString(" · ").ifBlank { null }, onClick = {
                        shown = "Loading..."
                        scope.launch {
                            val res = Net.get("memory/drive/file?path=${q(path)}&revision=${q(rev)}${driveQuery(drive, false)}")
                            shown = if (res is ApiResult.Ok) (res.value.obj.str("content") ?: res.value.obj.str("text") ?: "(empty)") else res.friendly("open that version")
                        }
                    })
                }
            }
        }
        shown?.let { SLabel("Version"); SNote(it) }
        SLabel("Copies")
        LoadView(copies, reloadCopies, "load the copies") { body ->
            val rows = body.listUnder("copies", "items")
            if (rows.isEmpty()) SNote("No copies set up.") else SCard {
                rows.forEachIndexed { i, c ->
                    if (i > 0) SDivider()
                    SRow(c.str("name") ?: c.str("id") ?: "Copy", "copy_$i", detail = c.str("last_run_at") ?: c.str("status"), chevron = false)
                }
            }
            Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                SButton("Run copies now", "copies_run") {
                    scope.launch {
                        val r = Net.send("POST", "memory/drive/copies/run")
                        message = if (r is ApiResult.Ok) "Copies started." else r.friendly("run the copies")
                        if (r is ApiResult.Ok) reloadCopies()
                    }
                }
            }
        }
        message?.let { SNote(it) }
    }
}
