package com.allternit.feature.bar

import com.allternit.core.api.ApiClients
import com.allternit.core.api.ApiConfig
import com.allternit.core.api.ApiJson
import com.allternit.core.api.NoComputerRule
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.auth.AuthState
import java.io.IOException
import kotlin.coroutines.resume
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.Call
import okhttp3.Callback
import okhttp3.HttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response

/** Why a bar call failed, as one plain sentence (iOS BarWire.failure). */
sealed class BarFailure(message: String) : Exception(message) {
    class SignedOut : BarFailure(BarWording.signedOut)
    class NoComputer : BarFailure(BarWording.noComputer)
    /** `owner_approval_required` (403): not an error for the bar, the job waits on [approverName]. */
    class Gate(val approverName: String?) : BarFailure("Waiting on ${approverName ?: "approval"}")
    class Failed(message: String) : BarFailure(message)
}

/** One live event from `GET /bar/jobs/:id/events` (wire form `{type, session_id, part}`). */
sealed interface BarEvent {
    data class Job(val job: BarJob) : BarEvent
    data class Task(val status: String, val message: String?) : BarEvent
    data object NeedsInput : BarEvent
    data object InputResolved : BarEvent
}

/**
 * The bar's calls on allternit-api (`/api/v1/bar/*`, CONTRACT section 1), with the ApiClients bearer like Bots mode
 * (LibraryNet). Every throw is a [BarFailure].
 */
