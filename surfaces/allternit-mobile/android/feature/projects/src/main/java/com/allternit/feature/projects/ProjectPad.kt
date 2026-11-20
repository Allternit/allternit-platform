package com.allternit.feature.projects

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.FilterLabel
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.PillRow
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.relativeTime
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import kotlinx.serialization.json.putJsonObject

// MARK: - Model (iOS ProjectPadStore: ProjectPad, PadTile, PadData)

enum class PadSize(val label: String) { S("Small"), M("Medium"), L("Large"), W("Wide") }

enum class PadTileId(val raw: String, val title: String, val style: Boolean, val defaultSize: PadSize) {
    Decision("decision", "Your decision", true, PadSize.M), Countdown("countdown", "Countdown", true, PadSize.S),
    Progress("progress", "Progress", true, PadSize.S), Team("team", "Team", true, PadSize.M), Chart("chart", "Activity", true, PadSize.S),
    Schedule("schedule", "Schedule", true, PadSize.M), Files("files", "Files", true, PadSize.M), Activity("activity", "Latest changes", true, PadSize.L),
    Actions("actions", "Quick actions", false, PadSize.M), Links("links", "Links", false, PadSize.M), Notes("notes", "Notes", false, PadSize.M),
    Checklist("checklist", "Checklist", false, PadSize.L), Threads("threads", "Threads", false, PadSize.M);

    companion object { fun of(raw: String?) = entries.firstOrNull { it.raw == raw } }
}

data class PadTile(val id: PadTileId, val size: PadSize)
data class PadLink(val title: String, val url: String)
data class PadCheck(val id: String, val text: String, val done: Boolean)
data class PadAction(val id: String, val kind: String, val label: String, val value: String)

data class ProjectPad(
    val view: String = "pad",
    val layout: String = "pad",
    val colors: String = "quiet",
    val template: String? = null,
    val tiles: List<PadTile> = emptyList(),
    val notes: String = "",
    val links: List<PadLink> = emptyList(),
    val checklist: List<PadCheck> = emptyList(),
    val actions: List<PadAction> = emptyList(),
    val updatedAt: Long = 0,
    /** Fields this app does not edit (hidden, and anything newer) ride along unchanged. */
    val extra: JsonObject = JsonObject(emptyMap()),
) {
    fun toJson(): JsonObject = buildJsonObject {
        extra.forEach { (k, v) -> put(k, v) }
        put("v", 1); put("view", view); put("layout", layout); put("colors", colors)
        if (extra["hidden"] == null) putJsonArray("hidden") {}
        if (template == null) put("template", JsonNull) else put("template", template)
        putJsonArray("tiles") { tiles.take(24).forEach { t -> add(buildJsonObject { put("id", t.id.raw); put("size", t.size.name) }) } }
        putJsonObject("data") {
            put("notes", notes.take(4000))
            putJsonArray("links") { links.take(20).forEach { l -> add(buildJsonObject { put("title", l.title.take(80)); put("url", l.url.take(2048)) }) } }
            putJsonArray("checklist") { checklist.take(50).forEach { c -> add(buildJsonObject { put("id", c.id.take(40)); put("text", c.text.take(200)); put("done", c.done) }) } }
            putJsonArray("actions") { actions.take(12).forEach { a -> add(buildJsonObject { put("id", a.id.take(40)); put("kind", a.kind); put("label", a.label.take(40)); put("value", a.value.take(2048)) }) } }
        }
        put("updatedAt", updatedAt)
    }

    /** A Style layout (one of the 10 Styles) draws the dashboard instead of the free grid. */
    val isStyle: Boolean get() = layout != "pad"

    companion object {
        fun from(o: JsonObject?): ProjectPad? {
            o ?: return null
            val data = o.child("data")
            return ProjectPad(
                view = o.str("view") ?: "pad", layout = o.str("layout") ?: "pad", colors = o.str("colors") ?: "quiet", template = o.str("template"),
                tiles = o.list("tiles").mapNotNull { t -> PadTileId.of(t.str("id"))?.let { id -> PadTile(id, PadSize.entries.firstOrNull { it.name == t.str("size") } ?: id.defaultSize) } },
                notes = data.str("notes") ?: "",
                links = data.list("links").mapNotNull { l -> l.str("url")?.let { PadLink(l.str("title") ?: it, it) } },
                checklist = data.list("checklist").mapNotNull { c -> c.str("id")?.let { PadCheck(it, c.str("text") ?: "", c.bool("done") ?: false) } },
                actions = data.list("actions").mapNotNull { a -> a.str("id")?.let { PadAction(it, a.str("kind") ?: "link", a.str("label") ?: "", a.str("value") ?: "") } },
                updatedAt = o.num("updatedAt")?.toLong() ?: 0,
                extra = JsonObject(o.filterKeys { it !in setOf("v", "view", "layout", "colors", "template", "tiles", "data", "updatedAt") }),
            )
        }
    }
}

