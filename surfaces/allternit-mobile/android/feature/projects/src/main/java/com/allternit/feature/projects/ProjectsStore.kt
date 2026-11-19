package com.allternit.feature.projects

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.parseTime
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject

/*
 * iOS ProjectsLibraryStore + ProjectStore: chat (cowork) projects from `GET cowork/projects`,
 * bot projects from `GET bot-projects`, code and design projects from `GET project-workspaces`
 * (keys `code-<id>` / `design-<id>`), thread counts from `GET agent-sessions`.
 */

enum class ProjectMode(val raw: String, val label: String, val appearanceMode: String) {
    Chat("chat", "Chat", "cowork"), Code("code", "Code", "code"), Design("design", "Design", "design"), Bot("bot", "Bot", "bot");
}

/** iOS LibraryProject: one row of the merged list. */
data class LibraryProject(
    val mode: ProjectMode,
    val nativeId: String,
    val title: String,
    val description: String?,
    val isFavorite: Boolean,
    val isArchived: Boolean,
    val updatedAt: Long,
    val createdAt: Long,
    val countLine: String,
    val waiting: Int = 0,
    val working: Int = 0,
    val instructions: String? = null,
    /** Code / design: the `project-workspaces` row id and key. */
    val workspaceRowId: String? = null,
    val directory: String? = null,
    /** Status from the workspace row (`state`, `next_action`, closed). */
    val state: String? = null,
    val nextAction: String? = null,
    val closed: Boolean = false,
) {
    val id: String get() = "${mode.raw}:$nativeId"
    val workspaceKey: String get() = "${mode.raw}-$nativeId"
}

/** A session linked to a project (iOS LinkedSessionList row). */
data class ProjectSession(val id: String, val name: String, val updatedAt: Long, val projectId: String?, val workspaceId: String?)

private fun plural(n: Int, one: String) = "$n $one${if (n == 1) "" else "s"}"

class ProjectsLibraryStore(private val context: Context) {
    var projects by mutableStateOf<List<LibraryProject>>(emptyList()); private set
    var sessions by mutableStateOf<List<ProjectSession>>(emptyList()); private set
    var loading by mutableStateOf(false); private set
    var loaded by mutableStateOf(false); private set
    /** iOS "Couldn't load everything" lines, one per failed source. */
    var failures by mutableStateOf<List<String>>(emptyList()); private set
    var noComputer by mutableStateOf(false); private set

    private val prefs get() = context.getSharedPreferences("allternit_projects", Context.MODE_PRIVATE)
    private fun localSet(key: String): Set<String> = prefs.getStringSet(key, emptySet()).orEmpty()
    private fun toggleLocal(key: String, id: String, on: Boolean) {
        val next = localSet(key).toMutableSet().apply { if (on) add(id) else remove(id) }
        prefs.edit().putStringSet(key, next).apply()
    }

