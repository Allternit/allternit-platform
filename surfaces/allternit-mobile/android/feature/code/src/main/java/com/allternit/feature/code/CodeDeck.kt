package com.allternit.feature.code

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.chat.RelayDevice
import com.allternit.core.design.R as DesignR

/**
 * The Code launch screen's deck row (iOS CodeDeckRow), left to right: environment, folder, branch
 * glyph (only with a branch), worktree toggle (only on main/master), permission mode in the accent
 * colour, a spacer, the five-block usage meter and the Code / Terminal view menu.
 */

/** iOS `CodePermissionMode.storageKey` and `CodeWorktree.storageKey`, kept in the shell's prefs file. */
private const val PREFS = "allternit_shell"
private const val KEY_PERMISSION = "allternit.code.permission-mode"
private const val KEY_WORKTREE = "allternit-code-pending-worktree"

/** The launch's permission pick and Worktree pick, applied to the next Code thread. */
object CodeDeckPrefs {
    fun permission(context: Context): CodePermissionMode {
        val raw = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY_PERMISSION, null)
        return CodePermissionMode.entries.firstOrNull { it.raw == raw } ?: CodePermissionMode.Manual
    }

    fun setPermission(context: Context, mode: CodePermissionMode) {
        context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(KEY_PERMISSION, mode.raw).apply()
    }

    fun worktree(context: Context): Boolean =
        context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(KEY_WORKTREE, false)

    fun setWorktree(context: Context, on: Boolean) {
        context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(KEY_WORKTREE, on).apply()
    }

    /**
     * iOS CodeModeView.sessionChanged for a new thread: the picked permission mode (unless Manual)
     * and the Worktree pick (`metadata.isolation = "worktree"`, then cleared). Returns the failure
     * to show, or null.
     */
    suspend fun applyToNewSession(context: Context, sessionId: String): String? {
        var failure: String? = null
        val mode = permission(context)
        if (mode != CodePermissionMode.Manual) failure = CodeApi.applyPermission(sessionId, mode)
        if (worktree(context)) {
            setWorktree(context, false)
            CodeApi.applyIsolation(sessionId)?.let { failure = failure ?: it }
        }
        return failure
    }
}

/** SF Symbols for the web's HandPointing / HandPalm / Scroll / ShieldCheck, as Phosphor. */
internal val CodePermissionMode.icon: Int
    get() = when (this) {
        CodePermissionMode.Manual -> DesignR.drawable.ph_hand_pointing
        CodePermissionMode.Auto -> DesignR.drawable.ph_hand
        CodePermissionMode.Plan -> DesignR.drawable.ph_scroll
        CodePermissionMode.Safe -> DesignR.drawable.ph_shield_check
    }

private fun isDefaultBranch(branch: String?): Boolean = branch?.lowercase().let { it == "main" || it == "master" }

private fun RelayDevice.isCloud(): Boolean = runtimeType.lowercase().let { it == "vps" || it == "hosted" }

private enum class DeckSheet { Environment, Folder, Permission }

/**
 * The deck row under the Code prompt. [planFraction] is the plan window used (0..1, from
 * `GET /api/v1/me/usage`; null until it loads). A tap on the meter opens [onOpenUsage]; the view
 * menu's Terminal opens [onOpenTerminal]; Pair or configure runtime opens [onConnect].
 */
