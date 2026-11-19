package com.allternit.feature.chat

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull

/*
 * The agents of a reply (iOS Features/RunTree): AgentRunTreeSheet (the tree of subagents with
 * live status, spend, elapsed time, Stop / Pause / Resume), SubagentFaceView and the
 * gizzi session tasks it polls (`GET agent-sessions/:id/tasks`).
 */

/** The session the rail's run tree polls (provided by [ChatScreen]). */
val LocalRunTreeSession = compositionLocalOf<String?> { null }

/** iOS RunStatus. */
enum class RunStatus(val label: String) {
    Queued("Queued"), Running("Running"), Done("Done"), Failed("Failed"), Stopped("Stopped"), Interrupted("Interrupted");
    val isLive get() = this == Queued || this == Running
}

/** iOS RunNode. */
data class RunNode(
    val id: String,
    val name: String,
    val slug: String? = null,
    val model: String? = null,
    val status: RunStatus,
    val task: String,
    val cost: Double? = null,
    val tokens: Int = 0,
    val startedAt: Long? = null,
    val finishedAt: Long? = null,
    val error: String? = null,
    val taskId: String? = null,
    val path: String? = null,
    val runs: Int = 1,
    val isControllable: Boolean = false,
    val children: List<RunNode> = emptyList(),
) {
    val descendantCount: Int get() = children.sumOf { 1 + it.descendantCount }
    fun walk(depth: Int = 0, visit: (RunNode, Int) -> Unit) { visit(this, depth); children.forEach { it.walk(depth + 1, visit) } }
    fun elapsedMs(now: Long): Long? = startedAt?.let { (finishedAt ?: now) - it }
}

/** iOS RunFormat. */
object RunFormat {
    fun cost(c: Double): String = when { c <= 0 -> "$0"; c < 0.01 -> "<$0.01"; else -> String.format(java.util.Locale.US, "$%.2f", c) }
    fun tokens(n: Int): String = when {
        n < 1000 -> "$n tok"
        n < 1_000_000 -> if (n < 10_000) String.format(java.util.Locale.US, "%.1fk tok", n / 1000.0) else "${n / 1000}k tok"
        else -> String.format(java.util.Locale.US, "%.1fM tok", n / 1_000_000.0)
    }
    fun spend(cost: Double?, tokens: Int): String? = when {
        cost != null && cost > 0 -> cost(cost)
        tokens > 0 -> tokens(tokens)
        cost == 0.0 -> "$0"
        else -> null
    }
    fun elapsed(ms: Long): String {
        val s = (ms / 1000.0).let { Math.round(it).toInt() }.coerceAtLeast(0)
        if (s < 60) return "${s}s"
        val m = s / 60
        return if (m < 60) String.format(java.util.Locale.US, "%dm %02ds", m, s % 60) else "${m / 60}h ${m % 60}m"
    }
    fun modelLabel(id: String?): String? {
        if (id.isNullOrEmpty()) return null
        var name = id.substringAfterLast('/').replace("::", "-").removePrefix("claude-").removeSuffix("-latest").replace(Regex("-\\d{8}$"), "")
        val upper = setOf("gpt", "glm", "llm")
        val out = mutableListOf<String>()
        for (part in name.split(Regex("[-_ ]")).filter { it.isNotEmpty() }) {
            val prev = out.lastOrNull()
            when {
                part.all { it.isDigit() } && prev != null && prev.last().isDigit() -> out[out.lastIndex] = "$prev.$part"
                part.first().isDigit() && prev != null && prev.lowercase() in upper -> out[out.lastIndex] = "$prev-$part"
                part.lowercase() in upper -> out.add(part.uppercase())
                else -> out.add(part.replaceFirstChar { it.uppercase() })
            }
        }
        name = out.joinToString(" ")
        return name.ifEmpty { null }
    }
}

/** iOS SubagentNames: "code-reviewer" → "Code reviewer". */
object SubagentNames {
    fun slug(raw: String?): String? = raw?.trim()?.lowercase()?.replace(' ', '-')?.takeIf { it.isNotEmpty() }
    fun title(raw: String?): String = slug(raw)?.replace('-', ' ')?.replaceFirstChar { it.uppercase() } ?: "Agent"
}

