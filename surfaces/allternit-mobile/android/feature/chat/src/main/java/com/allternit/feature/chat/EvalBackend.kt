package com.allternit.feature.chat

import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.doubleOrNull

/**
 * Gizzi test cases (eval loop, CONTRACT sections 6 and 7): `/api/v1/agent-evals/...`, hand-written like
 * the other agent-session calls (no codegen). One transport call per route, so a test can fake it.
 */
fun interface EvalTransport {
    suspend fun call(method: String, segments: List<String>, query: Map<String, String>, body: String?): ApiResult<JsonElement>
}

@Serializable
data class EvalSelfTest(val passed: Boolean = false, @SerialName("agrees_with_label") val agreesWithLabel: Boolean = false)

@Serializable
data class EvalCase(
    val id: String,
    val suite: String = "",
    val title: String = "",
    /** "good" or "bad". */
    val label: String = "good",
    @SerialName("saved_at") val savedAt: String? = null,
    @SerialName("needs_check") val needsCheck: Boolean = false,
    @SerialName("last_result") val lastResult: String? = null,
    @SerialName("last_run_at") val lastRunAt: String? = null,
    val bytes: Long = 0,
    @SerialName("workspace_kind") val workspaceKind: String = "none",
    @SerialName("session_id") val sessionId: String = "",
)

@Serializable
data class EvalSavedCase(
    val case: EvalCase,
    @SerialName("self_test") val selfTest: EvalSelfTest = EvalSelfTest(),
    @SerialName("needs_check") val needsCheck: Boolean = false,
)

@Serializable
data class EvalCaseList(val cases: List<EvalCase> = emptyList())

@Serializable
data class EvalEstimate(
    @SerialName("estimate_id") val estimateId: String,
    @SerialName("expires_at") val expiresAt: String? = null,
    val cases: Int = 0,
    val k: Int = 1,
    val attempts: Int = 0,
    @SerialName("tokens_est") val tokensEst: Long = 0,
    @SerialName("minutes_est") val minutesEst: Double = 0.0,
    /** "plan", "metered" or "unknown". */
    val billing: String = "unknown",
    @SerialName("usd_est") val usdEst: Double? = null,
    @SerialName("probe_required") val probeRequired: Boolean = false,
)

@Serializable
data class EvalProgress(val done: Int = 0, val total: Int = 0)

@Serializable
data class EvalRun(
    @SerialName("run_id") val runId: String,
    val mode: String = "",
    /** queued, running, done, failed or cancelled. */
    val state: String = "done",
    val progress: EvalProgress = EvalProgress(),
    val report: JsonElement? = null,
    val error: String? = null,
) {
    val finished get() = state == "done" || state == "failed" || state == "cancelled"
}

/** One engine failure, named for the clients. [text] is the CONTRACT section 9 copy. */
sealed class EvalError(val text: String) {
    data object NoComputer : EvalError("Needs a computer")
    data object Unreachable : EvalError("Couldn't reach your computer, try again")
    data object CaseLimit : EvalError("You have too many test cases. Delete one and try again.")
    data object EstimateRequired : EvalError("Get a price first.")
    data object NotFound : EvalError("That isn't there anymore.")
    data class Unknown(val detail: String? = null) : EvalError("Couldn't save, try again")
}

sealed interface EvalOutcome<out T> {
    data class Ok<T>(val value: T) : EvalOutcome<T>
    data class Err(val error: EvalError) : EvalOutcome<Nothing>
}

/** Replay results in the shape the page shows (a case id, passed or not). Reads `extra.per_case` wherever the report keeps it. */
data class EvalCaseResult(val id: String, val pass: Boolean)

object EvalReportView {
    fun results(report: JsonElement?): List<EvalCaseResult> =
        find(report, "per_case").firstOrNull()?.let { arr ->
            (arr as? JsonArray)?.mapNotNull { e ->
                val o = e as? JsonObject ?: return@mapNotNull null
                val id = (o["id"] as? JsonPrimitive)?.content ?: return@mapNotNull null
                EvalCaseResult(id, (o["pass"] as? JsonPrimitive)?.booleanOrNull ?: false)
            }
        }.orEmpty()

    /** 0..1, or null when the report has none. */
    fun passRate(report: JsonElement?): Double? =
        find(report, "pass_rate").firstNotNullOfOrNull { (it as? JsonPrimitive)?.doubleOrNull }

    private fun find(e: JsonElement?, key: String): List<JsonElement> = when (e) {
        is JsonObject -> listOfNotNull(e[key]) + e.values.flatMap { find(it, key) }
        is JsonArray -> e.flatMap { find(it, key) }
        else -> emptyList()
    }
}

class EvalBackend(private val transport: EvalTransport) {
    private val json = ApiJson.default

