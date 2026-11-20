package com.allternit.feature.automation

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.core.api.ApiResult
import com.allternit.core.design.FriendlyErrorMessage
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import com.allternit.feature.bots.BotsApi
import com.allternit.feature.bots.BotsResult
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.RuntimeCall
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject

/*
 * Automation data (iOS AutomationClient, RoutinesClient, LoopsClient, CronClient, TaskBoardClient and
 * the AutomationStore). Goals, routines and loops are `/api/v1/automation/…` on allternit-api (LibraryNet,
 * like the bots wave); the computer's own routines, loops and heartbeats (`v1/automations/…`, `v1/cron/…`)
 * and the Factory board (`api/factory/tasks/board`) ride the relay to the user's computer. A phone with
 * no computer gets [AResult.NoComputer], never a raw error.
 */

sealed interface AResult<out T> {
    data class Ok<T>(val value: T) : AResult<T>
    /** No computer on the account (428 / compute_required): an empty state, not an error. */
    data object NoComputer : AResult<Nothing>
    data class Failed(val message: String, val status: Int? = null) : AResult<Nothing>
}

internal fun <T> AResult<T>.orNull(): T? = (this as? AResult.Ok)?.value

internal suspend fun v1(method: String, path: String, body: JsonElement? = null, doing: String): AResult<JsonElement> {
    val r = if (method == "GET") LibraryNet.get(path) else LibraryNet.send(method, path, body)
    return when (r) {
        is ApiResult.Ok -> AResult.Ok(r.value)
        is ApiResult.HttpError -> if (r.status == 428 || r.code == "compute_required") AResult.NoComputer else AResult.Failed(r.friendly(doing), r.status)
        else -> AResult.Failed(r.friendly(doing))
    }
}

internal suspend fun ChatBackend.relay(method: String, path: String, body: JsonElement? = null, doing: String): AResult<JsonElement> =
    when (val r = runtimeJson(method, path, body?.toString())) {
        is RuntimeCall.Ok -> AResult.Ok(r.body)
        RuntimeCall.NoComputer -> AResult.NoComputer
        is RuntimeCall.Http -> AResult.Failed(FriendlyErrorMessage.from(r.message ?: r.code ?: "HTTP ${r.status}", doing), r.status)
        is RuntimeCall.Failed -> AResult.Failed(r.message)
    }

internal fun esc(s: String): String = LibraryNet.escape(s)
internal fun tz(): String = java.util.TimeZone.getDefault().id
private fun JsonObject?.s(vararg keys: String): String? = keys.firstNotNullOfOrNull { str(it) }
private fun JsonObject?.n(vararg keys: String): Double? = keys.firstNotNullOfOrNull { num(it) }

// MARK: - Records

/** iOS AutomationTaskItem.Kind. */
enum class TaskType(val label: String, val plural: String) {
    Goal("Goal", "Goals"), Routine("Routine", "Routines"), Loop("Loop", "Loops"), Heartbeat("Heartbeat", "Agent Heartbeats")
}

/** One run in a routine's strip (iOS AutomationRunTick). */
data class RunTick(val id: String, val status: String, val startedAt: String?, val durationMs: Double?, val reason: String?) {
    /** A heartbeat check that found nothing to tell. */
    val isQuiet: Boolean get() = status == "skipped" && reason == "quiet"
}

/** A goal, routine, loop or heartbeat as one row (iOS AutomationTaskItem). */
data class AutoTask(
    val type: TaskType,
    val id: String,
    val name: String,
    val description: String,
    val status: String,
    val scheduleType: String,
    val scheduleExpression: String,
    val agentId: String?,
    val goalId: String?,
    val nextRunAt: String?,
    val lastRunAt: String?,
    val createdAt: String?,
    val runs: List<RunTick>,
    val instruction: String,
    /** "message" (a reminder) or "heartbeat" (a check) on tasks the agent made. */
    val kind: String? = null,
    val madeByAgent: Boolean = false,
    val progress: Double? = null,
    val priority: String? = null,
    val targetDate: String? = null,
) {
    val key: String get() = "${type.name}/$id"
    val isActive: Boolean get() = status == "active"
    val isCron: Boolean get() = scheduleType == "cron"
    val canRun: Boolean get() = type != TaskType.Goal
    val badge: String? get() = when (kind) { "message" -> "Reminder"; "heartbeat" -> "Check"; else -> null }
}