@Composable
fun CodeDeckRow(
    host: CodeHost,
    planFraction: Double?,
    onOpenTerminal: () -> Unit,
    onOpenUsage: () -> Unit,
    onConnect: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    var sheet by remember { mutableStateOf<DeckSheet?>(null) }
    var folder by remember { mutableStateOf<CodeFolder?>(null) }
    var devices by remember { mutableStateOf<List<RelayDevice>?>(null) }
    var permission by remember { mutableStateOf(CodePermissionMode.Manual) }
    var worktree by remember { mutableStateOf(false) }
    var tick by remember { mutableStateOf(0) }
    LaunchedEffect(Unit) {
        permission = CodeDeckPrefs.permission(context)
        worktree = CodeDeckPrefs.worktree(context)
    }
    LaunchedEffect(tick) {
        folder = (CodeApi.folder(host.backend) as? CodeResult.Ok)?.value
        devices = host.backend.relay.listDevices()
    }
    val active = devices?.firstOrNull { it.id == host.backend.relay.activeRuntimeId }
    val cloud = active?.isCloud() == true
    // iOS cloudStatus: "Connecting" until the cloud runtime answers.
    val connecting = cloud && active?.isOnline != true
    val envLabel = if (cloud) (active?.name?.ifEmpty { null } ?: "Cloud") else "This computer"
    // The connected computer's name sits by its icon (it left the top bar, Eoj 2026-11-24).
    val computerName = active?.let { it.name.ifEmpty { "Allternit runtime" } } ?: envLabel
    val branch = folder?.branch?.takeIf { it.isNotEmpty() }

    Row(
        modifier.fillMaxWidth().padding(start = 6.dp, end = 6.dp, top = 4.dp, bottom = 12.dp).testTag("codeDeckRow"),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Box {
            DeckIcon(
                if (cloud) DesignR.drawable.ph_cloud else DesignR.drawable.ph_desktop,
                "Environment: $computerName${if (connecting) ", Connecting" else ""}",
                "codeEnvironment",
            ) { sheet = DeckSheet.Environment }
            if (connecting) {
                Box(Modifier.align(Alignment.TopEnd).offset(x = (-8).dp, y = 10.dp).size(6.dp).clip(CircleShape).background(AllternitStatic.StatusWarning).testTag("codeEnvironmentDot"))
            }
        }
        Text(
            computerName,
            color = c.textPrimary,
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
            fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.widthIn(max = 96.dp).testTag("codeComputerName")
                .clickable(role = Role.Button, onClickLabel = "Environment") { sheet = DeckSheet.Environment }
                .semantics { contentDescription = "Environment: $computerName" },
        )
        DeckIcon(DesignR.drawable.ph_folder, "Working folder: ${folder?.name ?: "Select folder"}", "codeFolder") { sheet = DeckSheet.Folder }
        if (branch != null) {
            Box(
                Modifier.width(36.dp).height(40.dp).testTag("codeBranch").semantics { contentDescription = "Branch $branch" },
                contentAlignment = Alignment.Center,
            ) { Icon(painterResource(DesignR.drawable.ph_git_branch), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(16.dp)) }
        }
        if (isDefaultBranch(branch)) {
            DeckIcon(
                DesignR.drawable.ph_squares_four,
                "Worktree isolation, ${if (worktree) "on" else "off"}",
                "codeWorktree",
                tint = if (worktree) c.accentPrimary else c.textPrimary,
            ) {
                worktree = !worktree
                CodeDeckPrefs.setWorktree(context, worktree)
            }
        }
        DeckIcon(permission.icon, "Code permission mode: ${permission.label}", "codePermission", tint = c.accentPrimary) { sheet = DeckSheet.Permission }
        Spacer(Modifier.weight(1f))
        UsageMeter(planFraction, onOpenUsage)
        ViewMenu(onOpenTerminal)
    }

    when (sheet) {
        DeckSheet.Environment -> EnvironmentSheet(
            devices = devices,
            activeId = active?.id,
            onPick = { d -> host.backend.relay.selectedRuntimeId = d.id; sheet = null; tick++ },
            onShell = { d -> host.backend.relay.selectedRuntimeId = d.id; sheet = null; onOpenTerminal() },
            onPair = { sheet = null; onConnect() },
            onDismiss = { sheet = null },
        )
        DeckSheet.Folder -> FolderSheet(folder, onDismiss = { sheet = null })
        DeckSheet.Permission -> PermissionSheet(
            selection = permission,
            onPick = { permission = it; CodeDeckPrefs.setPermission(context, it); sheet = null },
            onDismiss = { sheet = null },
        )
        null -> Unit
    }
}

@Composable
private fun DeckIcon(icon: Int, label: String, testTag: String, tint: Color? = null, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        Modifier.width(36.dp).height(40.dp).testTag(testTag).clickable(role = Role.Button, onClickLabel = label, onClick = onClick).semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) { Icon(painterResource(icon), contentDescription = null, tint = tint ?: c.textPrimary, modifier = Modifier.size(16.dp)) }
}

