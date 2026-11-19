package com.allternit.feature.computer

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
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
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import com.allternit.feature.artifacts.long
import com.allternit.feature.chat.RuntimeCall
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject

/** One Files root (`GET /roots`), a folder item (`GET /roots/:id/list`) and an inbox task (`/inbox/tasks`). */
internal data class FilesRoot(val id: String, val kind: String, val name: String, val rootItemId: String)
internal data class FilesItem(val id: String, val name: String, val isFolder: Boolean, val size: Long, val modifiedAt: String?, val canDelete: Boolean = true)
internal data class FilesTask(val id: String, val name: String, val droppedBy: String, val createdAt: String?)
internal data class FilesFreshness(val state: String, val snapshotAt: String?, val servedBy: String?) {
    val asleep: Boolean get() = servedBy == "snapshot" || state == "asleep"
    /** iOS FilesComputerModel.stateLine. */
    val line: String get() = when (state) {
        "awake" -> "Awake"
        "waking" -> "Waking up"
        "offline" -> "Offline"
        else -> formatWhen(snapshotAt).let { if (it.isEmpty()) "Asleep. Saved list." else "Asleep. Saved list from $it." }
    }
}

/**
 * The Files drive of one computer over the relay (iOS FilesComputerModel + AllternitFilesKit's FilesClient, Files drive
 * protocol v2). Browsing never wakes the computer: an asleep computer answers from its snapshot. The :app module
 * has the full client (uploads, wake consent, the system picker); a feature module can't depend on :app, so this is
 * the read side plus inbox approval, through the same `/api/v1/files/v2` routes.
 */
internal class FilesApi(private val runtimeId: String) {
    sealed interface Out<out T> {
        data class Ok<T>(val value: T) : Out<T>
        data class Failed(val message: String, val gate: Boolean = false) : Out<Nothing>
    }

    private suspend fun run(method: String, path: String, body: String? = null): Out<JsonObject> = when (val r = AciRelay.call(runtimeId, method, "/api/v1/files/v2$path", body)) {
        is RuntimeCall.Ok -> Out.Ok(r.body.obj ?: JsonObject(emptyMap()))
        RuntimeCall.NoComputer -> Out.Failed("No computer is connected.", gate = true)
        else -> Out.Failed(r.failureText() ?: "Couldn’t load this. Try again.")
    }

    private fun JsonObject.freshness(): FilesFreshness? = (this["freshness"] as? JsonObject)?.let {
        FilesFreshness(it.str("computer_state") ?: "awake", it.str("snapshot_at"), it.str("served_by"))
    }

    suspend fun roots(): Out<Pair<List<FilesRoot>, FilesFreshness?>> = when (val r = run("GET", "/roots")) {
        is Out.Ok -> Out.Ok(r.value.list("roots").mapNotNull { o ->
            FilesRoot(o.str("id") ?: return@mapNotNull null, o.str("kind").orEmpty(), o.str("name").orEmpty(), o.str("root_item_id").orEmpty())
        }.sortedWith(compareBy({ rootOrder(it.kind) }, { it.name.lowercase() })) to r.value.freshness())
        is Out.Failed -> r
    }

    /** Every page of one folder, folders first then by name (iOS list(rootId:parentId:)). */
    suspend fun list(rootId: String, parentId: String): Out<Pair<List<FilesItem>, FilesFreshness?>> {
        val items = ArrayList<FilesItem>()
        var token: String? = null
        var fresh: FilesFreshness? = null
        do {
            val q = "/roots/${enc(rootId)}/list?parent_id=${enc(parentId)}&limit=500" + (token?.let { "&page_token=${enc(it)}" } ?: "")
            when (val r = run("GET", q)) {
                is Out.Ok -> {
                    r.value.list("items").forEach { o ->
                        items += FilesItem(o.str("id") ?: return@forEach, o.str("name").orEmpty(), o.str("kind") == "folder", o.long("size") ?: 0L, o.str("modified_at"), o.child("permissions").bool("delete") != false)
                    }
                    token = r.value.str("next_page_token")
                    fresh = r.value.freshness()
                }
                is Out.Failed -> return r
            }
        } while (token != null && items.size < 5_000)
        return Out.Ok(items.sortedWith(compareBy({ if (it.isFolder) 0 else 1 }, { it.name.lowercase() })) to fresh)
    }

