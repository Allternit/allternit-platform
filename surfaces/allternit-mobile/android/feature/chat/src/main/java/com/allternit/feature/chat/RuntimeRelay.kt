package com.allternit.feature.chat

import android.content.Context
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiConfig
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.serialization.json.JsonObject
import okhttp3.HttpUrl
import okhttp3.OkHttpClient

/** One of the user's computers (iOS RelayRuntimeDevice, `GET {cloud}/api/v1/runtime-devices`). */
data class RelayDevice(
    val id: String,
    val name: String,
    val runtimeType: String,
    val status: String,
    val preferred: Boolean,
    val wakeable: Boolean,
    val relayConnections: Int,
) {
    val isOnline: Boolean get() = status == "online" || status == "busy"
}

/** What the relay found when the chat asked for a computer. */
sealed interface RelayPick {
    data class Computer(val runtimeId: String) : RelayPick
    /** No computer paired at all: the compute gate (link one, subscribe, buy credits). */
    data object NoComputer : RelayPick
    /** Computers exist but none answers: wake / open Allternit Desktop. */
    data object Unreachable : RelayPick
    /** The device list itself could not be read. [gate] = cloud-api refused for compute. */
    data class Failed(val message: String, val gate: Boolean) : RelayPick
}

/**
 * Which computer `/api/agent-chat` is relayed to: a port of iOS RuntimeRelayStore.pick (PWA
 * `pickActiveRuntime`, lib/runtime-target.ts). The account's preferred computer wins whenever
 * it answers, then the persisted pick, then the first online one (preferring rows holding a live
 * relay connection), then a sleeping cloud computer (the first request wakes it). The list is
 * refreshed when older than 30 s; a runtime that went offline is skipped for 2 min.
 */
