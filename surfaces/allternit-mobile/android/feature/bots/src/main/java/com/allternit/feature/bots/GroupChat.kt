package com.allternit.feature.bots

import android.content.Context
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiJson
import com.allternit.core.api.gen.AgentChatRequest
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.EmptyBox
import com.allternit.core.design.GlassPill
import com.allternit.core.design.ProvideDockScroll
import com.allternit.core.design.SectionLabel
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyErrorMessage
import com.allternit.core.design.Haptics
import com.allternit.core.stream.ChatStreamEvent
import com.allternit.feature.artifacts.LibraryField
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.RelayPick
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.transformWhile
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale
import java.util.UUID
import kotlin.math.roundToInt

// -----------------------------------------------------------------------------
// Group chats (iOS Features/Bots/GroupChatViews.swift + VendorGroupTurn.swift;
// allternit-ai GroupsListView.tsx, GroupChatView.tsx, GroupStage.tsx). Rooms
// persist through `/api/v1/bot-groups`; member turns run on this device the way
// the web's group-chat-turn-runner does: each responding bot answers in a
// short-lived agent session, sees the room history, may "(pass)", and may
// @mention another member into the next round. Bots on another vendor answer
// through their own connection on one standing thread per room.
// -----------------------------------------------------------------------------

/** One member of a room (iOS GroupChatMember). */
data class GroupMember(val botId: String, val displayName: String, val handle: String, val source: String, val providerId: String?) {
    fun json(): JsonObject = buildJsonObject {
        put("botId", botId); put("displayName", displayName); put("handle", handle); put("source", source); put("providerId", providerId)
    }

    companion object {
        fun from(o: JsonObject): GroupMember? {
            val id = o.str("botId") ?: return null
            val name = o.str("displayName") ?: "Bot"
            return GroupMember(id, name, o.str("handle") ?: BotModeStore.slugify(name), o.str("source") ?: "native", o.str("providerId"))
        }
    }
}

/** One line of a room's log (iOS GroupChatMessage). [from] is "user" or "bot". */
data class GroupMessage(val id: String, val from: String, val botId: String?, val displayName: String?, val text: String, val timestamp: String, val threadId: String?) {
    fun json(): JsonObject = buildJsonObject {
        put("id", id); put("from", from); put("botId", botId); put("displayName", displayName); put("text", text); put("timestamp", timestamp); put("threadId", threadId)
    }

    companion object {
        fun from(o: JsonObject): GroupMessage? = GroupMessage(
            o.str("id") ?: return null, o.str("from") ?: "bot", o.str("botId"), o.str("displayName"), o.str("text").orEmpty(), o.str("timestamp").orEmpty(), o.str("threadId"),
        )
    }
}

/** A room as the app uses it (iOS BotGroupChat), decoded from the server's BotGroupRecord. */
data class GroupRoom(
    val id: String,
    val name: String,
    val image: String?,
    val members: List<GroupMember>,
    val log: List<GroupMessage>,
    val createdAt: String,
    val updatedAt: String,
) {
    /** The roster row (members, the latest line). */
    fun summary(): BotGroupChat {
        val last = log.lastOrNull()
        val who = last?.let { if (it.from == "user") "You" else it.displayName ?: "Bot" }
        return BotGroupChat(id, name, members.map { it.botId }, members.map { it.displayName }, last?.let { "$who: ${it.text}" }, updatedAt, this)
    }

    /** The wire record: members and the last 80 lines as JSON text (snake_case keys). */
    fun record(): JsonObject = buildJsonObject {
        put("id", id); put("name", name); put("image", image)
        put("members_json", JsonArray(members.map { it.json() }).toString())
        put("log_json", JsonArray(log.takeLast(80).map { it.json() }).toString())
        put("created_at", createdAt); put("updated_at", updatedAt)
    }

    fun appending(m: GroupMessage) = copy(log = log + m, updatedAt = m.timestamp)

    companion object {
        fun now(): String = Instant.now().toString()

        private fun parseList(text: String?): List<JsonObject> =
            runCatching { ApiJson.default.parseToJsonElement(text ?: "[]") as? JsonArray }.getOrNull()?.mapNotNull { it as? JsonObject }.orEmpty()

        fun fromRecord(o: JsonObject): GroupRoom? {
            val id = o.str("id") ?: return null
            return GroupRoom(
                id, o.str("name") ?: "Group", o.str("image"),
                parseList(o.str("members_json")).mapNotNull { GroupMember.from(it) },
                parseList(o.str("log_json")).mapNotNull { GroupMessage.from(it) },
                o.str("created_at").orEmpty(), o.str("updated_at").orEmpty(),
            )
        }
    }
}

// MARK: - One bot turn (iOS AgentChatClient createSession + sendMessageStream)

/** Runs one agent turn through the relay and returns the reply text (group members, Edit with chat). */
internal object BotTurn {
    /** The computer the turn runs on, or why there is none. */
    suspend fun runtime(backend: ChatBackend): BotsResult<String> = when (val pick = backend.relay.resolve()) {
        is RelayPick.Computer -> BotsResult.Ok(pick.runtimeId)
        RelayPick.NoComputer -> BotsResult.Failed("Connect a computer so your bots can answer.")
        RelayPick.Unreachable -> BotsResult.Failed("Your computer isn't answering. Open Allternit Desktop or wake your cloud computer.")
        is RelayPick.Failed -> BotsResult.Failed(pick.message)
    }

    suspend fun session(backend: ChatBackend, name: String, agentId: String, agentName: String): BotsResult<String> =
        when (val r = backend.createSession(name = name, originSurface = "chat", agentId = agentId, agentName = agentName, agentModeId = null, permissionMode = null, sessionMode = "agent")) {
            is com.allternit.core.api.ApiResult.Ok -> BotsResult.Ok(r.value.id)
            else -> BotsResult.Failed("Couldn't start a session for $agentName.")
        }