/** `GET …/routines/:id/live`: the run in progress. */
data class LiveRun(val running: Boolean, val runId: String?, val steps: List<String>, val text: String?, val canStop: Boolean)

/** A run of a loop (`GET …/loops/:id/runs`). */
data class RunRecord(val id: String, val status: String, val scheduledAt: String?, val durationMs: Double?, val output: String?, val error: String?)

data class BotLite(val id: String, val name: String)

/** The computer's own routine (`v1/automations/routines`, iOS Routine). */
data class LocalRoutine(val id: String, val name: String, val steps: List<Pair<String, String>>, val trigger: String?, val schedule: String?, val state: String, val updated: Double)

data class LoopIteration(val iteration: Int, val output: String, val exitCode: Int, val timestamp: String)
data class LoopLedgerRow(val iteration: Int, val hypothesis: String, val verdict: String, val outcome: String)
data class LoopMetric(val command: String, val direction: String, val keepBar: Double, val minPairs: Int)

/** The computer's own loop (`v1/automations/loops`, iOS Loop). */
data class LocalLoop(
    val id: String, val command: String, val exitCondition: String?, val maxIterations: Int, val iterations: List<LoopIteration>,
    val state: String, val updated: Double, val metric: LoopMetric?, val ledger: List<LoopLedgerRow>,
)

/** A card on the Tasks board (iOS TaskBoardCard). */
data class BoardCard(val dagId: String, val nodeId: String, val title: String, val status: String, val assignee: String?, val proven: Int, val total: Int, val needsYou: Boolean) {
    val key: String get() = "$dagId/$nodeId"
    val column: BoardColumn get() = when (status) {
        "new" -> BoardColumn.Backlog
        "working" -> BoardColumn.InProgress
        "checking", "needs_you" -> BoardColumn.InReview
        "done", "failed" -> BoardColumn.Done
        else -> BoardColumn.Todo
    }
    val detailLine: String get() = listOfNotNull(
        if (needsYou) "Needs you" else null,
        if (status == "failed") "Failed" else null,
        assignee?.let { if (it.startsWith("user:")) it.drop(5) else it } ?: "Unassigned",
        if (total > 0) "proof $proven/$total" else null,
    ).joinToString(" · ")
}

enum class BoardColumn(val label: String) { InReview("In review"), InProgress("In progress"), Todo("To do"), Backlog("Backlog"), Done("Done") }

// MARK: - Parsing

private fun JsonObject.ticks(): List<RunTick> =
    (list("recentRuns").ifEmpty { list("recent_runs") }).map {
        RunTick(it.str("id").orEmpty(), it.str("status").orEmpty(), it.s("startedAt", "started_at"), it.n("durationMs", "duration_ms"), it.str("reason"))
    }

private fun JsonObject.promptOf(): String = child("config").str("prompt")?.takeIf { it.isNotEmpty() } ?: str("description").orEmpty()

internal fun parseRoutine(o: JsonObject, type: TaskType = TaskType.Routine): AutoTask? {
    val id = o.str("id") ?: return null
    return AutoTask(
        type, id, o.str("name").orEmpty(), o.str("description").orEmpty(), o.str("status") ?: "active",
        o.s("scheduleType", "schedule_type") ?: "cron", o.s("scheduleExpression", "schedule_expression").orEmpty(),
        o.s("agentId", "agent_id"), o.s("goalId", "goal_id"), o.s("nextRunAt", "next_run_at"), o.s("lastRunAt", "last_run_at"),
        o.s("createdAt", "created_at"), o.ticks(), o.promptOf(), o.child("config").str("kind"),
        o.child("metadata").str("origin") == "agent",
    )
}

