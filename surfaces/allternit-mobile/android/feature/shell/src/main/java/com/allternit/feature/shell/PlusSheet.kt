package com.allternit.feature.shell

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR

/**
 * The composer "+" sheet, "Add to chat" (iOS ComposerPlusSheet root page): ✕ and Photos in the
 * header, then three grouped lists. Group 1 adds something (Camera, Add files, GitHub link);
 * group 2 is how and where the chat runs (Permission settings, Computer, Project); group 3 is
 * tools (Web search and Research toggles, Connectors, Skills, Plugins). Rows whose screens are
 * not on this app yet open their section page. Test ids match iOS (`plusSheet*`).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun PlusSheet(
    showWatch: Boolean = false,
    onWatchEvent: () -> Unit = {},
    noComputer: Boolean,
    webSearch: Boolean,
    research: Boolean,
    onWebSearch: (Boolean) -> Unit,
    onResearch: (Boolean) -> Unit,
    onAttach: (ComposerAttachment) -> Unit,
    /** Types `/` in the composer to open the server skills list; null hides the row (no skills, flag off). */
    onServerSkills: (() -> Unit)? = null,
    onOpenSection: (RailRow) -> Unit,
    onDismiss: () -> Unit,
) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val view = LocalView.current
    val state = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var githubOpen by remember { mutableStateOf(false) }
    var githubUrl by remember { mutableStateOf("") }
    var sketchOpen by remember { mutableStateOf(false) }
    // Start anywhere: the Permission, Computer and Folder pages open inside this sheet (mockup E).
    var page by remember { mutableStateOf("menu") }
    val startBackend = com.allternit.feature.chat.PendingStart.backend
    val runtime = com.allternit.feature.bots.runtimes.ChatRuntime.runtimeId
    if (startBackend != null) com.allternit.feature.chat.LoadPendingOptions(startBackend, runtime)

    val photos = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(10)) { uris ->
        uris.forEach { onAttach(ComposerAttachment(displayName(context, it), it.toString(), "photo")) }
        if (uris.isNotEmpty()) onDismiss()
    }
    val files = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        uris.forEach { onAttach(ComposerAttachment(displayName(context, it), it.toString(), "file")) }
        if (uris.isNotEmpty()) onDismiss()
    }
    val camera = rememberLauncherForActivityResult(ActivityResultContracts.TakePicturePreview()) { bitmap ->
        if (bitmap != null) {
            // The preview bitmap is the photo: JPEG it now so the send can upload it (iOS AttachmentStore
            // stages camera shots as JPEG data).
            val jpeg = java.io.ByteArrayOutputStream().use { out ->
                bitmap.compress(android.graphics.Bitmap.CompressFormat.JPEG, 85, out)
                out.toByteArray()
            }
            onAttach(ComposerAttachment("Photo.jpg", null, "camera", bytes = jpeg, mediaType = "image/jpeg"))
            onDismiss()
        }
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = state,
        containerColor = c.bgPrimary,
        shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
        dragHandle = null,
    ) {
        // Polish 6.6: the content scales up from the plus (bottom-left) as the sheet rises; rows unchanged.
        val reduced = remember(context) { android.provider.Settings.Global.getFloat(context.contentResolver, android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }
        val grow = remember { androidx.compose.animation.core.Animatable(if (reduced) 1f else 0f) }
        androidx.compose.runtime.LaunchedEffect(Unit) { grow.animateTo(1f, androidx.compose.animation.core.tween(300, easing = androidx.compose.animation.core.FastOutSlowInEasing)) }
        Column(Modifier.fillMaxWidth().navigationBarsPadding().testTag("plusSheet").graphicsLayer {
            val sc = 0.92f + 0.08f * grow.value
            scaleX = sc; scaleY = sc
            transformOrigin = androidx.compose.ui.graphics.TransformOrigin(0.1f, 1f)
            alpha = 0.4f + 0.6f * grow.value
        }) {
            // Header: grabber, centered title, ✕ left, Photos right.
            Column(Modifier.fillMaxWidth().padding(start = 10.dp, end = 10.dp, top = 8.dp, bottom = 6.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                Box(Modifier.width(36.dp).height(5.dp).clip(CircleShape).background(c.borderStrong))
                Box(Modifier.fillMaxWidth().padding(top = 6.dp), contentAlignment = Alignment.Center) {
                    Text("Add to chat", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.semantics { heading() })
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        Box(
                            modifier = Modifier
                                .size(44.dp)
                                .testTag("plusSheetClose")
                                .clickable(role = Role.Button, onClickLabel = "Close", onClick = onDismiss)
                                .semantics { contentDescription = "Close" },
                            contentAlignment = Alignment.Center,
                        ) {
                            Box(Modifier.size(34.dp).clip(CircleShape).background(c.fillNeutral), contentAlignment = Alignment.Center) {
                                Icon(painterResource(com.allternit.core.design.R.drawable.ph_x), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(14.dp))
                            }
                        }
                        Spacer(Modifier.weight(1f))
                        Text(
                            "Photos",
                            color = c.textSecondary,
                            fontSize = 14.sp,
                            fontWeight = FontWeight.Medium,
                            modifier = Modifier
                                .heightIn(min = 44.dp)
                                .clip(RoundedCornerShape(8.dp))
                                .testTag("plusSheetPhotos")
                                .clickable(role = Role.Button) {
                                    Haptics.perform(view, Haptics.Kind.Tap)
                                    photos.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageAndVideo))
                                }
                                .padding(horizontal = 8.dp, vertical = 12.dp),
                        )
                    }
                }
            }
            Column(
                modifier = Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 16.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                if (page != "menu" && startBackend != null) {
                    StartSubPage(page, startBackend, runtime, onBack = { page = "menu" }, onOpenSection = onOpenSection)
                } else {
                PlusGroup {
                    PlusRow(com.allternit.core.design.R.drawable.ph_camera, "Camera", testTag = "plusSheetCamera") { camera.launch(null) }
                    PlusDivider()
                    PlusRow(com.allternit.core.design.R.drawable.ph_file_plus, "Add files", testTag = "plusSheetAddFiles") { files.launch(arrayOf("*/*")) }
                    PlusDivider()
                    PlusRow(com.allternit.core.design.R.drawable.ph_link, "GitHub link", active = githubOpen, testTag = "plusSheetGithub") { githubOpen = !githubOpen }
                    if (githubOpen) {
                        GithubField(githubUrl, onChange = { githubUrl = it }) {
                            val url = githubUrl.trim()
                            onAttach(ComposerAttachment(url.substringAfterLast('/').ifEmpty { url }, url, "github"))
                            githubUrl = ""
                            onDismiss()
                        }
                    }
                    PlusDivider()
                    // iOS: Camera · Add files · GitHub link · Sketch (full-screen canvas).
                    PlusRow(com.allternit.core.design.R.drawable.ph_scribble_loop, "Sketch", testTag = "plusSheetSketch") { sketchOpen = true }
                    if (com.allternit.feature.bar.BarFlag.isOn(context)) {
                        PlusDivider()
                        // The island bar (feature/bar): run one tool of a connected app (iOS plusSheetIslandBar).
                        PlusRow(com.allternit.core.design.R.drawable.ph_lightning, com.allternit.feature.bar.BarWording.entry, testTag = "plusSheetIslandBar") {
                            onDismiss()
                            com.allternit.feature.bar.BarDeepLink.open(context)
                        }
                    }
                }
                PlusGroup {
                    if (startBackend != null) {
                        val pending = com.allternit.feature.chat.PendingStart
                        PlusRow(com.allternit.core.design.R.drawable.ph_shield_check, "Permission", value = com.allternit.feature.chat.permissionValue(), chevron = true, testTag = "plusSheetPermission") { page = "perm" }
                        PlusDivider()
                        PlusRow(
                            com.allternit.core.design.R.drawable.ph_desktop, "Computer",
                            value = pending.shownDeviceName ?: if (noComputer) "No computer" else "Automatic",
                            chevron = true, testTag = "plusSheetComputer",
                        ) { page = "computer" }
                        if (pending.shownDeviceName != null) {
                            PlusDivider()
                            PlusRow(
                                com.allternit.core.design.R.drawable.ph_folder, "Folder",
                                value = pending.folder?.label ?: pending.options?.folders?.firstOrNull()?.name ?: "Pick a folder",
                                chevron = true, testTag = "plusSheetFolder",
                            ) { page = "folder" }
                        }
                    } else {
                        PlusRow(com.allternit.core.design.R.drawable.ph_shield_check, "Permission settings", chevron = true) {
                            onOpenSection(RailRow("permission-settings", "Permission settings", com.allternit.core.design.R.drawable.ph_shield_check))
                        }
                        PlusDivider()
                        PlusRow(com.allternit.core.design.R.drawable.ph_desktop, "Computer", value = if (noComputer) "No computer" else "Connected", chevron = true) {
                            onOpenSection(RailRow("computers", "Computer", com.allternit.core.design.R.drawable.ph_desktop))
                        }
                    }
                    PlusDivider()
                    PlusRow(com.allternit.core.design.R.drawable.ph_folder, "Project", value = "None", chevron = true) {
                        onOpenSection(RailRow("projects", "Projects", com.allternit.core.design.R.drawable.ph_folder))
                    }
                }
                PlusGroup {
                    PlusToggleRow(DesignR.drawable.ph_globe, "Web search", "Let the agent search the web for current information", webSearch, onWebSearch)
                    PlusDivider()
                    PlusToggleRow(DesignR.drawable.ph_magnifying_glass, "Research", "Deeper, multi-source research before answering", research, onResearch)
                    PlusDivider()
                    PlusRow(com.allternit.core.design.R.drawable.ph_lightning, "Connectors", chevron = true, testTag = "plusSheetConnectors") {
                        onOpenSection(RailRow("connectors", "Connectors", com.allternit.core.design.R.drawable.ph_lightning))
                    }
                    if (showWatch) {
                        PlusDivider()
                        PlusRow(DesignR.drawable.ph_broadcast, "Watch an app event", chevron = true, testTag = "plusSheetWatch") { onWatchEvent() }
                    }
                    PlusDivider()
                    PlusRow(com.allternit.core.design.R.drawable.ph_graduation_cap, "Skills", chevron = true) {
                        onOpenSection(RailRow("skills", "Skills", com.allternit.core.design.R.drawable.ph_graduation_cap))
                    }
                    PlusDivider()
                    if (onServerSkills != null) {
                        PlusRow(DesignR.drawable.ph_sparkle, "Server skills", chevron = true, testTag = "plusSheetServerSkills") { onServerSkills() }
                        PlusDivider()
                    }
                    PlusRow(DesignR.drawable.ph_puzzle_piece, "Plugins", chevron = true) {
                        onOpenSection(RailRow("plugins", "Plugins", DesignR.drawable.ph_puzzle_piece))
                    }
                    PlusDivider()
                    PlusRow(DesignR.drawable.ph_check_circle_fill, "Cowork tasks", chevron = true, testTag = "plusSheetCoworkTasks") {
                        onOpenSection(RailRow("cowork-tasks", "Cowork tasks", DesignR.drawable.ph_check_circle_fill))
                    }
                }
                }
            }
        }
    }
    if (sketchOpen) {
        SketchSheet(
            onAttach = { a ->
                onAttach(a)
                onDismiss()
            },
            onDismiss = { sketchOpen = false },
        )
    }
}