    /** Streams the reply; [onText] gets the text so far. A stop keeps what came in. */
    suspend fun stream(
        backend: ChatBackend,
        runtimeId: String,
        sessionId: String,
        agentId: String,
        text: String,
        systemPrompt: String?,
        isCancelled: () -> Boolean,
        onText: (String) -> Unit = {},
    ): BotsResult<String> {
        val out = StringBuilder()
        var failure: String? = null
        try {
            backend.streamTurn(runtimeId, AgentChatRequest(chatId = sessionId, message = text, agentId = agentId, systemPrompt = systemPrompt))
                .transformWhile { e -> emit(e); !e.isTerminal && !isCancelled() }
                .collect { e ->
                    when (e) {
                        is ChatStreamEvent.TextDelta -> { out.append(e.text); onText(out.toString()) }
                        is ChatStreamEvent.Failed -> failure = FriendlyErrorMessage.from(e.message ?: e.code ?: "The reply didn't finish")
                        else -> Unit
                    }
                }
        } catch (c: CancellationException) {
            throw c
        } catch (t: Throwable) {
            failure = FriendlyErrorMessage.from(t.message ?: "network error")
        }
        val f = failure
        return if (f != null && out.isEmpty()) BotsResult.Failed(f) else BotsResult.Ok(out.toString())
    }
}

// MARK: - Vendor members (iOS VendorGroupTurn)

/** A bot's execution binding: "vendor" bots answer through their own connection. */
internal data class ExecutionBinding(val botId: String, val type: String, val vendor: String?, val externalAgentName: String?) {
    val isVendor get() = type == "vendor"
}

internal object VendorGroupTurn {
    private const val PENDING_TIMEOUT_MS = 150_000L
    private const val POLL_MS = 2_000L
    private const val ROOM_THREADS = "allternit-group-vendor-threads"
    private val cache = mutableMapOf<String, ExecutionBinding?>()
    private var bulkAt = 0L

    private fun binding(o: JsonObject?): ExecutionBinding? = o?.let {
        ExecutionBinding(it.str("botId").orEmpty(), it.str("type") ?: "allternit", it.str("vendor"), it.str("externalAgentName"))
    }

    /** Cached per launch; refreshed in bulk at most every 30 s. */
    suspend fun binding(botId: String): ExecutionBinding? {
        if (cache.containsKey(botId)) return cache[botId]
        if (System.currentTimeMillis() - bulkAt > 30_000) {
            val list = BotsApi.fetch("gateway/execution-bindings", "load bot connections").orNull() as? JsonObject
            if (list != null) {
                bulkAt = System.currentTimeMillis()
                (list["bindings"] as? JsonArray)?.mapNotNull { binding(it as? JsonObject) }?.filter { it.botId.isNotEmpty() }?.forEach { cache[it.botId] = it }
                if (!cache.containsKey(botId)) cache[botId] = null
                return cache[botId]
            }
        }
        val env = BotsApi.fetch("gateway/bots/${BotsApi.esc(botId)}/execution-binding", "load the bot connection").orNull() as? JsonObject
        return binding(env?.get("binding") as? JsonObject).also { cache[botId] = it }
    }

    fun invalidate() { cache.clear(); bulkAt = 0 }

    suspend fun vendorBound(members: List<GroupMember>): Set<String> =
        members.filter { it.source == "vendor" || binding(it.botId)?.isVendor == true }.map { it.botId }.toSet()

    /** Which bots run on another vendor, with the vendor's name for "Runs on …" (iOS GroupVendorLabels). */
    suspend fun labels(botIds: List<String>): Map<String, String> = botIds.mapNotNull { id ->
        val b = binding(id)?.takeIf { it.isVendor } ?: return@mapNotNull null
        id to (b.externalAgentName ?: b.vendor?.split('-')?.joinToString(" ") { w -> w.replaceFirstChar { it.uppercase() } } ?: "another service")
    }.toMap()

    private fun remembered(context: Context, room: String, botId: String): String? =
        context.getSharedPreferences(ROOM_THREADS, Context.MODE_PRIVATE).getString("$room|$botId", null)

    private fun remember(context: Context, threadId: String, room: String, botId: String) =
        context.getSharedPreferences(ROOM_THREADS, Context.MODE_PRIVATE).edit().putString("$room|$botId", threadId).apply()

    /** The room's standing thread for this bot: this device's, else another device's, else a new one. */
    private suspend fun roomThread(context: Context, room: GroupRoom, m: GroupMember): BotsResult<Pair<String, String>> {
        remembered(context, room.id, m.botId)?.let { known ->
            val sid = ((BotsApi.fetch("threads/${BotsApi.esc(known)}", "load the room thread").orNull() as? JsonObject)?.get("thread") as? JsonObject)?.str("currentSessionId")
            if (sid != null) return BotsResult.Ok(known to sid)
        }
        val title = "#${room.name}"
        val list = (BotsApi.fetch("threads?botId=${BotsApi.esc(m.botId)}", "load threads").orNull() as? JsonObject)?.get("threads") as? JsonArray
        list?.mapNotNull { it as? JsonObject }?.firstOrNull { it.str("title") == title && it.str("createdBy") == "group" && it.str("currentSessionId") != null }?.let { t ->
            val id = t.str("id").orEmpty()
            remember(context, id, room.id, m.botId)
            return BotsResult.Ok(id to t.str("currentSessionId").orEmpty())
        }
        val body = buildJsonObject {
            put("botId", m.botId); put("title", title); put("kind", "standing"); put("objective", "Group chat \"${room.name}\""); put("createdBy", "group")
        }
        return when (val r = BotsApi.call("POST", "threads", body, "start ${m.displayName}'s room thread")) {
            is BotsResult.Ok -> {
                val t = (r.value as? JsonObject)?.get("thread") as? JsonObject
                val sid = t?.str("currentSessionId")
                val id = t?.str("id")
                if (sid == null || id == null) BotsResult.Failed("No session for ${m.displayName}'s room thread")
                else { remember(context, id, room.id, m.botId); BotsResult.Ok(id to sid) }
            }
            is BotsResult.Failed -> r
        }
    }

