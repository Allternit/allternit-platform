package com.allternit.feature.computer

import android.annotation.SuppressLint
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.util.Base64
import android.webkit.WebChromeClient
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.compose.BackHandler
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlin.coroutines.cancellation.CancellationException
import kotlin.math.max
import kotlin.math.min

/** Action kind → highlight colour (iOS ACIActionPalette). */
private fun actionColor(kind: String?): Color = when (kind?.lowercase()) {
    "type" -> Color(0xFFA855F7)
    "scroll" -> Color(0xFFFBBF24)
    "read" -> Color(0xFF3B82F6)
    "navigate" -> Color(0xFF22C55E)
    "extract" -> Color(0xFFF97316)
    else -> Color(0xFF63FCF1)
}

private val Red = Color(0xFFEF4444)

data class AciTraceLine(val id: Int, val text: String, val kind: String?)

/** A live ACI run (iOS ACIAgentRunViewModel): start, stream, stop, approve / deny, retry. */
class AciRunModel(private val goal: String, private val client: AciAgentClient) {
    var status by mutableStateOf(AciRunStatus.Idle)
        private set
    var lastMessage by mutableStateOf<String?>(null)
        private set
    var adapterId by mutableStateOf<String?>(null)
        private set
    var stepIndex by mutableStateOf<Int?>(null)
        private set
    var totalSteps by mutableStateOf<Int?>(null)
        private set
    var currentAction by mutableStateOf<AciAction?>(null)
        private set
    var screenshot by mutableStateOf<ImageBitmap?>(null)
        private set
    var errorMessage by mutableStateOf<String?>(null)
        private set
    val traceLines = mutableStateListOf<AciTraceLine>()

    /** What the run is waiting on: a credential or action approval, a "needs you", or a block. Null when nothing. */
    var gate by mutableStateOf<AciRunGate?>(null)
        private set
    /** Who is acting (from `computer.action`), for the cursor badge and the "<bot> is working" line. */
    var actor by mutableStateOf<AciActor?>(null)
        private set
    /** The agent cursor as fractions of the screen (x / screen_w, y / screen_h), or null before the first action. */
    var cursor by mutableStateOf<Pair<Float, Float>?>(null)
        private set
    var actionStep by mutableStateOf<Int?>(null)
        private set
    val botName: String get() = actor?.name?.takeIf { it.isNotBlank() } ?: "Your bot"

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var runId: String? = null
    private var job: Job? = null
    private var eventsJob: Job? = null
    private var computerId: String? = null
    private var counter = 0
    private var started = false

    val requiresApproval: Boolean get() = status == AciRunStatus.WaitingApproval
    val isActive: Boolean get() = status in setOf(AciRunStatus.Idle, AciRunStatus.Running, AciRunStatus.WaitingApproval, AciRunStatus.Unknown)

    fun start() {
        if (started) return
        started = true
        job = scope.launch {
            try {
                val run = client.startRun(goal)
                runId = run.sessionId
                if (adapterId == null) adapterId = run.adapterId
                listenToComputer(run.sessionId)
                client.stream(run.sessionId).collect { apply(it) }
                if (isActive) status = AciRunStatus.Done
            } catch (e: CancellationException) {
                throw e
            } catch (e: Throwable) {
                errorMessage = e.message ?: "The run stopped."
                status = AciRunStatus.Error
            }
        }
    }

    /** The computer's toolset events for this run: cursor, credential approvals, needs-you and blocks. A failed stream just means no gates. */
    private fun listenToComputer(run: String) {
        val id = client.currentComputerId() ?: return
        computerId = id
        eventsJob?.cancel()
        eventsJob = scope.launch {
            try {
                client.toolsetEvents(id).collect { e -> if (e.runId == null || e.runId == run) applyComputer(e) }
            } catch (e: CancellationException) {
                throw e
            } catch (_: Throwable) {
            }
        }
    }