    suspend fun load() = coroutineScope {
        loading = true
        val cowork = async { LibraryNet.get("cowork/projects") }
        val bots = async { LibraryNet.get("bot-projects") }
        val workspaces = async { LibraryNet.get("project-workspaces") }
        val linked = async { LibraryNet.get("agent-sessions") }
        val rCowork = cowork.await(); val rBots = bots.await(); val rWs = workspaces.await(); val rSessions = linked.await()
        val errors = mutableListOf<String>()
        fun note(r: ApiResult<*>, what: String) {
            if (r !is ApiResult.Ok && !(r is ApiResult.HttpError && r.status == 404)) errors += "Couldn't load $what. " + (if (r is ApiResult.HttpError && (r.status == 401 || r.status == 403)) r.friendly("load $what") else "Check the connection and try again.")
        }
        note(rCowork, "projects"); note(rBots, "bot projects"); note(rWs, "code and design projects")
        noComputer = listOf(rCowork, rWs).any { it is ApiResult.HttpError && (it.status == 503 || it.status == 428 || it.code == "compute_required" || it.code == "no_brain" || it.code == "no_computer") }

        val sessionRows = (rSessions as? ApiResult.Ok)?.value.listUnder("sessions").mapNotNull { o ->
            val id = o.str("id") ?: return@mapNotNull null
            val meta = o.child("metadata")
            if (meta.str("projectRole") == "coordinator") return@mapNotNull null
            ProjectSession(
                id, o.str("name")?.takeIf { it.isNotBlank() } ?: "", parseTime(o.str("updated_at") ?: o.str("updatedAt")),
                o.str("project_id") ?: meta.str("project_id") ?: meta.str("projectId"), meta.str("workspaceId"),
            )
        }
        sessions = sessionRows
        val favorites = localSet(FAV_KEY); val archived = localSet(ARCHIVE_KEY)

        val botItems = (rBots as? ApiResult.Ok)?.value.listUnder("projects", "items").mapNotNull { o ->
            val id = o.str("id") ?: return@mapNotNull null
            val counts = o.child("counts")
            val botIds = (o["botIds"] ?: o["bot_ids"]) as? kotlinx.serialization.json.JsonArray
            val n = botIds?.size ?: 0
            LibraryProject(
                ProjectMode.Bot, id, o.str("title") ?: "Untitled project", o.str("description"),
                isFavorite = o["favorite"].let { (it as? JsonPrimitive)?.content == "true" },
                isArchived = o["archived"].let { (it as? JsonPrimitive)?.content == "true" },
                updatedAt = parseTime(o.str("lastActivityAt") ?: o.str("last_activity_at")), createdAt = 0,
                countLine = "${plural(n, "bot")} · Al coordinates",
                waiting = counts.num("waiting")?.toInt() ?: 0, working = counts.num("working")?.toInt() ?: 0,
            )
        }
        val botIds = botItems.map { it.nativeId }.toSet()
        val chats = (rCowork as? ApiResult.Ok)?.value.listUnder("projects").mapNotNull { o ->
            val id = o.str("id") ?: return@mapNotNull null
            if (id in botIds) return@mapNotNull null
            val n = sessionRows.count { it.projectId == id }
            LibraryProject(
                ProjectMode.Chat, id, o.str("title") ?: "Untitled project", o.str("description"),
                isFavorite = "chat:$id" in favorites, isArchived = "chat:$id" in archived,
                updatedAt = parseTime(o.str("updated_at")), createdAt = parseTime(o.str("created_at")),
                countLine = plural(n, "thread"), instructions = o.str("instructions"),
            )
        }
        val ws = (rWs as? ApiResult.Ok)?.value.listUnder("workspaces").mapNotNull { o ->
            val key = o.str("key") ?: return@mapNotNull null
            val mode = when { key.startsWith("code-") -> ProjectMode.Code; key.startsWith("design-") -> ProjectMode.Design; else -> return@mapNotNull null }
            val nativeId = o.str("nativeId") ?: key.substringAfter('-')
            val rowId = o.str("projectId") ?: return@mapNotNull null
            val status = o.child("status")
            val linkedN = sessionRows.filter { it.workspaceId == rowId || it.workspaceId == nativeId }
            val last = linkedN.maxOfOrNull { it.updatedAt } ?: 0
            val id = "${mode.raw}:$nativeId"
            LibraryProject(
                mode, nativeId, o.str("title") ?: "Untitled project", o.str("description"),
                isFavorite = id in favorites, isArchived = id in archived,
                updatedAt = maxOf(parseTime(o.str("updatedAt")), last), createdAt = parseTime(o.str("createdAt")),
                countLine = if (mode == ProjectMode.Code) plural(linkedN.size, "session") else plural(linkedN.size, "thread"),
                workspaceRowId = rowId, directory = o.str("directory"),
                state = status.str("state"), nextAction = status.str("next_action"), closed = status.str("lifecycle") == "closed",
            )
        }
        // iOS order: chats + code + design + bots.
        projects = chats + ws.filter { it.mode == ProjectMode.Code } + ws.filter { it.mode == ProjectMode.Design } + botItems
        failures = errors
        loading = false
        loaded = true
    }

    // MARK: Create (iOS createProject per mode)

    suspend fun createChat(title: String, description: String?): Result<LibraryProject> {
        val r = LibraryNet.send("POST", "cowork/projects", buildJsonObject { put("title", title); description?.let { put("description", it) } })
        val id = (r as? ApiResult.Ok)?.value.obj.child("project").str("id") ?: (r as? ApiResult.Ok)?.value.obj.str("id")
            ?: return Result.failure(IllegalStateException("Couldn't create the project. ${r.friendly("create the project")}"))
        load()
        return Result.success(projects.firstOrNull { it.mode == ProjectMode.Chat && it.nativeId == id }
            ?: LibraryProject(ProjectMode.Chat, id, title, description, false, false, System.currentTimeMillis(), System.currentTimeMillis(), "0 threads"))
    }

    /** Code: `ws_` + base36(epoch ms); design: `design-<ms>-<random>` (iOS native ids). PUT project-workspaces/:key. */
    suspend fun createWorkspace(mode: ProjectMode, title: String, description: String?): Result<LibraryProject> {
        val now = System.currentTimeMillis()
        val nativeId = if (mode == ProjectMode.Code) "ws_" + now.toString(36) else "design-$now-" + (Math.random() * 1e9).toLong().toString(36)
        val key = "${mode.raw}-$nativeId"
        val r = LibraryNet.send("PUT", "project-workspaces/${LibraryNet.escape(key)}", buildJsonObject {
            put("nativeId", nativeId); put("mode", mode.raw); put("title", title); description?.let { put("description", it) }
            putJsonObject("settings") { put("context", "") }
        })
        val rowId = (r as? ApiResult.Ok)?.value.obj.str("projectId")
            ?: return Result.failure(IllegalStateException("Couldn't create the project. ${r.friendly("create the project")}"))
        load()
        return Result.success(projects.firstOrNull { it.mode == mode && it.nativeId == nativeId }
            ?: LibraryProject(mode, nativeId, title, description, false, false, now, now, if (mode == ProjectMode.Code) "0 sessions" else "0 threads", workspaceRowId = rowId))
    }