/** A Start anywhere page inside the + sheet: a back row with the page title, then the shared page body. */
@Composable
private fun StartSubPage(page: String, backend: com.allternit.feature.chat.ChatBackend, runtime: String, onBack: () -> Unit, onOpenSection: (RailRow) -> Unit) {
    val c = AllternitTheme.colors
    val title = when (page) { "perm" -> "Permission"; "folder" -> "Folder"; else -> "Computer" }
    Box(Modifier.fillMaxWidth().heightIn(min = 44.dp), contentAlignment = Alignment.Center) {
        Text(title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.semantics { heading() })
        Row(Modifier.fillMaxWidth()) {
            Box(
                Modifier.size(44.dp).testTag("plusSheetBack").clickable(role = Role.Button, onClickLabel = "Back", onClick = onBack).semantics { contentDescription = "Back" },
                contentAlignment = Alignment.Center,
            ) { Icon(painterResource(DesignR.drawable.ph_caret_left), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(18.dp)) }
        }
    }
    val pending = com.allternit.feature.chat.PendingStart
    when (page) {
        "perm" -> {
            com.allternit.feature.chat.PermissionPage(supported = pending.options?.modes?.get(com.allternit.feature.chat.contractRuntime(runtime))?.supported?.takeIf { it.isNotEmpty() }, onDone = onBack)
            com.allternit.feature.chat.PickGroup {
                com.allternit.feature.chat.PickRow("Permission settings", chevron = true) {
                    onOpenSection(RailRow("permission-settings", "Permission settings", DesignR.drawable.ph_shield_check))
                }
            }
        }
        "folder" -> com.allternit.feature.chat.FolderPage(backend, runtime, onDone = onBack)
        else -> com.allternit.feature.chat.DeviceList(backend, selectedId = pending.runtimeId, folder = pending.folder) { device, asleep, folder ->
            pending.pick(device, asleep)
            if (folder != null && device != null) pending.folder = folder
            // An asleep pick stays on the page: its plain line and the cloud offer show on send.
            if (!asleep) onBack()
        }
    }
}