/** iOS PadTemplate.builtIns (order and tiles verbatim). */
data class PadTemplate(val id: String, val name: String, val blurb: String, val tiles: List<PadTile>)

private fun t(id: PadTileId, size: PadSize) = PadTile(id, size)

val builtInPadTemplates = listOf(
    PadTemplate("launch", "Launch", "Decisions, countdown and the launch list", listOf(t(PadTileId.Decision, PadSize.M), t(PadTileId.Countdown, PadSize.S), t(PadTileId.Progress, PadSize.S), t(PadTileId.Checklist, PadSize.L), t(PadTileId.Schedule, PadSize.M), t(PadTileId.Actions, PadSize.M))),
    PadTemplate("client", "Client work", "What the client needs, files and notes", listOf(t(PadTileId.Decision, PadSize.M), t(PadTileId.Progress, PadSize.S), t(PadTileId.Schedule, PadSize.S), t(PadTileId.Files, PadSize.M), t(PadTileId.Notes, PadSize.M), t(PadTileId.Links, PadSize.W))),
    PadTemplate("build", "Build", "Agents, changes and run buttons", listOf(t(PadTileId.Decision, PadSize.M), t(PadTileId.Actions, PadSize.M), t(PadTileId.Activity, PadSize.L), t(PadTileId.Team, PadSize.M), t(PadTileId.Progress, PadSize.S), t(PadTileId.Chart, PadSize.S))),
    PadTemplate("research", "Research", "Notes, sources and threads", listOf(t(PadTileId.Notes, PadSize.L), t(PadTileId.Threads, PadSize.M), t(PadTileId.Links, PadSize.M), t(PadTileId.Files, PadSize.W))),
    PadTemplate("launchpad", "Launchpad", "Your buttons and links, nothing else", listOf(t(PadTileId.Actions, PadSize.W), t(PadTileId.Links, PadSize.M), t(PadTileId.Notes, PadSize.M), t(PadTileId.Threads, PadSize.W))),
)

/** Default template per mode (chat research, code build, design client, bot launchpad). */
fun defaultPad(mode: ProjectMode): ProjectPad {
    val id = when (mode) { ProjectMode.Chat -> "research"; ProjectMode.Code -> "build"; ProjectMode.Design -> "client"; ProjectMode.Bot -> "launchpad" }
    val tpl = builtInPadTemplates.first { it.id == id }
    return ProjectPad(template = tpl.id, tiles = tpl.tiles)
}

/** iOS ProjectColorSet: the six presets and their main colour. */
val padColorSets = listOf(
    "quiet" to ("Quiet orange" to Color(0xFFC8451F)), "blue" to ("Quiet blue" to Color(0xFF2F54EB)), "green" to ("Quiet green" to Color(0xFF0F7F57)),
    "mono" to ("Black and white" to Color(0xFF3A3D45)), "bright" to ("Bright" to Color(0xFFE5484D)), "sunset" to ("Sunset" to Color(0xFFF76B15)),
)

fun padAccent(colors: String): Color =
    padColorSets.firstOrNull { it.first == colors }?.second?.second
        ?: colors.split(",").firstOrNull()?.takeIf { it.startsWith("#") }?.let { hexColor(it) }
        ?: Color(0xFFC8451F)

// MARK: - Store (GET/PUT project-pad/<scope>, local copy, 400 ms debounce, last write wins)

