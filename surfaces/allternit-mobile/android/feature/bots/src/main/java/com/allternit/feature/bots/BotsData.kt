package com.allternit.feature.bots

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.long
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.parseTime
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.RuntimeCall
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject

/*
 * Bots mode data (iOS BotModeClient, BotModeStore, AgentRecord, ThreadsClient, AgentClient,
 * HermesClient, WebhookClient, AgentMarketplaceClient). Bodies are read as JsonElement and picked
 * apart tolerantly, like the iOS Codable structs with decodeIfPresent defaults.
 */

// MARK: - Records

/** A bot or agent (iOS AgentRecord); [raw] keeps every field for edits that send the full config back. */
data class BotRecord(val raw: JsonObject) {
    val id: String get() = raw.str("id").orEmpty()
    val name: String get() = raw.str("name") ?: "Agent"
    val description: String? get() = raw.str("description")
    val type: String get() = raw.str("type") ?: "worker"
    val model: String get() = raw.str("model").orEmpty()
    val provider: String get() = raw.str("provider").orEmpty()
    val systemPrompt: String? get() = raw.str("system_prompt")
    val status: String get() = raw.str("status") ?: "idle"
    val avatar: String? get() = raw.str("avatar")
    val trustTier: String get() = raw.str("trust_tier") ?: "standard"
    val isPrimary: Boolean get() = raw.bool("is_primary") == true
    val parentAgentId: String? get() = raw.str("parent_agent_id")
    val createdAt: String get() = raw.str("created_at").orEmpty()
    val updatedAt: String get() = raw.str("updated_at").orEmpty()
    val lastRunAt: String? get() = raw.str("last_run_at")
    val config: JsonObject get() = raw.child("config") ?: JsonObject(emptyMap())
    val profile: JsonObject? get() = raw.child("bot_profile") ?: config.child("botProfile")

