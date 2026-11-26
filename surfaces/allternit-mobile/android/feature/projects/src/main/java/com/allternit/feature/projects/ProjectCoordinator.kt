package com.allternit.feature.projects

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Base64
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
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
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.EmptyArtKind
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.InlineEmpty
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.LibraryField
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.LibraryRowButton
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.LibrarySpinner
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.parseTime
import com.allternit.feature.artifacts.str
import java.text.SimpleDateFormat
import java.util.Locale
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject

/*
 * The parts of iOS ProjectDetailView that talk to the project coordinator (BotProjectClient):
 * registration (`PUT project-workspaces/chat-<id>`), the conversation (`projects/<id>/messages`),
 * the status record, the project's server threads, Delegate (`projects/<id>/delegate`), project
 * memory (`memory/v2`), Add files (`POST uploads` + `cowork/projects/<id>/files`) and the folder
 * picker over the computer's folders (`v1/path`, `v1/file/tree` through the runtime relay).
 */

/** A GET on the user's computer through the runtime relay (the shell's ChatBackend.runtimeJson). */
typealias RuntimeGet = suspend (path: String) -> Result<JsonElement>

data class ProjectMessage(val id: String, val role: String, val text: String, val kind: String?, val threadCount: Int, val threadTitles: List<String>, val at: Long)
data class ProjectThread(val id: String, val title: String, val group: String, val statusLine: String?, val sessionId: String?, val at: Long)

object CoordinatorClient {
    private fun esc(s: String) = LibraryNet.escape(s)

    /** The coordinator's id if it is registered already (no write). */
    suspend fun existing(p: LibraryProject): String? =
        (LibraryNet.get("project-workspaces/${esc("chat-${p.nativeId}")}") as? ApiResult.Ok)?.value.obj.str("projectId")

    /** iOS ensureCoordinator: the coordinator's id (`projectId` of the chat workspace), registering it on first use. */
    suspend fun ensure(p: LibraryProject, directory: String?): String? {
        val key = "project-workspaces/${esc("chat-${p.nativeId}")}"
        (LibraryNet.get(key) as? ApiResult.Ok)?.value.obj.str("projectId")?.let { return it }
        val r = LibraryNet.send("PUT", key, buildJsonObject {
            put("nativeId", p.nativeId); put("mode", "chat"); put("title", p.title)
            if (p.description == null) put("description", JsonNull) else put("description", p.description)
            putJsonObject("settings") { put("context", p.description ?: ""); if (directory == null) put("directory", JsonNull) else put("directory", directory) }
        })
        return (r as? ApiResult.Ok)?.value.obj.str("projectId")
    }

    /** iOS setWorkspaceDirectory: merge `settings.directory` into the stored workspace and PUT it back. */
    suspend fun setDirectory(p: LibraryProject, directory: String?): Boolean {
        val key = "project-workspaces/${esc("chat-${p.nativeId}")}"
        val current = (LibraryNet.get(key) as? ApiResult.Ok)?.value.obj ?: return false
        val settings = current.child("settings") ?: JsonObject(emptyMap())
        val next = if (directory == null) JsonObject(settings - "directory") else JsonObject(settings + ("directory" to JsonPrimitive(directory)))
        return LibraryNet.send("PUT", key, buildJsonObject {
            put("nativeId", p.nativeId); put("mode", "chat"); put("title", p.title)
            if (p.description == null) put("description", JsonNull) else put("description", p.description)
            put("settings", next)
        }) is ApiResult.Ok
    }

    suspend fun messages(id: String): List<ProjectMessage>? =
        (LibraryNet.get("projects/${esc(id)}/messages") as? ApiResult.Ok)?.value.listUnder("messages").mapNotNull { o ->
            val mid = o.str("id") ?: return@mapNotNull null
            val payload = o.child("payload")
            val threads = payload.list("threads")
            ProjectMessage(
                mid, o.str("role") ?: "coordinator", o.str("text") ?: "", payload.str("kind"),
                threads.size.takeIf { it > 0 } ?: payload.num("threads")?.toInt() ?: 0,
                threads.mapNotNull { it.str("title") } + listOfNotNull(payload.str("threadTitle")),
                parseTime(o.str("createdAt") ?: o.str("created_at")),
            )
        }