internal fun parseGoal(o: JsonObject): AutoTask? {
    val id = o.str("id") ?: return null
    return AutoTask(
        TaskType.Goal, id, o.str("title").orEmpty(), o.str("description").orEmpty(), o.str("status") ?: "active", "manual", "",
        o.s("agentId", "agent_id"), null, null, null, o.s("createdAt", "created_at"), emptyList(), "",
        progress = o.n("progress") ?: 0.0, priority = o.str("priority"), targetDate = o.s("targetDate", "target_date"),
    )
}

/** A cron job on the computer (iOS CronJob): the schedule is a string or an object with the expression. */
internal fun parseHeartbeat(o: JsonObject): AutoTask? {
    val id = o.str("id") ?: return null
    val raw = o["schedule"]
    val expr = when (raw) {
        is JsonPrimitive -> raw.content
        is JsonObject -> raw.s("expression", "cron", "value", "expr") ?: raw.n("seconds", "everySeconds")?.let { "${it.toLong()}s" }.orEmpty()
        else -> ""
    }
    val cfg = o.child("config")
    return AutoTask(
        TaskType.Heartbeat, id, o.str("name").orEmpty(), o.str("description").orEmpty(), o.str("status") ?: "active",
        if (raw is JsonObject && raw.str("type") == "interval") "interval" else "cron", expr,
        cfg.s("agentId", "agent_id"), null, o.s("nextRunAt", "next_run_at"), o.s("lastRunAt", "last_run_at"), o.s("createdAt", "created_at"),
        emptyList(), cfg.str("prompt").orEmpty(),
    )
}

internal fun parseLocalRoutine(o: JsonObject): LocalRoutine? {
    val id = o.str("id") ?: return null
    return LocalRoutine(id, o.str("name").orEmpty(), o.list("steps").map { it.str("command").orEmpty() to (it.str("status") ?: "pending") }, o.str("trigger"), o.str("schedule"), o.str("state") ?: "idle", o.n("time_updated", "timeUpdated") ?: 0.0)
}

internal fun parseLocalLoop(o: JsonObject): LocalLoop? {
    val id = o.str("id") ?: return null
    val m = o.child("metric")?.let { LoopMetric(it.str("command").orEmpty(), it.str("direction") ?: "lower", it.n("keepBar", "keep_bar") ?: 0.0, it.n("minPairs", "min_pairs")?.toInt() ?: 2) }
    return LocalLoop(
        id, o.str("command").orEmpty(), o.s("exit_condition", "exitCondition"), o.n("max_iterations", "maxIterations")?.toInt() ?: 0,
        o.list("iteration_log").ifEmpty { o.list("iterationLog") }.map { LoopIteration(it.n("iteration")?.toInt() ?: 0, it.str("output").orEmpty(), it.n("exitCode", "exit_code")?.toInt() ?: 0, it.str("timestamp").orEmpty()) },
        o.str("state") ?: "idle", o.n("time_updated", "timeUpdated") ?: 0.0, m,
        o.list("ledger").map { LoopLedgerRow(it.n("iteration")?.toInt() ?: 0, it.str("hypothesis").orEmpty(), it.str("verdict").orEmpty(), it.str("outcome").orEmpty()) },
    )
}

// MARK: - Calls

/** The `/api/v1/automation/…` routes plus the relay routes, one object per backend (iOS AutomationClient and friends). */
class AutomationApi(val backend: ChatBackend) {
    suspend fun routines(): AResult<List<AutoTask>> = v1("GET", "automation/routines?recent_runs=14", doing = "load routines").mapList("routines") { parseRoutine(it) }
    suspend fun loops(): AResult<List<AutoTask>> = v1("GET", "automation/loops?recent_runs=14", doing = "load loops").mapList("loops") { parseRoutine(it, TaskType.Loop) }
    suspend fun goals(): AResult<List<AutoTask>> = v1("GET", "automation/goals", doing = "load goals").mapList("goals") { parseGoal(it) }
    suspend fun heartbeats(): AResult<List<AutoTask>> =
        backend.relay("GET", "v1/cron/jobs", doing = "load heartbeats").mapList("jobs") { o -> if (o.str("type").let { it == null || it == "agent" }) parseHeartbeat(o) else null }