/** One rounded group of rows on the neutral fill (iOS PlusGroup, 16dp corners). */
@Composable
private fun PlusGroup(content: @Composable ColumnScope.() -> Unit) {
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(AllternitTheme.colors.fillNeutral), content = content)
}

@Composable
private fun PlusDivider() {
    Box(Modifier.fillMaxWidth().padding(start = 48.dp).height(1.dp).background(AllternitTheme.colors.borderSubtle))
}

/** Icon, title, optional value and chevron; the whole row is the button (iOS PlusRow). */
@Composable
private fun PlusRow(icon: Int, title: String, value: String? = null, chevron: Boolean = false, active: Boolean = false, testTag: String? = null, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(if (active) c.borderSubtle.copy(alpha = c.borderSubtle.alpha * 0.6f) else Color.Transparent)
            .then(if (testTag != null) Modifier.testTag(testTag) else Modifier)
            .clickable(role = Role.Button) {
                Haptics.perform(view, Haptics.Kind.Tap)
                onClick()
            }
            .semantics(mergeDescendants = true) { contentDescription = value?.let { "$title, $it" } ?: title }
            .heightIn(min = 48.dp)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(painterResource(icon), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(22.dp))
        Text(title, color = c.textPrimary, fontSize = 15.sp, maxLines = 1, modifier = Modifier.weight(1f))
        if (value != null) Text(value, color = c.textSecondary, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        if (chevron) Icon(painterResource(com.allternit.core.design.R.drawable.ph_caret_right), contentDescription = null, tint = c.textSecondary.copy(alpha = 0.7f), modifier = Modifier.size(12.dp))
    }
}