    suspend fun send(id: String, text: String): Boolean =
        LibraryNet.send("POST", "projects/${esc(id)}/messages", buildJsonObject { put("text", text); put("workdir", JsonNull) }) is ApiResult.Ok

    suspend fun status(id: String): ProjectStatus? =
        ProjectStatus.from((LibraryNet.get("cowork/projects/${esc(id)}/status") as? ApiResult.Ok)?.value.obj.child("status"))

    suspend fun threads(id: String): List<ProjectThread> =
        (LibraryNet.get("threads?projectId=${esc(id)}&includeResolved=true") as? ApiResult.Ok)?.value.listUnder("threads")
            .filter { it.bool("incognito") != true }
            .mapNotNull { o ->
                ProjectThread(
                    o.str("id") ?: return@mapNotNull null, o.str("title") ?: "Untitled", o.str("group") ?: "idle",
                    o.str("statusLine"), o.str("currentSessionId"), parseTime(o.str("lastActivityAt")),
                )
            }.orEmpty()

    /** iOS memoryFacts: facts whose source observation came from one of [sessionIds]. Silent on failure. */
    suspend fun memoryFacts(sessionIds: Set<String>): List<String> {
        if (sessionIds.isEmpty()) return emptyList()
        val obs = (LibraryNet.get("memory/v2/observations?limit=500") as? ApiResult.Ok)?.value.listUnder("observations")
            .filter { it.str("session_id") in sessionIds }.mapNotNull { it.str("id") }.toSet()
        if (obs.isEmpty()) return emptyList()
        return (LibraryNet.get("memory/v2/facts?limit=500") as? ApiResult.Ok)?.value.listUnder("facts")
            .filter { it.str("source_observation_id") in obs }.mapNotNull { it.str("fact") }
    }
}

// MARK: - Conversation (iOS ProjectConversationView)

private val timeFmt = SimpleDateFormat("h:mm a", Locale.getDefault())
private val dayFmt = SimpleDateFormat("MMM d", Locale.getDefault())

private fun dayLabel(t: Long): String {
    val day = 86_400_000L
    val today = System.currentTimeMillis() / day
    return when (t / day) { today -> "Today"; today - 1 -> "Yesterday"; else -> dayFmt.format(t) }
}

@Composable
fun ProjectConversation(messages: List<ProjectMessage>, threads: List<ProjectThread>, onOpenThread: (ProjectThread) -> Unit) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().testTag("projectConversation"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        var lastDay: String? = null
        messages.forEachIndexed { i, m ->
            val d = dayLabel(m.at)
            if (d != lastDay) {
                lastDay = d
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.weight(1f).height(0.5.dp).background(c.borderSubtle))
                    Text(d, color = c.textTertiary, fontSize = 11.5.sp, modifier = Modifier.padding(horizontal = 8.dp))
                    Box(Modifier.weight(1f).height(0.5.dp).background(c.borderSubtle))
                }
            }
            if (m.role == "user") {
                Column(Modifier.fillMaxWidth().padding(start = 36.dp), horizontalAlignment = Alignment.End) {
                    Text(
                        m.text, color = c.textPrimary, fontSize = 13.5.sp,
                        modifier = Modifier.clip(RoundedCornerShape(topStart = 12.dp, topEnd = 12.dp, bottomStart = 12.dp, bottomEnd = 4.dp)).background(c.fillNeutral).padding(horizontal = 12.dp, vertical = 8.dp),
                    )
                    val next = messages.getOrNull(i + 1)
                    if (next != null && next.role != "user" && (next.kind == "fanout" || next.kind == "routed")) {
                        val sent = if (next.kind == "routed") "Sent to one thread" else if (next.threadCount == 1) "Sent to 1 thread" else "Sent to ${next.threadCount} threads"
                        Text("↩ $sent ${timeFmt.format(m.at)}", color = c.textTertiary, fontSize = 11.5.sp)
                    }
                }
            } else {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text("Al", color = c.textPrimary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                        Text(" · coordinator · ${timeFmt.format(m.at)}", color = c.textTertiary, fontSize = 12.sp)
                    }
                    if (m.kind == "synthesis") {
                        Text(m.text, color = c.textPrimary, fontSize = 13.5.sp, modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).border(1.dp, c.borderSubtle, RoundedCornerShape(12.dp)).padding(12.dp))
                    } else {
                        Text(m.text, color = if (m.kind == "blocked") AllternitStatic.StatusWarning else c.textPrimary, fontSize = 13.5.sp)
                    }
                    m.threadTitles.forEach { t -> ThreadChip(t, threads.firstOrNull { it.title == t }, onOpenThread) }
                }
            }
        }
        // A decision card for every thread that waits on you (iOS).
        threads.filter { it.group == "waiting" }.forEach { t ->
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(AllternitStatic.StatusWarning.copy(alpha = 0.12f)).clickable { onOpenThread(t) }.padding(12.dp)) {
                Text("Needs you", color = AllternitStatic.StatusWarning, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                Text(t.title, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                t.statusLine?.let { Text(it, color = c.textSecondary, fontSize = 12.sp) }
            }
        }
    }
}