    private fun <T> AResult<JsonElement>.mapList(key: String, f: (JsonObject) -> T?): AResult<List<T>> = when (this) {
        is AResult.Ok -> AResult.Ok(value.listUnder(key).mapNotNull(f))
        AResult.NoComputer -> AResult.NoComputer
        is AResult.Failed -> this
    }

    private fun path(t: AutoTask): String = when (t.type) {
        TaskType.Routine -> "automation/routines/${esc(t.id)}"
        TaskType.Loop -> "automation/loops/${esc(t.id)}"
        TaskType.Goal -> "automation/goals/${esc(t.id)}"
        TaskType.Heartbeat -> "v1/cron/jobs/${esc(t.id)}"
    }

    private suspend fun call(t: AutoTask, method: String, suffix: String, body: JsonElement?, doing: String): AResult<JsonElement> =
        if (t.type == TaskType.Heartbeat) backend.relay(method, path(t) + suffix, body, doing) else v1(method, path(t) + suffix, body, doing)

    suspend fun runNow(t: AutoTask) = call(t, "POST", "/run", null, "start ${t.name}")

    suspend fun setEnabled(t: AutoTask, enabled: Boolean): AResult<JsonElement> = when (t.type) {
        TaskType.Heartbeat -> call(t, "POST", if (enabled) "/resume" else "/pause", null, "change ${t.name}")
        else -> call(t, "PUT", "", buildJsonObject { put("status", if (enabled) "active" else "paused") }, "change ${t.name}")
    }

    suspend fun setCron(t: AutoTask, cron: String): AResult<JsonElement> = when (t.type) {
        TaskType.Routine -> call(t, "PUT", "", buildJsonObject { put("schedule_type", "cron"); put("schedule_expression", cron); put("timezone", tz()) }, "save the schedule")
        TaskType.Heartbeat -> call(t, "PUT", "", buildJsonObject { put("schedule", cron) }, "save the schedule")
        else -> call(t, "PUT", "", buildJsonObject { put("schedule_expression", cron) }, "save the schedule")
    }

    suspend fun setGoal(t: AutoTask, status: String?, progress: Double?): AResult<JsonElement> =
        call(t, "PUT", "", buildJsonObject { status?.let { put("status", it) }; progress?.let { put("progress", it) } }, "update the goal")

    suspend fun delete(t: AutoTask) = call(t, "DELETE", "", null, "delete ${t.name}")

    suspend fun live(id: String): AResult<LiveRun> = when (val r = v1("GET", "automation/routines/${esc(id)}/live", doing = "check the run")) {
        is AResult.Ok -> r.value.obj.let { o ->
            AResult.Ok(LiveRun(o.bool("running") == true, o.s("runId", "run_id"), o.list("steps").mapNotNull { it.str("content") }, o.str("text"), o.bool("canStop") == true))
        }
        AResult.NoComputer -> AResult.NoComputer
        is AResult.Failed -> r
    }

    suspend fun cancel(routineId: String, runId: String) = v1("POST", "automation/routines/${esc(routineId)}/runs/${esc(runId)}/cancel", doing = "stop the run")

    suspend fun loopRuns(id: String): AResult<List<RunRecord>> = v1("GET", "automation/loops/${esc(id)}/runs", doing = "load runs").mapList("runs") {
        RunRecord(it.str("id") ?: return@mapList null, it.str("status").orEmpty(), it.s("scheduledAt", "scheduled_at"), it.n("durationMs", "duration_ms"), it.str("output"), it.str("error"))
    }

    suspend fun createRoutine(name: String, prompt: String, agentId: String, cron: String, goalId: String? = null) = v1("POST", "automation/routines", buildJsonObject {
        put("name", name); put("agent_id", agentId); put("schedule_type", "cron"); put("schedule_expression", cron); put("timezone", tz())
        put("execution_domain", "local"); goalId?.let { put("goal_id", it) }
        putJsonObject("config") { put("jobType", "agent"); put("prompt", prompt) }
    }, "create the routine")