/** One gizzi session task (iOS SessionTask). */
private data class SessionTask(
    val id: String, val kind: String, val status: String, val description: String, val agent: String?, val childSessionId: String?,
    val name: String?, val path: String?, val runs: Int, val background: Boolean, val error: String?, val created: Double, val finished: Double?,
    val model: String?, val cost: Double?, val tokens: Int, val children: List<SessionTask>,
) {
    val runStatus get() = when (status) {
        "queued" -> RunStatus.Queued; "running" -> RunStatus.Running; "completed" -> RunStatus.Done
        "cancelled" -> RunStatus.Stopped; "interrupted" -> RunStatus.Interrupted; else -> RunStatus.Failed
    }
    val isLive get() = status == "queued" || status == "running"
    val isControllable get() = background && kind == "subagent" && childSessionId != null

    companion object {
        fun from(o: JsonObject): SessionTask? {
            fun s(k: String) = (o[k] as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotEmpty() }
            val time = o["time"] as? JsonObject
            val tokens = o["tokens"] as? JsonObject
            fun n(obj: JsonObject?, k: String) = (obj?.get(k) as? JsonPrimitive)?.contentOrNull?.toDoubleOrNull()
            return SessionTask(
                s("id") ?: return null, s("kind") ?: "subagent", s("status") ?: "running", s("description").orEmpty(), s("agent"), s("childSessionID"),
                s("name"), s("path"), (n(o, "runs") ?: 1.0).toInt().coerceAtLeast(1), s("background") == "true", s("error"),
                n(time, "created") ?: 0.0, n(time, "finished"), s("model"), n(o, "cost"),
                ((n(tokens, "input") ?: 0.0) + (n(tokens, "output") ?: 0.0)).toInt(),
                (o["children"] as? JsonArray)?.mapNotNull { (it as? JsonObject)?.let(::from) }.orEmpty(),
            )
        }
    }
}

/** iOS RunTreeBuilder. */
private object RunTreeBuilder {
    fun subagents(tasks: List<SessionTask>) = tasks.filter { it.kind == "subagent" }.sortedBy { it.created }

    fun node(t: SessionTask): RunNode = RunNode(
        id = t.id, name = SubagentNames.title(t.agent), slug = SubagentNames.slug(t.agent), model = RunFormat.modelLabel(t.model),
        status = t.runStatus, task = t.description, cost = t.cost, tokens = t.tokens,
        startedAt = if (t.created > 0) t.created.toLong() else null, finishedAt = t.finished?.toLong(), error = t.error,
        taskId = t.id, path = t.path ?: t.name, runs = t.runs, isControllable = t.isControllable, children = subagents(t.children).map(::node),
    )

    private fun norm(s: String) = s.trim().lowercase().split(Regex("\\s+")).joinToString(" ")

    fun tree(rootName: String, steps: List<ToolCallRecord>, tasks: List<SessionTask>): RunNode {
        val pool = subagents(tasks)
        val agentSteps = steps.filter { isAgentStep(it) }
        val children: List<RunNode> = if (agentSteps.isEmpty()) pool.map(::node) else {
            val used = mutableSetOf<String>()
            agentSteps.map { step ->
                val description = (step.input?.get("description") as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotEmpty() } ?: step.title.orEmpty()
                val agent = ((step.input?.get("subagent_type") ?: step.input?.get("agent")) as? JsonPrimitive)?.contentOrNull.orEmpty().let { if (it == "agent") "" else it }
                val match = pool.firstOrNull { t -> t.id !in used && norm(t.description) == norm(description) && (agent.isEmpty() || t.agent.isNullOrEmpty() || t.agent == agent) }
                if (match != null) {
                    used.add(match.id)
                    var n = node(match)
                    if (step.state == ToolState.Running && !n.status.isLive && n.status != RunStatus.Interrupted) n = n.copy(status = RunStatus.Running)
                    if (step.state == ToolState.Failed && n.status == RunStatus.Done) n = n.copy(status = RunStatus.Failed, error = n.error ?: step.error)
                    n = n.copy(id = step.id)
                    if (agent.isNotEmpty()) n = n.copy(name = SubagentNames.title(agent), slug = SubagentNames.slug(agent))
                    n
                } else RunNode(
                    id = step.id, name = SubagentNames.title(agent.ifEmpty { null }), slug = SubagentNames.slug(agent.ifEmpty { null }),
                    status = when (step.state) { ToolState.Running -> RunStatus.Running; ToolState.Failed -> RunStatus.Failed; ToolState.Done -> RunStatus.Done },
                    task = description, startedAt = step.startedAt, finishedAt = step.finishedAt, error = step.error,
                )
            }
        }
        val live = children.any { it.status.isLive }
        return RunNode(
            id = "root", name = rootName, status = if (live) RunStatus.Running else RunStatus.Done, task = "",
            startedAt = children.mapNotNull { it.startedAt }.minOrNull(), finishedAt = if (live) null else children.mapNotNull { it.finishedAt }.maxOrNull(), children = children,
        )
    }