    private fun obj(e: JsonElement?) = e as? JsonObject
    private fun s(e: JsonElement?) = (e as? JsonPrimitive)?.contentOrNull

    private fun correlation(e: JsonObject): String? {
        val inner = obj(e["event"]) ?: obj(e["payload"])
        return s(e["correlationId"]) ?: s(e["correlation_id"]) ?: s(inner?.get("correlationId")) ?: s(inner?.get("correlation_id"))
    }

    private fun reply(e: JsonObject): String? {
        val inner = obj(e["event"]) ?: e
        if ((s(inner["type"]) ?: s(e["type"])) != "agent.message.completed") return null
        val payload = obj(inner["payload"]) ?: obj(e["payload"])
        return s(payload?.get("reply")) ?: s(payload?.get("text")) ?: s(inner["content"])
    }

    private suspend fun awaitPending(threadId: String, correlationId: String, isCancelled: () -> Boolean): String? {
        val deadline = System.currentTimeMillis() + PENDING_TIMEOUT_MS
        var cursor: String? = null
        while (System.currentTimeMillis() < deadline) {
            if (isCancelled()) return null
            BotsApi.call("POST", "threads/${BotsApi.esc(threadId)}/gateway/sync", JsonObject(emptyMap()), "sync")
            val after = cursor?.let { "?after=${BotsApi.esc(it)}" }.orEmpty()
            (BotsApi.fetch("threads/${BotsApi.esc(threadId)}/events$after", "read the thread").orNull() as? JsonObject)?.let { page ->
                (page["events"] as? JsonArray)?.mapNotNull { it as? JsonObject }?.forEach { e -> if (correlation(e) == correlationId) reply(e)?.let { return it } }
                cursor = (page["cursor"] as? JsonPrimitive)?.contentOrNull?.let { c -> c.toDoubleOrNull()?.toLong()?.toString() ?: c } ?: cursor
            }
            delay(POLL_MS)
        }
        return null
    }

    /** One vendor member turn: text, or nothing plus why (needs approval, signed out, timed out). */
    suspend fun run(context: Context, room: GroupRoom, m: GroupMember, prompt: String, isCancelled: () -> Boolean): Pair<String?, String?> {
        val correlationId = "room-${room.id}-${m.botId}-${System.currentTimeMillis()}-${UUID.randomUUID().toString().take(6)}"
        val thread = when (val t = roomThread(context, room, m)) {
            is BotsResult.Ok -> t.value
            is BotsResult.Failed -> return null to "${m.displayName} couldn't be reached."
        }
        val body = buildJsonObject { put("text", prompt); putJsonObject("metadata") { put("correlationId", correlationId) } }
        val first = when (val r = BotsApi.call("POST", "agent-sessions/${BotsApi.esc(thread.second)}/messages", body, "ask ${m.displayName}")) {
            is BotsResult.Ok -> r.value as? JsonObject
            is BotsResult.Failed -> return null to when (r.status) {
                428 -> "${m.displayName} needs your approval in its chat."
                409 -> "${m.displayName} needs attention: ${r.message}"
                else -> "${m.displayName} couldn't be reached."
            }
        }
        val pending = (obj(first?.get("metadata"))?.get("pending") as? JsonPrimitive)?.contentOrNull == "true"
        var text = if (pending) null else first?.str("content")?.trim()
        if (text.isNullOrEmpty()) text = awaitPending(thread.first, correlationId, isCancelled)?.trim()
        BotsApi.call("POST", "threads/${BotsApi.esc(thread.first)}/gateway/sync", JsonObject(emptyMap()), "sync")
        if (text.isNullOrEmpty()) return null to (if (isCancelled()) null else "${m.displayName} didn't answer in time.")
        if (text.lowercase() in GroupTurnRunner.PASS) return null to null
        return text to null
    }
}

// MARK: - Turn runner (lib/bots/group-chat-turn-runner.ts, native subset)

internal class GroupTurnRunner(private val context: Context, private val backend: ChatBackend) {
    data class Outcome(val replies: Int, val cancelled: Boolean, val problems: List<String>)

    @Volatile private var cancelled = false
    fun cancel() { cancelled = true }

    suspend fun run(start: GroupRoom, userText: String, bots: List<BotRecord>, onSpeaker: (String?) -> Unit, onReply: (GroupRoom) -> Unit): Outcome {
        cancelled = false
        var room = start
        var replies = 0
        var responders = responders(userText, room.members)
        val all = mutableListOf<GroupMessage>()
        val problems = mutableListOf<String>()
        val vendorBound = VendorGroupTurn.vendorBound(room.members)
        var runtimeId: String? = null
        try {
            for (round in 0 until MAX_ROUNDS) {
                if (responders.isEmpty() || cancelled) break
                val roundReplies = mutableListOf<GroupMessage>()
                for (m in responders) {
                    if (cancelled || replies >= MAX_PER_SEND) continue
                    val answer: String? = if (m.botId in vendorBound) {
                        onSpeaker(m.botId)
                        val (text, problem) = VendorGroupTurn.run(context, room, m, prompt(m, room, userText)) { cancelled }
                        problem?.let { problems.add(it) }
                        text
                    } else {
                        val bot = bots.firstOrNull { it.id == m.botId } ?: continue
                        val rid = runtimeId ?: when (val r = BotTurn.runtime(backend)) {
                            is BotsResult.Ok -> r.value.also { runtimeId = it }
                            is BotsResult.Failed -> { problems.add(r.message); return Outcome(replies, cancelled, problems.distinct()) }
                        }
                        onSpeaker(m.botId)
                        reply(rid, m, bot, room, userText)
                    }
                    val text = answer ?: continue
                    val msg = GroupMessage(UUID.randomUUID().toString(), "bot", m.botId, m.displayName, text, GroupRoom.now(), null)
                    room = room.appending(msg)
                    roundReplies.add(msg)
                    all.add(msg)
                    replies++
                    onReply(room)
                }
                // Only members @mentioned by this round's replies go again.
                val named = roundReplies.flatMap { mentions(it.text) }.toSet()
                responders = if (named.isEmpty()) emptyList() else room.members.filter { it.handle.lowercase() in named || it.displayName.lowercase() in named }
            }
        } finally {
            onSpeaker(null)
            rememberRound(room, userText, all)
        }
        return Outcome(replies, cancelled, problems.distinct())
    }