@Composable
private fun ThreadChip(title: String, thread: ProjectThread?, onOpen: (ProjectThread) -> Unit) {
    val c = AllternitTheme.colors
    val dot = when (thread?.group) { "waiting" -> AllternitStatic.StatusWarning; "working" -> AllternitStatic.AccentChat; "resolved" -> AllternitStatic.StatusSuccess; else -> c.textTertiary }
    Row(
        Modifier.fillMaxWidth().heightIn(min = 44.dp).clip(RoundedCornerShape(12.dp)).border(1.dp, c.borderSubtle, RoundedCornerShape(12.dp))
            .clickable(enabled = thread != null) { thread?.let(onOpen) }.padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Box(Modifier.size(8.dp).clip(CircleShape).background(dot))
        Column {
            Text(title, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            thread?.statusLine?.let { Text(it, color = c.textSecondary, fontSize = 12.sp, maxLines = 1) }
        }
    }
}

/** The coordinator composer (iOS ComposerView on the project page). */
@Composable
fun CoordinatorComposer(sending: Boolean, onSend: (String) -> Unit) {
    val c = AllternitTheme.colors
    var text by remember { mutableStateOf("") }
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(22.dp)).background(c.fillNeutral).border(1.dp, c.borderSubtle, RoundedCornerShape(22.dp)).padding(start = 14.dp, end = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.weight(1f).padding(vertical = 12.dp)) {
            if (text.isEmpty()) Text("Ask your coordinator a question or start work…", color = c.textTertiary, fontSize = 15.sp)
            BasicTextField(text, { text = it }, textStyle = TextStyle(color = c.textPrimary, fontSize = 15.sp), cursorBrush = SolidColor(c.textPrimary), modifier = Modifier.fillMaxWidth().testTag("projectComposerInput"))
        }
        LibraryIconButton(DesignR.drawable.ph_arrow_up, "Send", "projectComposerSend", enabled = text.isNotBlank() && !sending) {
            val t = text.trim(); text = ""; onSend(t)
        }
    }
}

// MARK: - Delegate (iOS BotProjectPanes Delegate pane)

private data class DelegateTask(val id: String, val title: String, val estimateMin: Int?)
private data class DelegateBot(val id: String, val name: String, val running: Boolean, val queue: List<Pair<String, String>>)

