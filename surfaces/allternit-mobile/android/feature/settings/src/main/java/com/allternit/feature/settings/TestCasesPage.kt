package com.allternit.feature.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.border
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ModalBottomSheet
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
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.chat.EvalBackend
import com.allternit.feature.chat.EvalCase
import com.allternit.feature.chat.EvalError
import com.allternit.feature.chat.EvalEstimate
import com.allternit.feature.chat.EvalOutcome
import com.allternit.feature.chat.EvalReportView
import com.allternit.feature.chat.EvalRun
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR

private const val SUITE = "my-cases"
private const val POLL_MS = 2000L

/** Settings › Test cases (eval loop 2.9): one list for the whole account, same rows as iOS. */
@Composable
internal fun TestCasesPage(host: SettingsHost, back: () -> Unit) {
    val c = AllternitTheme.colors
    val eval = host.backend.eval
    val scope = rememberCoroutineScope()
    var nonce by remember { mutableStateOf(0) }
    var list by remember { mutableStateOf<Load<List<EvalCase>>>(Load.Loading) }
    var pendingDelete by remember { mutableStateOf<EvalCase?>(null) }
    var replayNote by remember { mutableStateOf<String?>(null) }
    var replaying by remember { mutableStateOf(false) }
    var liveOpen by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(nonce) {
        if (list !is Load.Ready) list = Load.Loading
        list = when (val r = eval.listCases()) {
            is EvalOutcome.Ok -> Load.Ready(r.value)
            is EvalOutcome.Err -> if (r.error == EvalError.NoComputer) Load.NoComputer else Load.Failed(r.error.text)
        }
    }

    SPage("Test cases", "testCasesPage", onBack = back) {
        SLabel("Test cases")
        SNote("Chats you saved as checks. Replay is free. A live run uses the model, so it shows the price first.")
        val cases = (list as? Load.Ready)?.value.orEmpty()
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            SButton(if (replaying) "Running…" else "Run checks", tag = "testCasesRun", enabled = cases.isNotEmpty() && !replaying) {
                replaying = true; error = null; replayNote = null
                scope.launch {
                    when (val r = eval.startRun(SUITE, live = false)) {
                        is EvalOutcome.Ok -> {
                            val results = EvalReportView.results(r.value.report)
                            replayNote = if (results.isEmpty()) "Done" else "${results.count { it.pass }} of ${results.size} passed"
                            nonce++
                        }
                        is EvalOutcome.Err -> error = r.error.text
                    }
                    replaying = false
                }
            }
            SButton("Run live…", tag = "testCasesRunLive", enabled = cases.isNotEmpty()) { liveOpen = true }
        }
        replayNote?.let { Text("Last replay: $it", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp).testTag("testCasesReplayNote")) }
        SError(error)
        LoadView(list, retry = { nonce++ }, doing = "load test cases") {
            if (it.isEmpty()) StateBox("No test cases yet", "Open a chat, use its menu, and pick Save as test case.", null, null)
            else SCard {
                it.forEachIndexed { i, case ->
                    if (i > 0) SDivider()
                    CaseRow(case, onDelete = { pendingDelete = case })
                }
            }
        }
    }

    pendingDelete?.let { case ->
        AlertDialog(
            onDismissRequest = { pendingDelete = null },
            title = { Text("Delete this test case?") },
            text = { Text(case.title.ifEmpty { "Test case" }) },
            confirmButton = {
                TextButton(onClick = {
                    pendingDelete = null
                    scope.launch {
                        when (val r = eval.deleteCase(case.id)) {
                            is EvalOutcome.Ok -> nonce++
                            is EvalOutcome.Err -> if (r.error == EvalError.NotFound) nonce++ else error = r.error.text
                        }
                    }
                }, modifier = Modifier.testTag("testCaseDeleteConfirm")) { Text("Delete", color = Danger) }
            },
            dismissButton = { TextButton(onClick = { pendingDelete = null }) { Text("Cancel") } },
        )
    }
    if (liveOpen) RunLiveSheet(eval, onDone = { nonce++ }, onDismiss = { liveOpen = false })
}

@Composable
private fun CaseRow(case: EvalCase, onDelete: () -> Unit) {
    val c = AllternitTheme.colors
    val last = when (case.lastResult) { "pass" -> "pass"; "fail" -> "fail"; else -> "never run" }
    val size = if (case.workspaceKind == "archive") sizeText(case.bytes) else "Trace only"
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp).testTag("testCaseRow_${case.id}"), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                androidx.compose.material3.Icon(
                    androidx.compose.ui.res.painterResource(if (case.label == "good") DesignR.drawable.ph_thumbs_up else DesignR.drawable.ph_thumbs_down),
                    contentDescription = if (case.label == "good") "Good" else "Bad", tint = c.textSecondary, modifier = Modifier.size(14.dp),
                )
                Text(case.title.ifEmpty { "Test case" }, color = c.textPrimary, fontSize = 16.sp, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                if (case.needsCheck) Text(
                    "Needs a check", color = c.textPrimary, fontSize = 12.sp, fontWeight = FontWeight.Medium,
                    modifier = Modifier.border(1.dp, c.borderStrong, RoundedCornerShape(50)).padding(horizontal = 8.dp, vertical = 2.dp),
                )
            }
            Text("Last result: $last · $size", color = c.textSecondary, fontSize = 13.sp)
        }
        SButton("Delete", tag = "testCaseDelete_${case.id}", destructive = true, onClick = onDelete)
    }
}