    private suspend fun reply(runtimeId: String, m: GroupMember, bot: BotRecord, room: GroupRoom, userText: String): String? {
        val system = listOf("You are ${m.displayName}. You must ALWAYS identify yourself as ${m.displayName}.", bot.systemPrompt.orEmpty()).filter { it.isNotEmpty() }.joinToString("\n\n")
        val sid = BotTurn.session(backend, m.displayName, bot.id, m.displayName).orNull() ?: return null
        try {
            val text = BotTurn.stream(backend, runtimeId, sid, bot.id, prompt(m, room, userText), system, { cancelled }).orNull()?.trim().orEmpty()
            return if (text.isEmpty() || text.lowercase() in PASS) null else text
        } finally {
            backend.deleteSession(sid)
        }
    }

    /** Each member that took part keeps one memory of the round, so its main chat knows (group-chat-memory.ts). */
    private suspend fun rememberRound(room: GroupRoom, userText: String, replies: List<GroupMessage>) {
        if (replies.isEmpty()) return
        room.members.forEach { m ->
            val line = roundMemory(room, m, userText, replies) ?: return@forEach
            BotsApi.call("POST", "cowork/memory", buildJsonObject { put("bot", m.botId); put("content", line); put("type", "group-chat"); put("source", "group-chat") }, "remember the round")
        }
    }

    companion object {
        const val MAX_ROUNDS = 3
        const val MAX_PER_SEND = 10
        const val MAX_HISTORY = 30
        val PASS = setOf("(pass)", "pass", "pass.")

        /** The member's turn prompt: who's in the room, the recent log, what the user said (buildMemberHistory). */
        fun prompt(m: GroupMember, room: GroupRoom, userText: String): String = listOf(
            "You are ${m.displayName}, a member of the group chat \"${room.name}\".",
            "Other members in this room: ${room.members.joinToString(", ") { it.displayName }}.",
            "",
            "Recent conversation:",
            room.log.takeLast(MAX_HISTORY).joinToString("\n") { "${it.displayName ?: "User"}: ${it.text}" },
            "",
            "The user said: $userText",
            "",
            "Reply briefly as ${m.displayName}, or say \"(pass)\" to stay silent.",
            "You may @mention another member to pull them into the next round.",
            "Escalate to @user if a real human judgment is needed.",
        ).joinToString("\n")

        fun mentions(text: String): List<String> = text.split(Regex("\\s+")).mapNotNull { w ->
            if (!w.startsWith("@")) return@mapNotNull null
            val name = w.drop(1).takeWhile { it.isLetterOrDigit() || it == '_' || it == '-' }
            name.lowercase().takeIf { it.isNotEmpty() }
        }

        fun responders(userText: String, members: List<GroupMember>): List<GroupMember> {
            val named = mentions(userText).toSet()
            if (named.isEmpty() || "everyone" in named || "all" in named) return members
            val picked = members.filter { it.handle.lowercase() in named || it.displayName.lowercase() in named || it.botId.lowercase() in named }
            return picked.ifEmpty { members }
        }

        fun roundMemory(room: GroupRoom, m: GroupMember, userText: String, replies: List<GroupMessage>, now: Instant = Instant.now()): String? {
            fun clip(t: String, max: Int): String { val x = t.split(Regex("\\s+")).filter { it.isNotEmpty() }.joinToString(" "); return if (x.length > max) x.take(max - 1) + "…" else x }
            fun addresses(t: String): Boolean { val l = t.lowercase(); return "@${m.handle.lowercase()}" in l || "@${m.displayName.lowercase()}" in l }
            val own = replies.filter { it.botId == m.botId }.map { it.text }
            val asks = replies.filter { it.botId != m.botId && addresses(it.text) }.map { "${it.displayName ?: "A member"} asked you: \"${clip(it.text, 220)}\"" }
            if (own.isEmpty() && asks.isEmpty()) return null
            val stamp = DateTimeFormatter.ofPattern("MMM d, h:mm a", Locale.US).withZone(ZoneId.systemDefault()).format(now)
            return (listOf(
                "Group chat \"#${room.name}\" ($stamp).",
                "Your user said: \"${clip(userText, 220)}\"",
                if (own.isEmpty()) "" else "You said: \"${clip(own.joinToString(" "), 260)}\"",
            ) + asks).filter { it.isNotEmpty() }.joinToString(" ")
        }
    }
}

// MARK: - Screens

private val GROUP_PALETTE = listOf("#a78bfa", "#f472b6", "#fb923c", "#4ade80", "#60a5fa", "#facc15", "#2dd4bf", "#f87171")

@Composable
private fun MemberFace(store: BotModeStore, botId: String, size: androidx.compose.ui.unit.Dp, live: Boolean = false) {
    val bot = store.bot(botId)
    if (bot != null) BotAvatar(bot, size, live = live) else AgentIdenticon(botId, size)
}

/** A row that slides left to show Delete (iOS swipeActions). */
@Composable
private fun SwipeDeleteRow(testTag: String, onDelete: () -> Unit, content: @Composable () -> Unit) {
    val scope = rememberCoroutineScope()
    val offset = remember { Animatable(0f) }
    val reveal = 88f * LocalContext.current.resources.displayMetrics.density
    Box(Modifier.fillMaxWidth()) {
        Box(Modifier.matchParentSize().background(AllternitStatic.StatusError).clickable { scope.launch { offset.animateTo(0f) }; onDelete() }.testTag("${testTag}Delete"), contentAlignment = Alignment.CenterEnd) {
            Text("Delete", color = Color.White, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(end = 20.dp))
        }
        Box(
            Modifier.offset { IntOffset(offset.value.roundToInt(), 0) }.background(AllternitTheme.colors.bgPanel).pointerInput(Unit) {
                detectHorizontalDragGestures(
                    onDragEnd = { scope.launch { offset.animateTo(if (offset.value < -reveal / 2) -reveal else 0f) } },
                    onHorizontalDrag = { _, d -> scope.launch { offset.snapTo((offset.value + d).coerceIn(-reveal, 0f)) } },
                )
            },
        ) { content() }
    }
}