    val greeting: String? get() = config.str("greeting")?.takeIf { it.isNotEmpty() }
    val suggestedPrompts: List<String>
        get() = (config["suggested_prompts"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }.orEmpty()

    /** iOS AgentRecord.isBot. */
    val isBot: Boolean get() = profile != null || config.bool("isBot") == true || config.bool("is_bot") == true || raw.bool("is_bot") == true
    val displayName: String get() = profile.str("displayName")?.takeIf { it.isNotEmpty() } ?: name
    val tagline: String get() = profile.str("tagline")?.takeIf { it.isNotEmpty() } ?: (description ?: "@$name")
    val accentHex: String? get() = profile.str("accentColor")?.takeIf { it.isNotEmpty() }
    /** iOS AgentRecord.isMainBot: the chief of staff. */
    val isMainBot: Boolean
        get() = config.child("botProfile").bool("chiefOfStaff") == true || config.child("bot_profile").bool("chiefOfStaff") == true ||
            raw.child("bot_profile").bool("chiefOfStaff") == true
    /** A vendor bot (Hermes, OpenClaw, …) brought in from a pack. */
    val vendorPackId: String? get() = profile.str("vendorPackId")
    val canonicalThreadId: String? get() = config.str("canonicalThreadId")?.takeIf { it.isNotEmpty() }
    /** iOS isPrimaryTemplateBot: the account's primary bot can't be deleted from the phone. */
    val isPrimaryTemplateBot: Boolean get() = isPrimary
    val email: String? get() = (raw.child("identity_channels") ?: config.child("identityChannels")).child("email").str("address")
    val modelShort: String? get() = model.split("/").lastOrNull()?.takeIf { it.isNotEmpty() }
    /** The bot's greeting for a fresh main chat (iOS botGreeting). */
    val botGreeting: String get() = profile.str("welcomeMessage")?.takeIf { it.isNotBlank() } ?: greeting ?: "Hi, I'm $displayName. What should we work on?"
    val starterPrompts: List<String>
        get() = (profile?.get("starterPrompts") as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }.orEmpty()
}

/** A bot thread (iOS BotThread). */
data class BotThread(
    val id: String,
    val botId: String,
    val title: String,
    val kind: String,
    val incognito: Boolean,
    val status: String,
    val group: String,
    val statusLine: String?,
    val progress: List<Int>,
    val currentSessionId: String?,
    val lastActivityAt: String,
    val createdBy: String?,
    val projectId: String?,
    val todo: List<Pair<String, String>>,
    val summary: String?,
) {
    /** iOS AutomationScheduleLogic.displayName: drop a `[bot:Name] ` prefix. */
    val displayTitle: String
        get() = if (title.startsWith("[bot:")) title.substringAfter("]").trim() else title
    val isMainChat: Boolean get() = kind == "standing" && createdBy == "import" && title == "Main chat"
    val groupOrder: Int get() = listOf("waiting", "working", "queued", "idle", "resolved").indexOf(group).let { if (it < 0) 3 else it }
    val progressLabel: String? get() = if (progress.size == 2 && progress[1] > 0) "${progress[0]}/${progress[1]}" else null

    companion object {
        fun from(o: JsonObject): BotThread? {
            val id = o.str("id") ?: return null
            return BotThread(
                id = id,
                botId = o.str("botId").orEmpty(),
                title = o.str("title").orEmpty(),
                kind = o.str("kind") ?: "task",
                incognito = o.bool("incognito") == true,
                status = o.str("status").orEmpty(),
                group = o.str("group") ?: "idle",
                statusLine = o.str("statusLine"),
                progress = (o["progress"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull?.toDoubleOrNull()?.toInt() }.orEmpty(),
                currentSessionId = o.str("currentSessionId"),
                lastActivityAt = o.str("lastActivityAt").orEmpty(),
                createdBy = o.str("createdBy"),
                projectId = o.str("projectId"),
                todo = (o["todo"] as? JsonArray)?.mapNotNull { e -> (e as? JsonObject)?.let { (it.str("text") ?: return@let null) to (it.str("state") ?: "pending") } }.orEmpty(),
                summary = o.str("summary"),
            )
        }

        /** iOS BotModeStore.threadOrder: group, then most recent. */
        val order: Comparator<BotThread> = compareBy<BotThread> { it.groupOrder }.thenByDescending { it.lastActivityAt }
    }
}

/** iOS BotThreadGroup. */
enum class BotThreadGroup(val raw: String, val title: String) {
    Waiting("waiting", "Waiting on you"),
    Working("working", "Working"),
    Queued("queued", "Queued"),
    Idle("idle", "Idle"),
    Resolved("resolved", "Resolved");

    companion object {
        fun of(raw: String) = entries.firstOrNull { it.raw == raw } ?: Idle
    }
}

/** A bot project (iOS BotProjectSummary, `GET bot-projects`). */
data class BotProjectSummary(
    val id: String,
    val title: String,
    val description: String?,
    val botIds: List<String>,
    val waiting: Int,
    val working: Int,
    val queued: Int,
    val idle: Int,
    val resolved: Int,
    val lastActivityAt: String?,
    val archived: Boolean,
    val favorite: Boolean,
) {
    companion object {
        fun from(o: JsonObject): BotProjectSummary? {
            val id = o.str("id") ?: return null
            val counts = o.child("counts")
            fun n(k: String) = counts.num(k)?.toInt() ?: 0
            return BotProjectSummary(
                id, o.str("title") ?: "Untitled project", o.str("description"),
                (o["botIds"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }.orEmpty(),
                n("waiting"), n("working"), n("queued"), n("idle"), n("resolved"),
                o.str("lastActivityAt"), o.bool("archived") == true, o.bool("favorite") == true,
            )
        }
    }
}

/** A group chat room (iOS BotGroupChat, `bot-groups` rows). */
data class BotGroupChat(val id: String, val name: String, val memberIds: List<String>, val memberNames: List<String>, val lastLine: String?, val updatedAt: String, val room: GroupRoom? = null)

/** A messaging channel a bot can answer on (iOS BotChannel). */
data class BotChannel(val accountId: String, val provider: String, val displayName: String?, val handle: String?, val state: String, val enabled: Boolean, val isDefault: Boolean) {
    val label: String get() = displayName?.takeIf { it.isNotEmpty() } ?: provider.replaceFirstChar { it.uppercase() }
}

/** iOS BotMemoryItem. */
data class BotMemoryItem(val id: String, val content: String, val type: String, val source: String?, val createdAt: String)

/** iOS WorkspaceFileInfo. */
data class WorkspaceFileInfo(val path: String, val sizeBytes: Int, val modifiedAt: String?) {
    val sizeLabel: String get() = if (sizeBytes < 1024) "$sizeBytes B" else String.format(java.util.Locale.US, "%.1f KB", sizeBytes / 1024.0)
    val fileName: String get() = path.substringAfterLast('/')
}

/** iOS WebhookSubscription. */
data class WebhookSubscription(val id: String, val url: String, val events: List<String>, val active: Boolean)

/** iOS MarketplaceListing. */
data class MarketplaceListing(
    val id: String, val title: String, val description: String, val category: String?, val tags: List<String>,
    val publisherName: String?, val ratingAvg: Double, val ratingCount: Int, val installCount: Int,
)

/** iOS AgentTemplate (`agent-templates`). */
data class AgentTemplate(val id: String, val name: String, val description: String, val category: String?)

/** iOS AutomationRoutine (the scheduled ones `automation/routines` returns). */
data class AutomationRoutine(val id: String, val name: String, val agentId: String?, val scheduleType: String?, val scheduleExpression: String, val isEnabled: Boolean)

/** iOS AgentSession (the rows `agent-sessions` returns), cut to what Bots mode reads. */
data class BotSessionRow(val id: String, val name: String, val agentId: String?, val agentName: String?, val messageCount: Int, val updatedAt: String)

/** iOS ComputerRecord (`computers?bot_id=`). */
data class ComputerRow(val id: String, val kind: String, val status: String, val name: String, val botId: String?) {
    val isRunning: Boolean get() = status == "running"
}

/** iOS BotSummary (`bot-hub/:id/summary`), the tiles on the bot page. */
data class BotSummary(val model: String?, val sessions: Int?, val messages: Int?, val tokens: Long?, val costUsd: Double?, val memories: Int?, val skills: Int?)

// MARK: - Client

/** What a call returns to a screen: the value, or product copy for the failure. */
sealed interface BotsResult<out T> {
    data class Ok<T>(val value: T) : BotsResult<T>
    data class Failed(val message: String, val status: Int? = null) : BotsResult<Nothing>
}

internal fun <T> BotsResult<T>.orNull(): T? = (this as? BotsResult.Ok)?.value

/** The routes Bots mode calls (iOS BotModeClient + AgentClient + friends), relative to `/api/v1`. */
object BotsApi {
    fun esc(s: String): String = LibraryNet.escape(s)

    private suspend fun <T> get(path: String, doing: String, map: (JsonElement) -> T): BotsResult<T> =
        when (val r = LibraryNet.get(path)) {
            is ApiResult.Ok -> BotsResult.Ok(map(r.value))
            else -> BotsResult.Failed(r.friendly(doing), (r as? ApiResult.HttpError)?.status)
        }

    private suspend fun send(method: String, path: String, body: JsonElement?, doing: String): BotsResult<JsonElement> =
        when (val r = LibraryNet.send(method, path, body)) {
            is ApiResult.Ok -> BotsResult.Ok(r.value)
            else -> BotsResult.Failed(r.friendly(doing), (r as? ApiResult.HttpError)?.status)
        }

    // Agents (AgentClient)
    suspend fun listAgents(): BotsResult<List<BotRecord>> =
        get("agents", "load your bots") { j -> j.listUnder("agents").map { BotRecord(it) }.filter { it.id.isNotEmpty() } }

    suspend fun getAgent(id: String): BotsResult<BotRecord> =
        get("agents/${esc(id)}", "load this bot") { j -> BotRecord(j.obj?.child("agent") ?: j.obj ?: JsonObject(emptyMap())) }

    /** `POST agents`; a runtime that rejects the full body gets the minimal fields (iOS MeetAgentsAPI.create). */
    suspend fun createAgent(body: JsonObject): BotsResult<String> {
        when (val r = send("POST", "agents", body, "create the bot")) {
            is BotsResult.Ok -> r.value.obj?.child("agent")?.str("id")?.let { return BotsResult.Ok(it) }
            is BotsResult.Failed -> if (r.status != 400 && r.status != 422) return r
        }
        val keys = listOf("name", "description", "type", "model", "provider", "harness_config", "enabled_modes", "trust_tier", "is_bot", "bot_profile", "config", "system_prompt")
        val minimal = JsonObject(body.filterKeys { it in keys })
        return when (val r = send("POST", "agents", minimal, "create the bot")) {
            is BotsResult.Ok -> r.value.obj?.child("agent")?.str("id")?.let { BotsResult.Ok(it) } ?: BotsResult.Failed("The bot wasn't created.")
            is BotsResult.Failed -> r
        }
    }

    /** `PATCH agents/:id`, falling back to PUT on runtimes without PATCH (iOS MeetAgentsAPI.update). */
    suspend fun updateAgent(id: String, body: JsonObject): BotsResult<Unit> =
        when (val r = send("PATCH", "agents/${esc(id)}", body, "save the bot")) {
            is BotsResult.Ok -> BotsResult.Ok(Unit)
            is BotsResult.Failed -> if (r.status == 404 || r.status == 405) {
                when (val p = send("PUT", "agents/${esc(id)}", body, "save the bot")) {
                    is BotsResult.Ok -> BotsResult.Ok(Unit)
                    is BotsResult.Failed -> p
                }
            } else r
        }

    suspend fun deleteAgent(id: String): BotsResult<Unit> =
        when (val r = send("DELETE", "agents/${esc(id)}", null, "delete the bot")) {
            is BotsResult.Ok -> BotsResult.Ok(Unit)
            is BotsResult.Failed -> r
        }

    suspend fun listTemplates(): BotsResult<List<AgentTemplate>> = get("agent-templates", "load templates") { j ->
        j.listUnder("templates").mapNotNull { o ->
            AgentTemplate(o.str("id") ?: return@mapNotNull null, o.str("name") ?: "Template", o.str("description").orEmpty(), o.str("category"))
        }
    }

    suspend fun createFromTemplate(templateId: String): BotsResult<String> =
        when (val r = send("POST", "agents/from-template", buildJsonObject { put("template_id", templateId) }, "create the bot")) {
            is BotsResult.Ok -> BotsResult.Ok(r.value.obj?.child("agent")?.str("id") ?: r.value.obj?.str("agent_id") ?: r.value.obj?.str("id").orEmpty())
            is BotsResult.Failed -> r
        }

    // Workspace files (AgentClient / HermesClient)
    suspend fun listWorkspaceFiles(agentId: String): BotsResult<List<WorkspaceFileInfo>> =
        get("agents/${esc(agentId)}/workspace/files", "load workspace files") { j ->
            j.listUnder("files").mapNotNull { o -> WorkspaceFileInfo(o.str("path") ?: return@mapNotNull null, o.num("size_bytes")?.toInt() ?: 0, o.str("modified_at")) }
        }

    /** The file, its text and the hash it was read at (HermesClient.readFile). */
    suspend fun readWorkspaceFile(agentId: String, path: String): BotsResult<Triple<String, String, String?>> =
        get("agents/${esc(agentId)}/workspace/file?path=${esc(path)}", "load the file") { j ->
            val o = j.obj
            Triple(o.str("path") ?: path, o.str("content").orEmpty(), o.str("hash"))
        }

    /** A save with the hash it was loaded at; a 409 hands back the newer text (HermesClient.writeFile). */
    sealed interface FileSave {
        data class Saved(val hash: String?) : FileSave
        data class Conflict(val hash: String?, val content: String) : FileSave
        data class Failed(val message: String) : FileSave
    }

    suspend fun writeWorkspaceFile(agentId: String, path: String, content: String, expectedHash: String?): FileSave {
        val body = buildJsonObject {
            put("path", path)
            put("content", content)
            if (expectedHash != null) put("expectedHash", expectedHash)
        }
        return when (val r = LibraryNet.send("PUT", "agents/${esc(agentId)}/workspace/file", body)) {
            is ApiResult.Ok -> FileSave.Saved(r.value.obj.str("hash"))
            is ApiResult.HttpError -> if (r.status == 409) {
                when (val again = readWorkspaceFile(agentId, path)) {
                    is BotsResult.Ok -> FileSave.Conflict(again.value.third, again.value.second)
                    is BotsResult.Failed -> FileSave.Failed(again.message)
                }
            } else FileSave.Failed("Couldn't save: " + r.friendly("save the file"))
            else -> FileSave.Failed("Couldn't save: " + r.friendly("save the file"))
        }
    }

    // Threads (BotModeClient / ThreadsClient)
    suspend fun listAllThreads(): BotsResult<List<BotThread>> = get("threads?includeResolved=true", "load threads") { j ->
        j.listUnder("threads").mapNotNull(BotThread::from).filter { !it.incognito }
    }

    suspend fun listBotThreads(botId: String): BotsResult<List<BotThread>> = get("threads?botId=${esc(botId)}&includeResolved=true", "load threads") { j ->
        j.listUnder("threads").mapNotNull(BotThread::from).filter { !it.incognito }
    }

    suspend fun listProjectThreads(projectId: String): BotsResult<List<BotThread>> =
        get("threads?projectId=${esc(projectId)}&includeResolved=true", "load threads") { j -> j.listUnder("threads").mapNotNull(BotThread::from).filter { !it.incognito } }

    suspend fun threadAction(id: String, action: String): BotsResult<JsonElement> =
        send("POST", "threads/${esc(id)}/$action", if (action == "resolve") buildJsonObject { put("status", "done") } else null, "update the thread")

    // Bot projects
    suspend fun listBotProjects(): BotsResult<List<BotProjectSummary>> = get("bot-projects", "load bot projects") { j ->
        j.listUnder("projects").mapNotNull(BotProjectSummary::from)
    }

    suspend fun createBotProject(title: String): BotsResult<String> =
        when (val r = send("POST", "cowork/projects", buildJsonObject { put("title", title); put("metadata", "{\"kind\":\"bots\"}") }, "start the project")) {
            is BotsResult.Ok -> (r.value.obj?.child("project")?.str("id") ?: r.value.obj?.str("id"))?.let { BotsResult.Ok(it) } ?: BotsResult.Failed("The project wasn't created.")
            is BotsResult.Failed -> r
        }

    suspend fun sendToCoordinator(projectId: String, text: String): BotsResult<JsonElement> =
        send("POST", "projects/${esc(projectId)}/messages", buildJsonObject { put("text", text) }, "send it")

    suspend fun projectMessages(projectId: String): BotsResult<List<JsonObject>> =
        get("projects/${esc(projectId)}/messages", "load the project") { j -> j.listUnder("messages") }

    suspend fun coworkProject(projectId: String): BotsResult<JsonObject> =
        get("cowork/projects/${esc(projectId)}", "load the project") { j -> j.obj?.child("project") ?: j.obj ?: JsonObject(emptyMap()) }

    suspend fun patchCoworkProject(projectId: String, body: JsonObject): BotsResult<JsonElement> =
        send("PATCH", "cowork/projects/${esc(projectId)}", body, "save the project")

    suspend fun closeCoworkProject(projectId: String): BotsResult<JsonElement> =
        send("POST", "cowork/projects/${esc(projectId)}/close", null, "close the project")

    // Groups
    suspend fun listGroups(): BotsResult<List<BotGroupChat>> = get("bot-groups", "load group chats") { j ->
        j.listUnder("groups").mapNotNull { o -> GroupRoom.fromRecord(o)?.summary() }
    }

    /** iOS BotModeClient.saveGroup: `PUT bot-groups` with the members and the last 80 lines as JSON text. */
    suspend fun saveGroup(room: GroupRoom): BotsResult<JsonElement> = send("PUT", "bot-groups", room.record(), "save the group chat")

    suspend fun deleteGroup(id: String): BotsResult<JsonElement> = send("DELETE", "bot-groups/${esc(id)}", null, "delete the group chat")

    /** A plain cloud-api call for the group runner, the zip import and the impact map. */
    suspend fun call(method: String, path: String, body: JsonElement?, doing: String): BotsResult<JsonElement> = send(method, path, body, doing)

    suspend fun fetch(path: String, doing: String): BotsResult<JsonElement> = get(path, doing) { it }

    // Memory and channels
    suspend fun listMemories(botId: String): BotsResult<List<BotMemoryItem>> = get("cowork/memory?bot=${esc(botId)}&limit=200", "load memory") { j ->
        j.listUnder("memories").mapNotNull { o -> BotMemoryItem(o.str("id") ?: return@mapNotNull null, o.str("content").orEmpty(), o.str("type") ?: "fact", o.str("source"), o.str("created_at").orEmpty()) }
    }

    suspend fun addMemory(botId: String, content: String): BotsResult<JsonElement> =
        send("POST", "cowork/memory", buildJsonObject { put("bot", botId); put("content", content); put("type", "fact"); put("source", "user") }, "teach the bot")

    suspend fun forgetMemory(id: String): BotsResult<JsonElement> = send("DELETE", "cowork/memory/${esc(id)}", null, "forget that")

    suspend fun listChannels(botId: String): BotsResult<List<BotChannel>> = get("gateway/bots/${esc(botId)}/channels", "load channels") { j ->
        j.listUnder("channels").mapNotNull { o ->
            BotChannel(o.str("accountId") ?: return@mapNotNull null, o.str("provider") ?: "channel", o.str("displayName"), o.str("handle"), o.str("state") ?: "unknown", o.bool("enabled") == true, o.bool("isDefault") == true)
        }
    }

    suspend fun setChannel(botId: String, accountId: String, enabled: Boolean): BotsResult<JsonElement> =
        send("PUT", "gateway/bots/${esc(botId)}/channels/${esc(accountId)}", buildJsonObject { put("enabled", enabled) }, "change the channel")

    // Sessions
    suspend fun createBotSession(bot: BotRecord, name: String, metadata: Map<String, String>): BotsResult<String> {
        val body = buildJsonObject {
            put("name", name)
            put("originSurface", "chat")
            put("sessionMode", "agent")
            put("agentId", bot.id)
            put("agentName", bot.displayName)
            putJsonObject("metadata") {
                metadata.forEach { (k, v) -> put(k, v) }
                put("isBot", "true")
                put("agentId", bot.id)
                put("botName", bot.displayName)
            }
        }
        return when (val r = send("POST", "agent-sessions", body, "open the chat")) {
            is BotsResult.Ok -> (r.value.obj?.str("id") ?: r.value.obj?.child("session")?.str("id"))?.let { BotsResult.Ok(it) } ?: BotsResult.Failed("The chat wasn't created.")
            is BotsResult.Failed -> r
        }
    }

    suspend fun storeGreeting(sessionId: String, text: String) {
        send("POST", "agent-sessions/${esc(sessionId)}/messages", buildJsonObject {
            put("text", text); put("role", "assistant"); put("noReply", true)
            putJsonObject("metadata") { put("source", "bot-greeting") }
        }, "store the greeting")
    }

    suspend fun sendUserMessage(sessionId: String, text: String): BotsResult<JsonElement> =
        send("POST", "agent-sessions/${esc(sessionId)}/messages", buildJsonObject {
            put("text", text)
            putJsonObject("metadata") { put("time_zone", java.util.TimeZone.getDefault().id) }
        }, "send it")

    suspend fun listSessions(): BotsResult<List<BotSessionRow>> = get("agent-sessions", "load sessions") { j ->
        j.listUnder("sessions", "items").mapNotNull { o ->
            BotSessionRow(o.str("id") ?: return@mapNotNull null, o.str("name") ?: o.str("title") ?: "Chat", o.str("agentId") ?: o.str("agent_id"),
                o.str("agentName") ?: o.str("agent_name"), o.num("messageCount")?.toInt() ?: o.num("message_count")?.toInt() ?: 0, o.str("updatedAt") ?: o.str("updated_at").orEmpty())
        }
    }

    // Routines, summary, computers, webhooks
    suspend fun listRoutines(): BotsResult<List<AutomationRoutine>> = get("automation/routines?recentRuns=0", "load routines") { j ->
        j.listUnder("routines").mapNotNull { o ->
            AutomationRoutine(o.str("id") ?: return@mapNotNull null, o.str("name").orEmpty(), o.str("agentId") ?: o.str("agent_id"),
                o.str("scheduleType") ?: o.str("schedule_type"), o.str("scheduleExpression") ?: o.str("schedule_expression").orEmpty(),
                (o.bool("isEnabled") ?: o.bool("enabled")) != false)
        }
    }

    suspend fun updateSchedule(id: String, cron: String): BotsResult<JsonElement> =
        send("PATCH", "automation/routines/${esc(id)}", buildJsonObject { put("scheduleType", "cron"); put("scheduleExpression", cron); put("timezone", java.util.TimeZone.getDefault().id) }, "save the schedule")

    suspend fun summary(botId: String): BotsResult<BotSummary> = get("bot-hub/${esc(botId)}/summary", "load the summary") { j ->
        val o = j.obj
        BotSummary(o.str("model"), o.num("sessions")?.toInt(), o.num("messages")?.toInt(), o.long("tokens"), o.num("costUsd") ?: o.num("cost_usd"), o.num("memories")?.toInt(), o.num("skills")?.toInt())
    }

    suspend fun listComputers(botId: String): BotsResult<List<ComputerRow>> = get("computers?bot_id=${esc(botId)}", "load computers") { j ->
        j.listUnder("computers").mapNotNull { o -> ComputerRow(o.str("id") ?: return@mapNotNull null, o.str("kind") ?: "managed", o.str("status") ?: "unknown", o.str("name") ?: "Computer", o.str("bot_id")) }
    }

    suspend fun listWebhooks(): BotsResult<List<WebhookSubscription>> = get("beta/webhooks", "load webhooks") { j ->
        j.listUnder("subscriptions").mapNotNull { o ->
            WebhookSubscription(o.str("id") ?: return@mapNotNull null, o.str("url").orEmpty(),
                (o["events"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }.orEmpty(), o.bool("active") == true)
        }
    }

    // Marketplace (AgentMarketplaceClient)
    suspend fun listListings(query: String?): BotsResult<List<MarketplaceListing>> =
        get("agent-marketplace/listings" + (query?.takeIf { it.isNotBlank() }?.let { "?q=${esc(it)}" } ?: ""), "load agents") { j -> j.listUnder("listings").mapNotNull(::listing) }

    suspend fun getListing(id: String): BotsResult<Pair<MarketplaceListing, List<Triple<Int, String?, String?>>>> =
        get("agent-marketplace/listings/${esc(id)}", "load agent details") { j ->
            val o = j.obj
            val l = listing(o?.child("listing") ?: o ?: JsonObject(emptyMap())) ?: MarketplaceListing(id, "Agent", "", null, emptyList(), null, 0.0, 0, 0)
            l to o.listUnderKey("reviews").map { r -> Triple(r.num("rating")?.toInt() ?: 0, r.str("review"), r.str("reviewer_name") ?: r.str("user_name")) }
        }

    suspend fun install(id: String): BotsResult<String> =
        when (val r = send("POST", "agent-marketplace/listings/${esc(id)}/install", null, "install the agent")) {
            is BotsResult.Ok -> BotsResult.Ok(r.value.obj?.str("agent_id") ?: r.value.obj?.child("agent")?.str("id").orEmpty())
            is BotsResult.Failed -> r
        }

    suspend fun rate(id: String, rating: Int, review: String?): BotsResult<JsonElement> =
        send("POST", "agent-marketplace/listings/${esc(id)}/rate", buildJsonObject { put("rating", rating); if (!review.isNullOrBlank()) put("review", review) }, "save the rating")

    suspend fun publish(sourceAgentId: String, title: String, description: String, category: String?, tags: List<String>): BotsResult<JsonElement> =
        send("POST", "agent-marketplace/listings", buildJsonObject {
            put("source_agent_id", sourceAgentId); put("title", title); put("description", description)
            if (!category.isNullOrBlank()) put("category", category)
            if (tags.isNotEmpty()) put("tags", JsonArray(tags.map { JsonPrimitive(it) }))
        }, "publish the agent")

    private fun listing(o: JsonObject): MarketplaceListing? = MarketplaceListing(
        o.str("id") ?: return null, o.str("title") ?: "Agent", o.str("description").orEmpty(), o.str("category"),
        (o["tags"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }.orEmpty(),
        o.str("publisher_name"), o.num("rating_avg") ?: 0.0, o.num("rating_count")?.toInt() ?: 0, o.num("install_count")?.toInt() ?: 0,
    )

    private fun JsonObject?.listUnderKey(key: String): List<JsonObject> = (this?.get(key) as? JsonArray)?.mapNotNull { it as? JsonObject }.orEmpty()

    // Account pad templates (wave D leftover: "Your templates" in the pad editor)
    suspend fun listPadTemplates(): BotsResult<List<JsonObject>> = get("templates?kind=project-pad", "load your templates") { j -> j.listUnder("templates") }
}

/** Runtime routes on the user's computer (bots pause, the Factory mail threads) through the chat relay. */
internal suspend fun ChatBackend.runtimeResult(method: String, path: String, body: String? = null, doing: String): BotsResult<JsonElement> =
    when (val r = runtimeJson(method, path, body)) {
        is RuntimeCall.Ok -> BotsResult.Ok(r.body)
        RuntimeCall.NoComputer -> BotsResult.Failed("Connect a computer to $doing.")
        is RuntimeCall.Http -> BotsResult.Failed(com.allternit.core.design.FriendlyErrorMessage.from(r.message ?: r.code ?: "HTTP ${r.status}", doing), r.status)
        is RuntimeCall.Failed -> BotsResult.Failed(r.message)
    }

// MARK: - Store

/**
 * Bots mode's shared state (iOS BotModeStore + AgentHubStore.agents): the account's bots, every
 * thread, group chats and bot projects; pins and the canonical main-chat ids live on the device.
 */
class BotModeStore(private val context: Context) {
    private val prefs = context.getSharedPreferences("allternit_bots", Context.MODE_PRIVATE)

    var agents by mutableStateOf<List<BotRecord>>(emptyList()); private set
    var agentsLoading by mutableStateOf(false); private set
    var agentsError by mutableStateOf<String?>(null); private set
    var threads by mutableStateOf<List<BotThread>>(emptyList()); private set
    var threadsError by mutableStateOf<String?>(null); private set
    var isLoadingThreads by mutableStateOf(false); private set
    var groups by mutableStateOf<List<BotGroupChat>>(emptyList()); private set
    var projects by mutableStateOf<List<BotProjectSummary>>(emptyList()); private set
    var pinnedBotIds by mutableStateOf(prefs.getString(PINS, "").orEmpty().split(',').filter { it.isNotEmpty() }); private set
    private val canonical = mutableMapOf<String, String>().apply {
        prefs.getString(CANONICAL, "").orEmpty().split(';').forEach { kv -> kv.split('=').takeIf { it.size == 2 }?.let { put(it[0], it[1]) } }
    }
    private val creatingMain = mutableMapOf<String, Deferred<BotsResult<String>>>()

    /** iOS BotModeStore.bots: pinned first, then by latest thread activity, then by name. */
    val bots: List<BotRecord>
        get() {
            val activity = threads.groupBy { it.botId }.mapValues { (_, l) -> l.maxOfOrNull { it.lastActivityAt }.orEmpty() }
            return agents.filter { it.isBot }.sortedWith(
                compareByDescending<BotRecord> { it.id in pinnedBotIds }
                    .thenByDescending { activity[it.id].orEmpty() }
                    .thenBy { it.displayName.lowercase() },
            )
        }

    fun bot(id: String): BotRecord? = agents.firstOrNull { it.id == id }
    fun isPinned(id: String) = id in pinnedBotIds
    fun togglePin(id: String) {
        pinnedBotIds = if (id in pinnedBotIds) pinnedBotIds - id else pinnedBotIds + id
        prefs.edit().putString(PINS, pinnedBotIds.joinToString(",")).apply()
    }

    val waitingCount: Int get() = threads.count { it.group == "waiting" }
    fun threads(botId: String) = threads.filter { it.botId == botId }.sortedWith(BotThread.order)
    fun waitingCount(botId: String) = threads.count { it.botId == botId && it.group == "waiting" }
    fun workingCount(botId: String) = threads.count { it.botId == botId && it.group == "working" }
    fun openThreadCount(botId: String): Int {
        val main = canonical[botId]
        return threads.count { it.botId == botId && it.group != "resolved" && !it.isMainChat && it.currentSessionId != main }
    }

    suspend fun refreshAll() = coroutineScope {
        val a = async { refreshAgents() }
        val t = async { refreshThreads() }
        val g = async { refreshGroups() }
        val p = async { refreshProjects() }
        a.await(); t.await(); g.await(); p.await()
    }

    suspend fun refreshAgents() {
        agentsLoading = true
        when (val r = BotsApi.listAgents()) {
            is BotsResult.Ok -> { agents = r.value; agentsError = null }
            is BotsResult.Failed -> agentsError = r.message
        }
        agentsLoading = false
    }

    suspend fun refreshThreads() {
        isLoadingThreads = true
        when (val r = BotsApi.listAllThreads()) {
            is BotsResult.Ok -> { threads = r.value; threadsError = null }
            is BotsResult.Failed -> threadsError = r.message
        }
        isLoadingThreads = false
    }

    suspend fun refreshGroups() { BotsApi.listGroups().orNull()?.let { groups = it.sortedByDescending { g -> g.updatedAt } } }

    // Groups (iOS BotModeStore createGroup / saveGroup / deleteGroup)
    fun group(id: String): GroupRoom? = groups.firstOrNull { it.id == id }?.room

    private fun putGroup(room: GroupRoom) {
        val row = room.summary()
        groups = if (groups.any { it.id == room.id }) groups.map { if (it.id == room.id) row else it } else listOf(row) + groups
    }

    /** Shows the room at once, then saves it (a failed save keeps the local copy, like iOS). */
    suspend fun saveGroup(room: GroupRoom) {
        putGroup(room)
        BotsApi.saveGroup(room)
    }

    suspend fun createGroup(name: String, members: List<BotRecord>, vendorIds: Set<String>): BotsResult<GroupRoom> {
        val base = slugify(name).ifEmpty { "group" }
        var id = base
        var n = 2
        while (groups.any { it.id == id }) { id = "$base-$n"; n++ }
        val now = GroupRoom.now()
        val room = GroupRoom(
            id, name, null,
            members.map { GroupMember(it.id, it.displayName, slugify(it.displayName), if (it.id in vendorIds) "vendor" else "native", null) },
            emptyList(), now, now,
        )
        return when (val r = BotsApi.saveGroup(room)) {
            is BotsResult.Ok -> { putGroup(room); BotsResult.Ok(room) }
            is BotsResult.Failed -> r
        }
    }

    suspend fun deleteGroup(id: String): BotsResult<JsonElement> =
        BotsApi.deleteGroup(id).also { if (it is BotsResult.Ok) groups = groups.filter { g -> g.id != id } }
    suspend fun refreshProjects() { projects = BotsApi.listBotProjects().orNull()?.filter { !it.archived }.orEmpty() }

    suspend fun act(thread: BotThread, action: String): BotsResult<JsonElement> = BotsApi.threadAction(thread.id, action).also { refreshThreads() }

    /** iOS mainChatSessionId: the device pin, the server pin, the newest main-chat thread, else a new chat with the greeting. */
    suspend fun mainChatSessionId(bot: BotRecord): BotsResult<String> = coroutineScope {
        canonical[bot.id]?.let { return@coroutineScope BotsResult.Ok(it) }
        bot.canonicalThreadId?.let { remember(it, bot.id); return@coroutineScope BotsResult.Ok(it) }
        val botThreads = BotsApi.listBotThreads(bot.id).orNull() ?: threads(bot.id)
        botThreads.filter { it.isMainChat && it.currentSessionId != null }.maxByOrNull { it.lastActivityAt }?.currentSessionId?.let {
            remember(it, bot.id); return@coroutineScope BotsResult.Ok(it)
        }
        val running = creatingMain[bot.id] ?: async {
            when (val s = BotsApi.createBotSession(bot, "Bot Chat", mapOf("botCanonicalFor" to bot.id))) {
                is BotsResult.Ok -> {
                    if (s.value.startsWith("ses")) BotsApi.storeGreeting(s.value, bot.botGreeting)
                    s
                }
                is BotsResult.Failed -> s
            }
        }.also { creatingMain[bot.id] = it }
        val result = running.await()
        creatingMain.remove(bot.id)
        if (result is BotsResult.Ok) {
            remember(result.value, bot.id)
            // The server pin, so every device opens the same main chat (iOS writeServerPin).
            BotsApi.updateAgent(bot.id, buildJsonObject { put("config", JsonObject(bot.config + ("canonicalThreadId" to JsonPrimitive(result.value)))) })
        }
        result
    }

    suspend fun newThreadSession(bot: BotRecord, title: String = "New thread"): BotsResult<String> =
        BotsApi.createBotSession(bot, title, mapOf("botThreadOf" to bot.id))

    fun forgetMainChat(botId: String) {
        canonical.remove(botId)
        saveCanonical()
    }

    private fun remember(sessionId: String, botId: String) {
        canonical[botId] = sessionId
        saveCanonical()
    }

    private fun saveCanonical() = prefs.edit().putString(CANONICAL, canonical.entries.joinToString(";") { "${it.key}=${it.value}" }).apply()

    companion object {
        private const val PINS = "allternit-bot-mode-pinned"
        private const val CANONICAL = "allternit-bot-mode-canonical-chats"

        /** iOS BotLaunchpadView.projectTitle: the first line, squeezed, at most 60 characters. */
        fun projectTitle(text: String): String {
            val line = text.trim().lineSequence().firstOrNull().orEmpty().split(Regex("\\s+")).filter { it.isNotEmpty() }.joinToString(" ")
            if (line.length > 60) return line.take(57).trim() + "…"
            return line.ifEmpty { "New project" }
        }

        /** iOS BotStore.slugify. */
        fun slugify(text: String): String {
            val out = StringBuilder()
            var lastDash = false
            for (ch in text.lowercase().trim()) {
                if (ch.code < 128 && ch.isLetterOrDigit()) { out.append(ch); lastDash = false } else if (!lastDash) { out.append('-'); lastDash = true }
            }
            return out.toString().trim('-')
        }
    }
}

/** iOS BotTime.short: now, 5m, 3h, 2d, 1w. */
internal fun botTimeShort(iso: String?): String {
    val t = parseTime(iso)
    if (t <= 0) return ""
    val s = ((System.currentTimeMillis() - t) / 1000).coerceAtLeast(0)
    return when {
        s < 60 -> "now"
        s < 3600 -> "${s / 60}m"
        s < 86_400 -> "${s / 3600}h"
        s < 604_800 -> "${s / 86_400}d"
        else -> "${s / 604_800}w"
    }
}

/** iOS BotTime.relative (RelativeDateTimeFormatter, short). */
internal fun botTimeRelative(iso: String?): String = com.allternit.feature.artifacts.relativeTime(parseTime(iso))

internal fun JsonElement?.isNull(): Boolean = this == null || this is JsonNull