    /** Bots without a computer go to the account store (cloud-api `account-projects`, kind bot). */
    suspend fun createAccountBot(title: String, brief: String?): Result<Unit> {
        val r = com.allternit.core.api.ApiCall.execute(
            LibraryNet.okHttp, com.allternit.core.api.ApiJson.default,
            com.allternit.core.api.ApiCall.request("POST", LibraryNet.config.cloudBaseUrl.newBuilder().addPathSegments("api/v1/account-projects").build(),
                buildJsonObject { put("kind", "bot"); put("title", title); brief?.let { put("instructions", it) } }.toString()),
            JsonElement.serializer(),
        )
        return if (r is ApiResult.Ok || r is ApiResult.Decode) { load(); Result.success(Unit) } else Result.failure(IllegalStateException("Couldn't create the project. ${r.friendly("create the project")}"))
    }

    // MARK: Flags, edit, delete

    suspend fun setFavorite(p: LibraryProject, on: Boolean) = setFlag(p, "favorite", FAV_KEY, on)
    suspend fun setArchived(p: LibraryProject, on: Boolean) = setFlag(p, "archived", ARCHIVE_KEY, on)

    private suspend fun setFlag(p: LibraryProject, flag: String, key: String, on: Boolean) {
        if (p.mode == ProjectMode.Bot) {
            // Bot flags live on the server: read-merge-write the project's metadata JSON string.
            val path = "cowork/projects/${LibraryNet.escape(p.nativeId)}"
            val current = (LibraryNet.get(path) as? ApiResult.Ok)?.value.obj.child("project")
            val meta = current.str("metadata")?.let { runCatching { com.allternit.core.api.ApiJson.default.parseToJsonElement(it) as? JsonObject }.getOrNull() } ?: JsonObject(emptyMap())
            val merged = JsonObject(meta + (flag to JsonPrimitive(on)))
            LibraryNet.send("PATCH", path, buildJsonObject { put("metadata", merged.toString()) })
        } else {
            toggleLocal(key, p.id, on)
        }
        projects = projects.map { if (it.id == p.id) (if (flag == "favorite") it.copy(isFavorite = on) else it.copy(isArchived = on)) else it }
    }

    /** iOS saveDetails: `PATCH cowork/projects/:id {title, description}` (null clears). */
    suspend fun saveDetails(p: LibraryProject, title: String, description: String?): Result<Unit> {
        val id = if (p.mode == ProjectMode.Chat || p.mode == ProjectMode.Bot) p.nativeId else p.workspaceRowId ?: p.nativeId
        val r = LibraryNet.send("PATCH", "cowork/projects/${LibraryNet.escape(id)}", buildJsonObject {
            put("title", title); if (description == null) put("description", JsonNull) else put("description", description)
        })
        if (r !is ApiResult.Ok) return Result.failure(IllegalStateException("Couldn't save the project details. Try again."))
        projects = projects.map { if (it.id == p.id) it.copy(title = title, description = description) else it }
        return Result.success(Unit)
    }

    suspend fun saveInstructions(p: LibraryProject, instructions: String): Result<Unit> {
        val r = LibraryNet.send("PUT", "cowork/projects/${LibraryNet.escape(p.nativeId)}", buildJsonObject { put("instructions", instructions) })
        if (r !is ApiResult.Ok) return Result.failure(IllegalStateException("Couldn't save instructions: ${r.friendly("save instructions")}"))
        projects = projects.map { if (it.id == p.id) it.copy(instructions = instructions) else it }
        return Result.success(Unit)
    }

    suspend fun delete(p: LibraryProject): Result<Unit> {
        val path = when (p.mode) {
            ProjectMode.Chat, ProjectMode.Bot -> "cowork/projects/${LibraryNet.escape(p.nativeId)}"
            else -> "cowork/projects/${LibraryNet.escape(p.workspaceRowId ?: p.nativeId)}"
        }
        val r = LibraryNet.send("DELETE", path)
        if (r !is ApiResult.Ok && !(r is ApiResult.HttpError && r.status == 404)) {
            return Result.failure(IllegalStateException("Couldn't delete ${p.title}. ${r.friendly("delete it")}"))
        }
        projects = projects.filterNot { it.id == p.id }
        return Result.success(Unit)
    }