/**
 * Press feedback that watches the pointer without consuming it, so the child's own click still
 * fires: scales to [scale] while a finger is down, on a quick spring.
 */
@Composable
internal fun Modifier.observedPressScale(scale: Float): Modifier {
    var pressed by remember { mutableStateOf(false) }
    val s by androidx.compose.animation.core.animateFloatAsState(
        if (pressed) scale else 1f,
        androidx.compose.animation.core.spring(dampingRatio = 0.5f, stiffness = 600f),
        label = "observedPress",
    )
    return this
        .pointerInput(Unit) {
            awaitPointerEventScope {
                while (true) {
                    val e = awaitPointerEvent(androidx.compose.ui.input.pointer.PointerEventPass.Initial)
                    pressed = e.changes.any { it.pressed }
                }
            }
        }
        .graphicsLayer { scaleX = s; scaleY = s }
}

/** Group chats (iOS GroupsListView): the open rooms, search, New group chat, swipe to delete. */
@Composable
internal fun GroupsListScreen(store: BotModeStore, nav: BotNavigator) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var creating by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    var loading by remember { mutableStateOf(store.groups.isEmpty()) }
    var error by remember { mutableStateOf<String?>(null) }
    var confirmDelete by remember { mutableStateOf<BotGroupChat?>(null) }
    suspend fun load() {
        loading = true
        when (val r = BotsApi.listGroups()) { is BotsResult.Ok -> { store.refreshGroups(); error = null }; is BotsResult.Failed -> error = r.message }
        loading = false
    }
    LaunchedEffect(Unit) { load() }
    val shown = query.trim().lowercase().let { q -> if (q.isEmpty()) store.groups else store.groups.filter { it.name.lowercase().contains(q) } }
    val scroll = rememberScrollState()
    ProvideDockScroll(scroll) {
    BotScreen("Group chats", nav, testTag = "groupsListScreen", trailing = {
        GlassPill(onClick = { creating = true }, modifier = Modifier.testTag("groupsNew"), height = 36.dp, contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 10.dp)) {
            Icon(painterResource(com.allternit.core.design.R.drawable.ph_plus), "New group chat", tint = c.textPrimary, modifier = Modifier.size(18.dp))
        }
    }) {
        Column(Modifier.fillMaxSize().verticalScroll(scroll).padding(vertical = 8.dp)) {
            if (store.groups.isNotEmpty()) {
                SectionLabel("${store.groups.size} OPEN", Modifier.padding(horizontal = 20.dp))
                Box(Modifier.padding(horizontal = 16.dp)) { LibraryField("", query, "Search group chats", "groupsSearch") { query = it } }
                Gap(8.dp)
            }
            if (store.groups.isEmpty() && !loading && error == null) {
                EmptyBox("No group chats yet.", "Pick two or more bots and start a room.", Modifier.padding(horizontal = 16.dp).testTag("groupsEmptyNew"), onClick = { creating = true })
            } else if (!ListState(loading && store.groups.isEmpty(), error, store.groups.isEmpty(), "Couldn't load group chats", "No group chats yet.") { scope.launch { load() } }) {
                Column(Modifier.padding(horizontal = 16.dp).fillMaxWidth().clip(RoundedCornerShape(20.dp)).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(20.dp))) {
                    shown.forEachIndexed { i, g ->
                        SwipeDeleteRow("groupRow${g.id}", onDelete = { confirmDelete = g }) {
                            Row(Modifier.fillMaxWidth().observedPressScale(0.98f).clickable { nav.open(BotRoute.Group(g.id)) }.testTag("groupRow${g.id}").padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                                GroupFaces(store, g.memberIds, 26.dp)
                                Column(Modifier.weight(1f)) {
                                    Text("# ${g.name}", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    Text(g.lastLine ?: "${g.memberIds.size} bots", color = c.textSecondary, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                }
                                Text(botTimeShort(g.updatedAt), color = c.textTertiary, fontSize = 12.sp)
                            }
                        }
                        if (i < shown.lastIndex) Hairline()
                    }
                }
            }
        }
    }
    }
    if (creating) NewGroupChatSheet(store, onDismiss = { creating = false }) { room -> creating = false; nav.open(BotRoute.Group(room.id)) }
    confirmDelete?.let { g ->
        ConfirmSheet("Delete # ${g.name}?", "The room and its messages are deleted on every device. The bots stay.", "Delete", "groupDeleteConfirm", onDismiss = { confirmDelete = null }) {
            confirmDelete = null
            scope.launch { store.deleteGroup(g.id) }
        }
    }
}

/**
 * A vertical rail of the room's bots on the right edge. Drag a finger along it: the face under the finger
 * grows, its neighbours ease back, and the bot's name shows to the left. Release and it settles. Reduced
 * motion (animator scale 0) skips the spring and the fade; the magnify still follows the finger.
 */