    suspend fun pendingTasks(): Out<List<FilesTask>> = when (val r = run("GET", "/inbox/tasks?limit=100")) {
        is Out.Ok -> Out.Ok(r.value.list("tasks").filter { it.str("state") == "needs_approval" }.mapNotNull { o ->
            val by = o["dropped_by"] as? JsonObject
            FilesTask(o.str("task_id") ?: return@mapNotNull null, o.str("name") ?: "File", by?.str("display_name") ?: by?.str("kind") ?: "someone", o.str("created_at"))
        })
        is Out.Failed -> r
    }

    suspend fun approve(taskId: String): String? = (run("POST", "/inbox/tasks/${enc(taskId)}/approve", "{}") as? Out.Failed)?.message
    suspend fun reject(taskId: String): String? = (run("POST", "/inbox/tasks/${enc(taskId)}/reject", "{}") as? Out.Failed)?.message

    private fun enc(s: String) = java.net.URLEncoder.encode(s, "UTF-8").replace("+", "%20")

    companion object {
        /** iOS FilesFormat.order: where each root sits in the list. */
        fun rootOrder(kind: String) = when (kind) { "inbox" -> 0; "outbox" -> 1; "workspace" -> 2; "home" -> 3; else -> 4 }
    }
}

private sealed interface FilesNav {
    data object Roots : FilesNav
    data class Folder(val root: FilesRoot, val parentId: String, val title: String) : FilesNav
    data object Inbox : FilesNav
}