class RuntimeRelay(private val okHttp: OkHttpClient, private val config: ApiConfig, context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences("allternit_shell", Context.MODE_PRIVATE)
    private val mutex = Mutex()
    private var devices: List<RelayDevice> = emptyList()
    private var refreshedAt = 0L
    private var lastError: RelayPick.Failed? = null
    private val unreachableAt = mutableMapOf<String, Long>()

    /** The user's explicit pick (iOS key `allternit.relay-runtime-id`); null = automatic. */
    var selectedRuntimeId: String?
        get() = prefs.getString(KEY_SELECTED, null)
        set(value) { prefs.edit().putString(KEY_SELECTED, value).apply() }

    fun proxyUrl(runtimeId: String): HttpUrl = config.cloudBaseUrl.newBuilder()
        .addPathSegments("api/v1/runtime-devices")
        .addPathSegment(runtimeId)
        .addPathSegment("proxy")
        .build()

    fun markUnreachable(runtimeId: String) {
        unreachableAt[runtimeId] = System.currentTimeMillis()
    }

    fun markReachable(runtimeId: String) {
        unreachableAt.remove(runtimeId)
    }

    /** Refreshes the list when stale, then picks. */
    suspend fun resolve(force: Boolean = false): RelayPick = mutex.withLock {
        val now = System.currentTimeMillis()
        if (force || now - refreshedAt > STALE_MS) refresh()
        lastError?.let { if (devices.isEmpty()) return@withLock it }
        if (devices.isEmpty()) return@withLock RelayPick.NoComputer
        pick(now)?.let { RelayPick.Computer(it) } ?: RelayPick.Unreachable
    }

    /** The device list for the Connect a computer sheet (forces a refresh). Null when it can't be read. */
    suspend fun listDevices(): List<RelayDevice>? {
        resolve(force = true)
        return if (lastError != null && devices.isEmpty()) null else devices
    }

    /** The computer relayed calls go to right now (no refresh). */
    val activeRuntimeId: String? get() = pick(System.currentTimeMillis())

    private suspend fun refresh() {
        val url = config.cloudBaseUrl.newBuilder().addPathSegments("api/v1/runtime-devices").build()
        when (val r = ApiCall.execute(okHttp, ApiJson.default, ApiCall.request("GET", url, null), JsonObject.serializer())) {
            is ApiResult.Ok -> {
                devices = r.value.arr("runtimes").mapNotNull { el ->
                    val o = el as? JsonObject ?: return@mapNotNull null
                    RelayDevice(
                        id = o.str("id") ?: return@mapNotNull null,
                        name = o.str("name").orEmpty(),
                        runtimeType = o.str("runtimeType") ?: "desktop",
                        status = o.str("status") ?: "offline",
                        preferred = o.bool("preferred") == true,
                        wakeable = o.bool("wakeable") == true,
                        relayConnections = o.arr("relayConnections").size,
                    )
                }.filter { it.runtimeType.lowercase() !in NON_COMPUTER_TYPES }
                lastError = null
            }
            is ApiResult.HttpError -> lastError = RelayPick.Failed(
                r.message ?: "Couldn't load your computers.",
                gate = ChatErrors.isComputeGate(r.status, r.code, r.message) || r.status == 404 || r.status == 403,
            )
            is ApiResult.Network -> lastError = RelayPick.Failed("Could not connect to the server. Check your connection and try again.", false)
            is ApiResult.Decode -> lastError = RelayPick.Failed("Couldn't load your computers.", false)
        }
        refreshedAt = System.currentTimeMillis()
    }

    private fun recentlyUnreachable(id: String, now: Long): Boolean =
        unreachableAt[id]?.let { now - it < UNREACHABLE_TTL_MS } ?: false

    private fun pick(now: Long): String? {
        val selected = selectedRuntimeId
        val heartbeating = devices.filter { it.isOnline }
        val answering = heartbeating.filter { !recentlyUnreachable(it.id, now) }
        val online = answering.ifEmpty { heartbeating }
        val usable = if (online.any { it.relayConnections > 0 }) online.filter { it.relayConnections > 0 } else online
        usable.firstOrNull { it.preferred }?.let { return it.id }
        if (selected != null && usable.any { it.id == selected }) return selected
        usable.firstOrNull()?.let { return it.id }
        val wakeable = devices.filter { it.wakeable }
        wakeable.firstOrNull { it.preferred }?.let { return it.id }
        if (selected != null && wakeable.any { it.id == selected }) return selected
        return wakeable.firstOrNull()?.id
    }

    private companion object {
        const val KEY_SELECTED = "allternit.relay-runtime-id"
        const val STALE_MS = 30_000L
        const val UNREACHABLE_TTL_MS = 120_000L
        /** A phone is not a computer: an account with only a phone registered has no computer. */
        val NON_COMPUTER_TYPES = setOf("phone", "mobile", "ios", "android", "tablet")
    }
}

/** Error classification shared by the relay and the chat (iOS APIError.isComputeGateRefusal). */
internal object ChatErrors {
    /**
     * cloud-api's account-state refusals: 428 "No data-plane node" (pair a device), and 409/410
     * `compute_required` / `free_computers_retired`. iOS only catches 428 on the chat stream
     * (AgentChatClient throws a bare httpError for a relayed 409); Android catches all three.
     */
    fun isComputeGate(status: Int?, code: String?, message: String?): Boolean =
        com.allternit.core.api.NoComputerRule.isGate(status, code, message)

    /** iOS statusSentence / web statusSentence. */
    fun statusSentence(status: Int): String? = when (status) {
        401 -> "Sign in again to continue."
        403 -> "You don't have access to that."
        404 -> "That isn't there anymore."
        408, 504 -> "That took too long. Try again."
        429 -> "Too many requests. Wait a moment and try again."
        in 500..599 -> "Something went wrong on our side. Try again."
        else -> null
    }

    /** iOS RuntimeRelayError.unreachable. */
    const val UNREACHABLE = "Your computer isn't responding. Open Allternit Desktop or wake your cloud computer, then try again."
}