@Composable
fun DelegatePane(coordinatorId: String, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var auto by remember { mutableStateOf(false) }
    var tasks by remember { mutableStateOf<List<DelegateTask>>(emptyList()) }
    var bots by remember { mutableStateOf<List<DelegateBot>>(emptyList()) }
    var loaded by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var reload by remember { mutableIntStateOf(0) }
    var newTask by remember { mutableStateOf("") }
    var assignFor by remember { mutableStateOf<DelegateTask?>(null) }
    val base = "projects/${LibraryNet.escape(coordinatorId)}/delegate"
    BackHandler(onBack = onClose)
    LaunchedEffect(reload) {
        while (true) {
            when (val r = LibraryNet.get(base)) {
                is ApiResult.Ok -> {
                    val o = r.value.obj
                    auto = o.bool("auto") ?: false
                    tasks = o.list("tasks").mapNotNull { t -> t.str("id")?.let { DelegateTask(it, t.str("title") ?: "", t.num("estimateMin")?.toInt()) } }
                    bots = o.list("bots").mapNotNull { b ->
                        b.str("id")?.let { DelegateBot(it, b.str("name") ?: "Bot", b.bool("running") ?: false, b.list("queue").map { q -> (q.str("title") ?: "") to (q.str("status") ?: "") }) }
                    }
                    error = o.str("autoError")
                }
                else -> error = "Couldn't load Delegate. ${r.friendly("load Delegate")}"
            }
            loaded = true
            kotlinx.coroutines.delay(10_000)
        }
    }
    fun act(fail: String, block: suspend () -> ApiResult<*>) {
        scope.launch { if (block() !is ApiResult.Ok) error = fail else reload += 1 }
    }
    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().navigationBarsPadding().testTag("projectDelegate")) {
        LibraryHeader("Delegate", subtitle = if (auto) "New tasks go to whoever is free first" else "Tap Assign to hand a task to a bot", onBack = onClose, trailing = {
            Text(
                if (auto) "Auto ✓" else "Auto", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold,
                modifier = Modifier.clip(CircleShape).background(c.fillNeutral).testTag("delegateAuto")
                    .clickable { val next = !auto; act("Couldn't change Auto. Try again.") { LibraryNet.send("PUT", "$base/auto", buildJsonObject { put("auto", next) }) } }
                    .padding(horizontal = 12.dp, vertical = 10.dp),
            )
        })
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp) }
            if (!loaded) LibrarySpinner("Loading Delegate")
            Text("To do (${tasks.size})", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            if (tasks.isEmpty()) InlineEmpty("Nothing here", art = EmptyArtKind.Sheets)
            tasks.forEach { t ->
                Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(start = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f).padding(vertical = 10.dp)) {
                        Text(t.title, color = c.textPrimary, fontSize = 14.sp)
                        Text(t.estimateMin?.let { "~$it min" } ?: "No estimate", color = c.textTertiary, fontSize = 12.sp)
                    }
                    TextButton(onClick = { assignFor = t }) { Text("Assign") }
                    LibraryIconButton(DesignR.drawable.ph_x, "Remove ${t.title}", iconSize = 14) {
                        act("Couldn't remove ${t.title}. Try again.") { LibraryNet.send("DELETE", "$base/tasks/${LibraryNet.escape(t.id)}") }
                    }
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.weight(1f)) { LibraryField("", newTask, "Add a task", "delegateNewTask") { newTask = it } }
                TextButton(enabled = newTask.isNotBlank(), onClick = {
                    val title = newTask.trim(); newTask = ""
                    act("Couldn't add that task. Try again.") { LibraryNet.send("POST", "$base/tasks", buildJsonObject { put("title", title); put("estimateMin", JsonNull) }) }
                }) { Text("Add") }
            }
            for ((label, wanted) in listOf("Working" to "working", "Needs you" to "waiting", "Done" to "resolved")) {
                val rows = bots.flatMap { b -> b.queue.filter { it.second == wanted }.map { b.name to it.first } }
                Text("$label (${rows.size})", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                if (rows.isEmpty()) InlineEmpty("Nothing here", art = EmptyArtKind.Sheets)
                rows.forEach { (bot, title) -> Text("$title · $bot", color = c.textSecondary, fontSize = 14.sp) }
            }
            Text("Bots", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            if (bots.isEmpty()) Text("Add a bot to this project to hand tasks to.", color = c.textTertiary, fontSize = 13.sp)
            bots.forEach { b -> Text("${b.name}${if (b.running) " · working" else ""}", color = c.textPrimary, fontSize = 14.sp) }
        }
    }
    assignFor?.let { t ->
        LibrarySheet("Assign to…", "delegateAssignSheet", onDismiss = { assignFor = null }) {
            Column(Modifier.padding(horizontal = 16.dp).padding(bottom = 20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (bots.isEmpty()) Text("Add a bot to this project to hand tasks to.", color = c.textSecondary, fontSize = 14.sp)
                bots.forEach { b ->
                    LibraryRowButton(b.name, DesignR.drawable.ph_cpu, "delegateAssign${b.id}") {
                        assignFor = null
                        act("Couldn't hand ${t.title} to ${b.name}. Try again.") { LibraryNet.send("POST", "$base/tasks/${LibraryNet.escape(t.id)}/assign", buildJsonObject { put("botId", b.id) }) }
                    }
                }
            }
        }
    }
}