class ProjectPadStore(private val context: Context, private val scope: String, private val mode: ProjectMode) {
    var pad by mutableStateOf(defaultPad(mode)); private set
    var status by mutableStateOf<String?>(null); private set
    var error by mutableStateOf<String?>(null); private set
    var signedOut by mutableStateOf(false); private set
    private var pending: Job? = null
    private val prefs get() = context.getSharedPreferences("allternit_pads", Context.MODE_PRIVATE)
    private val localKey get() = "allternit.pad.v1.$scope"

    private val path get() = "project-pad/" + java.net.URLEncoder.encode(scope, "UTF-8").replace("+", "%20")

    suspend fun load() {
        val local = prefs.getString(localKey, null)?.let { runCatching { ApiJson.default.parseToJsonElement(it) as? JsonObject }.getOrNull() }?.let { ProjectPad.from(it) }
        local?.let { pad = it }
        when (val r = LibraryNet.get(path)) {
            is ApiResult.Ok -> {
                val remote = ProjectPad.from(r.value.obj.child("pad"))
                if (remote != null) {
                    pad = remote; writeLocal(remote); status = null
                } else if (r.value.obj?.get("pad") != null && r.value.obj?.get("pad") !is JsonNull) {
                    error = "The runtime sent a pad this app could not read."
                } else {
                    // Nothing on the runtime yet: upload this device's copy (or the default).
                    put(local ?: pad)
                }
            }
            is ApiResult.HttpError -> setStatus(r.status, loading = true)
            else -> status = "Saved on this device only. Connect a computer to sync."
        }
    }

    private fun setStatus(code: Int, loading: Boolean) {
        when (code) {
            503 -> status = "Saved on this device only. Connect a computer to sync."
            404 -> status = "Update your computer to sync pads."
            401, 403 -> { signedOut = true; status = "Sign in to customize" }
            else -> error = if (loading) "The pad could not be loaded. It shows what this device has." else "The pad could not be synced. It is saved on this device."
        }
    }

    private fun writeLocal(p: ProjectPad) { prefs.edit().putString(localKey, p.toJson().toString()).apply() }

    /** Shows the change at once, keeps it locally, PUTs it 400 ms later (the newest change wins). */
    fun save(next: ProjectPad, launcher: kotlinx.coroutines.CoroutineScope) {
        val stamped = next.copy(updatedAt = System.currentTimeMillis())
        pad = stamped
        writeLocal(stamped)
        pending?.cancel()
        pending = launcher.launch { delay(400); put(stamped) }
    }

    private suspend fun put(p: ProjectPad) {
        when (val r = LibraryNet.send("PUT", path, buildJsonObject { put("pad", p.toJson()) })) {
            is ApiResult.Ok -> { status = null; error = null }
            is ApiResult.HttpError -> setStatus(r.status, loading = false)
            else -> status = "Saved on this device only. Connect a computer to sync."
        }
    }

    companion object {
        /** `project:<mode>:<id>` (runtime regex `^project:[a-z]{2,16}:[A-Za-z0-9_.:-]{1,128}$`). */
        fun scope(mode: ProjectMode, id: String) = "project:${mode.appearanceMode}:$id"
    }
}

// MARK: - Pad card (iOS ProjectPadCard + ProjectTabCard + ProjectPadView)

/** What the pad's live tiles read: the project's threads and files. */
data class PadBoardData(
    val threads: List<ProjectSession>,
    val files: List<String>,
    val needsYou: Int = 0,
    /** The board the Style tiles read (iOS ProjectBoardData). */
    val board: ProjectBoardData = ProjectBoardData(null, emptyList(), files),
    val onOpenStatus: () -> Unit = {},
)