    fun retryPrompt(n: RunNode): String {
        val who = if (n.name.isEmpty() || n.name == "Agent") "the agent" else "the ${n.name} agent"
        return "Run $who again for this task: \"${n.task}\".${n.error?.let { " The last attempt failed: $it" } ?: ""}"
    }
}

/** A tool call that handed work to another agent (iOS ToolCallRecord.isAgentStep). */
fun isAgentStep(call: ToolCallRecord): Boolean = call.toolName.lowercase() in setOf("task", "agent", "subagent", "spawn_agent")

/** iOS SubagentCatalog faces: built-in roles keep their face; others hash onto the seed faces. */
private object SubagentFaces {
    private val builtin = mapOf(
        "explorer" to "kest", "builder" to "daniel", "code-reviewer" to "maya", "verifier" to "ray", "web-researcher" to "saanvi",
        "browser-operator" to "li-wei", "computer-operator" to "mara", "data-analyst" to "amir", "ui-designer" to "kira",
        "release-engineer" to "grant", "security-reviewer" to "jonah", "planner" to "theo", "debugger" to "nia", "docs-writer" to "imani", "scribe" to "elena",
    )
    private val aliases = mapOf("general" to "builder", "general-purpose" to "builder", "worker" to "builder", "default" to "builder", "explore" to "explorer", "verification" to "verifier")
    private val seeds = listOf("kest", "theo", "daniel", "maya", "ray", "nia", "saanvi", "li-wei", "mara", "imani", "amir", "kira", "grant", "jonah", "elena")

    fun url(raw: String?, working: Boolean): String {
        val slug = (SubagentNames.slug(raw) ?: "builder").let { aliases[it] ?: it }
        val face = builtin[slug] ?: seeds[(fnv(slug) % seeds.size.toUInt()).toInt()]
        return "https://ai.allternit.com/avatar-packs/office-professionals/$face${if (working) "-working" else ""}.png"
    }

    private fun fnv(s: String): UInt {
        var h = 2166136261u
        for (b in s.toByteArray()) { h = h xor (b.toInt() and 0xff).toUInt(); h *= 16777619u }
        return h
    }
}

private object FaceCache {
    private val http by lazy { okhttp3.OkHttpClient() }
    private val cache = android.util.LruCache<String, ImageBitmap>(32)
    suspend fun load(url: String): ImageBitmap? = cache.get(url) ?: withContext(Dispatchers.IO) {
        runCatching {
            http.newCall(okhttp3.Request.Builder().url(url).build()).execute().use { r ->
                if (!r.isSuccessful) null else r.body?.bytes()?.let { android.graphics.BitmapFactory.decodeByteArray(it, 0, it.size)?.asImageBitmap() }
            }
        }.getOrNull()?.also { cache.put(url, it) }
    }
}

/** A subagent's face (iOS SubagentFaceView): its office portrait, the working pose while it runs, initials until it loads. */
@Composable
fun SubagentFace(subagent: String?, name: String, running: Boolean, size: Dp = 30.dp) {
    val c = AllternitTheme.colors
    val url = SubagentFaces.url(subagent, running)
    val bmp by produceState<ImageBitmap?>(null, url) { value = FaceCache.load(url) ?: if (running) FaceCache.load(SubagentFaces.url(subagent, false)) else null }
    Box(Modifier.size(size).clip(CircleShape).background(c.bgSecondary).semantics { contentDescription = "$name${if (running) ", working" else ""}" }, contentAlignment = Alignment.Center) {
        val b = bmp
        if (b != null) Image(b, null, contentScale = ContentScale.Crop, modifier = Modifier.size(size))
        else Text(name.take(1).uppercase(), color = c.textSecondary, fontSize = (size.value * 0.42f).sp, fontWeight = FontWeight.SemiBold)
    }
}