    suspend fun createLoop(name: String, prompt: String, agentId: String?, cron: String, goalId: String? = null) = v1("POST", "automation/loops", buildJsonObject {
        put("name", name); put("description", prompt); agentId?.let { put("agent_id", it) }; put("schedule_type", "cron"); put("schedule_expression", cron)
        put("timezone", tz()); put("execution_domain", "local"); goalId?.let { put("goal_id", it) }
        putJsonObject("config") { put("jobType", "agent"); put("prompt", prompt) }
    }, "create the loop")

    suspend fun createGoal(title: String, description: String?, priority: String, agentId: String?, targetDate: String?) = v1("POST", "automation/goals", buildJsonObject {
        put("title", title); description?.let { put("description", it) }; put("priority", priority); agentId?.let { put("agent_id", it) }; targetDate?.let { put("target_date", it) }
    }, "create the goal")

    suspend fun createHeartbeat(name: String, prompt: String, schedule: String, agentId: String? = null) = backend.relay("POST", "v1/cron/jobs", buildJsonObject {
        put("name", name); put("type", "agent"); put("schedule", schedule)
        putJsonObject("config") { put("prompt", prompt); agentId?.let { put("agentId", it) } }
    }, "create the task")

    suspend fun bots(): List<BotLite> = (BotsApi.listAgents() as? BotsResult.Ok)?.value.orEmpty().map { BotLite(it.id, it.name) }

    // The computer's own routines and loops (iOS RoutinesClient, LoopsClient).
    suspend fun localRoutines(): AResult<List<LocalRoutine>> = backend.relay("GET", "v1/automations/routines", doing = "load routines").mapList("routines") { parseLocalRoutine(it) }
    suspend fun localLoops(): AResult<List<LocalLoop>> = backend.relay("GET", "v1/automations/loops", doing = "load loops").mapList("loops") { parseLocalLoop(it) }
    suspend fun runLocalRoutine(id: String) = backend.relay("POST", "v1/automations/routines/${esc(id)}/run", doing = "run the routine")
    suspend fun deleteLocalRoutine(id: String) = backend.relay("DELETE", "v1/automations/routines/${esc(id)}", doing = "delete the routine")
    suspend fun runLocalLoop(id: String) = backend.relay("POST", "v1/automations/loops/${esc(id)}/run", doing = "restart the loop")
    suspend fun deleteLocalLoop(id: String) = backend.relay("DELETE", "v1/automations/loops/${esc(id)}", doing = "delete the loop")

    suspend fun createLocalRoutine(name: String, steps: List<String>, trigger: String?, schedule: String?) = backend.relay("POST", "v1/automations/routines", buildJsonObject {
        put("name", name)
        put("steps", JsonArray(steps.map { c -> buildJsonObject { put("command", c); put("status", "pending") } }))
        trigger?.let { put("trigger", it) }; schedule?.let { put("schedule", it) }
    }, "create the routine")

    suspend fun createLocalLoop(command: String, exitCondition: String?, maxIterations: Int, metric: LoopMetric?) = backend.relay("POST", "v1/automations/loops", buildJsonObject {
        put("command", command); exitCondition?.let { put("exit_condition", it) }; put("max_iterations", maxIterations)
        metric?.let { m -> putJsonObject("metric") { put("command", m.command); put("direction", m.direction); put("keepBar", m.keepBar); put("noiseRuns", 5); put("minPairs", m.minPairs) } }
    }, "create the loop")

    /** `GET /api/factory/tasks/board`: each card once, even when it sits in more than one wave. */
    suspend fun board(): AResult<List<BoardCard>> = when (val r = backend.relay("GET", "api/factory/tasks/board", doing = "load your tasks")) {
        is AResult.Ok -> {
            val seen = HashSet<String>()
            val cards = (r.value.obj?.get("waves") as? JsonArray).orEmpty().flatMap { w -> (w as? JsonObject).list("nodes") }.mapNotNull { n ->
                val dag = n.str("dagId") ?: return@mapNotNull null
                val node = n.str("nodeId") ?: return@mapNotNull null
                val proof = n.child("proof")
                BoardCard(dag, node, n.str("title").orEmpty(), n.str("status").orEmpty(), n.str("assignee"), proof.n("proven")?.toInt() ?: 0, proof.n("total")?.toInt() ?: 0, n.bool("needsYou") == true)
            }.filter { seen.add(it.key) }
            AResult.Ok(cards)
        }
        AResult.NoComputer -> AResult.NoComputer
        is AResult.Failed -> r
    }
}