@Composable
fun ProjectPadCard(
    title: String,
    mode: ProjectMode,
    store: ProjectPadStore,
    folderColor: Color,
    data: PadBoardData,
    onEdit: () -> Unit,
    onOpenThread: (String) -> Unit,
) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val pad = store.pad
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).border(1.dp, c.borderSubtle, RoundedCornerShape(16.dp)).testTag("projectPadCard")) {
        Box(Modifier.fillMaxWidth().height(3.dp).background(folderColor))
        Row(
            Modifier.fillMaxWidth().heightIn(min = 34.dp).background(folderColor.copy(alpha = 0.22f)).padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Column(Modifier.weight(1f)) {
                Text(title, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(mode.label, color = c.textSecondary, fontSize = 12.sp)
            }
            PillRow(listOf("pad", "list"), pad.view, { if (it == "pad") "Pad" else "List" }, { "projectPadView$it" }) { v -> store.save(pad.copy(view = v), scope) }
            if (!store.signedOut) {
                Text(
                    "Edit", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.clip(CircleShape).background(c.fillNeutral).testTag("projectPadEdit").clickable(onClick = onEdit)
                        .semantics { contentDescription = "Edit pad" }.padding(horizontal = 12.dp, vertical = 8.dp),
                )
            }
        }
        if (data.needsYou > 0) {
            Text("Needs you · ${data.needsYou}", color = c.textPrimary, fontSize = 13.sp, modifier = Modifier.padding(12.dp).clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).padding(10.dp))
        }
        Column(Modifier.fillMaxWidth().background(c.bgPanel).background(folderColor.copy(alpha = 0.05f)).padding(12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            if (pad.view == "list") {
                ListGroup("Needs you", if (data.needsYou == 0) listOf() else listOf("${data.needsYou} waiting"), "Nothing needs you")
                ListGroup("Threads", data.threads.take(8).map { it.name.ifEmpty { "Untitled" } }, "No threads yet")
                ListGroup("Files", data.files, "No files yet")
            } else if (pad.tiles.isEmpty()) {
                Text(if (store.signedOut) "No tiles yet." else "No tiles yet. Choose Edit to add some.", color = c.textSecondary, fontSize = 14.sp)
            } else {
                // Phones use 2 columns: S takes 1, M / L / W take the row (iOS PadFlowLayout on a phone).
                val rows = mutableListOf<List<PadTile>>()
                var pendingSmall: PadTile? = null
                pad.tiles.forEach { tile ->
                    if (tile.size == PadSize.S) {
                        if (pendingSmall == null) pendingSmall = tile else { rows += listOf(pendingSmall!!, tile); pendingSmall = null }
                    } else {
                        pendingSmall?.let { rows += listOf(it); pendingSmall = null }
                        rows += listOf(tile)
                    }
                }
                pendingSmall?.let { rows += listOf(it) }
                rows.forEach { row ->
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        row.forEach { tile ->
                            PadTileCard(tile, pad, store, data, Modifier.weight(1f), onOpenThread = onOpenThread, onOpenLink = { com.allternit.feature.artifacts.openUrlExternally(context, it) })
                        }
                        if (row.size == 1 && row[0].size == PadSize.S) Spacer(Modifier.weight(1f))
                    }
                }
            }
            (store.status ?: store.error)?.let { Text(it, color = c.textTertiary, fontSize = 12.sp) }
        }
    }
}

@Composable
private fun ListGroup(title: String, rows: List<String>, empty: String) {
    val c = AllternitTheme.colors
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(title, color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
        if (rows.isEmpty()) Text(empty, color = c.textTertiary, fontSize = 13.sp)
        rows.forEach { Text(it, color = c.textPrimary, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis) }
    }
}