private fun sizeText(bytes: Long): String = when {
    bytes >= 1_000_000 -> "%.1f MB".format(bytes / 1_000_000.0)
    bytes >= 1_000 -> "${bytes / 1_000} KB"
    else -> "$bytes B"
}

/** Run live: the price first, Run only once it has loaded, then a 2 s poll with progress and Cancel. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun RunLiveSheet(eval: EvalBackend, onDone: () -> Unit, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var estimate by remember { mutableStateOf<EvalEstimate?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    var run by remember { mutableStateOf<EvalRun?>(null) }
    var starting by remember { mutableStateOf(false) }

    LaunchedEffect(Unit) {
        when (val r = eval.estimateRun(SUITE)) {
            is EvalOutcome.Ok -> estimate = r.value
            is EvalOutcome.Err -> failure = r.error.text
        }
    }
    val runId = run?.runId
    LaunchedEffect(runId) {
        if (runId == null) return@LaunchedEffect
        while (run?.finished != true) {
            delay(POLL_MS)
            when (val r = eval.getRun(runId)) {
                is EvalOutcome.Ok -> run = r.value
                is EvalOutcome.Err -> { failure = r.error.text; break }
            }
        }
        onDone()
    }

    ModalBottomSheet(onDismissRequest = { if (run?.finished != false) onDismiss() }, containerColor = c.bgPrimary) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 24.dp).navigationBarsPadding().testTag("runLiveSheet"), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Text("Run live", color = c.textPrimary, fontSize = 22.sp, modifier = Modifier.semantics { heading() })
            val active = run
            val est = estimate
            when {
                active != null -> {
                    val p = active.progress
                    when (active.state) {
                        "done" -> Text("Done", color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.testTag("runLiveState"))
                        "failed" -> Text(active.error ?: "The run failed.", color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.testTag("runLiveState"))
                        "cancelled" -> Text("Cancelled", color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.testTag("runLiveState"))
                        else -> Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            if (p.total > 0) LinearProgressIndicator(progress = { p.done.toFloat() / p.total }, modifier = Modifier.fillMaxWidth(), color = c.textPrimary, trackColor = c.fillNeutral)
                            else LinearProgressIndicator(Modifier.fillMaxWidth(), color = c.textPrimary, trackColor = c.fillNeutral)
                            Text("${p.done} of ${p.total} runs", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.testTag("runLiveProgress"))
                        }
                    }
                    if (!active.finished) Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                        SButton("Cancel", tag = "runLiveCancel") { scope.launch { eval.cancelRun(active.runId) } }
                    }
                    else Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) { SButton("Done", tag = "runLiveDone", onClick = onDismiss) }
                }
                failure != null -> Text(failure!!, color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.testTag("runLiveState"))
                est == null -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.testTag("runLiveState")) {
                    CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary)
                    Text("Getting the price…", color = c.textSecondary, fontSize = 14.sp)
                }
                else -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text("${est.attempts} runs · about ${"%,d".format(est.tokensEst)} tokens · about ${Math.round(est.minutesEst).coerceAtLeast(1)} min", color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.testTag("runLivePrice"))
                    Text(
                        when (est.billing) { "plan" -> "Uses your plan"; "metered" -> "About $" + "%.2f".format(est.usdEst ?: 0.0); else -> "Price unknown" },
                        color = c.textSecondary, fontSize = 14.sp, fontWeight = FontWeight.Medium,
                    )
                }
            }
            if (run == null) Row(Modifier.fillMaxWidth().padding(bottom = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                SButton("Cancel", tag = "runLiveClose", onClick = onDismiss)
                SButton(if (starting) "Starting…" else "Run", tag = "runLiveStart", enabled = estimate != null && !starting) {
                    val e = estimate ?: return@SButton
                    starting = true
                    scope.launch {
                        when (val r = eval.startRun(SUITE, live = true, estimateId = e.estimateId)) {
                            is EvalOutcome.Ok -> run = r.value.copy(state = "running")
                            is EvalOutcome.Err -> { failure = r.error.text; estimate = null }
                        }
                        starting = false
                    }
                }
            }
        }
    }
}