class BarClient(
    private val config: ApiConfig = ApiConfig(),
    private val okHttp: OkHttpClient = ApiClients.okHttp(bearer = AllternitAuth.tokenProvider),
) {
    private val jsonType = "application/json; charset=utf-8".toMediaType()

    private fun url(path: String): HttpUrl = config.apiBaseUrl.newBuilder().addPathSegments(path).build()

    private fun esc(s: String): String = java.net.URLEncoder.encode(s, "UTF-8").replace("+", "%20")

    suspend fun apps(): List<BarApp> =
        call("GET", "bar/apps")["apps"].a().mapNotNull { BarParse.app(it.o()) }

    suspend fun tools(app: BarApp): List<BarTool> =
        call("GET", "bar/apps/${esc(app.kind)}/${esc(app.ref)}/tools")["tools"].a().mapNotNull { BarParse.tool(it.o()) }

    suspend fun intent(app: BarApp, tool: String, botId: String?): BarIntent {
        val body = buildJsonObject {
            put("app", buildJsonObject { put("kind", app.kind); put("ref", app.ref) })
            put("tool", tool)
            if (!botId.isNullOrEmpty()) put("bot_id", botId)
        }
        return BarParse.intent(call("POST", "bar/intent", body), app, tool) ?: throw BarFailure.Failed(BarWording.fallback)
    }

    suspend fun review(intent: BarIntent, values: Map<String, BarValue>, botId: String?): BarReview {
        val body = buildJsonObject {
            put("intent_id", intent.intentId)
            put("args", JsonObject(values.mapValues { it.value.json() }))
            if (!botId.isNullOrEmpty()) put("bot_id", botId)
        }
        return BarParse.review(call("POST", "bar/review", body)) ?: throw BarFailure.Failed(BarWording.fallback)
    }

    /** Throws [BarFailure.Gate] when a rule routes it to someone; no reply at all means no computer. */
    suspend fun run(jobId: String, approverName: String?): BarJob? =
        BarParse.job(call("POST", "bar/jobs/${esc(jobId)}/run", null, noReplyIsNoComputer = true)["job"].o(), approverName)

    suspend fun job(jobId: String, approverName: String?): BarJob? =
        BarParse.job(call("GET", "bar/jobs/${esc(jobId)}")["job"].o(), approverName)

    suspend fun cancel(jobId: String) {
        runCatching { call("POST", "bar/jobs/${esc(jobId)}/cancel") }
    }

    /** The job's SSE stream; completes when the server closes it, throws on a dropped or refused stream. */
    fun events(jobId: String, approverName: String?): Flow<BarEvent> = callbackFlow {
        val request = Request.Builder().url(url("bar/jobs/${esc(jobId)}/events")).header("Accept", "text/event-stream").get().build()
        val call = okHttp.newCall(request)
        val reader = Thread {
            try {
                call.execute().use { r ->
                    if (!r.isSuccessful) throw IOException("HTTP ${r.code}")
                    val source = r.body?.source() ?: throw IOException("no body")
                    while (!source.exhausted()) {
                        val line = source.readUtf8Line() ?: break
                        if (!line.startsWith("data:")) continue
                        val obj = runCatching { ApiJson.default.parseToJsonElement(line.removePrefix("data:").trim()) as? JsonObject }.getOrNull() ?: continue
                        decode(obj, approverName)?.let { trySend(it) }
                    }
                }
                close()
            } catch (e: Exception) {
                close(e)
            }
        }.apply { isDaemon = true; name = "bar-events-$jobId" }
        reader.start()
        awaitClose { call.cancel() }
    }.flowOn(Dispatchers.IO)

    internal fun decode(obj: JsonObject, approverName: String?): BarEvent? =
        when (obj.s("type") ?: obj.s("kind")) {
            "bar_job" -> BarParse.job(obj["job"].o(), approverName)?.let { BarEvent.Job(it) }
            "mcp_task" -> obj["part"].o().let { p -> BarEvent.Task(p.s("status") ?: "working", p.s("statusMessage")?.takeIf { it.isNotBlank() }) }
            "mcp_elicitation" -> BarEvent.NeedsInput
            "mcp_elicitation_resolved" -> BarEvent.InputResolved
            else -> null
        }

    // MARK: - Transport

    private suspend fun call(method: String, path: String, body: JsonObject? = null, noReplyIsNoComputer: Boolean = false): JsonObject {
        // A cold process (widget, notification): wait briefly for the stored session, like StatusSync.
        if (AllternitAuth.state.value is AuthState.Starting) withTimeoutOrNull(6_000) { AllternitAuth.state.first { it !is AuthState.Starting } }
        if (AllternitAuth.state.value !is AuthState.SignedIn) throw BarFailure.SignedOut()
        val request = Request.Builder().url(url(path)).header("Accept", "application/json")
            .method(method, body?.toString()?.toRequestBody(jsonType) ?: if (method == "POST") "{}".toRequestBody(jsonType) else null)
            .build()
        val (status, text) = try {
            send(request)
        } catch (e: IOException) {
            throw if (noReplyIsNoComputer) BarFailure.NoComputer() else BarFailure.Failed(BarWording.fallback)
        }
        if (status in 200..299) return runCatching { ApiJson.default.parseToJsonElement(text) as? JsonObject }.getOrNull() ?: JsonObject(emptyMap())
        throw failure(status, text)
    }

    private suspend fun send(request: Request): Pair<Int, String> = suspendCancellableCoroutine { cont ->
        val call = okHttp.newCall(request)
        cont.invokeOnCancellation { call.cancel() }
        call.enqueue(object : Callback {
            override fun onFailure(call: Call, e: IOException) {
                if (cont.isActive) cont.resumeWith(Result.failure(e))
            }

            override fun onResponse(call: Call, response: Response) {
                val result = runCatching { response.use { it.code to it.body?.string().orEmpty() } }
                if (cont.isActive) result.fold({ cont.resume(it) }, { cont.resumeWith(Result.failure(it)) })
            }
        })
    }

    /** Reads the four error shapes (CONTRACT section 3): bar `{error, code}`, JSON-RPC `{error: {code, message, data}}`, plain. */
    internal fun failure(status: Int, text: String): BarFailure {
        val raw = runCatching { ApiJson.default.parseToJsonElement(text) as? JsonObject }.getOrNull()
        val errObj = raw?.get("error").o()
        val data = raw?.get("data").o() ?: errObj?.get("data").o()
        val code = (data.s("code") ?: raw.s("code") ?: errObj.s("code") ?: (raw?.get("error") as? JsonPrimitive)?.takeIf { errObj == null && raw.s("code") == null }?.content)
            ?.lowercase().orEmpty()
        val message = (raw?.get("error") as? JsonPrimitive)?.content?.takeIf { errObj == null } ?: errObj.s("message") ?: raw.s("message")
        return when {
            code == "owner_approval_required" -> BarFailure.Gate((data?.get("approver").o() ?: raw?.get("approver").o()).s("name"))
            (status == 409 && code == "no_runtime") || status == 428 || code == "compute_required" || code == "free_computers_retired" ||
                NoComputerRule.isGate(status, code, message) -> BarFailure.NoComputer()
            status == 401 -> BarFailure.SignedOut()
            code == "no_grant" -> BarFailure.Failed("This bot has no access to that app. Ask the owner to grant access.")
            code == "level_read_only" -> BarFailure.Failed("This app is at Browser level here, so it can only read.")
            code == "approver_required" -> BarFailure.Failed(message?.takeIf { it.isNotBlank() && it != code } ?: "A team rule needs an approver, and none is named.")
            code == "team_rules_unavailable" || code == "tier_check_unavailable" -> BarFailure.Failed("Can't check the team rules right now. Try again in a moment.")
            code == "app_kind_unsupported" -> BarFailure.Failed("That app can't run tools from here yet.")
            status == 404 -> BarFailure.Failed("Couldn't find that app or tool.")
            else -> BarFailure.Failed(message?.takeIf { it.isNotBlank() && it != code && !it.startsWith("{") } ?: BarWording.fallback)
        }
    }
}
