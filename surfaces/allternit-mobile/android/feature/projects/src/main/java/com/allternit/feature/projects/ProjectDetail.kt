package com.allternit.feature.projects

import com.allternit.core.design.AllternitSerif
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
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
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.ArtifactLiveWebView
import com.allternit.feature.artifacts.ArtifactWeb
import com.allternit.feature.artifacts.LibraryField
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.LibraryRowButton
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.LibrarySpinner
import com.allternit.feature.artifacts.LiveRoute
import com.allternit.feature.artifacts.PillRow
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.relativeTime
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.launch
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject

/** A file attached to a chat project (`GET cowork/projects/:id/files`). */
data class ProjectFile(val id: String, val name: String)

/** Creates a session tied to a project and returns its id (iOS: the composer's first send does this). */
internal suspend fun createProjectSession(name: String, surface: String, metadata: Map<String, String>): Result<String> {
    val r = LibraryNet.send("POST", "agent-sessions", buildJsonObject {
        put("name", name); put("origin_surface", surface); put("session_mode", "regular")
        putJsonObject("metadata") { metadata.forEach { (k, v) -> put(k, v) } }
    })
    val o = (r as? ApiResult.Ok)?.value.obj
    val id = o.str("id") ?: (o?.get("session")).obj.str("id")
    return if (id != null) Result.success(id) else Result.failure(IllegalStateException("Couldn't open chat: ${r.friendly("start a chat")}"))
}