@Composable
private fun GroupScrubRail(store: BotModeStore, members: List<GroupMember>, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    val reduce = remember(LocalContext.current) { android.provider.Settings.Global.getFloat(LocalContext.current.contentResolver, android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }
    val density = androidx.compose.ui.platform.LocalDensity.current
    val slot = 34.dp
    val slotPx = with(density) { slot.toPx() }
    var focus by remember { mutableStateOf<Float?>(null) }
    val shown by androidx.compose.animation.core.animateFloatAsState(
        if (focus != null) 1f else 0f,
        if (reduce) tween(0) else androidx.compose.animation.core.spring(dampingRatio = 0.7f, stiffness = 500f),
        label = "railFocus",
    )
    val label = focus?.let { members.getOrNull(it.roundToInt().coerceIn(0, members.lastIndex)) }
    Row(modifier.padding(end = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        if (label != null) {
            Text(
                label.displayName, color = c.textPrimary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(end = 8.dp).widthIn(max = 140.dp).background(c.bgPanel, RoundedCornerShape(10.dp)).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(10.dp))
                    .padding(horizontal = 8.dp, vertical = 4.dp).testTag("groupRailLabel"),
            )
        }
        Column(
            Modifier.semantics { contentDescription = "Bots in this room. Drag to magnify." }.testTag("groupScrubRail")
                .pointerInput(members.size) {
                    detectVerticalDragGestures(
                        onDragStart = { o -> focus = (o.y / slotPx - 0.5f).coerceIn(0f, members.lastIndex.toFloat()) },
                        onDragEnd = { focus = null },
                        onDragCancel = { focus = null },
                        onVerticalDrag = { change, _ -> focus = (change.position.y / slotPx - 0.5f).coerceIn(0f, members.lastIndex.toFloat()) },
                    )
                },
        ) {
            members.forEachIndexed { i, m ->
                val near = focus?.let { f -> (1f - kotlin.math.abs(f - i) / 1.6f).coerceAtLeast(0f) } ?: 0f
                val scale = 1f + 0.55f * near * shown.coerceAtLeast(near.coerceAtMost(1f))
                Box(Modifier.size(slot), contentAlignment = Alignment.Center) {
                    Box(Modifier.graphicsLayer { scaleX = scale; scaleY = scale; translationX = -(scale - 1f) * 10.dp.toPx() }.clip(CircleShape).border(BorderStroke(1.5.dp, c.bgPanel), CircleShape)) {
                        MemberFace(store, m.botId, 26.dp)
                    }
                }
            }
        }
    }
}

/** Up to four overlapping faces (iOS GroupAvatarStack). */
@Composable
private fun GroupFaces(store: BotModeStore, ids: List<String>, size: androidx.compose.ui.unit.Dp) {
    Box(Modifier.width(size + (size * 0.55f) * (ids.take(4).size - 1).coerceAtLeast(0))) {
        ids.take(4).forEachIndexed { i, id ->
            Box(Modifier.offset(x = size * 0.55f * i).clip(CircleShape).border(BorderStroke(1.5.dp, AllternitTheme.colors.bgPanel), CircleShape)) { MemberFace(store, id, size) }
        }
    }
}

/** Pick 2 to 6 bots and a name (iOS NewGroupChatSheet). */
@Composable
internal fun NewGroupChatSheet(store: BotModeStore, onDismiss: () -> Unit, onCreated: (GroupRoom) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var name by remember { mutableStateOf("") }
    val selected = remember { mutableStateListOf<String>() }
    var vendors by remember { mutableStateOf<Map<String, String>>(emptyMap()) }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(store.bots.size) { vendors = VendorGroupTurn.labels(store.bots.map { it.id }) }
    val canCreate = name.isNotBlank() && selected.size in 2..6 && !saving
    fun create() {
        saving = true
        scope.launch {
            when (val r = store.createGroup(name.trim(), store.bots.filter { it.id in selected }, vendors.keys)) {
                is BotsResult.Ok -> onCreated(r.value)
                is BotsResult.Failed -> error = r.message
            }
            saving = false
        }
    }
    LibrarySheet(
        title = "New group chat", testTag = "newGroupSheet", onDismiss = onDismiss,
        leading = LibrarySheetAction("Cancel", enabled = !saving, onClick = onDismiss),
        trailing = LibrarySheetAction(if (saving) "Creating…" else "Create", enabled = canCreate, bold = true, testTag = "newGroupCreate", onClick = ::create),
    ) {
        Column(Modifier.fillMaxWidth().heightIn(max = 620.dp).verticalScroll(rememberScrollState()).padding(vertical = 8.dp)) {
            SectionHeader("Name")
            Box(Modifier.padding(horizontal = 16.dp)) { LibraryField("", name, "Launch crew", "newGroupName") { name = it } }
            SectionHeader("Members")
            RowGroup {
                store.bots.forEachIndexed { i, b ->
                    val on = b.id in selected
                    Row(
                        Modifier.fillMaxWidth().clickable {
                            if (on) selected.remove(b.id) else if (selected.size < 6) selected.add(b.id)
                        }.testTag("newGroupMember${b.id}").padding(horizontal = 16.dp, vertical = 10.dp),
                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        BotAvatar(b, 28.dp)
                        Column(Modifier.weight(1f)) {
                            com.allternit.core.design.BotNameText(b.displayName, color = c.textPrimary, fontSize = 15.sp)
                            vendors[b.id]?.let { Text("Runs on $it", color = c.textSecondary, fontSize = 12.sp) }
                        }
                        if (on) Text("✓", color = c.accentPrimary, fontSize = 16.sp, fontWeight = FontWeight.Bold)
                    }
                    if (i < store.bots.lastIndex) Hairline()
                }
            }
            Text("Pick 2 to 6 bots. Bots on another service (Grok, Poke…) answer through their own connection.", color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp))
            error?.let { ErrorLine(it) }
        }
    }
}

