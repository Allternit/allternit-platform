package com.allternit.feature.code

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.clickable
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.ui.semantics.Role
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.chat.ChatBackend
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/*
 * Diffs and review (iOS DiffRenderer, SessionDiffListView, FileDiffDetailView, ChangesetReviewSheet).
 */

/** Lines with old/new line numbers; added rows green, removed rows red (iOS DiffRenderer). */
@Composable
internal fun DiffView(lines: List<DiffLine>, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    Column(modifier.fillMaxWidth().verticalScroll(rememberScrollState()).horizontalScroll(rememberScrollState()).testTag("codeDiffView")) {
        lines.forEach { l ->
            val bg = when (l.kind) {
                DiffKind.Added -> AllternitStatic.StatusSuccess.copy(alpha = 0.14f)
                DiffKind.Removed -> AllternitStatic.StatusError.copy(alpha = 0.14f)
                DiffKind.Hunk -> c.fillNeutral
                DiffKind.Context -> androidx.compose.ui.graphics.Color.Transparent
            }
            val sign = when (l.kind) { DiffKind.Added -> "+"; DiffKind.Removed -> "-"; else -> " " }
            Row(Modifier.fillMaxWidth().background(bg).padding(horizontal = 8.dp)) {
                Text(l.oldNo?.toString().orEmpty(), color = c.textTertiary, fontSize = 11.sp, fontFamily = CodeMono, textAlign = TextAlign.End, modifier = Modifier.width(34.dp))
                Text(l.newNo?.toString().orEmpty(), color = c.textTertiary, fontSize = 11.sp, fontFamily = CodeMono, textAlign = TextAlign.End, modifier = Modifier.width(34.dp).padding(end = 6.dp))
                Text(
                    if (l.kind == DiffKind.Hunk) l.text else sign + l.text.ifEmpty { " " },
                    color = when (l.kind) { DiffKind.Added -> AllternitStatic.StatusSuccess; DiffKind.Removed -> AllternitStatic.StatusError; else -> c.textPrimary },
                    fontSize = 12.sp, fontFamily = CodeMono, softWrap = false,
                )
            }
        }
    }
}

/** "+3 -1" counts (iOS adds/dels labels). */
@Composable
internal fun ChangeCounts(additions: Int, deletions: Int) {
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        if (additions > 0) Text("+$additions", color = AllternitStatic.StatusSuccess, fontSize = 12.sp, fontFamily = CodeMono)
        if (deletions > 0) Text("-$deletions", color = AllternitStatic.StatusError, fontSize = 12.sp, fontFamily = CodeMono)
    }
}

/** Changes: every file the session edited (iOS SessionDiffListView), pull to refresh via the retry. */
@Composable
internal fun SessionDiffScreen(backend: ChatBackend, sessionId: String, onBack: () -> Unit) {
    var selected by remember { mutableStateOf<FileDiff?>(null) }
    val open = selected
    if (open != null) {
        FileDiffDetailScreen(open, onBack = { selected = null })
        return
    }
    CodeScaffold(title = "Changes", onBack = onBack, backTag = "codeChangesBack") {
        CodeLoader(key = sessionId, failedTitle = "Couldn't load changes", load = { CodeApi.sessionDiff(backend, sessionId) }) { diffs, reload ->
            if (diffs.isEmpty()) {
                Column(Modifier.fillMaxSize(), horizontalAlignment = Alignment.CenterHorizontally) {
                    EmptyState("No changes yet", "When this session edits files, they'll appear here.")
                    CodePill("Refresh", testTag = "codeChangesRefresh", onClick = reload)
                }
            } else {
                LazyColumn(Modifier.fillMaxSize().testTag("codeChangesList")) {
                    items(diffs, key = { it.file }) { d ->
                        CodeRow(
                            title = d.file.substringAfterLast('/'),
                            detail = d.file,
                            testTag = "codeChangeRow-${d.file.substringAfterLast('/')}",
                            mono = true,
                            trailing = { ChangeCounts(d.additions, d.deletions) },
                            onClick = { selected = d },
                        )
                    }
                }
            }
        }
    }
}