@Composable
private fun PadTileCard(tile: PadTile, pad: ProjectPad, store: ProjectPadStore, data: PadBoardData, modifier: Modifier, onOpenThread: (String) -> Unit, onOpenLink: (String) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val readOnly = store.signedOut
    if (tile.id.style) {
        // iOS draws a Style tile with ProjectDashboardView (Bento) holding only that tile.
        StyleTile(tile.id, data.board, pad.colors, modifier, onOpenStatus = data.onOpenStatus)
        return
    }
    Column(
        modifier.heightIn(min = if (tile.size == PadSize.L) 220.dp else 110.dp).clip(RoundedCornerShape(20.dp)).background(c.bgPanel)
            .border(1.dp, c.borderSubtle, RoundedCornerShape(20.dp)).padding(16.dp).testTag("padTile${tile.id.raw}"),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(tile.id.title, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
        when (tile.id) {
            PadTileId.Notes -> {
                var text by remember(pad.notes) { mutableStateOf(pad.notes) }
                var job by remember { mutableStateOf<Job?>(null) }
                if (readOnly) Text(pad.notes.ifEmpty { "No notes yet." }, color = c.textSecondary, fontSize = 14.sp)
                else {
                    Box(Modifier.fillMaxWidth().heightIn(min = 88.dp).clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).padding(10.dp)) {
                        if (text.isEmpty()) Text("No notes yet. Write anything this project should remember.", color = c.textTertiary, fontSize = 14.sp)
                        BasicTextField(
                            text, { v -> text = v.take(4000); job?.cancel(); job = scope.launch { delay(500); store.save(store.pad.copy(notes = text), scope) } },
                            textStyle = TextStyle(color = c.textPrimary, fontSize = 14.sp), cursorBrush = SolidColor(c.textPrimary),
                            modifier = Modifier.fillMaxWidth().testTag("padNotes"),
                        )
                    }
                    if (4000 - text.length <= 400) Text("${4000 - text.length} characters left", color = c.textTertiary, fontSize = 12.sp)
                }
            }
            PadTileId.Links -> {
                if (pad.links.isEmpty()) Text("No links yet. Add the sites this project uses.", color = c.textSecondary, fontSize = 13.sp)
                pad.links.forEachIndexed { i, l ->
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(l.title, color = c.textPrimary, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f).heightIn(min = 44.dp).clickable { onOpenLink(l.url) }.padding(vertical = 12.dp))
                        if (!readOnly) LibraryIconButton(DesignR.drawable.ph_x, "Remove link ${l.title}", iconSize = 14) { store.save(store.pad.copy(links = store.pad.links.filterIndexed { j, _ -> j != i }), scope) }
                    }
                }
                if (!readOnly) AddLinkRow(pad.links.size >= 20) { title, url -> store.save(store.pad.copy(links = store.pad.links + PadLink(title.ifBlank { url }, url)), scope) }
            }
            PadTileId.Checklist -> {
                val done = pad.checklist.count { it.done }
                Text("$done of ${pad.checklist.size} done", color = c.textTertiary, fontSize = 12.sp)
                if (pad.checklist.isEmpty()) Text("Nothing on the list yet.", color = c.textSecondary, fontSize = 13.sp)
                pad.checklist.forEach { item ->
                    Row(
                        Modifier.fillMaxWidth().heightIn(min = 40.dp).clickable(enabled = !readOnly) {
                            store.save(store.pad.copy(checklist = store.pad.checklist.map { if (it.id == item.id) it.copy(done = !it.done) else it }), scope)
                        },
                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Icon(painterResource(if (item.done) DesignR.drawable.ph_check_circle_fill else DesignR.drawable.ph_check), null, tint = if (item.done) c.accentPrimary else c.textTertiary, modifier = Modifier.size(18.dp))
                        Text(item.text, color = if (item.done) c.textTertiary else c.textPrimary, fontSize = 14.sp)
                    }
                }
                if (!readOnly) {
                    if (pad.checklist.size >= 50) Text("Up to 50 items.", color = c.textTertiary, fontSize = 12.sp)
                    else InlineAdd("Add an item", "padChecklistAdd") { text -> store.save(store.pad.copy(checklist = store.pad.checklist + PadCheck("c" + System.currentTimeMillis().toString(36), text.take(200), false)), scope) }
                }
            }
            PadTileId.Actions -> {
                if (pad.actions.isEmpty()) Text("No quick actions yet. Add a button that opens a thread or a link.", color = c.textSecondary, fontSize = 13.sp)
                pad.actions.forEach { a ->
                    Text(
                        a.label, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium,
                        modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral)
                            .clickable { if (a.kind == "thread") onOpenThread(a.value) else onOpenLink(a.value) }.padding(12.dp),
                    )
                }
                if (!readOnly && pad.actions.size < 12) AddLinkRow(false, buttonTitle = "Add action", nameLabel = "Button name") { title, url ->
                    if (title.isNotBlank()) store.save(store.pad.copy(actions = store.pad.actions + PadAction("a" + System.currentTimeMillis().toString(36), "link", title, url)), scope)
                }
                if (pad.actions.size >= 12) Text("Up to 12 actions.", color = c.textTertiary, fontSize = 12.sp)
            }
            PadTileId.Threads -> {
                val shown = data.threads.sortedByDescending { it.updatedAt }.take(if (tile.size == PadSize.L) 8 else 4)
                if (shown.isEmpty()) Text("No threads yet.", color = c.textSecondary, fontSize = 13.sp)
                shown.forEach { s ->
                    Row(Modifier.fillMaxWidth().heightIn(min = 40.dp).clickable { onOpenThread(s.id) }, verticalAlignment = Alignment.CenterVertically) {
                        Text(s.name.ifEmpty { "Untitled" }, color = c.textPrimary, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                        Text(relativeTime(s.updatedAt), color = c.textTertiary, fontSize = 12.sp)
                    }
                }
            }
            else -> Unit
        }
    }
}

