package com.allternit.feature.settings

import android.content.Context
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.mutableStateOf
import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.parseTime
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import java.io.IOException

/**
 * "Save to my Allternit account" for the user's own voice keys (Settings > Voice), off by default; the port of iOS
 * `Core/Voice/VoiceKeySync.swift`. When on, keys are also kept, encrypted, in the user's account (cloud-api
 * `/api/v1/voice/keys`, migration 128) so their other signed-in devices can use them. The same rules as iOS:
 *
 * - The toggle is account-wide: a pull that finds saved rows shows it ON on this phone, a pull that finds none shows
 *   it OFF (turning it on with no keys anywhere flips back off).
 * - The copy on this phone is the working copy. On Voice page open the newer `updated_at` wins, per vendor.
 * - A change on this phone while sync is on is sent up (PUT).
 * - A pull never changes Where voice runs.
 * - Disconnect removes the key from this phone only and opts that vendor out, so a pull won't bring it back.
 *   Connecting again clears it.
 * - Turning sync off deletes every saved row from the account; keys on this phone stay.
 *
 * Session auth only (the routes refuse API tokens): straight to cloud-api with the Clerk Bearer ([Net.cloud]),
 * never through a runtime relay. The keys stay Keystore-sealed on this phone ([VoiceKeys]); only the on/off flag and
 * the opt-outs (no secrets) live in prefs, and nothing about a key is logged.
 */
internal object VoiceKeySync {
    data class State(val enabled: Boolean = false, val busy: Boolean = false, val error: String? = null)

    private const val ENABLED = "voice.byok.accountSync"
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val mutex = Mutex()
    private var loaded = false

    val state: MutableState<State> = mutableStateOf(State())

    private fun load(context: Context) {
        if (loaded) return
        loaded = true
        state.value = state.value.copy(enabled = SettingsPrefs.flag(context, ENABLED, false))
    }

    private fun setStored(context: Context, on: Boolean) {
        SettingsPrefs.setFlag(context, ENABLED, on)
        state.value = state.value.copy(enabled = on)
    }

    fun isOptedOut(context: Context, v: VoiceVendor) = SettingsPrefs.flag(context, "voice.byok.optOut.${v.id}", false)
    fun setOptedOut(context: Context, v: VoiceVendor, on: Boolean) = SettingsPrefs.setFlag(context, "voice.byok.optOut.${v.id}", on)

    /** Keys on this phone changed (Connect, hand-off): with sync on, send that vendor up. */
    fun localChanged(context: Context, v: VoiceVendor) {
        val app = context.applicationContext
        load(app)
        if (!state.value.enabled) return
        scope.launch {
            try {
                mutex.withLock { push(app, v) }
            } catch (e: Exception) {
                state.value = state.value.copy(error = "Couldn't save your ${v.title} key to your Allternit account: ${describe(e)}")
            }
        }
    }

    /** The toggle. On: push every vendor not opted out, then pull and merge. Off: delete every saved row. */
    suspend fun setEnabled(context: Context, on: Boolean) {
        val app = context.applicationContext
        load(app)
        if (on == state.value.enabled) return
        state.value = state.value.copy(busy = true, error = null)
        try {
            mutex.withLock {
                if (on) {
                    setStored(app, true)
                    try {
                        for (v in VoiceVendor.entries) if (!isOptedOut(app, v)) push(app, v)
                        pullAndMerge(app)
                        if (!state.value.enabled) state.value = state.value.copy(error = "Connect a key first. There's nothing to save to your account yet.")
                    } catch (e: Exception) {
                        setStored(app, false)
                        state.value = state.value.copy(error = "Couldn't save your keys to your Allternit account: ${describe(e)}")
                    }
                } else {
                    try {
                        send("DELETE", "api/v1/voice/keys", null)
                        setStored(app, false)
                    } catch (e: Exception) {
                        state.value = state.value.copy(error = "Couldn't delete your keys from your Allternit account: ${describe(e)} Try again.")
                    }
                }
            }
        } finally {
            state.value = state.value.copy(busy = false)
        }
    }