/** "‹ Projects / <title>" (iOS breadcrumb, 44dp). */
@Composable
internal fun Breadcrumb(title: String, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    Row(Modifier.fillMaxWidth().heightIn(min = 44.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Row(
            Modifier.clip(RoundedCornerShape(10.dp)).testTag("projectBack").clickable(role = Role.Button, onClick = onBack)
                .semantics { contentDescription = "Back to Projects" }.padding(horizontal = 6.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(painterResource(DesignR.drawable.ph_caret_left), null, tint = c.textSecondary, modifier = Modifier.size(16.dp))
            Text("Projects", color = c.textSecondary, fontSize = 15.sp)
        }
        Text(" / ", color = c.textTertiary, fontSize = 15.sp)
        Text(title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
    }
}

private enum class DetailTab(val label: String) { Overview("Overview"), Delegate("Delegate"), Board("Board"), Details("Details") }

/**
 * The chat project detail (iOS ProjectDetailView): the StageBackdrop behind everything, the
 * breadcrumb, the hero (cover, serif title that opens the details sheet, description, pin and
 * Customize) and the tabs. Overview: the pad, the coordinator conversation, the threads (with New
 * chat) and the project's server threads, over the coordinator composer. Delegate and Board open
 * full-screen panes, as on iOS. Details: status, Memory, Add files / Add instructions,
 * Instructions, Files and Folder. The coordinator, threads and status refresh every 5 s.
 */
@Composable
fun ProjectDetailScreen(
    initial: LibraryProject,
    store: ProjectsLibraryStore,
    look: Appearance?,
    nav: ProjectsNav,
    onLookSaved: (Appearance) -> Unit,
    onClose: () -> Unit,
) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val project = store.projects.firstOrNull { it.id == initial.id } ?: initial
    val padScope = remember(project.id) { ProjectPadStore.scope(project.mode, project.nativeId) }
    val pad = remember(project.id) { ProjectPadStore(context, padScope, project.mode) }
    var tab by remember { mutableStateOf(DetailTab.Overview) }
    var pane by remember { mutableStateOf<DetailTab?>(null) }
    var customizeOpen by remember { mutableStateOf(false) }
    var detailsOpen by remember { mutableStateOf(false) }
    var padEditOpen by remember { mutableStateOf(false) }
    var memoryOpen by remember { mutableStateOf(false) }
    var statusOpen by remember { mutableStateOf(false) }
    var folderOpen by remember { mutableStateOf(false) }
    var addFilesOpen by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var files by remember { mutableStateOf<List<ProjectFile>?>(null) }
    var filesFailed by remember { mutableStateOf(false) }
    var filesReload by remember { mutableIntStateOf(0) }
    var instructions by remember(project.instructions) { mutableStateOf(project.instructions ?: "") }
    var editingInstructions by remember { mutableStateOf(false) }
    var savingInstructions by remember { mutableStateOf(false) }
    var starting by remember { mutableStateOf(false) }
    var coordinatorId by remember { mutableStateOf<String?>(null) }
    var messages by remember { mutableStateOf<List<ProjectMessage>>(emptyList()) }
    var serverThreads by remember { mutableStateOf<List<ProjectThread>>(emptyList()) }
    var status by remember { mutableStateOf<ProjectStatus?>(null) }
    var sending by remember { mutableStateOf(false) }
    var folder by remember(project.id) { mutableStateOf(ProjectDeviceFolder.get(context, project.nativeId)) }
    BackHandler(enabled = pane == null, onBack = onClose)

    LaunchedEffect(project.id) { pad.load() }
    LaunchedEffect(project.id) {
        coordinatorId = CoordinatorClient.existing(project)
        while (true) {
            coordinatorId?.let { id ->
                CoordinatorClient.messages(id)?.let { messages = it }
                serverThreads = CoordinatorClient.threads(id)
            }
            CoordinatorClient.status(coordinatorId ?: project.nativeId)?.let { status = it }
            kotlinx.coroutines.delay(5_000)
        }
    }
    LaunchedEffect(project.id, filesReload) {
        when (val r = LibraryNet.get("cowork/projects/${LibraryNet.escape(project.nativeId)}/files")) {
            is ApiResult.Ok -> { files = r.value.listUnder("files").mapNotNull { o -> o.str("id")?.let { ProjectFile(it, o.str("name") ?: "File") } }; filesFailed = false }
            else -> filesFailed = true
        }
    }
    val threads = store.sessions.filter { it.projectId == project.nativeId }.sortedByDescending { it.updatedAt }
    val color = hexColor(look?.color)
    val board = ProjectBoardData(
        status = status,
        work = threads.map { BoardWork(it.id, it.name.ifEmpty { "Untitled" }, "queued", it.updatedAt) } +
            serverThreads.map { BoardWork(it.id, it.title, ProjectBoardData.column(it.group), it.at) },
        made = files.orEmpty().map { it.name },
    )
    val padData = PadBoardData(threads, files.orEmpty().map { it.name }, board.needsYou, board, onOpenStatus = { statusOpen = true })
    val sessionIds = (threads.map { it.id } + serverThreads.mapNotNull { it.sessionId }).toSet()

    val pickPhotos = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(5)) { uris ->
        uris.forEachIndexed { i, u -> scope.launch { attachProjectFile(context, project.nativeId, u, "photo-${i + 1}.jpg")?.let { error = it }; filesReload += 1 } }
    }
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        uris.forEach { u -> scope.launch { attachProjectFile(context, project.nativeId, u, "file")?.let { error = it }; filesReload += 1 } }
    }

    fun newChat() {
        if (starting) return
        starting = true
        scope.launch {
            createProjectSession("New chat", "chat", mapOf("projectId" to project.nativeId, "project_id" to project.nativeId))
                .onSuccess { nav.openSession(it, "chat") }
                .onFailure { error = it.message }
            starting = false
        }
    }
    fun openPane(t: DetailTab) {
        scope.launch {
            if (coordinatorId == null) coordinatorId = CoordinatorClient.ensure(project, folder)
            pane = t
        }
    }

    when (pane) {
        DetailTab.Delegate -> {
            val id = coordinatorId
            if (id != null) { DelegatePane(id, onClose = { pane = null }); return }
            pane = null
        }
        DetailTab.Board -> {
            BackHandler { pane = null }
            Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().navigationBarsPadding().testTag("projectBoard")) {
                com.allternit.feature.artifacts.LibraryHeader("Board", subtitle = project.title, onBack = { pane = null })
                Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(16.dp)) {
                    ProjectPadCard(project.title, project.mode, pad, color, padData, onEdit = { padEditOpen = true }, onOpenThread = { nav.openSession(it, "chat") })
                }
            }
            if (padEditOpen) PadEditSheet(pad, onDismiss = { padEditOpen = false })
            return
        }
        else -> Unit
    }

    Box(Modifier.fillMaxSize().background(c.bgPrimary)) {
        nav.backdrop(padScope)
        Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().imePadding().testTag("projectDetail")) {
            Breadcrumb(project.title, onClose)
            Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    FolderCover(look, project.title, project.mode, width = 46.dp, flat = true)
                    Column(Modifier.weight(1f)) {
                        Text(
                            project.title, color = c.textPrimary, fontSize = 28.sp, fontFamily = AllternitSerif, maxLines = 2, overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.testTag("projectTitle").clickable(onClickLabel = "Rename project") { detailsOpen = true },
                        )
                        project.description?.takeIf { it.isNotBlank() }?.let { Text(it, color = c.textSecondary, fontSize = 14.sp) }
                    }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    LibraryIconButton(DesignR.drawable.ph_seal_check, if (project.isFavorite) "Unpin project" else "Pin project", "projectPin", tint = if (project.isFavorite) androidx.compose.ui.graphics.Color(0xFFEAB308) else null) {
                        scope.launch { store.setFavorite(project, !project.isFavorite) }
                    }
                    Text(
                        "Customize", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium,
                        modifier = Modifier.clip(CircleShape).background(c.fillNeutral).testTag("projectCustomize").clickable { customizeOpen = true }.padding(horizontal = 14.dp, vertical = 12.dp),
                    )
                }
                PillRow(DetailTab.entries, tab, { it.label }, { "projectTab${it.name}" }) { t ->
                    if (t == DetailTab.Delegate || t == DetailTab.Board) openPane(t) else tab = t
                }
                error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.clickable { error = null }) }
                if (tab == DetailTab.Overview) {
                    ProjectPadCard(project.title, project.mode, pad, color, padData, onEdit = { padEditOpen = true }, onOpenThread = { nav.openSession(it, "chat") })
                    if (threads.isEmpty() && messages.isEmpty()) {
                        Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.fillNeutral).padding(14.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                            Text("Keep goals, decisions, and updates here with your project coordinator. Open a thread alongside this conversation for a specific task.", color = c.textSecondary, fontSize = 14.sp)
                            LibraryRowButton(if (starting) "Starting…" else "Start a thread", DesignR.drawable.ph_plus, "projectStartThread", enabled = !starting, onClick = ::newChat)
                        }
                    }
                    if (messages.isNotEmpty()) ProjectConversation(messages, serverThreads) { t -> t.sessionId?.let { nav.openSession(it, "chat") } }
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text("Threads (${threads.size + serverThreads.size})", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                        Text(
                            "New chat", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold,
                            modifier = Modifier.clip(CircleShape).background(c.fillNeutral).testTag("projectNewChat").clickable(enabled = !starting, onClick = ::newChat).padding(horizontal = 12.dp, vertical = 9.dp),
                        )
                    }
                    if (threads.isEmpty() && serverThreads.isEmpty()) {
                        Text("No threads yet", color = c.textPrimary, fontSize = 14.sp)
                        Text("Start one from the composer.", color = c.textSecondary, fontSize = 13.sp)
                    }
                    threads.forEach { s -> ThreadRow(s.name.ifEmpty { "Untitled" }, relativeTime(s.updatedAt), "projectThread${s.id}") { nav.openSession(s.id, "chat") } }
                    serverThreads.forEach { t -> ThreadRow(t.title, t.statusLine ?: relativeTime(t.at), "projectServerThread${t.id}") { t.sessionId?.let { nav.openSession(it, "chat") } } }
                } else {
                    // Details.
                    val st = status
                    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.fillNeutral).testTag("projectStatusCard").clickable { statusOpen = true }.padding(14.dp)) {
                        Text("Status · ${st?.stateLabel ?: "On track"}", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                        Text(st?.nextAction ?: st?.sections?.get("status") ?: "Goals, decisions and blockers live on the status page.", color = c.textSecondary, fontSize = 13.sp)
                    }
                    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.fillNeutral).padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text("Memory", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                            TextButton(onClick = { memoryOpen = true }, modifier = Modifier.testTag("projectViewMemory")) { Text("View memory") }
                        }
                        Text("Project memory will show here after a few chats.", color = c.textSecondary, fontSize = 13.sp)
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        Tile76("Add files", DesignR.drawable.ph_upload_simple, "projectAddFiles", Modifier.weight(1f)) { addFilesOpen = true }
                        Tile76("Add instructions", DesignR.drawable.ph_sliders_horizontal, "projectAddInstructions", Modifier.weight(1f)) { editingInstructions = true }
                    }
                    Text("Instructions", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                    if (editingInstructions || instructions != (project.instructions ?: "")) {
                        LibraryField("", instructions, "Tell every chat in this project how to work", "projectInstructions", singleLine = false, minHeight = 120) { instructions = it }
                        Row {
                            TextButton(onClick = { instructions = project.instructions ?: ""; editingInstructions = false }) { Text("Cancel") }
                            TextButton(enabled = !savingInstructions, modifier = Modifier.testTag("projectInstructionsSave"), onClick = {
                                savingInstructions = true
                                scope.launch { store.saveInstructions(project, instructions).onSuccess { editingInstructions = false }.onFailure { error = it.message }; savingInstructions = false }
                            }) { Text(if (savingInstructions) "Saving…" else "Save", fontWeight = FontWeight.SemiBold) }
                        }
                    } else {
                        Text(project.instructions?.takeIf { it.isNotBlank() } ?: "No instructions yet.", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.clickable { editingInstructions = true })
                    }
                    Text("Files", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                    when {
                        filesFailed -> Row(verticalAlignment = Alignment.CenterVertically) {
                            Text("Couldn't load files", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.weight(1f))
                            TextButton(onClick = { filesReload += 1 }) { Text("Retry") }
                        }
                        files == null -> LibrarySpinner("Loading files")
                        files!!.isEmpty() -> Column {
                            Text("No files yet", color = c.textPrimary, fontSize = 14.sp)
                            Text("Add files to give every chat in this project the same context.", color = c.textSecondary, fontSize = 13.sp)
                        }
                        else -> files!!.forEach { f ->
                            Row(Modifier.fillMaxWidth().heightIn(min = 44.dp), verticalAlignment = Alignment.CenterVertically) {
                                Text(f.name, color = c.textPrimary, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                                LibraryIconButton(DesignR.drawable.ph_x, "Remove ${f.name}", iconSize = 14) {
                                    scope.launch {
                                        val r = LibraryNet.send("DELETE", "cowork/projects/${LibraryNet.escape(project.nativeId)}/files/${LibraryNet.escape(f.id)}")
                                        if (r is ApiResult.Ok) filesReload += 1 else error = "Couldn't remove ${f.name}: ${r.friendly("remove it")}"
                                    }
                                }
                            }
                        }
                    }
                    // iOS ProjectFolderCard.
                    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.fillNeutral).padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Row { Text("Folder", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f)); Text("Only you", color = c.textTertiary, fontSize = 12.sp) }
                        Text(folder ?: "Add a folder on your computer for Allternit to work in.", color = if (folder != null) c.textPrimary else c.textSecondary, fontSize = 13.sp, fontFamily = if (folder != null) FontFamily.Monospace else FontFamily.Default)
                        Row {
                            TextButton(onClick = { folderOpen = true }, modifier = Modifier.testTag("projectChooseFolder")) { Text("Choose folder") }
                            if (folder != null) TextButton(onClick = {
                                folder = null; ProjectDeviceFolder.set(context, project.nativeId, null)
                                scope.launch { if (coordinatorId != null && !CoordinatorClient.setDirectory(project, null)) error = "Couldn't save the folder: try again." }
                            }) { Text("Remove folder", color = AllternitStatic.StatusError) }
                        }
                    }
                }
                Box(Modifier.heightIn(min = 12.dp))
            }
            if (tab == DetailTab.Overview) {
                Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    CoordinatorComposer(sending) { text ->
                        sending = true
                        scope.launch {
                            val id = coordinatorId ?: CoordinatorClient.ensure(project, folder).also { coordinatorId = it }
                            if (id == null || !CoordinatorClient.send(id, text)) error = "Your coordinator didn't get that. Try sending it again."
                            else CoordinatorClient.messages(id)?.let { messages = it }
                            sending = false
                        }
                    }
                }
            }
        }
    }

    if (customizeOpen) {
        FolderLookSheet(project, look, onSaved = onLookSaved, onDismiss = { customizeOpen = false }) {
            LibraryRowButton("Project settings", DesignR.drawable.ph_sliders_horizontal, "projectLookSettings") { customizeOpen = false; detailsOpen = true }
            LibraryRowButton("Edit pad", DesignR.drawable.ph_sliders_horizontal, "projectLookEditPad") { customizeOpen = false; padEditOpen = true }
            LibraryRowButton("Edit details", DesignR.drawable.ph_sliders_horizontal, "projectLookEditDetails") { customizeOpen = false; detailsOpen = true }
            LibraryRowButton(if (project.isArchived) "Unarchive" else "Archive", DesignR.drawable.ph_floppy_disk, "projectLookArchive") {
                customizeOpen = false
                scope.launch { store.setArchived(project, !project.isArchived) }
            }
            LibraryRowButton("Delete", DesignR.drawable.ph_trash, "projectLookDelete", destructive = true) { customizeOpen = false; confirmDelete = true }
        }
    }
    if (detailsOpen) ProjectDetailsSheet(project, look, store, onDismiss = { detailsOpen = false })
    if (padEditOpen) PadEditSheet(pad, onDismiss = { padEditOpen = false })
    if (memoryOpen) ProjectMemorySheet(sessionIds, onDismiss = { memoryOpen = false })
    if (statusOpen) ProjectStatusSheet(status, onDismiss = { statusOpen = false })
    if (folderOpen) ProjectFolderPickerSheet(project.title, nav.runtimeGet, onPick = { picked ->
        folderOpen = false
        folder = picked.path
        ProjectDeviceFolder.set(context, project.nativeId, picked.path)
        scope.launch { if (coordinatorId != null && !CoordinatorClient.setDirectory(project, picked.path)) error = "Couldn't save the folder: try again." }
    }, onDismiss = { folderOpen = false })
    if (addFilesOpen) {
        LibrarySheet("Add files", "projectAddFilesSheet", onDismiss = { addFilesOpen = false }, leading = LibrarySheetAction("Cancel") { addFilesOpen = false }) {
            Column(Modifier.padding(horizontal = 16.dp).padding(bottom = 20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                LibraryRowButton("Photos", DesignR.drawable.ph_upload_simple, "projectAddPhotos") {
                    addFilesOpen = false
                    pickPhotos.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                }
                LibraryRowButton("Files", DesignR.drawable.ph_upload_simple, "projectAddFilesFiles") { addFilesOpen = false; pickFiles.launch(arrayOf("*/*")) }
            }
        }
    }
    if (confirmDelete) AlertDialog(
        onDismissRequest = { confirmDelete = false },
        title = { Text("Delete this project?") },
        text = { Text("Its chats are kept, but they're no longer grouped under a project.") },
        confirmButton = {
            TextButton(onClick = {
                confirmDelete = false
                scope.launch { store.delete(project).onSuccess { onClose() }.onFailure { error = "Couldn't delete the project: ${it.message}" } }
            }, modifier = Modifier.testTag("projectDeleteConfirm")) { Text("Delete", color = AllternitStatic.StatusError) }
        },
        dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text("Cancel") } },
    )
}

