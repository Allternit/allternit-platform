package com.allternit.feature.office

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.arr
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.long
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import com.allternit.feature.bots.BotModeStore
import com.allternit.feature.bots.BotThread
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.PermissionAsk
import com.allternit.feature.chat.QuestionAsk
import com.allternit.feature.chat.QuestionItem
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull

/**
 * One thing a bot is waiting on the person for (step 26, iOS OfficeNeed), in the card's priority:
 * approval (a permission ask), then question, then suggestion (a proposed thread).
 */
sealed interface OfficeNeed {
    val botId: String
    /** The feed's `needs.kind`: approval, question or suggestion. */
    val kind: String
    val id: String

    data class Approval(override val botId: String, val ask: PermissionAsk) : OfficeNeed {
        override val kind: String get() = "approval"
        override val id: String get() = "perm:${ask.id}"
    }
    data class Question(override val botId: String, val ask: QuestionAsk) : OfficeNeed {
        override val kind: String get() = "question"
        override val id: String get() = "q:${ask.id}"
    }
    data class Suggestion(override val botId: String, val thread: BotThread) : OfficeNeed {
        override val kind: String get() = "suggestion"
        override val id: String get() = "s:${thread.id}"
    }
}

private fun OfficeNeed.rank() = when (this) { is OfficeNeed.Approval -> 0; is OfficeNeed.Question -> 1; is OfficeNeed.Suggestion -> 2 }

/**
 * The client side of the floor's needs. The server's `needs.approval` is always 0 (cowork approvals carry no bot), so
 * the approvals come from the same calls the chat's ask cards use: the permission asks on each bot's live threads
 * (`GET permissions?sessionId=`, [ChatBackend.listPermissions]), the open questions (`GET questions`, as the Decisions
 * deck reads them) and the proposed threads ([BotModeStore.threads] status `proposed`).
 */
class OfficeNeeds(private val store: BotModeStore, private val backend: ChatBackend) {
    var items by mutableStateOf<List<OfficeNeed>>(emptyList())
        private set

    /** Items that were just answered, kept out until the next refresh confirms they are gone. */
    private var answered = emptySet<String>()

    suspend fun refresh() {
        val threads = store.threads
        val botOfSession = threads.mapNotNull { t -> t.currentSessionId?.let { it to t.botId } }.toMap()
        // Asks only live on threads that are waiting or still running; at most 16 sessions per pass.
        val live = threads.filter { (it.group == "waiting" || it.group == "working") && it.currentSessionId != null }
            .sortedBy { it.groupOrder }.take(16)
        val approvals = coroutineScope {
            live.map { t -> async { runCatching { backend.listPermissions(t.currentSessionId!!) }.getOrDefault(emptyList()).map { OfficeNeed.Approval(t.botId, it) } } }.awaitAll().flatten()
        }
        val questions = (LibraryNet.get("questions") as? ApiResult.Ok)?.value?.let { v ->
            (v.obj?.list("questions") ?: v.arr.orEmpty().mapNotNull { it as? JsonObject }).mapNotNull(::parseQuestion)
        }.orEmpty().mapNotNull { q -> q.sessionId?.let { botOfSession[it] }?.let { OfficeNeed.Question(it, q) } }
        val suggestions = threads.filter { it.status == "proposed" }.map { OfficeNeed.Suggestion(it.botId, it) }
        val all = (approvals + questions + suggestions).distinctBy { it.id }
        answered = answered.intersect(all.map { it.id }.toSet())
        items = all.filter { it.id !in answered }.sortedBy { it.rank() }
    }

    /** Drops an item at once after it was answered (the next refresh confirms). */
    fun markAnswered(need: OfficeNeed) {
        answered = answered + need.id
        items = items.filter { it.id != need.id }
    }

    fun first(): OfficeNeed? = items.firstOrNull()
    fun firstFor(botId: String): OfficeNeed? = items.firstOrNull { it.botId == botId }
    fun count(botId: String, kind: String) = items.count { it.botId == botId && it.kind == kind }

    /** Bots with something waiting, in need order (first need first), for the pill's dots. */
    fun waitingBotIds(): List<String> = items.map { it.botId }.distinct()

    private fun parseQuestion(o: JsonObject): QuestionAsk? {
        val id = o.str("id") ?: return null
        val qs = (o["questions"] as? JsonArray)?.mapNotNull { el ->
            val q = el as? JsonObject ?: return@mapNotNull null
            QuestionItem(
                q.str("header"), q.str("question") ?: return@mapNotNull null,
                (q["options"] as? JsonArray)?.mapNotNull { opt -> (opt as? JsonObject)?.str("label") ?: (opt as? JsonPrimitive)?.contentOrNull }.orEmpty(),
                (q["multiple"] as? JsonPrimitive)?.contentOrNull == "true",
            )
        }.orEmpty()
        if (qs.isEmpty()) return null
        return QuestionAsk(id, o.str("sessionID") ?: o.str("sessionId") ?: o.str("session_id"), qs)
    }
}

/**
 * The floor state with the client's needs merged in, per bot the larger of the server's and the client's count for each
 * kind (the server reports questions and suggestions, never approvals). Bots the server didn't list get a row.
 */
internal fun FloorState.withClientNeeds(needs: OfficeNeeds): FloorState {
    if (needs.items.isEmpty()) return this
    val merged = HashMap(bots)
    needs.items.map { it.botId }.distinct().forEach { id ->
        val b = merged[id] ?: FloorBot(null, 0, 0, 0, null, 0L)
        merged[id] = b.copy(
            approval = maxOf(b.approval, needs.count(id, "approval")),
            question = maxOf(b.question, needs.count(id, "question")),
            suggestion = maxOf(b.suggestion, needs.count(id, "suggestion")),
        )
    }
    return copy(bots = merged)
}

/** An autopilot answer or draft for a bot (`GET /floor/events` kind `autopilot`, outcome answer|draft). */
data class OfficeAutopilotLine(val id: String, val botId: String, val outcome: String, val need: String?, val text: String?, val at: Long)

/** A done bubble the host adds to the feed after an approval from the card (feed v2 `bubbles` kind `done`). */
data class OfficeDoneBubble(val id: String, val botId: String, val text: String, val at: Long)

/** The autopilot lines of the last day for one bot, newest first; empty on any failure. */
internal suspend fun autopilotLines(botId: String, now: Long = System.currentTimeMillis()): List<OfficeAutopilotLine> {
    val v = (LibraryNet.get("floor/events?since=${now - 86_400_000L}") as? ApiResult.Ok)?.value ?: return emptyList()
    val items = v.obj?.list("events") ?: v.arr.orEmpty().mapNotNull { it as? JsonObject }
    return items.mapNotNull { e ->
        if (e.str("kind") != "autopilot" || e.str("botId") != botId) return@mapNotNull null
        val outcome = e.str("outcome")?.takeIf { it == "answer" || it == "draft" } ?: return@mapNotNull null
        OfficeAutopilotLine(e.str("id") ?: return@mapNotNull null, botId, outcome, e.str("need"), e.str("text"), e.long("at") ?: 0L)
    }.sortedByDescending { it.at }.take(3)
}