    suspend fun saveCase(sessionId: String, good: Boolean, reason: String?, note: String?, throughMessageId: String? = null): EvalOutcome<EvalSavedCase> {
        val body = JsonObject(buildMap {
            put("session_id", JsonPrimitive(sessionId))
            put("label", JsonPrimitive(if (good) "good" else "bad"))
            note?.trim()?.takeIf { it.isNotEmpty() }?.let { put("note", JsonPrimitive(it)) }
            if (throughMessageId != null) put("through_message_id", JsonPrimitive(throughMessageId))
            if (!good && reason != null) put("quick_check", JsonPrimitive(reason))
        })
        return send("POST", listOf("cases"), emptyMap(), body.toString(), EvalSavedCase.serializer())
    }

    suspend fun listCases(sessionId: String? = null, suite: String? = null): EvalOutcome<List<EvalCase>> =
        when (val r = send("GET", listOf("cases"), buildMap { sessionId?.let { put("session_id", it) }; suite?.let { put("suite", it) } }, null, EvalCaseList.serializer())) {
            is EvalOutcome.Ok -> EvalOutcome.Ok(r.value.cases)
            is EvalOutcome.Err -> r
        }

    suspend fun deleteCase(id: String): EvalOutcome<Unit> =
        when (val r = send("DELETE", listOf("cases", id), emptyMap(), null, JsonElement.serializer(), emptyOk = true)) {
            is EvalOutcome.Ok -> EvalOutcome.Ok(Unit)
            is EvalOutcome.Err -> r
        }

    suspend fun estimateRun(suite: String, caseIds: List<String>? = null, k: Int? = null): EvalOutcome<EvalEstimate> =
        send("POST", listOf("runs", "estimate"), emptyMap(), runBody(suite, null, caseIds, k, null), EvalEstimate.serializer())

    /** Replay answers `{run_id, report}` at once; live answers `{run_id}` and is polled with [getRun]. */
    suspend fun startRun(suite: String, live: Boolean, caseIds: List<String>? = null, k: Int? = null, estimateId: String? = null): EvalOutcome<EvalRun> =
        send("POST", listOf("runs"), emptyMap(), runBody(suite, if (live) "live" else "replay", caseIds, k, estimateId), EvalRun.serializer())

    suspend fun getRun(runId: String): EvalOutcome<EvalRun> =
        send("GET", listOf("runs", runId), emptyMap(), null, EvalRun.serializer())

    suspend fun cancelRun(runId: String): EvalOutcome<Unit> =
        when (val r = send("POST", listOf("runs", runId, "cancel"), emptyMap(), "{}", JsonElement.serializer(), emptyOk = true)) {
            is EvalOutcome.Ok -> EvalOutcome.Ok(Unit)
            is EvalOutcome.Err -> r
        }

    private fun runBody(suite: String, mode: String?, caseIds: List<String>?, k: Int?, estimateId: String?): String =
        JsonObject(buildMap {
            put("suite", JsonPrimitive(suite))
            if (mode != null) put("mode", JsonPrimitive(mode))
            if (caseIds != null) put("case_ids", JsonArray(caseIds.map { JsonPrimitive(it) }))
            if (k != null) put("k", JsonPrimitive(k))
            if (estimateId != null) put("estimate_id", JsonPrimitive(estimateId))
        }).toString()

    private suspend fun <T> send(method: String, segments: List<String>, query: Map<String, String>, body: String?, ser: kotlinx.serialization.KSerializer<T>, emptyOk: Boolean = false): EvalOutcome<T> =
        when (val r = transport.call(method, segments, query, body)) {
            is ApiResult.Ok -> try {
                EvalOutcome.Ok(json.decodeFromJsonElement(ser, r.value))
            } catch (e: Exception) {
                EvalOutcome.Err(EvalError.Unknown("bad answer"))
            }
            // A 202/204 with no body is a success for calls that read nothing.
            is ApiResult.Decode -> if (emptyOk) {
                @Suppress("UNCHECKED_CAST")
                EvalOutcome.Ok(JsonNull as T)
            } else EvalOutcome.Err(EvalError.Unknown("bad answer"))
            is ApiResult.Network -> EvalOutcome.Err(EvalError.Unreachable)
            is ApiResult.HttpError -> EvalOutcome.Err(mapError(r.status, r.code, r.message))
        }

    companion object {
        fun mapError(status: Int, code: String?, message: String?): EvalError = when {
            code == "case_limit" -> EvalError.CaseLimit
            code == "estimate_required" -> EvalError.EstimateRequired
            code == "session_not_found" || code == "not_found" || status == 404 -> EvalError.NotFound
            ChatErrors.isComputeGate(status, code, message) -> EvalError.NoComputer
            status == 502 || status == 503 || status == 504 -> EvalError.Unreachable
            else -> EvalError.Unknown(message)
        }
    }
}
