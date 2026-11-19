package com.allternit.feature.phones

import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.parseTime
import com.allternit.feature.artifacts.str
import com.allternit.feature.bots.BotRecord
import com.allternit.feature.bots.BotsApi
import com.allternit.feature.bots.BotsResult
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.RuntimeCall
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody

/*
 * Phones data (iOS BotPhoneAPI + BotPhonesStore, AppPhoneService's unpair, AndroidPairing, PhoneTaskClient's
 * run reads, PhoneLiveSession's token calls). Cloud calls carry the account token (LibraryNet.okHttp);
 * calls to the user's computer ride the relay. Nothing here starts a phone task.
 */

/** A raw cloud call: the status and the JSON object body (null when empty or not an object). */
internal object CloudCall {
    suspend fun send(method: String, path: String, body: JsonObject? = null): Pair<Int, JsonObject?> = withContext(Dispatchers.IO) {
        try {
            val url = LibraryNet.config.cloudBaseUrl.toString().trimEnd('/') + "/" + path.trimStart('/')
            val req = Request.Builder().url(url).header("Accept", "application/json")
                .method(method, if (method == "GET" || method == "HEAD") null else (body?.toString() ?: "{}").toRequestBody("application/json".toMediaType()))
                .build()
            LibraryNet.okHttp.newCall(req).execute().use { r ->
                val text = r.body?.string().orEmpty()
                r.code to (runCatching { Json.parseToJsonElement(text) as? JsonObject }.getOrNull())
            }
        } catch (e: Exception) {
            0 to null
        }
    }

    fun component(s: String): String = LibraryNet.escape(s)
}

/** One bot's phone summary (iOS BotPhonesStore.Entry). */
data class PhoneEntry(val bot: BotRecord, val hasPhone: Boolean, val unread: Int)

sealed interface PhonesLoad {
    data class Ok(val entries: List<PhoneEntry>) : PhonesLoad
    data class Failed(val message: String) : PhonesLoad
}

object BotPhonesApi {
    /** iOS BotPhonesStore.refresh: every bot's channels, numbers and unread conversations. */
    suspend fun load(): PhonesLoad = coroutineScope {
        val bots = (BotsApi.listAgents() as? BotsResult.Ok)?.value
            ?: return@coroutineScope PhonesLoad.Failed("Couldn't reach your bots' phones.")
        if (bots.isEmpty()) return@coroutineScope PhonesLoad.Ok(emptyList())
        val numbers = (LibraryNet.get("phone/numbers") as? ApiResult.Ok)?.value.listUnder("numbers")
        val seen = (LibraryNet.get("bot-phone/seen") as? ApiResult.Ok)?.value.obj?.get("seen").let { it as? JsonObject }
        var failures = 0
        val entries = bots.map { bot ->
            async {
                val channels = (BotsApi.listChannels(bot.id) as? BotsResult.Ok)?.value
                if (channels == null) { failures++; return@async null }
                var unread = 0
                for (ch in channels.filter { it.enabled }) {
                    val convs = (LibraryNet.get("gateway/channel-accounts/${LibraryNet.escape(ch.accountId)}/conversations") as? ApiResult.Ok)?.value.listUnder("conversations")
                    for (c in convs) {
                        val updated = parseTime(c.str("updatedAt") ?: c.str("updated_at")).toDouble()
                        val mark = seen?.get(c.str("conversationId") ?: c.str("conversation_id") ?: "")?.let { (it as? kotlinx.serialization.json.JsonPrimitive)?.content?.toDoubleOrNull() } ?: 0.0
                        if (updated > mark) unread++
                    }
                }
                val hasNumber = numbers.any { it.str("botId") == bot.id || it.str("bot_id") == bot.id }
                PhoneEntry(bot, channels.any { it.enabled } || hasNumber, unread)
            }
        }.map { it.await() }.filterNotNull()
        if (failures == bots.size) PhonesLoad.Failed("Couldn't reach your bots' phones.") else PhonesLoad.Ok(entries)
    }
}

/** iOS AndroidPairing.Pending. */
data class PendingPair(val phoneId: String, val name: String, val model: String, val code: String)

/** iOS AndroidPairing + AppPhoneService.unpair: the cloud's `app-phones` routes. */
object AppPhonesApi {
    sealed interface Lane {
        data object Checking : Lane
        data class Ready(val runtimeId: String, val name: String) : Lane
        data object None : Lane
        data class Offline(val name: String) : Lane
        data class Old(val name: String) : Lane
        data object SignedOut : Lane
        data class Error(val message: String) : Lane
    }

    const val NO_RUNTIME = "Your phone’s screen goes through one of your computers. Open Allternit Desktop on your Mac or PC, or get an Allternit Cloud computer."
    const val OLD_RUNTIME = "Update Allternit on your computer to share an Android phone’s screen."