/** One file's before/after (iOS FileDiffDetailView). */
@Composable
internal fun FileDiffDetailScreen(diff: FileDiff, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    val lines = remember(diff) { DiffLine.diffLines(diff.before, diff.after) }
    CodeScaffold(title = diff.file.substringAfterLast('/'), subtitle = diff.file, onBack = onBack, backTag = "codeFileDiffBack") {
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp)) { ChangeCounts(diff.additions, diff.deletions) }
        DiffView(lines, Modifier.weight(1f))
    }
}

/**
 * The review sheet for one pending permission (iOS ChangesetReviewSheet): its diff, then Reject,
 * Always Allow and Approve. [onResolved] fires once the computer took the answer.
 */
@Composable
internal fun ChangesetReviewSheet(backend: ChatBackend, request: ChangeRequest, onDismiss: () -> Unit, onResolved: (ChangeRequest) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var sending by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val lines = remember(request) { request.diff?.takeIf { it.isNotEmpty() }?.let { DiffLine.parse(it) } }

    fun submit(reply: String) {
        scope.launch {
            sending = true
            error = null
            val failure = CodeApi.reply(backend, request.id, reply)
            sending = false
            if (failure == null) onResolved(request) else error = failure
        }
    }

    LibrarySheet(
        title = "Review change",
        testTag = "codeChangesetSheet",
        onDismiss = onDismiss,
        leading = LibrarySheetAction("Cancel", testTag = "codeChangesetCancel", onClick = onDismiss),
    ) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text(request.permission.replaceFirstChar { it.uppercase() }, color = c.textPrimary, fontSize = 18.sp)
            request.path?.let { Text(it, color = c.textSecondary, fontSize = 12.sp, fontFamily = CodeMono) }
            ChangeCounts(request.additions, request.deletions)
            if (lines != null) {
                DiffView(lines, Modifier.fillMaxWidth().weight(1f, fill = false))
            } else {
                Text("No diff preview for this permission.", color = c.textSecondary, fontSize = 14.sp)
                request.patterns.forEach { Text(it, color = c.textSecondary, fontSize = 12.sp, fontFamily = CodeMono) }
            }
            error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.testTag("codeChangesetError")) }
            Row(Modifier.fillMaxWidth().navigationBarsPadding().padding(bottom = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                CodePill("Reject", testTag = "codeChangesetReject", enabled = !sending) { submit("reject") }
                CodePill("Always Allow", testTag = "codeChangesetAlways", enabled = !sending) { submit("always") }
                CodePill("Approve", testTag = "codeChangesetApprove", filled = true, enabled = !sending) { submit("once") }
            }
        }
    }
}

/**
 * The "N changes awaiting review" banner over a Code thread (iOS CodeThreadChatView): polls the
 * session's pending permissions every 2 s and opens the oldest in the review sheet.
 */
@Composable
fun PendingChangesBanner(backend: ChatBackend, sessionId: String?, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    var pending by remember(sessionId) { mutableStateOf<List<ChangeRequest>>(emptyList()) }
    var reviewing by remember { mutableStateOf<ChangeRequest?>(null) }
    LaunchedEffect(sessionId) {
        if (sessionId == null) return@LaunchedEffect
        while (true) {
            pending = CodeApi.pendingPermissions(sessionId)
            delay(2_000L)
        }
    }
    val oldest = pending.firstOrNull()
    if (oldest != null) {
        Row(
            modifier.fillMaxWidth().background(c.bgPanel).testTag("codePendingBanner")
                .clickable(role = Role.Button) { reviewing = oldest }
                .padding(horizontal = 14.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(if (pending.size == 1) "1 change awaiting review" else "${pending.size} changes awaiting review", color = c.textPrimary, fontSize = 14.sp, modifier = Modifier.weight(1f))
            Text("›", color = c.textSecondary, fontSize = 18.sp)
        }
    }
    reviewing?.let { r ->
        ChangesetReviewSheet(backend, r, onDismiss = { reviewing = null }, onResolved = { done ->
            pending = pending.filterNot { it.id == done.id }
            reviewing = null
        })
    }
}