    /** Pull once and merge (Voice page open). True when a key on this phone changed. */
    suspend fun sync(context: Context): Boolean {
        val app = context.applicationContext
        load(app)
        return try {
            val changed = mutex.withLock { pullAndMerge(app) }
            state.value = state.value.copy(error = null)
            changed
        } catch (e: Exception) {
            // A background pull only reports problems while the toggle is on.
            if (state.value.enabled) state.value = state.value.copy(error = "Couldn't sync with your Allternit account: ${describe(e)}")
            false
        }
    }

    private class Remote(val vendor: VoiceVendor, val apiKey: String, val voiceId: String?, val label: String?, val updatedAt: Long)

    /** GET the account's rows: none → the toggle reads OFF; some → ON, and the newer copy of each vendor wins (iOS VoiceKeyMerge.plan). */
    private suspend fun pullAndMerge(context: Context): Boolean {
        val rows = send("GET", "api/v1/voice/keys", null).obj.list("keys").mapNotNull { o ->
            val vendor = VoiceVendor.of(o.str("provider").orEmpty()) ?: return@mapNotNull null
            val key = o.str("api_key")?.takeIf { it.isNotBlank() } ?: return@mapNotNull null
            val at = parseTime(o.str("updated_at")).takeIf { it > 0 } ?: throw IOException("Your Allternit account sent keys this app couldn't read.")
            Remote(vendor, key, o.str("voice_id"), o.str("label"), at)
        }
        if (rows.isEmpty()) {
            setStored(context, false)
            return false
        }
        setStored(context, true)
        val byVendor = rows.groupBy { it.vendor }.mapValues { (_, list) -> list.maxBy { it.updatedAt } }
        var changed = false
        for (v in VoiceVendor.entries) {
            if (isOptedOut(context, v)) continue
            val mine = withContext(Dispatchers.IO) { VoiceKeys.load(context, v) }
            val row = byVendor[v]
            when {
                mine == null && row == null -> Unit
                mine == null && row != null -> { pull(context, row); changed = true }
                mine != null && row == null -> push(context, v)
                mine != null && row != null -> {
                    val sameVoice = v != VoiceVendor.ElevenLabs || (mine.voiceId ?: "") == (row.voiceId ?: "")
                    if (mine.apiKey == row.apiKey && sameVoice) continue
                    if (mine.updatedAt > row.updatedAt) push(context, v)
                    else if (row.updatedAt > mine.updatedAt) { pull(context, row); changed = true }
                }
            }
        }
        return changed
    }

    /** Never touches Where voice runs. */
    private suspend fun pull(context: Context, row: Remote) = withContext(Dispatchers.IO) {
        VoiceKeys.write(context, row.vendor, row.apiKey, row.voiceId, row.label ?: "Saved", row.updatedAt)
    }

    private suspend fun push(context: Context, v: VoiceVendor) {
        val mine = withContext(Dispatchers.IO) { VoiceKeys.load(context, v) } ?: return
        val body = buildJsonObject {
            put("api_key", JsonPrimitive(mine.apiKey))
            mine.voiceId?.let { put("voice_id", JsonPrimitive(it)) }
            if (mine.label.isNotEmpty()) put("label", JsonPrimitive(mine.label.take(128)))
        }
        val row = send("PUT", "api/v1/voice/keys/${v.id}", body).obj
        // Adopt the server's change time so the next sync sees them equal.
        val at = parseTime(row.str("updated_at"))
        if (at > 0) withContext(Dispatchers.IO) {
            val now = VoiceKeys.load(context, v)
            if (now?.apiKey == mine.apiKey) VoiceKeys.write(context, v, mine.apiKey, mine.voiceId, mine.label, at)
        }
        state.value = state.value.copy(error = null)
    }

    private class Http(val status: Int) : IOException("HTTP $status")

    private suspend fun send(method: String, path: String, body: kotlinx.serialization.json.JsonElement?) = when (val r = Net.cloud(method, path, body)) {
        is ApiResult.Ok -> r.value
        is ApiResult.HttpError -> throw Http(r.status)
        is ApiResult.Network -> throw IOException("you're offline.")
        else -> throw IOException("Allternit sent a reply this app couldn't read.")
    }

    private fun describe(e: Exception): String = when (e) {
        is Http -> when (e.status) {
            401 -> "sign in to Allternit on this phone."
            403 -> "your Allternit session can't use this. Sign in again."
            else -> "the server answered ${e.status}."
        }
        else -> e.message ?: "something went wrong."
    }
}