@Composable
private fun InlineAdd(placeholder: String, tag: String, onAdd: (String) -> Unit) {
    val c = AllternitTheme.colors
    var text by remember { mutableStateOf("") }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.weight(1f).clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).padding(10.dp)) {
            if (text.isEmpty()) Text(placeholder, color = c.textTertiary, fontSize = 14.sp)
            BasicTextField(text, { text = it }, singleLine = true, textStyle = TextStyle(color = c.textPrimary, fontSize = 14.sp), cursorBrush = SolidColor(c.textPrimary), modifier = Modifier.fillMaxWidth().testTag(tag))
        }
        TextButton(enabled = text.isNotBlank(), onClick = { onAdd(text.trim()); text = "" }) { Text("Add") }
    }
}

/** "Add link": Name (optional) + https:// (iOS PadLinkForm), with the iOS address check. */
@Composable
private fun AddLinkRow(full: Boolean, buttonTitle: String = "Add link", nameLabel: String = "Name (optional)", onAdd: (String, String) -> Unit) {
    val c = AllternitTheme.colors
    var open by remember { mutableStateOf(false) }
    var name by remember { mutableStateOf("") }
    var url by remember { mutableStateOf("") }
    var err by remember { mutableStateOf<String?>(null) }
    if (full) { Text("Up to 20 links.", color = c.textTertiary, fontSize = 12.sp); return }
    if (!open) { TextButton(onClick = { open = true }) { Text(buttonTitle) }; return }
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        com.allternit.feature.artifacts.LibraryField("", name, nameLabel, "padLinkName") { name = it }
        com.allternit.feature.artifacts.LibraryField("", url, "https://", "padLinkUrl") { url = it }
        err?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp) }
        Row {
            TextButton(onClick = { open = false; name = ""; url = ""; err = null }) { Text("Cancel") }
            TextButton(onClick = {
                val u = url.trim()
                val ok = (u.startsWith("https://") || u.startsWith("http://")) && android.net.Uri.parse(u).host?.contains('.') == true
                if (!ok) err = "Enter a full web address, like https://example.com." else { onAdd(name.trim(), u); open = false; name = ""; url = ""; err = null }
            }) { Text("Add") }
        }
    }
}

// MARK: - Pad edit sheet (iOS PadEditSheet)

