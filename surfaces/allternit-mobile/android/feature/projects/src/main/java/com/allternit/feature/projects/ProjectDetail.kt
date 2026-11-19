package com.allternit.feature.projects

import androidx.activity.compose.BackHandler
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

private enum class DetailTab(val label: String) { Overview("Overview"), Board("Board"), Details("Details") }

/**
 * The chat project detail (iOS ProjectDetailView): the StageBackdrop behind everything, the
 * breadcrumb, the hero (cover, serif title that opens the details sheet, description, pin and
 * Customize), Overview (the pad and the threads with New chat), Board (the pad's own view) and
 * Details (instructions and files). Customize opens the folder look sheet with the Project
 * section (Project settings, Edit pad, Edit details, Archive, Delete).
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
    var customizeOpen by remember { mutableStateOf(false) }
    var detailsOpen by remember { mutableStateOf(false) }
    var padEditOpen by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var files by remember { mutableStateOf<List<ProjectFile>?>(null) }
    var filesFailed by remember { mutableStateOf(false) }
    var filesReload by remember { mutableIntStateOf(0) }
    var instructions by remember(project.instructions) { mutableStateOf(project.instructions ?: "") }
    var savingInstructions by remember { mutableStateOf(false) }
    var starting by remember { mutableStateOf(false) }
    BackHandler(onBack = onClose)

    LaunchedEffect(project.id) { pad.load() }
    LaunchedEffect(project.id, filesReload) {
        if (project.mode != ProjectMode.Chat) { files = emptyList(); return@LaunchedEffect }
        when (val r = LibraryNet.get("cowork/projects/${LibraryNet.escape(project.nativeId)}/files")) {
            is ApiResult.Ok -> { files = r.value.listUnder("files").mapNotNull { o -> o.str("id")?.let { ProjectFile(it, o.str("name") ?: "File") } }; filesFailed = false }
            else -> filesFailed = true
        }
    }
    val threads = store.sessions.filter { it.projectId == project.nativeId }.sortedByDescending { it.updatedAt }
    val color = hexColor(look?.color)

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

    Box(Modifier.fillMaxSize().background(c.bgPrimary)) {
        nav.backdrop(padScope)
        Column(Modifier.fillMaxSize().statusBarsPadding().testTag("projectDetail")) {
            Breadcrumb(project.title, onClose)
            Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).navigationBarsPadding(), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                // Hero.
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    FolderCover(look, project.title, project.mode, width = 46.dp, flat = true)
                    Column(Modifier.weight(1f)) {
                        Text(
                            project.title, color = c.textPrimary, fontSize = 28.sp, fontFamily = FontFamily.Serif, maxLines = 2, overflow = TextOverflow.Ellipsis,
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
                PillRow(DetailTab.entries, tab, { it.label }, { "projectTab${it.name}" }) { tab = it }
                error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.clickable { error = null }) }
                when (tab) {
                    DetailTab.Overview, DetailTab.Board -> {
                        ProjectPadCard(
                            project.title, project.mode, pad, color,
                            PadBoardData(threads, files.orEmpty().map { it.name }, project.waiting),
                            onEdit = { padEditOpen = true },
                            onOpenThread = { nav.openSession(it, "chat") },
                        )
                        if (tab == DetailTab.Overview) {
                            if (threads.isEmpty()) {
                                Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.fillNeutral).padding(14.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                                    Text("Keep goals, decisions, and updates here with your project coordinator. Open a thread alongside this conversation for a specific task.", color = c.textSecondary, fontSize = 14.sp)
                                    LibraryRowButton(if (starting) "Starting…" else "Start a thread", DesignR.drawable.ph_plus, "projectStartThread", enabled = !starting, onClick = ::newChat)
                                }
                            }
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text("Threads (${threads.size})", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                                Text(
                                    "New chat", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold,
                                    modifier = Modifier.clip(CircleShape).background(c.fillNeutral).testTag("projectNewChat").clickable(enabled = !starting, onClick = ::newChat).padding(horizontal = 12.dp, vertical = 9.dp),
                                )
                            }
                            if (threads.isEmpty()) {
                                Text("No threads yet", color = c.textPrimary, fontSize = 14.sp)
                                Text("Start one from the composer.", color = c.textSecondary, fontSize = 13.sp)
                            }
                            threads.forEach { s ->
                                Row(
                                    Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(RoundedCornerShape(10.dp)).testTag("projectThread${s.id}")
                                        .clickable { nav.openSession(s.id, "chat") }.padding(horizontal = 8.dp),
                                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                                ) {
                                    Icon(painterResource(DesignR.drawable.ph_identification_card), null, tint = c.textSecondary, modifier = Modifier.size(18.dp))
                                    Text(s.name.ifEmpty { "Untitled" }, color = c.textPrimary, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                                    Text(relativeTime(s.updatedAt), color = c.textTertiary, fontSize = 12.sp)
                                }
                            }
                        }
                    }
                    DetailTab.Details -> {
                        Text("Instructions", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                        LibraryField("", instructions, "Tell every chat in this project how to work", "projectInstructions", singleLine = false, minHeight = 120) { instructions = it }
                        if (instructions != (project.instructions ?: "")) Row {
                            TextButton(onClick = { instructions = project.instructions ?: "" }) { Text("Cancel") }
                            TextButton(enabled = !savingInstructions, modifier = Modifier.testTag("projectInstructionsSave"), onClick = {
                                savingInstructions = true
                                scope.launch { store.saveInstructions(project, instructions).onFailure { error = it.message }; savingInstructions = false }
                            }) { Text(if (savingInstructions) "Saving…" else "Save", fontWeight = FontWeight.SemiBold) }
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
                    }
                }
                Box(Modifier.heightIn(min = 24.dp))
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
        var path by remember { mutableStateOf(project.directory ?: "") }
        var saving by remember { mutableStateOf(false) }
        LibrarySheet(
            "Choose a folder", "codeProjectFolderSheet", onDismiss = { folderOpen = false },
            leading = LibrarySheetAction("Cancel") { folderOpen = false },
            trailing = LibrarySheetAction(if (saving) "Saving…" else "Save", enabled = path.isNotBlank() && !saving, bold = true, testTag = "codeProjectFolderSave") {
                saving = true
                scope.launch {
                    store.setDirectory(project, path.trim()).onSuccess { folderOpen = false }.onFailure { error = it.message }
                    saving = false
                }
            },
        ) {
            Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                LibraryField("Folder on your computer", path, "~/code/my-project", "codeProjectFolderPath") { path = it }
                Text("Sessions in this project run in this folder on your computer.", color = c.textSecondary, fontSize = 13.sp)
            }
        }
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