@Composable
private fun ThreadRow(title: String, meta: String, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(RoundedCornerShape(10.dp)).testTag(tag).clickable(onClick = onClick).padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(painterResource(DesignR.drawable.ph_identification_card), null, tint = c.textSecondary, modifier = Modifier.size(18.dp))
        Text(title, color = c.textPrimary, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        Text(meta, color = c.textTertiary, fontSize = 12.sp, maxLines = 1)
    }
}

/** The 76dp Add files / Add instructions tiles. */
@Composable
private fun Tile76(title: String, icon: Int, tag: String, modifier: Modifier, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Column(
        modifier.heightIn(min = 76.dp).clip(RoundedCornerShape(14.dp)).background(c.fillNeutral).testTag(tag).clickable(onClick = onClick).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Icon(painterResource(icon), null, tint = c.textPrimary, modifier = Modifier.size(20.dp))
        Text(title, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
    }
}

/** iOS status sheet: the state, next action and the status page's sections in order. */
@Composable
private fun ProjectStatusSheet(status: ProjectStatus?, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    LibrarySheet("Status", "projectStatusSheet", onDismiss = onDismiss, trailing = LibrarySheetAction("Done", bold = true, onClick = onDismiss)) {
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 24.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text(status?.stateLabel ?: "On track", color = c.textPrimary, fontSize = 20.sp, fontFamily = AllternitSerif)
            status?.nextAction?.let { Text("Next: $it", color = c.textSecondary, fontSize = 14.sp) }
            val labels = mapOf("goal" to "Goal", "status" to "Status", "next_actions" to "Next actions", "owners" to "Owners", "deadlines" to "Deadlines", "decisions" to "Decisions", "blockers" to "Blockers", "outcome" to "Outcome", "lessons" to "Lessons")
            ProjectStatus.sectionOrder.forEach { k ->
                status?.sections?.get(k)?.takeIf { it.isNotBlank() }?.let { text ->
                    Text(labels[k] ?: k, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                    Text(text, color = c.textSecondary, fontSize = 14.sp)
                }
            }
            if (status == null || status.sections.isEmpty()) Text("Nothing on the status page yet.", color = c.textTertiary, fontSize = 13.sp)
        }
    }
}

// MARK: - Code project (iOS CodeProjectDetailView)

@Composable
fun CodeProjectDetailScreen(initial: LibraryProject, store: ProjectsLibraryStore, asksForFolder: Boolean, nav: ProjectsNav, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val project = store.projects.firstOrNull { it.id == initial.id } ?: initial
    var folderOpen by remember { mutableStateOf(asksForFolder && project.directory == null) }
    var error by remember { mutableStateOf<String?>(null) }
    var starting by remember { mutableStateOf(false) }
    BackHandler(onBack = onClose)
    val sessions = store.sessions.filter { it.workspaceId != null && (it.workspaceId == project.workspaceRowId || it.workspaceId == project.nativeId) }.sortedByDescending { it.updatedAt }
    val scopeId = ProjectPadStore.scope(project.mode, project.nativeId)

    Box(Modifier.fillMaxSize().background(c.bgPrimary)) {
        nav.backdrop(scopeId)
        Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("codeProjectDetail")) {
            Breadcrumb(project.title, onClose)
            Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(project.title, color = c.textPrimary, fontSize = 24.sp, fontWeight = FontWeight.SemiBold)
                project.description?.takeIf { it.isNotBlank() }?.let { Text(it, color = c.textSecondary, fontSize = 14.sp) }
                val dir = project.directory
                Row(
                    Modifier.fillMaxWidth().heightIn(min = 44.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).testTag("codeProjectFolder")
                        .clickable { folderOpen = true }.semantics { contentDescription = if (dir != null) "Folder $dir. Change" else "Choose a folder" }.padding(horizontal = 12.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Icon(painterResource(DesignR.drawable.ph_terminal), null, tint = c.textSecondary, modifier = Modifier.size(16.dp))
                    Text(dir ?: "Choose a folder", color = if (dir != null) c.textPrimary else c.textSecondary, fontSize = 13.sp, fontFamily = if (dir != null) FontFamily.Monospace else FontFamily.Default, maxLines = 1, overflow = TextOverflow.MiddleEllipsis)
                }
                error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp) }
                LibraryRowButton(if (starting) "Starting…" else "New session", DesignR.drawable.ph_plus, "codeProjectNewSession", enabled = !starting) {
                    starting = true
                    scope.launch {
                        val meta = buildMap { put("workspaceId", project.workspaceRowId ?: project.nativeId); put("projectMode", "code"); project.directory?.let { put("directory", it) } }
                        createProjectSession("New session", "code", meta)
                            .onSuccess { nav.openSession(it, "code") }
                            .onFailure { error = "Couldn't add the session to this project. ${it.message ?: ""}".trim() }
                        starting = false
                    }
                }
                Text("Sessions", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                if (store.loading && sessions.isEmpty()) LibrarySpinner("Loading sessions")
                else if (sessions.isEmpty()) Text("No sessions yet. Start one to work on this project.", color = c.textSecondary, fontSize = 14.sp)
                sessions.forEach { s ->
                    Row(
                        Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(RoundedCornerShape(10.dp)).testTag("codeProjectSession${s.id}").clickable { nav.openSession(s.id, "code") }.padding(horizontal = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(s.name.ifEmpty { "Untitled session" }, color = c.textPrimary, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                        Text(relativeTime(s.updatedAt), color = c.textTertiary, fontSize = 12.sp)
                    }
                }
            }
        }
    }
    if (folderOpen) {
        // iOS ProjectFolderPickerSheet: browse the computer's folders (or type a full path).
        ProjectFolderPickerSheet(project.title, nav.runtimeGet, onPick = { picked ->
            folderOpen = false
            scope.launch { store.setDirectory(project, picked.path).onFailure { error = it.message } }
        }, onDismiss = { folderOpen = false })
    }
}

// MARK: - Design project (iOS DesignProjectCanvasView)

@Composable
fun DesignProjectCanvasScreen(project: LibraryProject, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val dark = c.bgPrimary.luminance() < 0.5f
    BackHandler(onBack = onClose)
    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().testTag("designProjectCanvas")) {
        Row(Modifier.fillMaxWidth().heightIn(min = 52.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            TextButton(onClick = onClose, modifier = Modifier.testTag("designCanvasClose")) { Text("Close", color = c.textPrimary) }
            Text(project.title, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f).padding(end = 60.dp))
        }
        // The canvas saves itself in the web app; the editor's own close message also closes it.
        ArtifactLiveWebView(
            LiveRoute(ArtifactWeb.designUrl(project.nativeId, project.title, fromProject = true, dark = dark), "design", editor = true),
            Modifier.weight(1f).fillMaxWidth(),
            onClose = onClose,
        )
    }
}