/**
 * The agents sheet (iOS AgentRunTreeSheet): the totals line, the root and its branches, each with
 * face, name, count, status (a failed one opens its reason with Retry), path, model, task, spend and
 * elapsed time, and Stop / Pause / Resume for background subagents. Polls every 2 s while one runs.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AgentRunTreeSheet(steps: List<ToolCallRecord>, sessionId: String?, isStreaming: Boolean, backend: ChatBackend?, rootName: String = "Main agent", onRetry: ((String) -> Unit)? = null, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var tasks by remember { mutableStateOf<List<SessionTask>>(emptyList()) }
    var loaded by remember { mutableStateOf(sessionId == null || backend == null) }
    var loadError by remember { mutableStateOf<String?>(null) }
    var actionError by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(setOf<String>()) }
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }

    suspend fun refresh() {
        val sid = sessionId ?: return
        val b = backend ?: return
        when (val r = b.sessionTasks(sid)) {
            is ApiResult.Ok -> { tasks = ((r.value as? JsonArray) ?: ((r.value as? JsonObject)?.get("tasks") as? JsonArray))?.mapNotNull { (it as? JsonObject)?.let(SessionTask::from) }.orEmpty(); loadError = null }
            else -> if (tasks.isEmpty()) loadError = "Couldn't load the agents' status."
        }
        loaded = true
    }
    LaunchedEffect(sessionId) {
        while (true) {
            refresh()
            val live = tasks.any { it.isLive || it.children.any { ch -> ch.isLive } }
            delay(if (live) 2_000 else 15_000)
        }
    }
    LaunchedEffect(Unit) { while (true) { delay(1000); now = System.currentTimeMillis() } }

    fun control(action: String, node: RunNode) {
        val sid = sessionId ?: return
        val b = backend ?: return
        val taskId = node.taskId ?: return
        if (taskId in busy) return
        busy = busy + taskId
        scope.launch {
            val seg = when (action) { "stop" -> "cancel"; "pause" -> "interrupt"; else -> "resume" }
            val body = when (action) { "pause" -> "{\"reason\":\"Paused from the Android app\"}"; else -> "{}" }
            actionError = when (b.backgroundTask(sid, taskId, seg, body)) {
                is ApiResult.Ok -> null
                else -> "Couldn't ${if (action == "stop") "stop" else action} the agent. Try again."
            }
            refresh()
            busy = busy - taskId
        }
    }

    val tree = RunTreeBuilder.tree(rootName, steps, tasks)
    val state = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = state, containerColor = c.bgPrimary, shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp)) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().testTag("agentRunTree")) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 20.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("Agents", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                Text("Done", color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.testTag("agentRunTreeDone").clickable(onClick = onDismiss).padding(8.dp))
            }
            Column(Modifier.fillMaxWidth().heightIn(max = 640.dp).verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                var agents = 0; var running = 0; var interrupted = 0; var failed = 0; var cost: Double? = null; var tokens = 0
                tree.walk { n, d ->
                    if (d > 0) { agents++; if (n.status.isLive) running++; if (n.status == RunStatus.Interrupted) interrupted++; if (n.status == RunStatus.Failed) failed++ }
                    n.cost?.let { cost = (cost ?: 0.0) + it }; tokens += n.tokens
                }
                val parts = mutableListOf("$agents ${if (agents == 1) "agent" else "agents"}")
                if (running > 0) parts.add("$running running")
                if (interrupted > 0) parts.add("$interrupted interrupted")
                if (failed > 0) parts.add("$failed failed")
                RunFormat.spend(cost, tokens)?.let { parts.add(it) }
                Text(parts.joinToString(" · "), color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.testTag("agentRunTreeTotals"))
                if (tree.children.isEmpty()) {
                    Column(Modifier.fillMaxWidth().padding(vertical = 28.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        if (!loaded) { CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp, color = c.textSecondary); Text("Loading agents…", color = c.textSecondary, fontSize = 14.sp) }
                        else Text(if (isStreaming) "No agents started yet." else "This reply didn't hand work to other agents.", color = c.textSecondary, fontSize = 14.sp)
                    }
                } else {
                    RunCard(tree, true, now, null, null, false, null)
                    tree.children.forEach { RunBranch(it, now, onRetry?.let { r -> { n: RunNode -> r(RunTreeBuilder.retryPrompt(n)); onDismiss() } }, if (sessionId != null) ::control else null, busy) }
                }
                actionError?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp) }
                if (tree.children.isEmpty()) loadError?.let { Text(it, color = c.textTertiary, fontSize = 12.sp) }
            }
        }
    }
}

@Composable
private fun RunBranch(node: RunNode, now: Long, onRetry: ((RunNode) -> Unit)?, control: ((String, RunNode) -> Unit)?, busy: Set<String>) {
    var open by remember { mutableStateOf(true) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        RunCard(node, false, now, onRetry, if (node.children.isEmpty()) null else ({ open = !open }), node.taskId in busy, control)
        if (open && node.children.isNotEmpty()) Row {
            Box(Modifier.padding(start = 14.dp).width(2.dp).heightIn(min = 20.dp).background(AllternitTheme.colors.borderSubtle))
            Column(Modifier.padding(start = 10.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                node.children.forEach { RunBranch(it, now, onRetry, control, busy) }
            }
        }
    }
}

@Composable
private fun RunCard(node: RunNode, isRoot: Boolean, now: Long, onRetry: ((RunNode) -> Unit)?, onToggle: (() -> Unit)?, isBusy: Boolean, control: ((String, RunNode) -> Unit)?) {
    val c = AllternitTheme.colors
    var showsFailure by remember { mutableStateOf(false) }
    val tone = when (node.status) {
        RunStatus.Running, RunStatus.Queued -> c.accentPrimary
        RunStatus.Failed -> AllternitStatic.StatusWarning
        RunStatus.Stopped -> c.textTertiary
        else -> c.textSecondary
    }
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(14.dp)).padding(12.dp).testTag("runNode${node.id}"),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (isRoot) Box(Modifier.size(30.dp).clip(CircleShape).background(c.bgSecondary), contentAlignment = Alignment.Center) { Text("◇", color = c.textSecondary) }
            else SubagentFace(node.slug ?: node.name, node.name, node.status == RunStatus.Running)
            Text(node.name, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            if (onToggle != null && node.descendantCount > 0) Text("${node.descendantCount} ▾", color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.clickable(onClick = onToggle).padding(4.dp))
            Spacer(Modifier.weight(1f))
            Row(
                Modifier.clip(CircleShape).background(tone.copy(alpha = 0.12f)).then(if (node.status == RunStatus.Failed) Modifier.clickable { showsFailure = !showsFailure } else Modifier).padding(horizontal = 8.dp, vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                if (node.status == RunStatus.Running) CircularProgressIndicator(Modifier.size(10.dp), strokeWidth = 1.5.dp, color = tone)
                Text(node.status.label + if (node.status == RunStatus.Failed) " ▾" else "", color = tone, fontSize = 12.sp, fontWeight = FontWeight.Medium)
            }
        }
        node.path?.let { p ->
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(p, color = c.textTertiary, fontSize = 11.5.sp, fontFamily = FontFamily.Monospace)
                if (node.runs > 1) Text("${node.runs} runs", color = c.textTertiary, fontSize = 11.5.sp)
            }
        }
        node.model?.let { Text(it, color = c.textTertiary, fontSize = 12.sp) }
        if (node.task.isNotEmpty()) Text(node.task, color = c.textSecondary, fontSize = 13.sp, maxLines = 4)
        val money = if (isRoot) {
            var cost: Double? = null; var tok = 0
            node.walk { n, _ -> n.cost?.let { cost = (cost ?: 0.0) + it }; tok += n.tokens }
            RunFormat.spend(cost, tok)
        } else RunFormat.spend(node.cost, node.tokens)
        val elapsed = node.elapsedMs(now)?.takeIf { it >= 1000 }?.let(RunFormat::elapsed)
        if (money != null || elapsed != null) Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            money?.let { Text(it, color = c.textTertiary, fontSize = 12.sp) }
            elapsed?.let { Text(it, color = c.textTertiary, fontSize = 12.sp) }
        }
        if (control != null && node.isControllable && node.taskId != null && (node.status.isLive || node.status == RunStatus.Interrupted)) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                if (node.status == RunStatus.Interrupted) CapsuleButton("Resume", enabled = !isBusy) { control("resume", node) }
                else CapsuleButton("Pause", enabled = !isBusy) { control("pause", node) }
                CapsuleButton("Stop", enabled = !isBusy) { control("stop", node) }
                if (isBusy) CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 1.5.dp, color = c.textSecondary)
            }
        }
        if (showsFailure) Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(AllternitStatic.StatusWarning.copy(alpha = 0.1f)).padding(10.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(node.error ?: "It stopped with an error and gave no reason.", color = c.textPrimary, fontSize = 13.sp)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                CapsuleButton("Dismiss") { showsFailure = false }
                if (onRetry != null) CapsuleButton("Retry", primary = true) { onRetry(node) }
            }
        }
    }
}