/** Add bots to a room, up to six members (iOS AddGroupMembersSheet). */
@Composable
private fun AddGroupMembersSheet(room: GroupRoom, store: BotModeStore, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val candidates = store.bots.filter { b -> room.members.none { it.botId == b.id } }
    val space = 6 - room.members.size
    val selected = remember { mutableStateListOf<String>() }
    var vendors by remember { mutableStateOf<Map<String, String>>(emptyMap()) }
    var saving by remember { mutableStateOf(false) }
    LaunchedEffect(candidates.size) { vendors = VendorGroupTurn.labels(candidates.map { it.id }) }
    fun add() {
        saving = true
        val added = candidates.filter { it.id in selected }.map { GroupMember(it.id, it.displayName, BotModeStore.slugify(it.displayName), if (vendors[it.id] != null) "vendor" else "native", null) }
        scope.launch {
            store.saveGroup(room.copy(members = room.members + added, updatedAt = GroupRoom.now()))
            saving = false
            onDismiss()
        }
    }
    LibrarySheet(
        title = "Add members", testTag = "addGroupMembersSheet", onDismiss = onDismiss,
        leading = LibrarySheetAction("Cancel", enabled = !saving, onClick = onDismiss),
        trailing = LibrarySheetAction(if (saving) "Adding…" else "Add", enabled = selected.isNotEmpty() && !saving, bold = true, testTag = "addGroupMembersAdd", onClick = ::add),
    ) {
        Column(Modifier.fillMaxWidth().heightIn(max = 560.dp).verticalScroll(rememberScrollState()).padding(vertical = 8.dp)) {
            when {
                candidates.isEmpty() -> Text("Every bot is already in this room.", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(20.dp))
                space <= 0 -> Text("This room is full (6 members).", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(20.dp))
                else -> {
                    RowGroup {
                        candidates.forEachIndexed { i, b ->
                            val on = b.id in selected
                            Row(
                                Modifier.fillMaxWidth().clickable { if (on) selected.remove(b.id) else if (selected.size < space) selected.add(b.id) }.testTag("addGroupMember${b.id}").padding(horizontal = 16.dp, vertical = 10.dp),
                                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                            ) {
                                BotAvatar(b, 28.dp)
                                Column(Modifier.weight(1f)) {
                                    com.allternit.core.design.BotNameText(b.displayName, color = c.textPrimary, fontSize = 15.sp)
                                    vendors[b.id]?.let { Text("Runs on $it", color = c.textSecondary, fontSize = 12.sp) }
                                }
                                if (on) Text("✓", color = c.accentPrimary, fontSize = 16.sp, fontWeight = FontWeight.Bold)
                            }
                            if (i < candidates.lastIndex) Hairline()
                        }
                    }
                    Text("Up to $space more.", color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp))
                }
            }
        }
    }
}

/**
 * One room (iOS GroupChatView): it opens on the group stage (the members on a strip over the
 * room's brief), and a swipe up or a send flies them into the header's members pill; a tap on the
 * pill brings them back. `@name` targets members; `@everyone` asks all.
 */
@Composable
internal fun GroupRoomScreen(groupId: String, host: BotsHost, store: BotModeStore, nav: BotNavigator) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val runner = remember(groupId) { GroupTurnRunner(context.applicationContext, host.backend) }
    var speaking by remember { mutableStateOf<String?>(null) }
    var running by remember { mutableStateOf(false) }
    var runError by remember { mutableStateOf<String?>(null) }
    var docked by remember(groupId) { mutableStateOf(false) }
    var adding by remember { mutableStateOf(false) }
    DisposableEffect(groupId) { onDispose { runner.cancel() } }
    LaunchedEffect(groupId) { if (store.group(groupId) == null) store.refreshGroups() }
    val room = store.group(groupId)
    if (room == null) {
        BotScreen("Group", nav, testTag = "botGroupScreen") {
            Text("Group not found. This room was deleted on another device.", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(20.dp))
        }
        return
    }
    fun send(raw: String) {
        val text = raw.trim()
        if (text.isEmpty() || running) return
        runError = null
        running = true
        docked = true
        val start = room.appending(GroupMessage(UUID.randomUUID().toString(), "user", null, "You", text, GroupRoom.now(), null))
        scope.launch {
            store.saveGroup(start)
            val out = runner.run(start, text, store.bots, onSpeaker = { speaking = it }, onReply = { updated -> scope.launch { store.saveGroup(updated) } })
            speaking = null
            running = false
            runError = when {
                out.problems.isNotEmpty() -> out.problems.joinToString("\n")
                out.replies == 0 && !out.cancelled -> "Nobody answered. Check that the members' brains are set up."
                else -> null
            }
        }
    }
    val count = room.members.size
    Column(Modifier.fillMaxSize().testTag("botGroupScreen")) {
        BotScreen("# ${room.name}", nav, testTag = "botGroupHeader", trailing = {
            Icon(painterResource(com.allternit.core.design.R.drawable.ph_plus), "Add members", tint = if (running) c.textTertiary else c.textPrimary, modifier = Modifier.size(40.dp).clip(CircleShape).clickable(enabled = !running) { adding = true }.padding(10.dp).testTag("groupAddMembers"))
        }) {
            Column(Modifier.fillMaxSize()) {
                // The members pill: N bots · you; docked, their faces, and a tap brings the stage back.
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center) {
                    Row(
                        Modifier.clip(CircleShape).border(BorderStroke(1.dp, c.borderSubtle), CircleShape).clickable(enabled = docked) { docked = false }
                            .semantics { contentDescription = if (docked) "Show the $count bots" else "$count bots and you" }.testTag("groupMembersPill").padding(horizontal = 10.dp, vertical = 4.dp),
                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp),
                    ) {
                        if (docked) GroupFaces(store, room.members.map { it.botId }, 16.dp)
                        Text("$count ${if (count == 1) "bot" else "bots"} · you", color = c.textSecondary, fontSize = 12.sp)
                    }
                }
                androidx.compose.animation.AnimatedVisibility(
                    visible = !docked,
                    enter = scaleIn(tween(440, easing = CubicBezierEasing(0.65f, 0f, 0.35f, 1f)), initialScale = 0.2f, transformOrigin = androidx.compose.ui.graphics.TransformOrigin(0.5f, 0f)) + fadeIn(tween(440)),
                    exit = scaleOut(tween(440, easing = CubicBezierEasing(0.65f, 0f, 0.35f, 1f)), targetScale = 0.15f, transformOrigin = androidx.compose.ui.graphics.TransformOrigin(0.5f, 0f)) + fadeOut(tween(440)),
                ) {
                    GroupStage(room, store, speaking, onSuggestion = { send(it) }, onDock = { docked = true })
                }
                Box(Modifier.weight(1f)) {
                    GroupLog(room, store, speaking, runError, docked, Modifier.fillMaxSize())
                    if (count > 1) GroupScrubRail(store, room.members, Modifier.align(Alignment.CenterEnd))
                }
                host.composer(
                    BotComposerSpec(
                        sessionOpen = true, onNewThread = { nav.reset() },
                        placeholder = "Message #${room.name} · @ a bot to start a thread",
                        isStreaming = running,
                        onSend = { req -> send(req.text) },
                        onStop = { runner.cancel() },
                    ),
                )
            }
        }
    }
    if (adding) AddGroupMembersSheet(room, store) { adding = false }
}