    private fun applyComputer(e: AciComputerEvent) {
        when (e) {
            is AciComputerEvent.Action -> {
                e.actor?.let { actor = it }
                e.step?.let { actionStep = it }
                val x = e.x
                val y = e.y
                val w = e.screenW
                val h = e.screenH
                if (x != null && y != null && w != null && h != null && w > 0 && h > 0) {
                    cursor = (x / w).toFloat().coerceIn(0f, 1f) to (y / h).toFloat().coerceIn(0f, 1f)
                }
            }
            is AciComputerEvent.ConfirmationRequired -> gate = AciRunGate.Approval(e)
            is AciComputerEvent.HumanRequested -> gate = AciRunGate.NeedsYou(e.runId, e.reason, e.secret)
            is AciComputerEvent.HumanResumed -> if (gate is AciRunGate.NeedsYou) gate = null
            is AciComputerEvent.BlockDetected -> gate = AciRunGate.Blocked(e)
            is AciComputerEvent.BlockResolved -> if (gate is AciRunGate.Blocked) gate = null
        }
    }

    fun dismissGate() { gate = null }

    /** Approve (once, or `scope = "run"`). The sheet has already confirmed with the device lock. */
    fun approveGate(scope: String?) {
        val g = gate as? AciRunGate.Approval ?: return
        gate = null
        io { client.approveHandoff(g.request.approvalId, scope) }
    }

    fun denyGate() {
        val g = gate as? AciRunGate.Approval ?: return
        gate = null
        io { client.denyHandoff(g.request.approvalId) }
    }

    /** Continue after "needs you" (human-done) or after a block (block-resolve continue). */
    fun continueGate() {
        val g = gate ?: return
        gate = null
        val id = computerId ?: return
        io { if (g is AciRunGate.Blocked) client.blockResolve(id, "continue") else client.humanDone(id) }
    }

    fun stopGate() {
        val g = gate
        gate = null
        val id = computerId
        if (g is AciRunGate.Blocked && id != null) io { client.blockResolve(id, "stop") }
        stop()
    }

    /** Re-run on another computer: resolve the block as `rerun`, then start a fresh run there with `rerun_of`. */
    fun rerun(target: AciRerunTarget) {
        val g = gate as? AciRunGate.Blocked ?: return
        gate = null
        val original = runId
        val id = computerId
        io {
            if (id != null) client.blockResolve(id, "rerun", target.computerId)
            runCatching { client.startRun(goal, rerunOf = original, targetRuntimeId = target.computerId) }
                .onSuccess { lastMessage = "Re-running on ${target.name}" }
                .onFailure { errorMessage = it.message ?: "Couldn't start the re-run." }
        }
        stop()
        lastMessage = "Re-running on ${target.name}"
    }

    private fun io(block: suspend () -> Unit) {
        CoroutineScope(Dispatchers.IO).launch { runCatching { block() } }
    }

    fun stop() {
        job?.cancel()
        job = null
        eventsJob?.cancel()
        if (!isActive) return
        status = AciRunStatus.Done
        lastMessage = "Stopped"
        val id = runId ?: return
        CoroutineScope(Dispatchers.IO).launch { runCatching { client.stop(id) } }
    }

    /** Leaving the screen stops a live run (iOS leave()). */
    fun leave() {
        if (!isActive) job?.cancel() else stop()
        eventsJob?.cancel()
        scope.cancel()
    }

    fun approve(deny: Boolean) {
        val id = runId ?: return
        if (!requiresApproval) return
        status = AciRunStatus.Running
        currentAction = null
        CoroutineScope(Dispatchers.IO).launch { runCatching { client.approve(id, deny) } }
    }

    fun retry() {
        job?.cancel()
        eventsJob?.cancel()
        gate = null
        cursor = null
        runId = null
        errorMessage = null
        status = AciRunStatus.Idle
        started = false
        start()
    }

    private suspend fun apply(e: AciEvent) {
        when (e) {
            is AciEvent.State -> {
                if (e.status != null) status = AciRunStatus.of(e.status)
                e.lastMessage?.let { lastMessage = it }
                e.adapterId?.let { adapterId = it }
                e.stepIndex?.let { stepIndex = it }
                e.totalSteps?.let { totalSteps = it }
                if (e.hasCurrentAction) {
                    currentAction = e.currentAction
                    e.currentAction?.let { a -> trace(a.label ?: a.selector ?: a.type ?: "Action", a.type) }
                }
            }
            is AciEvent.Screenshot -> {
                val bmp: Bitmap? = withContext(Dispatchers.Default) {
                    runCatching { Base64.decode(e.base64, Base64.DEFAULT).let { BitmapFactory.decodeByteArray(it, 0, it.size) } }.getOrNull()
                }
                if (bmp != null) screenshot = bmp.asImageBitmap()
            }
            is AciEvent.Trace -> {
                e.message?.let { lastMessage = it; trace(it, null) }
                e.adapterId?.let { adapterId = it }
            }
            AciEvent.Done -> if (isActive) status = AciRunStatus.Done
        }
    }