    /** Code: `GET project-workspaces/:key` → settings, set `directory`, PUT the whole row back. */
    suspend fun setDirectory(p: LibraryProject, path: String): Result<Unit> {
        val route = "project-workspaces/${LibraryNet.escape(p.workspaceKey)}"
        val settings = (LibraryNet.get(route) as? ApiResult.Ok)?.value.obj.child("settings") ?: JsonObject(emptyMap())
        val r = LibraryNet.send("PUT", route, buildJsonObject {
            put("nativeId", p.nativeId); put("mode", "code"); put("title", p.title); p.description?.let { put("description", it) }
            put("settings", JsonObject(settings + ("directory" to JsonPrimitive(path))))
        })
        if (r !is ApiResult.Ok) return Result.failure(IllegalStateException("Couldn't save the folder. ${r.friendly("save the folder")}"))
        projects = projects.map { if (it.id == p.id) it.copy(directory = path) else it }
        return Result.success(Unit)
    }

    companion object {
        const val FAV_KEY = "allternit-phone-projects-favorites"
        const val ARCHIVE_KEY = "allternit-phone-projects-archived"
    }
}

// MARK: - Appearance (iOS AppearanceClient: `appearance/<encoded ["project",mode,id]>`)

/** The folder look. The raw record round-trips, so fields this app does not draw are kept. */
data class Appearance(val raw: JsonObject) {
    val color: String get() = raw.str("color") ?: DEFAULT_COLOR
    val paperColor: String get() = raw.str("paperColor") ?: "#ffffff"
    val symbol: String get() = raw.str("symbol") ?: "sparkle"
    val text: String get() = raw.str("text") ?: ""
    val image: String? get() = raw.str("image")
    val cover: JsonObject? get() = raw.child("cover")?.takeIf { it.num("v")?.toInt() == 2 }
    /** No v2 cover = the Classic cover (iOS resolvedCover fallback). */
    val art: String get() = cover.str("art") ?: "title"
    val font: String get() = cover.str("font") ?: "mono"
    val place: String get() = cover.str("place") ?: "center"
    val pattern: String get() = cover.str("pattern") ?: "grain"
    val frame: String get() = cover.str("frame") ?: "dotted"
    val edge: String get() = cover.str("edge") ?: "ink"
    val sheets: Int get() = cover.num("sheets")?.toInt()?.coerceIn(0, 3) ?: 2
    val tabColor: String? get() = cover.str("tabColor")
    val tabLabel: String get() = cover.str("tabLabel") ?: ""
    val showMode: Boolean get() = cover?.get("showMode")?.let { (it as? JsonPrimitive)?.content == "true" } ?: true

    fun withField(key: String, value: JsonElement): Appearance = Appearance(JsonObject(raw + (key to value)))
    fun withCover(key: String, value: JsonElement): Appearance {
        val base = cover ?: buildJsonObject {
            put("v", 2); put("art", "title"); put("font", "mono"); put("place", "center"); put("pattern", "grain")
            put("frame", "dotted"); put("edge", "ink"); put("sheets", 2); put("tabLabel", ""); put("showMode", true)
        }
        return withField("cover", JsonObject(base + (key to value)))
    }

    companion object {
        const val DEFAULT_COLOR = "#397ad5"
        val Default = Appearance(buildJsonObject { put("version", 1); put("color", DEFAULT_COLOR); put("paperColor", "#ffffff"); put("overlay", "symbol"); put("symbol", "sparkle") })
    }
}

sealed interface AppearanceSave {
    data object Ok : AppearanceSave
    data class Failed(val message: String) : AppearanceSave
}

object AppearanceClient {
    fun key(mode: ProjectMode, id: String): String =
        kotlinx.serialization.json.JsonArray(listOf(JsonPrimitive("project"), JsonPrimitive(mode.appearanceMode), JsonPrimitive(id))).toString()

    private fun path(mode: ProjectMode, id: String) = "appearance/" + java.net.URLEncoder.encode(key(mode, id), "UTF-8").replace("+", "%20")

    suspend fun get(mode: ProjectMode, id: String): Appearance? =
        (LibraryNet.get(path(mode, id)) as? ApiResult.Ok)?.value.obj.child("appearance")?.let { Appearance(it) }

    suspend fun put(mode: ProjectMode, id: String, a: Appearance): AppearanceSave =
        when (val r = LibraryNet.send("PUT", path(mode, id), buildJsonObject { put("appearance", a.raw) })) {
            is ApiResult.Ok -> AppearanceSave.Ok
            is ApiResult.HttpError -> AppearanceSave.Failed(
                when (r.status) {
                    503 -> "No computer is connected, so this look was not saved. Connect a computer to save it."
                    404 -> "Update your computer to save folder looks."
                    else -> "Could not save the look. Try again."
                },
            )
            else -> AppearanceSave.Failed("Could not save the look. Try again.")
        }
}