@Composable
private fun GroupLog(room: GroupRoom, store: BotModeStore, speaking: String?, runError: String?, docked: Boolean, modifier: Modifier) {
    val c = AllternitTheme.colors
    val list = rememberLazyListState()
    LaunchedEffect(room.log.size, speaking) { val n = room.log.size + (if (speaking != null) 1 else 0); if (n > 0) list.animateScrollToItem(n - 1) }
    LazyColumn(modifier.fillMaxWidth().testTag("groupLog"), state = list, contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 14.dp, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        if (room.log.isEmpty() && docked) item { Text("Say hi. Everyone answers, or @mention one of them.", color = c.textSecondary, fontSize = 14.sp) }
        items(room.log, key = { it.id }) { m -> GroupMessageRow(m, store) }
        if (speaking != null) item {
            val bot = store.bot(speaking)
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                MemberFace(store, speaking, 22.dp, live = true)
                Text("${bot?.displayName ?: room.members.firstOrNull { it.botId == speaking }?.displayName ?: "A bot"} is typing…", color = c.textSecondary, fontSize = 13.sp)
            }
        }
        if (runError != null) item { Text(runError, color = AllternitStatic.StatusError, fontSize = 12.sp) }
    }
}

/** A user bubble on the right, a bot line with its face on the left (iOS GroupMessageRow). */
@Composable
private fun GroupMessageRow(m: GroupMessage, store: BotModeStore) {
    val c = AllternitTheme.colors
    if (m.from == "user") {
        Row(Modifier.fillMaxWidth()) {
            Spacer(Modifier.weight(1f).widthIn(min = 40.dp))
            Text(m.text, color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.clip(RoundedCornerShape(16.dp)).background(c.accentPrimary.copy(alpha = 0.14f)).padding(horizontal = 12.dp, vertical = 8.dp))
        }
    } else {
        Row(Modifier.fillMaxWidth().padding(end = 20.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            MemberFace(store, m.botId ?: "bot", 28.dp)
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                com.allternit.core.design.BotNameText(m.displayName ?: "Bot", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                SelectionContainer { Text(m.text, color = c.textPrimary, fontSize = 15.sp) }
            }
        }
    }
}

/**
 * The group stage, phone layout (iOS GroupStageView): the cast on one strip (the one talking
 * lifted), then the room's brief with two suggestions to send. A swipe up docks it.
 */
@Composable
private fun GroupStage(room: GroupRoom, store: BotModeStore, speaking: String?, onSuggestion: (String) -> Unit, onDock: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val reduceMotion = remember { android.provider.Settings.Global.getFloat(context.contentResolver, android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }
    val float by rememberInfiniteTransition(label = "groupFloat").animateFloat(0f, 1f, infiniteRepeatable(tween(1400), RepeatMode.Reverse), label = "groupFloatY")
    val width = LocalConfiguration.current.screenWidthDp
    val face = minOf(48f, (width - 28f) / maxOf(1, room.members.size) - 8f).dp
    fun color(i: Int, botId: String) = hexColor(store.bot(botId)?.accentHex ?: GROUP_PALETTE[i % GROUP_PALETTE.size])
    var drag by remember { mutableStateOf(0f) }
    Column(
        Modifier.fillMaxWidth()
            .background(Brush.radialGradient(listOf(color(0, room.members.firstOrNull()?.botId.orEmpty()).copy(alpha = 0.16f), Color.Transparent)))
            .pointerInput(Unit) {
                detectVerticalDragGestures(onDragStart = { drag = 0f }, onDragEnd = { if (drag < -40f * density) onDock() }) { _, d -> drag += d }
            }
            .testTag("groupStage").padding(start = 14.dp, end = 14.dp, top = 8.dp, bottom = 6.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(Modifier.fillMaxWidth().padding(top = 6.dp), verticalAlignment = Alignment.Bottom) {
            room.members.forEachIndexed { i, m ->
                val talking = m.botId == speaking
                val lift = if (reduceMotion) 0f else if (talking) -4f - 8f * float else -3f * float
                Column(Modifier.weight(1f), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Box(Modifier.offset(y = lift.dp)) { MemberFace(store, m.botId, face, live = talking) }
                    Box(Modifier.width(face * 0.5f).heightIn(min = 3.dp, max = 3.dp).clip(CircleShape).background(color(i, m.botId).copy(alpha = 0.35f)))
                    com.allternit.core.design.BotNameText(m.displayName, color = if (talking) c.textPrimary else c.textSecondary, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
        }
        Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(14.dp)).padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("# ${room.name}", color = c.textTertiary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold)
            val talkingName = speaking?.let { id -> room.members.firstOrNull { it.botId == id }?.displayName }
            val last = room.log.lastOrNull { it.from != "user" && it.text.isNotBlank() }
            when {
                talkingName != null -> Text("$talkingName is speaking…", color = c.textPrimary, fontSize = 14.sp)
                last != null -> Text(buildAnnotatedString {
                    withStyle(SpanStyle(fontWeight = FontWeight.SemiBold)) { append(last.displayName ?: "A bot") }
                    append(": “${last.text.lines().joinToString(" ").take(140)}”")
                }, color = c.textPrimary, fontSize = 14.sp, maxLines = 3, overflow = TextOverflow.Ellipsis)
                else -> Text("Everyone answers, or @mention one of them.", color = c.textSecondary, fontSize = 14.sp)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                (if (room.log.isEmpty()) listOf("Introduce yourselves", "What can this room do?") else listOf("Catch me up", "What's next?")).forEach { s ->
                    Pill(s, "groupSuggestion${s.take(8)}", enabled = speaking == null) { onSuggestion(s) }
                }
            }
        }
    }
}