    private fun trace(text: String, kind: String?) {
        if (text.isEmpty() || traceLines.lastOrNull()?.text == text) return
        counter += 1
        traceLines.add(AciTraceLine(counter, text, kind))
        if (traceLines.size > 100) traceLines.removeRange(0, traceLines.size - 100)
    }
}

/**
 * A live agent run (iOS ACIAgentRunView): the COMPUTER USE strip (status dot, message, step, adapter),
 * the screenshot with the action highlight and scanlines, the trace log, and Approve / Deny / Stop.
 */
@Composable
internal fun AciAgentRunView(
    goal: String,
    client: AciAgentClient,
    onExit: () -> Unit,
    /** Hands the run to the person. No Android control path exists yet, so the button shows only when this is set. */
    onTakeOver: (() -> Unit)? = null,
    controls: @Composable () -> Unit = {},
) {
    val c = AllternitTheme.colors
    val model = remember(goal) { AciRunModel(goal, client) }
    DisposableEffect(model) {
        model.start()
        onDispose { model.leave() }
    }
    BackHandler { onExit() }
    val dot = when (model.status) {
        AciRunStatus.Running -> AllternitStatic.StatusSuccess
        AciRunStatus.WaitingApproval -> AllternitStatic.StatusWarning
        AciRunStatus.Done -> AllternitStatic.StatusInfo
        AciRunStatus.Blocked, AciRunStatus.Error -> Red
        else -> c.textSecondary
    }
    val pulsing = model.status == AciRunStatus.Running || model.status == AciRunStatus.WaitingApproval
    val pulse by rememberInfiniteTransition(label = "aciPulse").animateFloat(1f, 0.45f, infiniteRepeatable(tween(900), RepeatMode.Reverse), label = "aciDot")
    val message = model.lastMessage?.takeIf { it.isNotEmpty() } ?: goal.ifEmpty { if (model.status == AciRunStatus.Done) "Task complete" else "Waiting…" }
    val adapter = model.adapterId?.let { id -> id.split(".").drop(1).joinToString(".").ifEmpty { id } }
    val warm = c.borderStrong

    Column(Modifier.fillMaxSize().background(c.bgPrimary).testTag("aciAgentRun")) {
        Row(Modifier.fillMaxWidth().height(34.dp).background(c.bgSecondary).padding(horizontal = 12.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("‹ Close", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold,
                modifier = Modifier.clip(RoundedCornerShape(8.dp)).border(1.dp, c.borderSubtle, RoundedCornerShape(8.dp)).clickable { onExit() }
                    .padding(horizontal = 8.dp, vertical = 4.dp).testTag("aciRunClose"))
            Box(Modifier.size(7.dp).clip(CircleShape).background(dot.copy(alpha = if (pulsing) pulse else 1f)))
            Text("COMPUTER USE", color = warm, fontSize = 10.sp, fontWeight = FontWeight.Bold, fontFamily = FontFamily.Monospace, letterSpacing = 1.2.sp)
            Divider()
            Text(message, color = if (model.requiresApproval) Color(0xFFFDE68A) else c.textSecondary, fontSize = 11.sp,
                fontWeight = if (model.requiresApproval) FontWeight.SemiBold else FontWeight.Normal, maxLines = 1, modifier = Modifier.weight(1f))
            val step = model.stepIndex
            val total = model.totalSteps
            if (step != null && total != null && total > 1) {
                Divider()
                Text("$step/$total", color = warm, fontSize = 10.sp, fontWeight = FontWeight.Bold, fontFamily = FontFamily.Monospace)
            }
            if (adapter != null) {
                Divider()
                Text(adapter, color = c.textSecondary.copy(alpha = 0.6f), fontSize = 10.sp, fontFamily = FontFamily.Monospace, maxLines = 1)
            }
        }
        BoxWithConstraints(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
            val shot = model.screenshot
            val err = model.errorMessage
            when {
                err != null -> Column(Modifier.padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Icon(painterResource(DesignR.drawable.ph_warning), null, tint = c.textSecondary, modifier = Modifier.size(24.dp))
                    Text(err, color = Red, fontSize = 11.sp, fontFamily = FontFamily.Monospace, textAlign = TextAlign.Center)
                    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        OutlineButton("Retry", c.accentBrowser, "aciRunRetry") { model.retry() }
                        OutlineButton("Close", c.textSecondary, "aciRunCloseError") { onExit() }
                    }
                }
                shot != null -> {
                    val density = LocalDensity.current
                    val wPx = with(density) { maxWidth.toPx() }
                    val hPx = with(density) { maxHeight.toPx() }
                    val scale = min(wPx / max(shot.width, 1), hPx / max(shot.height, 1))
                    val dw = shot.width * scale
                    val dh = shot.height * scale
                    val ox = (wPx - dw) / 2
                    val oy = (hPx - dh) / 2
                    val action = model.currentAction
                    val color = actionColor(action?.type)
                    Canvas(Modifier.fillMaxSize()) {
                        drawImage(shot, IntOffset.Zero, IntSize(shot.width, shot.height), IntOffset(ox.toInt(), oy.toInt()), IntSize(dw.toInt(), dh.toInt()))
                        val ax = action?.x
                        val ay = action?.y
                        if (ax != null && ay != null) {
                            val left = ox + ax.toFloat() * scale
                            val top = oy + ay.toFloat() * scale
                            val bw = max((action?.width ?: 40.0).toFloat() * scale, 1f)
                            val bh = max((action?.height ?: 20.0).toFloat() * scale, 1f)
                            drawRoundRect(color.copy(alpha = 0.07f), androidx.compose.ui.geometry.Offset(left, top), androidx.compose.ui.geometry.Size(bw, bh), androidx.compose.ui.geometry.CornerRadius(4.dp.toPx()))
                            drawRoundRect(color.copy(alpha = 0.85f), androidx.compose.ui.geometry.Offset(left, top), androidx.compose.ui.geometry.Size(bw, bh), androidx.compose.ui.geometry.CornerRadius(4.dp.toPx()),
                                style = androidx.compose.ui.graphics.drawscope.Stroke(2.dp.toPx()))
                        }
                        var y = 3.dp.toPx()
                        while (y < size.height) {
                            drawRect(Color.Black.copy(alpha = 0.06f), androidx.compose.ui.geometry.Offset(0f, y), androidx.compose.ui.geometry.Size(size.width, 1f))
                            y += 4.dp.toPx()
                        }
                    }
                    model.cursor?.let { (fx, fy) ->
                        // The agent cursor and the bot's badge, from computer.action x/y over screen_w/screen_h.
                        val cx = with(density) { (ox + fx * dw).toDp() }
                        val cy = with(density) { (oy + fy * dh).toDp() }
                        val botColor = runCatching { Color(android.graphics.Color.parseColor(model.actor?.color ?: "")) }.getOrDefault(c.accentBrowser)
                        Box(Modifier.align(Alignment.TopStart).offset(cx - 6.dp, cy - 6.dp).size(12.dp).clip(CircleShape).background(botColor)
                            .border(2.dp, Color.White, CircleShape).testTag("aciAgentCursor"))
                        Text(model.botName, color = Color.White, fontSize = 10.sp, fontWeight = FontWeight.SemiBold,
                            modifier = Modifier.align(Alignment.TopStart).offset(cx + 8.dp, cy + 4.dp).clip(RoundedCornerShape(6.dp)).background(botColor)
                                .padding(horizontal = 6.dp, vertical = 2.dp).testTag("aciAgentBadge"))
                    }
                    val label = action?.label ?: action?.selector
                    val ax = action?.x
                    val ay = action?.y
                    if (!label.isNullOrEmpty() && ax != null && ay != null) {
                        val lx = with(density) { (ox + ax.toFloat() * scale).toDp() }
                        val ly = with(density) { max(oy + ay.toFloat() * scale - 20.dp.toPx(), 0f).toDp() }
                        Text((action?.type?.let { "${it.uppercase()} · " } ?: "") + label, color = color, fontSize = 9.sp, fontWeight = FontWeight.Bold,
                            fontFamily = FontFamily.Monospace, letterSpacing = 0.6.sp,
                            modifier = Modifier.align(Alignment.TopStart).offset(lx, ly).clip(RoundedCornerShape(4.dp)).background(c.bgSecondary)
                                .border(1.dp, color.copy(alpha = 0.4f), RoundedCornerShape(4.dp)).padding(horizontal = 6.dp, vertical = 2.dp))
                    }
                }
                model.traceLines.isEmpty() && model.status == AciRunStatus.Idle -> Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    CircularProgressIndicator(Modifier.size(28.dp), color = c.textSecondary, strokeWidth = 2.dp)
                    Text("CONNECTING", color = c.textSecondary.copy(alpha = 0.6f), fontSize = 10.sp, fontFamily = FontFamily.Monospace, letterSpacing = 3.sp)
                    OutlineButton("Close", c.textSecondary, "aciRunCloseConnecting") { onExit() }
                }
                else -> Column(Modifier.padding(horizontal = 24.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(
                        when (model.status) {
                            AciRunStatus.Done -> "TASK COMPLETE"
                            AciRunStatus.Blocked -> "RUN BLOCKED"
                            AciRunStatus.Error -> "RUN ERROR"
                            else -> "NO LIVE VIDEO"
                        },
                        color = c.textSecondary.copy(alpha = 0.6f), fontSize = 10.sp, fontFamily = FontFamily.Monospace, letterSpacing = 3.sp,
                    )
                    Text("This backend streams status and actions only — no screen video.", color = c.textSecondary.copy(alpha = 0.45f), fontSize = 10.sp,
                        fontFamily = FontFamily.Monospace, textAlign = TextAlign.Center)
                }
            }
        }
        val listState = rememberLazyListState()
        LaunchedEffect(model.traceLines.size) { if (model.traceLines.isNotEmpty()) listState.animateScrollToItem(model.traceLines.lastIndex) }
        LazyColumn(Modifier.fillMaxWidth().height(84.dp).background(c.bgSecondary).padding(horizontal = 12.dp, vertical = 6.dp), state = listState) {
            items(model.traceLines, key = { it.id }) { line ->
                Text(line.text, color = if (line.kind != null) actionColor(line.kind) else c.textSecondary, fontSize = 10.sp, fontFamily = FontFamily.Monospace, maxLines = 1)
            }
        }
        controls()
        if (model.status == AciRunStatus.Running && model.actor != null) {
            val n = model.actionStep ?: model.stepIndex
            Row(Modifier.fillMaxWidth().background(c.bgSecondary).padding(horizontal = 16.dp, vertical = 6.dp).testTag("aciWorkingLine"),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(model.botName + " is working" + (n?.let { " · step $it" } ?: ""), color = c.textSecondary, fontSize = 12.sp, maxLines = 1, modifier = Modifier.weight(1f))
                if (onTakeOver != null) OutlineButton("Take over", c.textPrimary, "aciRunTakeOver") { onTakeOver() }
                OutlineButton("Stop", Red, "aciRunStopWorking") { model.stop() }
            }
        }
        Row(Modifier.fillMaxWidth().background(c.bgSecondary).padding(horizontal = 16.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            if (model.requiresApproval) {
                OutlineButton("Deny", c.textSecondary, "aciRunDeny") { model.approve(deny = true) }
                Text("Approve", color = c.bgPrimary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.clip(RoundedCornerShape(8.dp)).background(AllternitStatic.StatusWarning).clickable { model.approve(deny = false) }
                        .padding(horizontal = 14.dp, vertical = 7.dp).testTag("aciRunApprove"))
            }
            Spacer(Modifier.weight(1f))
            if (model.isActive && !(model.status == AciRunStatus.Running && model.actor != null)) OutlineButton("■ Stop", Red, "aciRunStop") { model.stop() }
        }
    }
    RunGateHost(model)
}

@Composable
private fun RunGateHost(model: AciRunModel) {
    val g = model.gate ?: return
    AciRunGateSheet(
        gate = g, botName = model.botName,
        onApprove = { model.approveGate(it) }, onDeny = { model.denyGate() },
        onContinue = { model.continueGate() }, onStop = { model.stopGate() },
        onRerun = { model.rerun(it) }, onDismiss = { model.dismissGate() },
    )
}

@Composable
private fun Divider() = Box(Modifier.width(1.dp).height(12.dp).background(AllternitTheme.colors.borderSubtle))

@Composable
private fun OutlineButton(title: String, color: Color, tag: String, onClick: () -> Unit) {
    Text(title, color = color, fontSize = 12.sp, fontWeight = FontWeight.SemiBold,
        modifier = Modifier.clip(RoundedCornerShape(8.dp)).border(1.dp, color.copy(alpha = 0.4f), RoundedCornerShape(8.dp)).clickable { onClick() }
            .padding(horizontal = 14.dp, vertical = 7.dp).testTag(tag))
}

/**
 * The in-app browser (iOS ACIWebBrowserView): back, the search / URL field with a spinner, the
 * progress line, the page, and back / forward with Extension Chat, whose sheet starts an agent run.
 */
@OptIn(ExperimentalMaterial3Api::class)
@SuppressLint("SetJavaScriptEnabled")
@Composable
internal fun AciWebBrowser(initialUrl: String, onExit: () -> Unit, onStartAgentRun: (String) -> Unit, controls: @Composable () -> Unit = {}) {
    val c = AllternitTheme.colors
    var urlText by remember { mutableStateOf(initialUrl) }
    var loading by remember { mutableStateOf(false) }
    var progress by remember { mutableFloatStateOf(0f) }
    var canBack by remember { mutableStateOf(false) }
    var canForward by remember { mutableStateOf(false) }
    var web by remember { mutableStateOf<WebView?>(null) }
    var chatOpen by remember { mutableStateOf(false) }
    var task by remember { mutableStateOf("") }
    var runError by remember { mutableStateOf<String?>(null) }
    BackHandler { if (web?.canGoBack() == true) web?.goBack() else onExit() }
    Column(Modifier.fillMaxSize().background(c.bgPrimary).testTag("aciBrowser")) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Box(Modifier.size(32.dp).clip(CircleShape).clickable(onClickLabel = "Back") { onExit() }.testTag("aciBrowserExit"), contentAlignment = Alignment.Center) {
                Icon(painterResource(DesignR.drawable.ph_caret_left), null, tint = c.textSecondary, modifier = Modifier.size(16.dp))
            }
            Row(Modifier.weight(1f).height(36.dp).clip(RoundedCornerShape(8.dp)).background(c.bgSecondary).border(1.dp, c.borderSubtle, RoundedCornerShape(8.dp))
                .padding(horizontal = 10.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Icon(painterResource(DesignR.drawable.ph_magnifying_glass), null, tint = c.textSecondary, modifier = Modifier.size(12.dp))
                BasicTextField(urlText, { urlText = it }, singleLine = true, textStyle = TextStyle(color = c.textPrimary, fontSize = 14.sp), cursorBrush = SolidColor(c.textPrimary),
                    keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go),
                    keyboardActions = KeyboardActions(onGo = { web?.loadUrl(AciWebInput.resolve(urlText)) }),
                    modifier = Modifier.weight(1f).testTag("aciBrowserUrl"))
                if (loading) CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 1.5.dp)
            }
        }
        Box(Modifier.fillMaxWidth().height(2.dp)) {
            if (loading) Box(Modifier.fillMaxWidth(progress).height(2.dp).background(c.accentBrowser))
        }
        AndroidView(
            factory = { ctx ->
                WebView(ctx).apply {
                    settings.javaScriptEnabled = true
                    settings.domStorageEnabled = true
                    webViewClient = object : WebViewClient() {
                        override fun onPageStarted(view: WebView, url: String?, favicon: Bitmap?) {
                            loading = true
                            url?.let { urlText = it }
                        }
                        override fun onPageFinished(view: WebView, url: String?) {
                            loading = false
                            canBack = view.canGoBack()
                            canForward = view.canGoForward()
                            url?.let { urlText = it }
                        }
                        override fun doUpdateVisitedHistory(view: WebView, url: String?, isReload: Boolean) {
                            canBack = view.canGoBack()
                            canForward = view.canGoForward()
                        }
                    }
                    webChromeClient = object : WebChromeClient() {
                        override fun onProgressChanged(view: WebView, newProgress: Int) {
                            progress = newProgress / 100f
                        }
                    }
                    loadUrl(initialUrl)
                    web = this
                }
            },
            onRelease = { it.destroy() },
            modifier = Modifier.weight(1f).fillMaxWidth(),
        )
        controls()
        Row(Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            NavArrow(DesignR.drawable.ph_caret_left, "Back", canBack, "aciBrowserBack") { web?.goBack() }
            Box(Modifier.width(44.dp).height(36.dp).clickable(enabled = canForward, onClickLabel = "Forward") { web?.goForward() }.testTag("aciBrowserForward"), contentAlignment = Alignment.Center) {
                Icon(painterResource(DesignR.drawable.ph_arrow_right), null, tint = if (canForward) c.accentBrowser else c.textSecondary.copy(alpha = 0.4f), modifier = Modifier.size(16.dp))
            }
            Spacer(Modifier.weight(1f))
            Row(Modifier.clip(RoundedCornerShape(8.dp)).background(c.accentBrowser.copy(alpha = 0.14f)).border(1.dp, c.accentBrowser.copy(alpha = 0.35f), RoundedCornerShape(8.dp))
                .clickable { chatOpen = true }.padding(horizontal = 12.dp, vertical = 6.dp).testTag("aciExtensionChat"),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                Icon(painterResource(DesignR.drawable.ph_puzzle_piece), null, tint = c.accentBrowser, modifier = Modifier.size(14.dp))
                Text("Extension Chat", color = c.accentBrowser, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
            }
            Spacer(Modifier.weight(1f))
        }
    }
    if (chatOpen) {
        ModalBottomSheet(onDismissRequest = { chatOpen = false }, containerColor = c.bgPrimary) {
            Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 24.dp).testTag("aciExtensionSheet"), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text("What should the Computer Agent do?", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                Text("Describe a page automation task, and the agent will run ACI to automate this browser tab.", color = c.textSecondary, fontSize = 13.sp)
                Text("Suggested Tasks", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                listOf("Extract page contact details", "Find all links on this page", "Search for specific tutorials", "Auto-fill dummy data on this form").forEach { s ->
                    Text(s, color = c.textPrimary, fontSize = 13.sp, modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(c.bgSecondary)
                        .border(1.dp, c.borderSubtle, RoundedCornerShape(8.dp)).clickable { task = s }.padding(10.dp))
                }
                Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(c.bgSecondary).border(1.dp, c.borderSubtle, RoundedCornerShape(10.dp)).padding(start = 12.dp, end = 6.dp, top = 6.dp, bottom = 6.dp),
                    verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.weight(1f)) {
                        if (task.isEmpty()) Text("Describe task for the browser agent...", color = c.textTertiary, fontSize = 14.sp)
                        BasicTextField(task, { task = it }, textStyle = TextStyle(color = c.textPrimary, fontSize = 14.sp), cursorBrush = SolidColor(c.textPrimary), modifier = Modifier.fillMaxWidth().testTag("aciExtensionInput"))
                    }
                    Box(Modifier.size(32.dp).clip(CircleShape).background(if (task.isBlank()) c.fillNeutral else c.accentBrowser)
                        .clickable(enabled = task.isNotBlank(), onClickLabel = "Start agent run") {
                            val goal = task.trim()
                            chatOpen = false
                            task = ""
                            onStartAgentRun(goal)
                        }.testTag("aciExtensionSend"), contentAlignment = Alignment.Center) {
                        Icon(painterResource(DesignR.drawable.ph_arrow_up), null, tint = Color.White, modifier = Modifier.size(16.dp))
                    }
                }
            }
        }
    }
    runError?.let { msg ->
        AlertDialog(onDismissRequest = { runError = null }, confirmButton = { TextButton({ runError = null }) { Text("OK") } },
            title = { Text("Couldn't start the run") }, text = { Text(msg) })
    }
}

@Composable
private fun NavArrow(icon: Int, label: String, enabled: Boolean, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(Modifier.width(44.dp).height(36.dp).clickable(enabled = enabled, onClickLabel = label) { onClick() }.testTag(tag), contentAlignment = Alignment.Center) {
        Icon(painterResource(icon), null, tint = if (enabled) c.accentBrowser else c.textSecondary.copy(alpha = 0.4f), modifier = Modifier.size(16.dp))
    }
}