@Composable
fun PadEditSheet(store: ProjectPadStore, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var confirmTemplate by remember { mutableStateOf<PadTemplate?>(null) }
    val pad = store.pad
    LibrarySheet(
        if (pad.isStyle) "Choose a pad" else "Edit pad", "padEditSheet", onDismiss = onDismiss,
        trailing = LibrarySheetAction("Done", bold = true, testTag = "padEditDone", onClick = onDismiss),
    ) {
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 28.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (!pad.isStyle) {
                FilterLabel("Tiles")
                pad.tiles.forEachIndexed { i, tile ->
                    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(10.dp).testTag("padEditTile${tile.id.raw}")) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(tile.id.title, color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
                            LibraryIconButton(DesignR.drawable.ph_arrow_up, "Move up", enabled = i > 0, iconSize = 16) { store.save(pad.copy(tiles = pad.tiles.toMutableList().apply { add(i - 1, removeAt(i)) }), scope) }
                            LibraryIconButton(DesignR.drawable.ph_arrow_down, "Move down", enabled = i < pad.tiles.size - 1, iconSize = 16) { store.save(pad.copy(tiles = pad.tiles.toMutableList().apply { add(i + 1, removeAt(i)) }), scope) }
                            LibraryIconButton(DesignR.drawable.ph_trash, "Remove", iconSize = 16) { store.save(pad.copy(tiles = pad.tiles.filterIndexed { j, _ -> j != i }), scope) }
                        }
                        PillRow(PadSize.entries, tile.size, { it.label }, { "padSize${tile.id.raw}${it.name}" }) { s ->
                            store.save(pad.copy(tiles = pad.tiles.mapIndexed { j, t -> if (j == i) t.copy(size = s) else t }), scope)
                        }
                    }
                }
                val missing = PadTileId.entries.filter { id -> pad.tiles.none { it.id == id } }
                if (missing.isNotEmpty() && pad.tiles.size < 24) {
                    FilterLabel("Add tile")
                    missing.forEach { id ->
                        com.allternit.feature.artifacts.LibraryRowButton("Add ${id.title}", DesignR.drawable.ph_plus, "padAdd${id.raw}") {
                            store.save(pad.copy(tiles = pad.tiles + PadTile(id, id.defaultSize)), scope)
                        }
                    }
                }
                Text("Up to 24 tiles.", color = c.textTertiary, fontSize = 12.sp)
            }
            FilterLabel("Colors")
            padColorSets.forEach { (id, v) ->
                Row(
                    Modifier.fillMaxWidth().heightIn(min = 44.dp).clip(RoundedCornerShape(10.dp)).clickable { store.save(pad.copy(colors = id), scope) }.padding(horizontal = 8.dp).testTag("padColors$id"),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Box(Modifier.size(16.dp).clip(CircleShape).background(v.second))
                    Text(v.first, color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
                    if (pad.colors == id) Icon(painterResource(DesignR.drawable.ph_check), "Selected", tint = c.textPrimary, modifier = Modifier.size(16.dp))
                }
            }
            // iOS "Your colors": `#main,#accent`.
            val own = pad.colors.split(",").takeIf { it.size == 2 && it.all { h -> h.startsWith("#") } }
            Text("Your colors", color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.padding(start = 8.dp, top = 4.dp))
            ColorWellRow("Main", own?.get(0) ?: "#C8451F", "padColorMain") { hex -> store.save(store.pad.copy(colors = "$hex,${own?.get(1) ?: "#2F54EB"}"), scope) }
            ColorWellRow("Accent", own?.get(1) ?: "#2F54EB", "padColorAccent") { hex -> store.save(store.pad.copy(colors = "${own?.get(0) ?: "#C8451F"},$hex"), scope) }
            FilterLabel("Starter templates")
            builtInPadTemplates.forEach { tpl ->
                Row(
                    Modifier.fillMaxWidth().heightIn(min = 52.dp).clip(RoundedCornerShape(10.dp)).clickable { if (pad.template != tpl.id) confirmTemplate = tpl }.padding(horizontal = 8.dp).testTag("padTemplate${tpl.id}"),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text(tpl.name, color = c.textPrimary, fontSize = 15.sp)
                        Text(tpl.blurb, color = c.textSecondary, fontSize = 12.sp)
                    }
                    if (pad.template == tpl.id) Text("In use", color = c.textTertiary, fontSize = 12.sp)
                }
            }
            (store.status ?: store.error)?.let { Text(it, color = c.textTertiary, fontSize = 12.sp) }
        }
    }
    confirmTemplate?.let { tpl ->
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { confirmTemplate = null },
            title = { Text("Use ${tpl.name}?") },
            text = { Text("Replaces the tiles. Notes, links and the checklist stay.") },
            confirmButton = { TextButton(onClick = { store.save(store.pad.copy(layout = "pad", template = tpl.id, tiles = tpl.tiles), scope); confirmTemplate = null }) { Text("Replace tiles") } },
            dismissButton = { TextButton(onClick = { confirmTemplate = null }) { Text("Cancel") } },
        )
    }
}
