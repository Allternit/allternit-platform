package com.allternit.feature.shell.firstrun

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.core.data.Prefs
import com.allternit.feature.chat.RelayDevice
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.first

/**
 * First-run setup state (iOS OnboardingStore), in DataStore:
 * - [completedOnDevice] (key `allternit-onboarding-complete`, the iOS UserDefaults key) gates the
 *   setup in MainActivity, before the workspace. It is true once setup finished on this phone.
 * - [skippedThisLaunch]: the gate let an existing user through for this launch only (the server
 *   could not answer and a computer is paired). In memory, so it asks again next launch.
 * - [forceRun]: show setup without asking the server (Settings → Replay setup).
 */
internal class FirstRunStore(context: Context) {
    private val prefs = Prefs(context)
    private val shellPrefs = context.applicationContext.getSharedPreferences("allternit_shell", Context.MODE_PRIVATE)

    var loaded by mutableStateOf(false)
        private set
    var completedOnDevice by mutableStateOf(false)
        private set
    var skippedThisLaunch by mutableStateOf(false)
        private set
    var forceRun by mutableStateOf(false)
        private set

    val isComplete: Boolean get() = completedOnDevice || skippedThisLaunch

    /**
     * Someone has used the app on this phone before: the shell's remembered mode (iOS reads the
     * mode store's keys for the same reason).
     */
    val usedAppBefore: Boolean get() = shellPrefs.contains("shell-mode")

    suspend fun load() {
        completedOnDevice = prefs.boolean(KEY_COMPLETE).first()
        loaded = true
    }

    /** Setup finished (or the server says it is done). */
    suspend fun complete() {
        forceRun = false
        completedOnDevice = true
        prefs.setBoolean(KEY_COMPLETE, true)
    }

    fun skipThisLaunch() {
        skippedThisLaunch = true
    }

    /** Settings → Replay setup: the root swaps back to setup, without waiting for the server. */
    suspend fun replay() {
        forceRun = true
        skippedThisLaunch = false
        completedOnDevice = false
        prefs.setBoolean(KEY_COMPLETE, false)
    }

    companion object {
        const val KEY_COMPLETE = "allternit-onboarding-complete"
    }
}

/**
 * The live connect-kit rows for setup (iOS ConnectionsStore, the part setup reads): cloud-api's
 * `GET /api/v1/connections`, with the catalog's fallbacks for rows the server didn't send.
 */
internal class ConnectionsStore {
    var rows by mutableStateOf<Map<String, ConnectionWire>>(emptyMap())
        private set
    var loaded by mutableStateOf(false)
        private set
    var loadError by mutableStateOf<String?>(null)
        private set
    var computerOnline by mutableStateOf(false)
    private var connecting by mutableStateOf<Set<String>>(emptySet())
    private var loading = false

    suspend fun load(devices: suspend () -> List<RelayDevice>?) {
        if (loading) return
        loading = true
        try {
            coroutineScope {
                val list = async { runCatching { ConnectionsClient.list() } }
                computerOnline = devices()?.any { it.isOnline } == true
                list.await()
                    .onSuccess { r ->
                        loadError = null
                        rows = r.filter { it.id in ConnectionsCatalog.knownIds }.associateBy { it.id }
                    }
                    .onFailure { loadError = ConnectionsClient.describe(it) }
            }
        } finally {
            loading = false
            loaded = true
        }
    }

    fun row(id: String): ConnectionWire {
        var r = rows[id] ?: ConnectionsCatalog.fallbackRow(id, computerOnline)
        when (id) {
            "api-key" -> r = keysSummary()
            // Alias of google-key (contract §3).
            "gemini" -> rows["google-key"]?.takeIf { rows["gemini"] == null }?.let {
                r = r.copy(state = it.state, found = it.found, reason = it.reason)
            }
            "this-mac" -> if (computerOnline) r = r.copy(state = ConnState.On)
        }
        if (id in connecting && r.state != ConnState.On) r = r.copy(state = ConnState.Connecting)
        return r
    }

    fun state(id: String) = row(id).state

    fun isAvailable(id: String): Boolean = row(id).let { it.available || it.state == ConnState.On }

    /** "An API key" stands for every key provider. */
    private fun keysSummary(): ConnectionWire {
        val keys = ConnectionsCatalog.keyProviders.map { it to rows[it.id] }
        keys.firstOrNull { it.second?.state == ConnState.Needs }?.let { (p, row) ->
            return ConnectionWire("api-key", ConnState.Needs,
                reason = row?.reason ?: ConnectionsCatalog.reasonText(row?.reasonCode, p.name) ?: "${p.name} key needs you")
        }
        val on = keys.filter { it.second?.state == ConnState.On }.map { it.first.name }
        if (on.isNotEmpty()) return ConnectionWire("api-key", ConnState.On, found = if (on.size <= 2) on.joinToString(", ") else "${on.size} keys")
        return ConnectionWire("api-key")
    }

    /** The one status line: the four words and nothing else. */
    fun statusLine(entry: ConnEntry): String {
        val r = row(entry.id)
        return when (r.state) {
            ConnState.Off -> entry.blurb?.let { "Not connected · $it" } ?: "Not connected"
            ConnState.Connecting -> "Connecting…"
            ConnState.On -> r.found?.let { "Connected · $it" } ?: "Connected"
            ConnState.Needs -> (r.reason ?: ConnectionsCatalog.reasonText(r.reasonCode, entry.name))?.let { "Needs you · $it" } ?: "Needs you"
        }
    }

    fun unavailableReason(id: String): String {
        val r = row(id)
        return r.unavailableReason ?: ConnectionsCatalog.reasonText(r.reasonCode, ConnectionsCatalog.entry(id)?.name ?: id) ?: "Not available yet."
    }

    fun setConnecting(id: String, on: Boolean) {
        connecting = if (on) connecting + id else connecting - id
    }

    fun apply(row: ConnectionWire) {
        if (row.id !in ConnectionsCatalog.knownIds) return
        connecting = connecting - row.id
        rows = rows + (row.id to row)
    }

    fun markOn(id: String) {
        connecting = connecting - id
        rows = rows + (id to (rows[id] ?: ConnectionWire(id)).copy(state = ConnState.On, reason = null, reasonCode = null))
    }
}