// MARK: - Memory (iOS ProjectMemorySheet)

@Composable
fun ProjectMemorySheet(sessionIds: Set<String>, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    var facts by remember { mutableStateOf<List<String>?>(null) }
    LaunchedEffect(sessionIds) { facts = CoordinatorClient.memoryFacts(sessionIds) }
    LibrarySheet("Memory", "projectMemorySheet", onDismiss = onDismiss, trailing = LibrarySheetAction("Done", bold = true, onClick = onDismiss)) {
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 24.dp).heightIn(min = 200.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            when {
                facts == null -> LibrarySpinner("Loading memory")
                facts!!.isEmpty() -> {
                    FriendlyState(FriendlyStyle.Empty, "No memory yet", "Facts from this project's chats show here after a few turns.", art = EmptyArtKind.Memory, panel = true, icon = {})
                }
                else -> facts!!.forEach { Text(it, color = c.textPrimary, fontSize = 14.sp) }
            }
        }
    }
}

// MARK: - Add files (iOS: Photos (up to 5) or Files → POST uploads → POST cowork/projects/<id>/files)

suspend fun attachProjectFile(context: Context, projectId: String, uri: Uri, fallbackName: String): String? = withContext(Dispatchers.IO) {
    val resolver = context.contentResolver
    val name = resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { if (it.moveToFirst()) it.getString(0) else null } ?: fallbackName
    val mime = resolver.getType(uri) ?: "application/octet-stream"
    val bytes = runCatching { resolver.openInputStream(uri)?.use { it.readBytes() } }.getOrNull() ?: return@withContext "Couldn't read that file."
    val up = LibraryNet.send("POST", "uploads", buildJsonObject { put("name", name); put("mediaType", mime); put("dataBase64", Base64.encodeToString(bytes, Base64.NO_WRAP)) })
    val o = (up as? ApiResult.Ok)?.value.obj ?: return@withContext "Couldn't attach $name: ${up.friendly("attach it")}"
    val rec = LibraryNet.send("POST", "cowork/projects/${LibraryNet.escape(projectId)}/files", buildJsonObject {
        put("name", name); o.str("url")?.let { put("url", it) }; o.str("uploadId")?.let { put("upload_id", it) }; put("media_type", mime)
    })
    if (rec is ApiResult.Ok) null else "Couldn't attach $name: ${rec.friendly("attach it")}"
}

// MARK: - Folder picker (iOS ProjectFolderPickerSheet)

data class PickedFolder(val path: String, val name: String)

private data class FolderNode(val name: String, val absolute: String, val relative: String?)

/**
 * Browses the computer's folders like iOS: the root from `GET v1/path` (`directory`), each level
 * from `GET v1/file/tree?path=` (directories only), "Use this folder"; or a typed full path.
 */
