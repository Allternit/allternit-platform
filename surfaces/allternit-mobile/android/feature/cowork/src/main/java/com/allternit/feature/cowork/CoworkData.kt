package com.allternit.feature.cowork

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.design.FriendlyErrorMessage
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.RuntimeCall
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.HttpUrl.Companion.toHttpUrl

/*
 * Cowork data (iOS CoworkTasksClient, CoworkSessionsClient, FabricTransportClient and their stores).
 * Tasks are `api/v1/tasks` on the cloud host; sessions (`cowork/sessions`) ride the relay to the user's
 * computer; Fabric Transport (approvals, runs) is `fabric/transport/*` and `runs` on the API router.
 * A phone with no computer gets [CRes.NoComputer], never a raw error.
 */

sealed interface CRes<out T> {
    data class Ok<T>(val value: T) : CRes<T>
    data object NoComputer : CRes<Nothing>
    data class Failed(val message: String) : CRes<Nothing>
}

/** iOS CoworkTaskStatus. */
enum class TaskStatus(val raw: String, val label: String) {
    Backlog("backlog", "Backlog"), Todo("todo", "Todo"), InProgress("in-progress", "In Progress"),
    InReview("in-review", "In Review"), Done("done", "Done");

    companion object { fun of(raw: String?) = entries.firstOrNull { it.raw == raw } ?: Todo }
}

/** iOS CoworkTask (the fields the screens show). */
data class CoworkTask(
    val id: String,
    val title: String,
    val description: String?,
    val status: TaskStatus,
    val priority: Int,
    val assigneeType: String?,
    val assigneeName: String?,
) {
    val isAgent: Boolean get() = assigneeType == "agent"
}

/** iOS CoworkSession; [displayTitle] falls back like the Swift model. */
data class CoworkSession(val id: String, val title: String?, val createdAt: String) {
    val displayTitle: String get() = title?.takeIf { it.isNotBlank() } ?: "Cowork Session"
}

data class Approval(val id: String, val runId: String, val capability: String, val target: String, val status: String, val decidedBy: String?)
data class FabricRun(val id: String, val state: String)
data class RunEvent(val id: String, val eventType: String, val executor: String?)

private fun JsonObject.task() = CoworkTask(
    id = str("id").orEmpty(),
    title = str("title").orEmpty(),
    description = str("description"),
    status = TaskStatus.of(str("status")),
    priority = (num("priority") ?: 0.0).toInt(),
    assigneeType = str("assignee_type"),
    assigneeName = str("assignee_name"),
)

class CoworkApi(private val backend: ChatBackend) {
    private suspend fun cloud(method: String, path: String, body: JsonElement? = null, doing: String): CRes<JsonElement> {
        val url = (LibraryNet.config.cloudBaseUrl.toString().trimEnd('/') + "/" + path.trimStart('/')).toHttpUrl()
        val req = ApiCall.request(method, url, body?.toString() ?: if (method == "GET" || method == "DELETE") null else "{}")
        return when (val r = ApiCall.execute(LibraryNet.okHttp, ApiJson.default, req, JsonElement.serializer())) {
            is ApiResult.Ok -> CRes.Ok(r.value)
            is ApiResult.Decode -> CRes.Ok(JsonNull)
            is ApiResult.HttpError -> if (r.status == 428 || r.code == "compute_required") CRes.NoComputer else CRes.Failed(r.friendly(doing))
            else -> CRes.Failed(r.friendly(doing))
        }
    }

    private suspend fun api(method: String, path: String, body: JsonElement? = null, doing: String): CRes<JsonElement> {
        val r = if (method == "GET") LibraryNet.get(path) else LibraryNet.send(method, path, body)
        return when (r) {
            is ApiResult.Ok -> CRes.Ok(r.value)
            is ApiResult.HttpError -> if (r.status == 428 || r.code == "compute_required") CRes.NoComputer else CRes.Failed(r.friendly(doing))
            else -> CRes.Failed(r.friendly(doing))
        }
    }

    private suspend fun relay(method: String, path: String, body: JsonElement? = null, doing: String): CRes<JsonElement> =
        when (val r = backend.runtimeJson(method, path, body?.toString())) {
            is RuntimeCall.Ok -> CRes.Ok(r.body)
            RuntimeCall.NoComputer -> CRes.NoComputer
            is RuntimeCall.Http -> CRes.Failed(FriendlyErrorMessage.from(r.message ?: r.code ?: "HTTP ${r.status}", doing))
            is RuntimeCall.Failed -> CRes.Failed(r.message)
        }

    // MARK: Tasks (CoworkTasksClient)

    suspend fun tasks(): CRes<List<CoworkTask>> = when (val r = cloud("GET", "api/v1/tasks?workspace_id=default", doing = "load tasks")) {
        is CRes.Ok -> CRes.Ok(r.value.listUnder("tasks", "items").map { it.task() }.filter { it.id.isNotEmpty() })
        CRes.NoComputer -> CRes.NoComputer
        is CRes.Failed -> r
    }

    suspend fun createTask(title: String, description: String?, assigneeType: String): CRes<CoworkTask> {
        val body = buildJsonObject {
            put("workspace_id", "default"); put("title", title); put("status", "todo"); put("assignee_type", assigneeType)
            if (description != null) put("description", description)
        }
        return when (val r = cloud("POST", "api/v1/tasks", body, "create the task")) {
            is CRes.Ok -> CRes.Ok((r.value as? JsonObject)?.task() ?: CoworkTask("", title, description, TaskStatus.Todo, 0, assigneeType, null))
            CRes.NoComputer -> CRes.NoComputer
            is CRes.Failed -> r
        }
    }