/** A toggle row; the old subtitle is the TalkBack description (iOS PlusToggleRow). */
@Composable
private fun PlusToggleRow(icon: Int, title: String, hint: String, on: Boolean, onChange: (Boolean) -> Unit) {
    val c = AllternitTheme.colors
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(role = Role.Switch) { onChange(!on) }
            .semantics(mergeDescendants = true) { contentDescription = "$title. $hint" }
            .heightIn(min = 48.dp)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(painterResource(icon), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(22.dp))
        Text(title, color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
        Switch(
            checked = on,
            onCheckedChange = onChange,
            colors = SwitchDefaults.colors(checkedTrackColor = c.accentPrimary, checkedThumbColor = c.onAccent, uncheckedTrackColor = c.borderStrong, uncheckedThumbColor = c.bgPrimary, uncheckedBorderColor = Color.Transparent),
        )
    }
}

/** The GitHub URL field with Add (iOS githubField). */
@Composable
private fun GithubField(url: String, onChange: (String) -> Unit, onAdd: () -> Unit) {
    val c = AllternitTheme.colors
    val valid = url.trim().startsWith("https://github.com/") || url.trim().startsWith("https://raw.githubusercontent.com/")
    Column(Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, bottom = 10.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Box(
                Modifier
                    .weight(1f)
                    .clip(RoundedCornerShape(10.dp))
                    .background(c.bgPrimary)
                    .padding(horizontal = 10.dp, vertical = 10.dp),
            ) {
                if (url.isEmpty()) Text("https://github.com/…", color = c.textTertiary, fontSize = 14.sp)
                BasicTextField(
                    value = url,
                    onValueChange = onChange,
                    singleLine = true,
                    textStyle = TextStyle(color = c.textPrimary, fontSize = 14.sp, fontFamily = ShellSans),
                    cursorBrush = SolidColor(c.textPrimary),
                    modifier = Modifier.fillMaxWidth().testTag("plusSheetGithubField"),
                )
            }
            Text(
                "Add",
                color = if (valid) c.onAccent else c.textTertiary,
                fontSize = 14.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier
                    .clip(RoundedCornerShape(10.dp))
                    .background(if (valid) c.accentPrimary else c.borderSubtle)
                    .clickable(enabled = valid, role = Role.Button, onClick = onAdd)
                    .padding(horizontal = 14.dp, vertical = 10.dp),
            )
        }
        Text(
            "Paste a GitHub file URL to fetch its raw contents as an attachment.",
            color = c.textSecondary,
            fontSize = 12.sp,
            modifier = Modifier.padding(top = 6.dp),
        )
    }
}