    suspend fun lane(devices: List<com.allternit.feature.computer.AciDevice>): Lane {
        val online = devices.firstOrNull { it.isOnline } ?: return if (devices.isEmpty()) Lane.None else Lane.Offline(devices.first().displayName)
        val (status, _) = CloudCall.send("GET", "api/v1/app-phones/pair/pending?runtimeDeviceId=${CloudCall.component(online.id)}")
        return when {
            status == 404 || status == 405 -> Lane.Old(online.displayName)
            status == 401 || status == 403 -> Lane.SignedOut
            status in 200..299 -> Lane.Ready(online.id, online.displayName)
            else -> Lane.Error("Couldn’t check your computers. Try again.")
        }
    }

    suspend fun pending(runtimeId: String): List<PendingPair> {
        val (status, body) = CloudCall.send("GET", "api/v1/app-phones/pair/pending?runtimeDeviceId=${CloudCall.component(runtimeId)}")
        if (status !in 200..299) return emptyList()
        return body.listUnder("pairs").mapNotNull { item ->
            val data = item.obj?.get("data") as? JsonObject ?: item["payload"] as? JsonObject ?: item
            val phoneId = data.str("phoneId") ?: return@mapNotNull null
            val code = (data.str("code") ?: "").filter { it.isDigit() }
            if (code.length != 6) return@mapNotNull null
            val platform = item.str("platform") ?: data.str("platform")
            val raw = data.str("name")?.trim().orEmpty()
            val name = if (raw.isEmpty() || (raw == "iPhone" && platform == "android")) "Android phone" else raw.take(80)
            PendingPair(phoneId, name, data.str("model").orEmpty().take(80), code)
        }
    }

    /** `POST app-phones/:id/claim`: a six-digit code for the phone to type. Returns (code, error). */
    suspend fun claim(phoneId: String, runtimeId: String): Pair<String?, String?> {
        val (status, body) = CloudCall.send("POST", "api/v1/app-phones/$phoneId/claim", buildJsonObject { put("runtimeDeviceId", runtimeId) })
        if (status == 429) return null to "Too many codes. Wait a few minutes, then try again."
        val code = body.str("code")
        return if (status in 200..299 && code != null) code to null else null to (body.str("message") ?: "Couldn’t make a code. Try again.")
    }

    /** `POST app-phones/:id/pair/decision`: null on success, else the sentence to show. */
    suspend fun decide(phoneId: String, approve: Boolean): String? {
        val (status, _) = CloudCall.send("POST", "api/v1/app-phones/${CloudCall.component(phoneId)}/pair/decision", buildJsonObject { put("approve", approve) })
        return when {
            status in 200..299 -> null
            status == 404 -> "That request expired. Ask the phone to try again."
            else -> "Couldn’t answer the request. Try again."
        }
    }

    suspend fun unpair(phoneId: String): Boolean = CloudCall.send("DELETE", "api/v1/app-phones/${CloudCall.component(phoneId)}").first in 200..299
}

/** iOS LinkBoard (the fields the screen shows). */
data class LinkBoard(val id: String, val name: String, val state: String, val thisPhone: Boolean)

internal suspend fun ChatBackend.boards(): Pair<List<LinkBoard>?, Boolean> = when (val r = runtimeJson("GET", "/api/v1/phones/boards")) {
    is RuntimeCall.Ok -> r.body.listUnder("boards").map { LinkBoard(it.str("id").orEmpty(), it.str("name") ?: it.str("id").orEmpty(), it.str("state") ?: it.str("status").orEmpty(), it.bool("thisPhone") == true) } to false
    RuntimeCall.NoComputer -> null to true
    else -> null to false
}

internal suspend fun ChatBackend.stopBoard(id: String): Boolean =
    runtimeJson("POST", "/api/v1/phones/boards/${LibraryNet.escape(id)}/stop", "{}") is RuntimeCall.Ok

/** iOS PhoneTaskClient.RunSummary (read only). */
data class PhoneRun(val runId: String, val serial: String, val goal: String, val status: String, val step: Int, val stepLabel: String?, val steps: Int, val failed: Int, val approvalId: String?) {
    val terminal: Boolean get() = status in setOf("completed", "done", "failed", "max_steps", "timeout", "refused", "stopped", "cancelled")
}

internal suspend fun ChatBackend.phoneRun(runId: String): PhoneRun? =
    (runtimeJson("GET", "/api/v1/phones/history") as? RuntimeCall.Ok)?.body.listUnder("runs").map { o ->
        fun s(vararg k: String) = k.firstNotNullOfOrNull { o.str(it)?.takeIf { v -> v.isNotEmpty() } }.orEmpty()
        PhoneRun(s("run_id", "runId"), s("serial"), s("goal", "skill"), s("status"), (o.num("step") ?: 0.0).toInt(), o.str("step_label")?.takeIf { it.isNotEmpty() }, (o.num("steps") ?: 0.0).toInt(), (o.num("failed") ?: 0.0).toInt(), o.str("approvalId")?.takeIf { it.isNotEmpty() })
    }.firstOrNull { it.runId == runId }

private fun JsonElement?.obj2(): JsonObject? = this as? JsonObject