    suspend fun updateStatus(id: String, status: TaskStatus): CRes<Unit> =
        when (val r = cloud("PUT", "api/v1/tasks/${LibraryNet.escape(id)}", buildJsonObject { put("status", status.raw) }, "update the task")) {
            is CRes.Ok -> CRes.Ok(Unit)
            CRes.NoComputer -> CRes.NoComputer
            is CRes.Failed -> r
        }

    suspend fun deleteTask(id: String): CRes<Unit> =
        when (val r = cloud("DELETE", "api/v1/tasks/${LibraryNet.escape(id)}", doing = "delete the task")) {
            is CRes.Ok -> CRes.Ok(Unit)
            CRes.NoComputer -> CRes.NoComputer
            is CRes.Failed -> r
        }

    // MARK: Sessions (CoworkSessionsClient, via the relay)

    suspend fun sessions(): CRes<List<CoworkSession>> = when (val r = relay("GET", "/api/v1/cowork/sessions?limit=30", doing = "load sessions")) {
        is CRes.Ok -> CRes.Ok(r.value.listUnder("sessions").map { CoworkSession(it.str("id").orEmpty(), it.str("title"), it.str("created_at").orEmpty()) }.filter { it.id.isNotEmpty() })
        CRes.NoComputer -> CRes.NoComputer
        is CRes.Failed -> r
    }

    suspend fun createSession(name: String): CRes<String> =
        when (val r = relay("POST", "/api/v1/cowork/sessions", buildJsonObject { put("name", name); put("sessionMode", "regular") }, "start a session")) {
            is CRes.Ok -> {
                val id = ((r.value as? JsonObject)?.get("session") as? JsonObject).str("id")
                if (id.isNullOrEmpty()) CRes.Failed("The session couldn't be created. Try again.") else CRes.Ok(id)
            }
            CRes.NoComputer -> CRes.NoComputer
            is CRes.Failed -> r
        }

    suspend fun deleteSession(id: String): CRes<Unit> =
        when (val r = relay("DELETE", "/api/v1/cowork/sessions/${LibraryNet.escape(id)}", doing = "delete the session")) {
            is CRes.Ok -> CRes.Ok(Unit)
            CRes.NoComputer -> CRes.NoComputer
            is CRes.Failed -> r
        }

    // MARK: Fabric Transport (FabricTransportClient)

    suspend fun approvals(): CRes<List<Approval>> = when (val r = api("GET", "fabric/transport/approvals?workspace=default", doing = "load approvals")) {
        is CRes.Ok -> CRes.Ok(r.value.listUnder("approvals").map {
            Approval(it.str("id").orEmpty(), it.str("run_id").orEmpty(), it.str("capability").orEmpty(), it.str("target").orEmpty(), it.str("status").orEmpty(), it.str("decided_by"))
        })
        CRes.NoComputer -> CRes.NoComputer
        is CRes.Failed -> r
    }

    suspend fun decide(id: String, grant: Boolean): CRes<Unit> =
        when (val r = api("POST", "fabric/transport/approvals/${LibraryNet.escape(id)}/${if (grant) "grant" else "deny"}", doing = "answer the approval")) {
            is CRes.Ok -> CRes.Ok(Unit)
            CRes.NoComputer -> CRes.NoComputer
            is CRes.Failed -> r
        }

    suspend fun runs(): CRes<List<FabricRun>> = when (val r = api("GET", "runs", doing = "load runs")) {
        is CRes.Ok -> CRes.Ok(r.value.listUnder("runs").map { FabricRun(it.str("id").orEmpty(), it.str("state").orEmpty()) }.take(20))
        CRes.NoComputer -> CRes.NoComputer
        is CRes.Failed -> r
    }

    suspend fun events(runId: String): List<RunEvent> =
        (api("GET", "runs/${LibraryNet.escape(runId)}/events", doing = "load run events") as? CRes.Ok)?.value.listUnder("events")
            .map { RunEvent(it.str("id").orEmpty(), it.str("event_type").orEmpty(), it.str("executor")) }
}

/** Shared list state for the tasks screens (iOS CoworkTasksStore): loaded once, refreshed on demand. */
class CoworkTasksState(private val api: CoworkApi) {
    var tasks by mutableStateOf<List<CoworkTask>>(emptyList()); private set
    var loading by mutableStateOf(false); private set
    var loaded by mutableStateOf(false); private set
    var noComputer by mutableStateOf(false); private set
    var error by mutableStateOf<String?>(null); private set

    suspend fun refresh() {
        loading = true
        when (val r = api.tasks()) {
            is CRes.Ok -> { tasks = r.value; error = null; noComputer = false }
            CRes.NoComputer -> { noComputer = true; error = null }
            is CRes.Failed -> error = r.message
        }
        loading = false
        loaded = true
    }

    fun task(id: String) = tasks.firstOrNull { it.id == id }

    suspend fun create(title: String, description: String?, assigneeType: String): String? = when (val r = api.createTask(title, description, assigneeType)) {
        is CRes.Ok -> { refresh(); null }
        CRes.NoComputer -> "Connect a computer first to create tasks."
        is CRes.Failed -> r.message
    }

    suspend fun updateStatus(id: String, status: TaskStatus): String? = when (val r = api.updateStatus(id, status)) {
        is CRes.Ok -> { tasks = tasks.map { if (it.id == id) it.copy(status = status) else it }; null }
        CRes.NoComputer -> "Connect a computer first."
        is CRes.Failed -> r.message
    }

    suspend fun delete(id: String): String? = when (val r = api.deleteTask(id)) {
        is CRes.Ok -> { tasks = tasks.filterNot { it.id == id }; null }
        CRes.NoComputer -> "Connect a computer first."
        is CRes.Failed -> r.message
    }
}