/**
 * ACIComputerDetailView's Files rows (design 4A): a "Files" section in the computer's detail, and the screens it pushes.
 * Roots, folders and Inbox requests. [onClose] returns to the detail.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun AciComputerFilesScreen(device: AciDevice, onClose: () -> Unit) {
    val api = remember(device.id) { FilesApi(device.id) }
    val stack = remember { mutableStateListOf<FilesNav>(FilesNav.Roots) }
    var pending by remember { mutableStateOf<List<FilesTask>>(emptyList()) }
    BackHandler { if (stack.size > 1) stack.removeAt(stack.lastIndex) else onClose() }
    LaunchedEffect(device.id) { (api.pendingTasks() as? FilesApi.Out.Ok)?.let { pending = it.value } }
    // Open, save, upload, scan, Share with… and the wake sheet (iOS FilesComputerModel + FilesSheets).
    val actions = rememberAciFileActions(device)
    Box(Modifier.fillMaxSize()) {
        when (val top = stack.last()) {
            FilesNav.Roots -> AciFilesRoots(device, api, actions, pending.size,
                onBack = onClose,
                onRoot = { stack += FilesNav.Folder(it, it.rootItemId, it.name) },
                onInbox = { stack += FilesNav.Inbox })
            is FilesNav.Folder -> AciFilesFolder(api, actions, top, onBack = { stack.removeAt(stack.lastIndex) },
                onFolder = { stack += FilesNav.Folder(top.root, it.id, it.name) })
            FilesNav.Inbox -> AciInboxRequests(api, onBack = { stack.removeAt(stack.lastIndex) }, onChanged = { pending = it })
        }
        Box(Modifier.align(Alignment.BottomCenter)) { AciFileSheets(actions) }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AciFilesRoots(device: AciDevice, api: FilesApi, actions: AciFileActions, pendingCount: Int, onBack: () -> Unit, onRoot: (FilesRoot) -> Unit, onInbox: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var roots by remember { mutableStateOf<List<FilesRoot>?>(null) }
    var fresh by remember { mutableStateOf<FilesFreshness?>(null) }
    var error by remember { mutableStateOf<FilesApi.Out.Failed?>(null) }
    var refreshing by remember { mutableStateOf(false) }
    suspend fun load() {
        when (val r = api.roots()) {
            is FilesApi.Out.Ok -> {
                roots = r.value.first; fresh = r.value.second; error = null
                actions.roots = r.value.first
                actions.asleep = r.value.second?.asleep == true
            }
            is FilesApi.Out.Failed -> error = r
        }
    }
    LaunchedEffect(device.id) { load() }
    AciFullPage {
        Column(Modifier.fillMaxSize()) {
            AciTopBar("Files", device.displayName, onBack)
            PullToRefreshBox(isRefreshing = refreshing, onRefresh = { scope.launch { refreshing = true; load(); refreshing = false } }, modifier = Modifier.fillMaxSize()) {
                Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp).testTag("aciFilesRoots")) {
                    fresh?.let { Text(it.line, color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(bottom = 8.dp).testTag("aciFilesState")) }
                    val e = error
                    val list = roots
                    when {
                        e != null && list == null -> AciStateCard(if (e.gate) "No computer is connected" else "Couldn’t load Files", e.message, "Retry", { scope.launch { load() } })
                        list == null -> AciStateCard("Loading Files…", "Reading this computer’s folders.")
                        list.isEmpty() -> AciStateCard("No folders yet", "This computer hasn’t shared any folders with Files.")
                        else -> {
                            AciListRow("Inbox requests", "Files dropped in Inbox that wait for you", if (pendingCount > 0) "$pendingCount" else null, "filesInboxRequestsRow", onInbox)
                            list.forEach { r -> AciListRow(r.name, r.kind.replaceFirstChar { it.uppercase() }, null, "filesRoot-${r.id}") { onRoot(r) } }
                            // iOS FilesDrive.Launch.scan: a scan lands in Scans (or Inbox when there is no Scans root).
                            AciListRow("Scan a document", "Saved as a PDF in Scans", null, "filesScanRow") { actions.scan(null, null) }
                        }
                    }
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AciFilesFolder(api: FilesApi, actions: AciFileActions, nav: FilesNav.Folder, onBack: () -> Unit, onFolder: (FilesItem) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var items by remember { mutableStateOf<List<FilesItem>?>(null) }
    var fresh by remember { mutableStateOf<FilesFreshness?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var refreshing by remember { mutableStateOf(false) }
    suspend fun load() {
        when (val r = api.list(nav.root.id, nav.parentId)) {
            is FilesApi.Out.Ok -> { items = r.value.first; fresh = r.value.second; error = null; actions.asleep = r.value.second?.asleep == true }
            is FilesApi.Out.Failed -> error = r.message
        }
    }
    LaunchedEffect(nav.parentId, actions.revision) { load() }
    AciFullPage {
        Column(Modifier.fillMaxSize()) {
            AciTopBar(nav.title, "Files", onBack, trailing = {
                Row(horizontalArrangement = Arrangement.spacedBy(14.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text("Share", color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.clickable {
                        actions.shareSheet = FilesShareTarget(nav.root.id, nav.parentId, nav.title, isFolder = true)
                    }.testTag("filesFolderShare"))
                    Text("+", color = c.textPrimary, fontSize = 24.sp, modifier = Modifier.clickable { actions.addSheet = nav.root to nav.parentId }.testTag("filesFolderAdd"))
                }
            })
            PullToRefreshBox(isRefreshing = refreshing, onRefresh = { scope.launch { refreshing = true; load(); refreshing = false } }, modifier = Modifier.fillMaxSize()) {
                Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp).testTag("aciFilesFolder")) {
                    fresh?.takeIf { it.asleep }?.let { Text(it.line, color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(bottom = 8.dp)) }
                    val list = items
                    when {
                        error != null && list == null -> AciStateCard("Couldn’t open this folder", error.orEmpty(), "Retry", { scope.launch { load() } })
                        list == null -> AciStateCard("Loading…", "Reading this folder.")
                        list.isEmpty() -> AciStateCard("This folder is empty", "Tap + to add a file or a scan.")
                        else -> list.forEach { item ->
                            AciListRow(
                                (if (item.isFolder) "▸ " else "") + item.name,
                                listOfNotNull(if (item.isFolder) null else formatBytes(item.size), formatWhen(item.modifiedAt).takeIf { it.isNotEmpty() }).joinToString(" · "),
                                null, "filesItem-${item.id}", if (item.isFolder) ({ onFolder(item) }) else ({ actions.itemSheet = nav.root to item }),
                            )
                        }
                    }
                }
            }
        }
    }
}

/** iOS FilesInboxRequestsView: files dropped in Inbox wait for the owner; Approve runs the fixed read-and-summarize pipeline. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AciInboxRequests(api: FilesApi, onBack: () -> Unit, onChanged: (List<FilesTask>) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var tasks by remember { mutableStateOf<List<FilesTask>?>(null) }
    var notice by remember { mutableStateOf<String?>(null) }
    var failed by remember { mutableStateOf<String?>(null) }
    var refreshing by remember { mutableStateOf(false) }
    suspend fun load() {
        when (val r = api.pendingTasks()) {
            is FilesApi.Out.Ok -> { tasks = r.value; failed = null; onChanged(r.value) }
            is FilesApi.Out.Failed -> failed = r.message
        }
    }
    LaunchedEffect(Unit) { load() }
    AciFullPage {
        Column(Modifier.fillMaxSize()) {
            AciTopBar("Inbox requests", "Files", onBack)
            PullToRefreshBox(isRefreshing = refreshing, onRefresh = { scope.launch { refreshing = true; load(); refreshing = false } }, modifier = Modifier.fillMaxSize()) {
                Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp).testTag("aciInboxRequests")) {
                    notice?.let { Text(it, color = c.textPrimary, fontSize = 13.sp, modifier = Modifier.padding(bottom = 8.dp).testTag("aciInboxNotice")) }
                    val list = tasks
                    when {
                        failed != null && list == null -> AciStateCard("Couldn’t load requests", failed.orEmpty(), "Retry", { scope.launch { load() } })
                        list == null -> AciStateCard("Loading…", "Checking Inbox.")
                        list.isEmpty() -> AciStateCard("Nothing waits for you", "Files dropped in Inbox show here until you approve or reject them.")
                        else -> {
                            list.forEach { task ->
                                Column(Modifier.fillMaxWidth().padding(vertical = 10.dp).testTag("aciInboxTask"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                                    Text(task.name, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                                    Text(listOfNotNull(task.droppedBy, formatWhen(task.createdAt).takeIf { it.isNotEmpty() }).joinToString(" · "), color = c.textSecondary, fontSize = 12.sp)
                                    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                                        Box(
                                            Modifier.weight(1f).heightIn(min = 44.dp).clip(RoundedCornerShape(10.dp)).background(c.textPrimary)
                                                .clickable { scope.launch { notice = api.approve(task.id) ?: "Your agent is reading ${task.name}."; load() } }.testTag("aciInboxApprove"),
                                            contentAlignment = Alignment.Center,
                                        ) { Text("Ask my agent to read it", color = c.bgPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold) }
                                        Box(
                                            Modifier.heightIn(min = 44.dp).clip(RoundedCornerShape(10.dp)).background(c.bgPanel).padding(horizontal = 22.dp)
                                                .clickable { scope.launch { notice = api.reject(task.id); load() } }.testTag("aciInboxReject"),
                                            contentAlignment = Alignment.Center,
                                        ) { Text("Reject", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold) }
                                    }
                                }
                                Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))
                            }
                            Text("Your agent reads only the file you approve, with no other tools, and saves its summary next to it.", color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.padding(top = 12.dp))
                        }
                    }
                }
            }
        }
    }
}

/**
 * ACIComputerDetailView: the computer's name and state, its desktop (opens the RFB screen) and its Files
 * (roots, folders, Inbox requests; design 4A, a row in the detail, no rail tab).
 */
@Composable
internal fun AciComputerDetail(device: AciDevice, onOpenDesktop: () -> Unit, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    var filesOpen by remember { mutableStateOf(false) }
    if (filesOpen) {
        AciComputerFilesScreen(device, onClose = { filesOpen = false })
        return
    }
    BackHandler { onBack() }
    AciFullPage {
        Column(Modifier.fillMaxSize()) {
            AciTopBar(device.displayName, "Computers", onBack)
            Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 12.dp).testTag("aciComputerDetail")) {
                Text(if (device.isOnline) "Online" else "Offline", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(bottom = 4.dp))
                Text(lastSeen(device.lastSeenAt), color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.padding(bottom = 12.dp))
                AciListRow("Desktop", "See and use this computer’s screen", null, "aciDetailDesktop", onOpenDesktop)
                AciListRow("Files", "Browse, open, save, upload and scan files", null, "aciDetailFiles") { filesOpen = true }
            }
        }
    }
}