/** The composer usage meter's five blocks (iOS CodeUsageBlocks): amber from 80%, red at the limit. */
@Composable
private fun UsageMeter(fraction: Double?, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val f = (fraction ?: 0.0).coerceIn(0.0, 1.0)
    val filled = kotlin.math.ceil(f * 5).toInt()
    val fill = when {
        f >= 1.0 -> AllternitStatic.StatusError
        f >= 0.8 -> AllternitStatic.StatusWarning
        else -> c.textSecondary
    }
    val label = if (fraction != null) "Usage: ${(f * 100).toInt()}% used. Open details." else "Usage. Open details."
    Row(
        Modifier.padding(horizontal = 6.dp).height(40.dp).testTag("codeUsageMeter")
            .clickable(role = Role.Button, onClickLabel = label, onClick = onClick).semantics { contentDescription = label },
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        repeat(5) { i ->
            Box(Modifier.width(5.dp).height(10.dp).clip(RoundedCornerShape(1.dp)).background(if (i < filled) fill else c.borderStrong))
        }
    }
}

/** The view switch (iOS CodeViewMenu): Code · Terminal, the current one checked. */
@Composable
private fun ViewMenu(onOpenTerminal: () -> Unit) {
    val c = AllternitTheme.colors
    var open by remember { mutableStateOf(false) }
    Box {
        Box(
            Modifier.size(40.dp).testTag("codeOpenTerminal").clickable(role = Role.Button, onClickLabel = "Switch between Code and Terminal") { open = true }
                .semantics { contentDescription = "View: Code" },
            contentAlignment = Alignment.Center,
        ) { Icon(painterResource(DesignR.drawable.ph_chat_circle), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(16.dp)) }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }, containerColor = c.bgPrimary) {
            DropdownMenuItem(
                text = { Text("Code", color = c.textPrimary, fontSize = 15.sp) },
                leadingIcon = { Icon(painterResource(DesignR.drawable.ph_check), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(16.dp)) },
                onClick = { open = false },
                modifier = Modifier.testTag("codeViewCode"),
            )
            DropdownMenuItem(
                text = { Text("Terminal", color = c.textPrimary, fontSize = 15.sp) },
                leadingIcon = { Icon(painterResource(DesignR.drawable.ph_terminal_window), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(16.dp)) },
                onClick = { open = false; onOpenTerminal() },
                modifier = Modifier.testTag("codeViewTerminal"),
            )
        }
    }
}

// MARK: - Sheets

@Composable
private fun SheetSection(text: String, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    Text(text.uppercase(), color = c.accentPrimary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.6.sp, modifier = modifier.padding(horizontal = 8.dp, vertical = 4.dp))
}