@Composable
fun ProjectFolderPickerSheet(projectTitle: String, runtimeGet: RuntimeGet, onPick: (PickedFolder) -> Unit, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    var root by remember { mutableStateOf<String?>(null) }
    var rootError by remember { mutableStateOf<String?>(null) }
    var stack by remember { mutableStateOf<List<FolderNode>>(emptyList()) }
    var children by remember { mutableStateOf<List<FolderNode>?>(null) }
    var levelError by remember { mutableStateOf<String?>(null) }
    var retry by remember { mutableIntStateOf(0) }
    var typed by remember { mutableStateOf("") }
    var typedError by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(retry) {
        rootError = null
        runtimeGet("/v1/path").onSuccess { root = it.obj.str("directory") }.onFailure { rootError = "Connect a computer or Allternit Cloud to pick a folder." }
    }
    val here = stack.lastOrNull()
    LaunchedEffect(root, here, retry) {
        if (root == null) return@LaunchedEffect
        children = null; levelError = null
        val q = here?.relative?.let { "?path=" + Uri.encode(it) } ?: ""
        runtimeGet("/v1/file/tree$q").onSuccess { el ->
            children = el.listUnder("nodes", "files").filter { it.str("type") == "directory" }
                .map { FolderNode(it.str("name") ?: "", it.str("absolute") ?: it.str("path") ?: "", it.str("path")) }
        }.onFailure { levelError = "Couldn't load this folder. ${it.message ?: ""}".trim() }
    }
    fun finish(path: String) { val p = path.trimEnd('/').ifEmpty { "/" }; onPick(PickedFolder(p, p.substringAfterLast('/').ifEmpty { p })) }
    LibrarySheet("Folder for $projectTitle", "projectFolderPicker", onDismiss = onDismiss, leading = LibrarySheetAction("Cancel", onClick = onDismiss)) {
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 24.dp).heightIn(min = 320.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text("Path on your computer", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.Medium)
            LibraryField("", typed, "/Users/you/project", "projectFolderTyped") { typed = it; typedError = null }
            TextButton(enabled = typed.isNotBlank(), onClick = {
                val t = typed.trim()
                if (t.startsWith("/") || t.startsWith("~") || t.contains(":\\") || t.contains(":/")) finish(t) else typedError = "Enter the full path, starting with /."
            }) { Text("Use this path") }
            typedError?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp) }
            Text("On your computer", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.Medium)
            when {
                rootError != null -> Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(rootError!!, color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.weight(1f))
                    TextButton(onClick = { retry += 1 }) { Text("Retry") }
                }
                root == null -> Text("Connecting…", color = c.textSecondary, fontSize = 14.sp)
                else -> {
                    val currentAbs = here?.absolute ?: root!!
                    if (stack.isNotEmpty()) TextButton(onClick = { stack = stack.dropLast(1) }) { Text("‹ ${stack.dropLast(1).lastOrNull()?.name ?: root!!.substringAfterLast('/')}") }
                    LibraryRowButton("Use this folder", DesignR.drawable.ph_check, "projectFolderUse") { finish(currentAbs) }
                    Text(currentAbs, color = c.textTertiary, fontSize = 12.sp, fontFamily = FontFamily.Monospace)
                    when {
                        levelError != null -> Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(levelError!!, color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.weight(1f))
                            TextButton(onClick = { retry += 1 }) { Text("Retry") }
                        }
                        children == null -> LibrarySpinner("Loading folders")
                        children!!.isEmpty() -> Text("No folders inside.", color = c.textTertiary, fontSize = 14.sp)
                        else -> children!!.forEach { f ->
                            Row(
                                Modifier.fillMaxWidth().heightIn(min = 44.dp).clip(RoundedCornerShape(10.dp)).testTag("projectFolderRow${f.name}").clickable { stack = stack + f }.padding(horizontal = 8.dp),
                                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                            ) {
                                Text("📁", fontSize = 16.sp)
                                Text(f.name, color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
                                Icon(painterResource(DesignR.drawable.ph_arrow_right), null, tint = c.textTertiary, modifier = Modifier.size(14.dp))
                            }
                        }
                    }
                }
            }
            Spacer(Modifier.width(1.dp))
        }
    }
}

/** iOS stores the picked folder per device (`allternit-bot-project-device`), keyed by project id. */
object ProjectDeviceFolder {
    private fun prefs(context: Context) = context.getSharedPreferences("allternit-bot-project-device", Context.MODE_PRIVATE)
    fun get(context: Context, projectId: String): String? = prefs(context).getString(projectId, null)
    fun set(context: Context, projectId: String, path: String?) { prefs(context).edit().apply { if (path == null) remove(projectId) else putString(projectId, path) }.apply() }
}
