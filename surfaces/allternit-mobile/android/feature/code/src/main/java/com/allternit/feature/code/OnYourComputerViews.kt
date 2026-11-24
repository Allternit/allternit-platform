package com.allternit.feature.code

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.core.design.R as DesignR

/*
 * The terminal previews and "On your computer" rows of the Code threads screen (iOS
 * OnYourComputerViews.swift, web TerminalPreview / OnThisComputerRows / MoveConfirmSheet).
 */

// MARK: - Terminal previews

@Composable
private fun toneColor(tone: TerminalPreview.Tone): Color = when (tone) {
    TerminalPreview.Tone.Working -> AllternitTheme.colors.accentCode
    TerminalPreview.Tone.Waiting -> AllternitStatic.StatusWarning
    TerminalPreview.Tone.Idle -> AllternitStatic.StatusSuccess
    TerminalPreview.Tone.Done -> AllternitTheme.colors.textTertiary
}

/**
 * One Allternit terminal: title, folder, status (dot + word read off the screen) and its last rows above the
 * agent's input box, monospace and clipped. A tap opens that terminal.
 */
@Composable
internal fun TerminalPreviewRow(terminal: TerminalListItem, layout: TerminalPreview.Layout, onOpen: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val status = TerminalPreview.status(terminal)
    val title = TerminalPreview.title(terminal)
    val detail = listOfNotNull(OnYourComputer.basename(terminal.cwd), status.agent).filter { it.isNotEmpty() }.joinToString(" · ")
    val rows = TerminalPreview.rows(terminal.preview, terminal.title, TerminalPreview.rowCount(layout))
    val label = listOf(title, detail, status.word).filter { it.isNotEmpty() }.joinToString(", ") +
        if (rows.isEmpty()) "" else ". Last lines: " + rows.joinToString(". ")
    Column(
        Modifier.fillMaxWidth()
            .testTag("codeTerminalPreview-${terminal.sessionId}")
            .clickable(role = Role.Button, onClickLabel = "Open this terminal") { Haptics.perform(view, Haptics.Kind.Tap); onOpen() }
            .semantics(mergeDescendants = true) { contentDescription = label }
            .padding(horizontal = 16.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(painterResource(DesignR.drawable.ph_terminal), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
            Column(Modifier.weight(1f)) {
                Text(title, color = c.textPrimary, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (detail.isNotEmpty()) Text(detail, color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            Box(Modifier.size(7.dp).clip(CircleShape).background(toneColor(status.tone)))
            Text(status.word, color = c.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.Medium)
        }
        if (rows.isNotEmpty()) {
            Column(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(c.bgTertiary.copy(alpha = 0.6f))
                    .padding(horizontal = 8.dp, vertical = 6.dp).clearAndSetSemantics { },
                verticalArrangement = Arrangement.spacedBy(1.dp),
            ) {
                rows.forEach { row ->
                    Text(
                        row.ifEmpty { " " }, color = c.textSecondary, fontSize = 11.sp, lineHeight = 14.sp, fontFamily = CodeMono,
                        maxLines = 1, softWrap = false, overflow = TextOverflow.Clip,
                    )
                }
            }
        }
    }
}

// MARK: - On your computer

/**
 * A coding-agent session running outside Allternit (Terminal.app, iTerm2, Ghostty…) and what can be done
 * with it here: Move here (always through the confirm sheet on a phone), Open a copy, New session here,
 * and the Move's progress or failure.
 */
@Composable
internal fun OnYourComputerRow(session: LiveSession, store: TerminalSessionsStore, onOpenTerminal: (terminalId: String, name: String) -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    var confirming by remember { mutableStateOf(false) }
    val name = OnYourComputer.displayName(session)
    val harness = OnYourComputer.harnessName(session.harness)
    val set = OnYourComputer.rowActions(session.movable, session.canCopy)
    val job = store.adoptJobs[session.sessionId]
    val progress = job?.takeIf { !it.finished }?.let { OnYourComputer.jobStateText(it.state) }
    val failure = job?.takeIf { it.state == "failed" }?.let { OnYourComputer.moveErrorText(it.code, it.error) }
    val flags = OnYourComputer.flagChips(session.carriedFlags)
    val detail = listOfNotNull(OnYourComputer.basename(session.cwd), OnYourComputer.appLine(session), OnYourComputer.statusWord(session.status))
        .filter { it.isNotEmpty() }.joinToString(" · ")

    Column(
        Modifier.fillMaxWidth().testTag("codeLiveSession-${session.sessionId}").padding(horizontal = 16.dp, vertical = 10.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(
            Modifier.fillMaxWidth().semantics(mergeDescendants = true) { contentDescription = "$harness, $name, $detail" },
            verticalAlignment = Alignment.Top,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                harness, color = c.textSecondary, fontSize = 10.sp, fontWeight = FontWeight.SemiBold,
                modifier = Modifier.padding(top = 2.dp).clip(RoundedCornerShape(4.dp)).background(c.bgTertiary).padding(horizontal = 5.dp, vertical = 2.dp),
            )
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(name, color = c.textPrimary, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(detail, color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            val dot = when (session.status) {
                "busy" -> c.accentCode
                "idle" -> AllternitStatic.StatusSuccess
                else -> c.textTertiary
            }
            Box(Modifier.padding(top = 6.dp).size(8.dp).clip(CircleShape).background(dot))
        }
        if (flags.isNotEmpty()) FlagChips(flags)
        if (set.openElsewhere) {
            Text(OnYourComputer.openElsewhereText(session.holders), color = AllternitStatic.StatusWarning, fontSize = 13.sp, modifier = Modifier.testTag("codeLiveElsewhere-${session.sessionId}"))
        }
        if (set.noResume) Text("This CLI can't resume a session by id.", color = c.textTertiary, fontSize = 13.sp)

        when {
            job != null && progress != null -> ProgressLine(progress, job, name, onCancel = { store.cancel(session) })
            failure != null -> FailureBox(failure, onDismiss = { store.dismiss(session.sessionId) })
            set.actions.isNotEmpty() -> {
                val pending = store.pending[session.sessionId]
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    set.actions.forEach { action ->
                        ActionPill(
                            label = if (pending == action) "Opening…" else action.label,
                            spoken = "${action.label}: $name",
                            primary = action.isMove,
                            busy = pending == action,
                            enabled = pending == null,
                            testTag = "codeLiveAction-${action.name}-${session.sessionId}",
                        ) {
                            if (action.isMove) {
                                // Eoj's rule: on a phone a Move always goes through the confirm sheet.
                                confirming = true
                            } else {
                                Haptics.perform(view, Haptics.Kind.Tap)
                                store.start(session, action, onOpenTerminal)
                            }
                        }
                    }
                }
            }
        }
    }

    if (confirming) {
        MoveConfirmSheet(
            session = session,
            onCancel = { confirming = false },
            onMove = {
                confirming = false
                Haptics.perform(view, Haptics.Kind.Tap)
                store.start(session, if (session.movable == "busy") OnYourComputer.RowAction.MoveWhenIdle else OnYourComputer.RowAction.Move, onOpenTerminal)
            },
        )
    }
}

@Composable
private fun ActionPill(label: String, spoken: String, primary: Boolean, busy: Boolean, enabled: Boolean, testTag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        Modifier.heightIn(min = 40.dp).clip(RoundedCornerShape(20.dp))
            .background(if (primary) c.textPrimary.copy(alpha = if (enabled) 1f else 0.4f) else c.bgTertiary)
            .testTag(testTag)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .semantics { contentDescription = spoken }
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        if (busy) CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 1.5.dp, color = c.textSecondary)
        Text(label, color = if (primary) c.bgPrimary else if (enabled) c.textPrimary else c.textTertiary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
    }
}

/** "Keeps: --model opus --dangerously-skip-permissions" as small monospace chips. */
@Composable
private fun FlagChips(chips: List<String>) {
    val c = AllternitTheme.colors
    Row(
        Modifier.horizontalScroll(rememberScrollState()).clearAndSetSemantics { contentDescription = "Keeps: " + chips.joinToString(" ") },
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Text("Keeps:", color = c.textTertiary, fontSize = 11.sp)
        chips.forEach { chip ->
            Text(
                chip, color = c.textSecondary, fontSize = 11.sp, fontFamily = CodeMono, maxLines = 1,
                modifier = Modifier.clip(RoundedCornerShape(4.dp)).background(c.bgTertiary).padding(horizontal = 5.dp, vertical = 2.dp),
            )
        }
    }
}

@Composable
private fun ProgressLine(text: String, job: AdoptJob, name: String, onCancel: () -> Unit) {
    val c = AllternitTheme.colors
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Row(Modifier.fillMaxWidth().heightIn(min = 40.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp, color = c.textSecondary)
            Text(text, color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.weight(1f).semantics { contentDescription = "Moving $name: $text" })
            if (job.state == "waiting_idle") {
                Text(
                    "Cancel", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.heightIn(min = 40.dp).testTag("codeLiveCancel")
                        .clickable(role = Role.Button, onClickLabel = "Cancel moving $name", onClick = onCancel)
                        .semantics { contentDescription = "Cancel moving $name" }
                        .padding(horizontal = 8.dp, vertical = 10.dp),
                )
            }
        }
        job.notice?.let { Text(it, color = c.textTertiary, fontSize = 12.sp) }
    }
}

@Composable
private fun FailureBox(failure: OnYourComputer.MoveErrorText, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val view = LocalView.current
    var copied by remember(failure) { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(verticalAlignment = Alignment.Top) {
            Text(failure.text, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.weight(1f).padding(top = 6.dp).testTag("codeLiveFailure"))
            Box(
                Modifier.size(32.dp).clickable(role = Role.Button, onClickLabel = "Dismiss", onClick = onDismiss).semantics { contentDescription = "Dismiss" },
                contentAlignment = Alignment.Center,
            ) { Icon(painterResource(DesignR.drawable.ph_x), contentDescription = null, tint = c.textTertiary, modifier = Modifier.size(12.dp)) }
        }
        failure.command?.let { command ->
            failure.cwd?.let { Text("In $it:", color = c.textSecondary, fontSize = 12.sp) }
            Row(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(c.bgTertiary).padding(8.dp),
                verticalAlignment = Alignment.Top,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                SelectionContainer(Modifier.weight(1f)) {
                    Text(command, color = c.textPrimary, fontSize = 12.sp, fontFamily = CodeMono)
                }
                Text(
                    if (copied) "Copied" else "Copy", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.heightIn(min = 32.dp).testTag("codeLiveCopyCommand")
                        .clickable(role = Role.Button) {
                            copyCommand(context, command)
                            copied = true
                            Haptics.perform(view, Haptics.Kind.Success)
                        }
                        .semantics { contentDescription = if (copied) "Copied" else "Copy the command" }
                        .padding(horizontal = 4.dp, vertical = 6.dp),
                )
            }
        }
    }
}

/**
 * The Move confirm (web MoveConfirmSheet, iOS MoveConfirmSheet): what Move runs, the flags it keeps, and what
 * happens to the old window, with Move / Cancel.
 */
@Composable
internal fun MoveConfirmSheet(session: LiveSession, onCancel: () -> Unit, onMove: () -> Unit) {
    val c = AllternitTheme.colors
    val name = OnYourComputer.displayName(session)
    val holder = session.holders.firstOrNull()
    val holderText = holder?.let { OnYourComputer.holderLabel(it.app, it.pid) } ?: session.pid?.let { "pid $it" }
    val whenIdle = session.movable == "busy"
    val command = OnYourComputer.moveCommandLine(session.moveCommand)
    val flags = OnYourComputer.flagChips(session.carriedFlags)
    LibrarySheet(title = "Move here", testTag = "codeMoveSheet", onDismiss = onCancel, leading = LibrarySheetAction("Cancel", onClick = onCancel)) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Text("Move $name here?", color = c.textPrimary, fontSize = 18.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.semantics { heading() })
            if (command != null) {
                Box(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(c.bgTertiary)
                        .horizontalScroll(rememberScrollState()).testTag("codeMoveCommand")
                        .semantics { contentDescription = "Command: $command" },
                ) {
                    SelectionContainer {
                        Text(command, color = c.textPrimary, fontSize = 12.sp, fontFamily = CodeMono, softWrap = false, modifier = Modifier.padding(10.dp))
                    }
                }
            }
            if (flags.isNotEmpty()) FlagChips(flags)
            Text(OnYourComputer.confirmText(holder?.app, holderText, whenIdle), color = c.textSecondary, fontSize = 14.sp, lineHeight = 20.sp, modifier = Modifier.testTag("codeMoveText"))
            Row(Modifier.fillMaxWidth().padding(bottom = 12.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                SheetButton("Cancel", "Cancel", primary = false, testTag = "codeMoveCancel", modifier = Modifier.weight(1f), onClick = onCancel)
                SheetButton(
                    if (whenIdle) "Move when idle" else "Move",
                    if (whenIdle) "Move $name when idle" else "Move $name here",
                    primary = true, testTag = "codeMoveConfirm", modifier = Modifier.weight(1f), onClick = onMove,
                )
            }
        }
    }
}

@Composable
private fun SheetButton(label: String, spoken: String, primary: Boolean, testTag: String, modifier: Modifier, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        modifier.heightIn(min = 48.dp).clip(RoundedCornerShape(24.dp))
            .then(if (primary) Modifier.background(c.textPrimary) else Modifier.background(c.bgTertiary).border(1.dp, c.borderSubtle, RoundedCornerShape(24.dp)))
            .testTag(testTag)
            .clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = spoken },
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = if (primary) c.bgPrimary else c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
    }
}

private fun copyCommand(context: Context, text: String) {
    val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager ?: return
    cm.setPrimaryClip(ClipData.newPlainText("Command", text))
}