// MARK: - Store

/** iOS AutomationStore + the unified task list of AutomationTasksPhoneView. */
class AutomationStore(val api: AutomationApi) {
    var tasks by mutableStateOf<List<AutoTask>>(emptyList())
    var loading by mutableStateOf(true)
    var loaded by mutableStateOf(false)
    var error by mutableStateOf<String?>(null)
    var noComputer by mutableStateOf(false)
    /** Heartbeats live on the computer: when it is away they just do not list, with a note. */
    var heartbeatsAway by mutableStateOf(false)
    var actionError by mutableStateOf<String?>(null)
    var bots by mutableStateOf<List<BotLite>>(emptyList())
    var runningKeys by mutableStateOf<Set<String>>(emptySet())
    var live by mutableStateOf<Map<String, LiveRun>>(emptyMap())

    fun of(type: TaskType): List<AutoTask> = tasks.filter { it.type == type }
    fun botName(id: String?): String? = id?.let { i -> bots.firstOrNull { it.id == i }?.name }
    fun goalName(id: String?): String? = id?.let { i -> tasks.firstOrNull { it.type == TaskType.Goal && it.id == i }?.name }

    suspend fun load() = coroutineScope {
        loading = true
        val r = async { api.routines() }
        val l = async { api.loops() }
        val g = async { api.goals() }
        val h = async { api.heartbeats() }
        val b = async { api.bots() }
        val routines = r.await()
        val goals = g.await()
        val loops = l.await()
        val beats = h.await()
        bots = b.await()
        noComputer = routines is AResult.NoComputer && goals is AResult.NoComputer
        error = (routines as? AResult.Failed)?.message
        heartbeatsAway = beats !is AResult.Ok
        tasks = goals.orNull().orEmpty() + routines.orNull().orEmpty() + loops.orNull().orEmpty() + beats.orNull().orEmpty()
        loaded = true
        loading = false
    }

    private suspend fun act(doing: AResult<*>, reload: Boolean = true): Boolean {
        val ok = doing is AResult.Ok
        actionError = when (doing) {
            is AResult.Failed -> doing.message
            AResult.NoComputer -> "Connect a computer to do that."
            else -> null
        }
        if (ok && reload) load()
        return ok
    }

    suspend fun toggle(t: AutoTask) { act(api.setEnabled(t, !t.isActive)) }
    suspend fun delete(t: AutoTask): Boolean = act(api.delete(t))
    suspend fun setCron(t: AutoTask, cron: String): Boolean = act(api.setCron(t, cron))
    suspend fun setGoal(t: AutoTask, status: String?, progress: Double?): Boolean = act(api.setGoal(t, status, progress))
    suspend fun create(doing: AResult<*>): String? { act(doing); return (doing as? AResult.Failed)?.message ?: if (doing is AResult.NoComputer) "Connect a computer to do that." else null }

    /** Run now, then poll the live run while it goes (iOS pollLive). */
    suspend fun runNow(t: AutoTask) {
        runningKeys = runningKeys + t.key
        val r = api.runNow(t)
        if (!act(r, reload = false)) { runningKeys = runningKeys - t.key; return }
        if (t.type == TaskType.Routine) {
            var tries = 0
            while (tries++ < 45) {
                delay(2_000)
                val s = api.live(t.id).orNull() ?: break
                live = live + (t.id to s)
                if (!s.running) break
            }
            live = live - t.id
        }
        runningKeys = runningKeys - t.key
        load()
    }

    suspend fun stop(t: AutoTask) {
        val runId = live[t.id]?.runId ?: return
        act(api.cancel(t.id, runId), reload = false)
    }
}