@Composable
private fun SheetRow(icon: Int, title: String, subtitle: String?, selected: Boolean, testTag: String, onClick: (() -> Unit)?) {
    val c = AllternitTheme.colors
    Row(
        Modifier.fillMaxWidth().heightIn(min = 44.dp).clip(RoundedCornerShape(10.dp))
            .background(if (selected) c.fillNeutral else Color.Transparent)
            .testTag(testTag)
            .then(if (onClick != null) Modifier.clickable(role = Role.Button, onClick = onClick) else Modifier)
            .padding(horizontal = 8.dp, vertical = 8.dp),
        verticalAlignment = Alignment.Top,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(painterResource(icon), contentDescription = null, tint = c.textSecondary, modifier = Modifier.padding(top = 2.dp).size(18.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (subtitle != null) Text(subtitle, color = c.textSecondary, fontSize = 12.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
        if (selected) Icon(painterResource(DesignR.drawable.ph_check), contentDescription = "Selected", tint = c.textSecondary, modifier = Modifier.size(16.dp))
    }
}

/**
 * The environment picker (iOS CodeEnvironmentSheet): the runtimes the thread can run on, Open shell
 * on a connected one, and Pair or configure runtime. A phone has no local environment, so there is
 * no "This computer" row of its own: every row is a computer the account can reach.
 */
@Composable
private fun EnvironmentSheet(
    devices: List<RelayDevice>?,
    activeId: String?,
    onPick: (RelayDevice) -> Unit,
    onShell: (RelayDevice) -> Unit,
    onPair: () -> Unit,
    onDismiss: () -> Unit,
) {
    val c = AllternitTheme.colors
    LibrarySheet(title = "Environment", testTag = "codeEnvironmentSheet", onDismiss = onDismiss, leading = LibrarySheetAction("Close", onClick = onDismiss)) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 14.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            SheetSection("Runs on")
            when {
                devices == null -> SheetRow(DesignR.drawable.ph_arrow_clockwise, "Loading runtimes…", null, false, "codeEnvLoading", null)
                devices.isEmpty() -> com.allternit.core.design.EmptyBox("No computer connected", "Connect a computer or an Allternit cloud computer to run Code.", Modifier.testTag("codeEnvNone"), art = com.allternit.core.design.EmptyArtKind.ConnectScene, panel = true)
                else -> devices.forEach { d ->
                    SheetRow(
                        icon = if (d.isCloud()) DesignR.drawable.ph_cloud else DesignR.drawable.ph_desktop,
                        title = d.name.ifEmpty { "Allternit runtime" },
                        subtitle = if (d.isOnline) (if (d.preferred) "Default" else null) else "Asleep · wakes on first request",
                        selected = d.id == activeId,
                        testTag = "codeEnv-${d.id}",
                    ) { onPick(d) }
                    // The shell lives here: only a connected computer has one.
                    if (d.isOnline) {
                        Text(
                            "Open shell",
                            color = c.accentPrimary,
                            fontSize = 13.sp,
                            fontWeight = FontWeight.Medium,
                            modifier = Modifier.padding(start = 36.dp, bottom = 6.dp).heightIn(min = 36.dp).testTag("codeEnvShell-${d.id}")
                                .clickable(role = Role.Button, onClickLabel = "Open shell on ${d.name.ifEmpty { "Allternit runtime" }}") { onShell(d) }
                                .padding(top = 8.dp),
                        )
                    }
                }
            }
            Box(Modifier.padding(vertical = 6.dp).fillMaxWidth().height(1.dp).background(c.borderSubtle))
            SheetRow(DesignR.drawable.ph_plus, "Pair or configure runtime…", null, false, "codeEnvPair", onPair)
            Box(Modifier.padding(bottom = 16.dp))
        }
    }
}

/** The working-folder sheet (iOS CodeFolderSheet): the folder the computer's gizzi-code runs in. */
@Composable
private fun FolderSheet(folder: CodeFolder?, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    LibrarySheet(title = "Working folder", testTag = "codeFolderSheet", onDismiss = onDismiss, leading = LibrarySheetAction("Close", onClick = onDismiss)) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            SheetSection("Recent")
            if (folder != null) {
                SheetRow(DesignR.drawable.ph_folder, folder.name, folder.directory, true, "codeFolderCurrent", null)
            } else {
                Text(
                    "Connect a computer or Allternit Cloud to pick a folder.",
                    color = c.textSecondary, fontSize = 13.sp,
                    modifier = Modifier.padding(horizontal = 8.dp).heightIn(min = 44.dp).testTag("codeFolderNone"),
                )
            }
            Box(Modifier.padding(bottom = 16.dp))
        }
    }
}

/** The permission-mode overlay (iOS CodePermissionSheet): four modes with descriptions, a check on the active one. */
@Composable
private fun PermissionSheet(selection: CodePermissionMode, onPick: (CodePermissionMode) -> Unit, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    LibrarySheet(title = "Permissions", testTag = "codePermissionSheet", onDismiss = onDismiss, leading = LibrarySheetAction("Close", onClick = onDismiss)) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(1.dp)) {
            CodePermissionMode.entries.forEach { mode ->
                val active = mode == selection
                Row(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp))
                        .background(if (active) c.accentPrimary.copy(alpha = 0.1f) else Color.Transparent)
                        .testTag("codePermissionMode-${mode.raw}")
                        .clickable(role = Role.Button) { onPick(mode) }
                        .padding(10.dp),
                    verticalAlignment = Alignment.Top,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Box(Modifier.size(30.dp).clip(RoundedCornerShape(8.dp)).background(c.accentPrimary.copy(alpha = 0.1f)), contentAlignment = Alignment.Center) {
                        Icon(painterResource(mode.icon), contentDescription = null, tint = c.accentPrimary, modifier = Modifier.size(16.dp))
                    }
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(mode.label, color = c.textPrimary, fontSize = 13.sp, fontWeight = if (active) FontWeight.Bold else FontWeight.SemiBold)
                        Text(mode.blurb, color = c.textSecondary, fontSize = 11.5.sp)
                    }
                    if (active) Icon(painterResource(DesignR.drawable.ph_check), contentDescription = "Selected", tint = c.accentPrimary, modifier = Modifier.padding(top = 2.dp).size(16.dp))
                }
            }
            Box(Modifier.padding(bottom = 16.dp))
        }
    }
}
