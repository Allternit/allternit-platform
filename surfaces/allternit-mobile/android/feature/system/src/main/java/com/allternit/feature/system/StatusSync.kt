package com.allternit.feature.system

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.ShortcutInfo
import android.content.pm.ShortcutManager
import android.graphics.drawable.Icon
import android.net.Uri
import android.service.quicksettings.TileService
import android.util.Log
import androidx.glance.appwidget.updateAll
import com.allternit.core.api.ApiClients
import com.allternit.core.api.ApiConfig
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.auth.AuthState
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import okhttp3.Request

/**
 * Keeps the status snapshot fresh and fans it out to every system surface (iOS `AppGroupStore` + `StatusSummaryClient`):
 * the Glance widget, the Quick Settings tiles and the dynamic shortcuts. Runs on every app foreground, on every push
 * (the data messages the app already gets) and on a widget update. The caller of [refresh] updates the ongoing
 * notification (`LiveRunNotification`, in :feature:push) with the snapshot it returns.
 */
object StatusSync {
    private const val TAG = "StatusSync"
    private const val PREFS = "allternit_system"
    private const val KEY_SNAPSHOT = "snapshot"
    private const val KEY_SIGNED_OUT = "signed-out"
    private const val KEY_AS_OF = "as-of"
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val running = AtomicBoolean(false)

    /** What the widget and tiles draw right now (iOS `StatusProvider.resolveContent`, from the stored snapshot). */
    fun content(context: Context): StatusContent {
        val p = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        if (p.getBoolean(KEY_SIGNED_OUT, false)) return StatusContent.SignedOut
        val raw = p.getString(KEY_SNAPSHOT, null) ?: return StatusContent.Quiet(StatusSnapshot())
        val snap = StatusSnapshot.parse(raw, p.getLong(KEY_AS_OF, 0L)) ?: return StatusContent.Quiet(StatusSnapshot())
        return snap.toContent()
    }

    fun snapshot(context: Context): StatusSnapshot? = content(context).snapshotOrNull

    /** iOS refetches when the stored snapshot is older than 15 minutes (`StatusWidgetTuning.staleAfter`). */
    fun isStale(context: Context): Boolean {
        val asOf = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getLong(KEY_AS_OF, 0L)
        return System.currentTimeMillis() - asOf > 15 * 60 * 1000L
    }

    /** Fire and forget, for receivers and the messaging service. */
    fun refreshAsync(context: Context) {
        val app = context.applicationContext
        scope.launch { refresh(app) }
    }

    /**
     * `GET /api/v1/status/summary` on the cloud, store it, redraw the widget, tiles and shortcuts. Returns the snapshot, or
     * the last stored one when the call failed, or null when signed out.
     */
    suspend fun refresh(context: Context): StatusSnapshot? = withContext(Dispatchers.IO) {
        val app = context.applicationContext
        AllternitAuth.start(app)
        // A cold process (widget update, push): wait briefly for the stored session.
        withTimeoutOrNull(6_000) { AllternitAuth.state.first { it !is AuthState.Starting } }
        val prefs = app.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        if (AllternitAuth.state.value !is AuthState.SignedIn) {
            prefs.edit().putBoolean(KEY_SIGNED_OUT, true).apply()
            redraw(app)
            return@withContext null
        }
        if (!running.compareAndSet(false, true)) return@withContext snapshot(app)
        try {
            val url = ApiConfig().cloudBaseUrl.newBuilder().encodedPath("/api/v1/status/summary").build()
            val client = ApiClients.okHttp(bearer = AllternitAuth.tokenProvider)
            val body = runCatching {
                client.newCall(Request.Builder().url(url).get().build()).execute().use { r ->
                    if (r.isSuccessful) r.body?.string() else null
                }
            }.onFailure { Log.i(TAG, "status summary failed: ${it.message}") }.getOrNull()
            val parsed = body?.let { StatusSnapshot.parse(it) }
            if (parsed != null) {
                prefs.edit().putString(KEY_SNAPSHOT, body).putLong(KEY_AS_OF, parsed.asOfMillis).putBoolean(KEY_SIGNED_OUT, false).apply()
            } else {
                prefs.edit().putBoolean(KEY_SIGNED_OUT, false).apply()
            }
            redraw(app)
            snapshot(app)
        } finally {
            running.set(false)
        }
    }

    /** Sign-out: the next widget draw shows "Sign in to Allternit" and the stored snapshot is dropped. */
    fun clear(context: Context) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().clear().putBoolean(KEY_SIGNED_OUT, true).apply()
        scope.launch { redraw(context.applicationContext) }
    }

    private suspend fun redraw(app: Context) {
        runCatching { StatusWidget().updateAll(app) }
        TILES.forEach { runCatching { TileService.requestListeningState(app, ComponentName(app, it)) } }
        runCatching { publishShortcuts(app, snapshot(app)) }
    }

    /**
     * The dynamic half of the iOS App Intents (`BotEntity`): "Ask <bot>" for the first bots of the roster, next to the
     * four static shortcuts (res/xml/shortcuts.xml). Android shows four shortcuts in total, so two slots.
     */
    private fun publishShortcuts(app: Context, snap: StatusSnapshot?) {
        val manager = app.getSystemService(ShortcutManager::class.java) ?: return
        val bots = snap?.bots.orEmpty().filter { it.name.isNotBlank() }.take(2)
        val infos = bots.mapIndexed { index, bot ->
            val icon = app.resources.getIdentifier("ic_shortcut_ask", "drawable", app.packageName)
            ShortcutInfo.Builder(app, "ask-bot-${bot.botId}")
                .setShortLabel("Ask ${bot.name}")
                .setLongLabel("Ask ${bot.name} a question")
                .setRank(index)
                .apply { if (icon != 0) setIcon(Icon.createWithResource(app, icon)) }
                .setIntent(SystemLinks.intent(app, SystemLinks.botChat(bot.botId)))
                .build()
        }
        manager.setDynamicShortcuts(infos)
    }

    /** Every tile service class, so [redraw] can ask each to refresh. */
    private val TILES = listOf(
        "com.allternit.feature.system.CallMainBotTile",
        "com.allternit.feature.system.AskBotTile",
        "com.allternit.feature.system.HandOffTaskTile",
        "com.allternit.feature.system.NeedsMeTile",
        "com.allternit.feature.system.RunRoutineTile",
        "com.allternit.feature.system.PauseBotsTile",
    ).map { Class.forName(it) }
}

/** The `allternit://` links the system surfaces open (iOS `DeepLinkRouter`), handled by MainActivity. */
object SystemLinks {
    const val SCHEME = "allternit"
    const val CALL_MAIN = "allternit://call/main"
    const val ASK_MAIN = "allternit://bot/main/chat"
    const val HAND_OFF = "allternit://handoff"
    const val INBOX = "allternit://inbox"
    const val ROUTINES = "allternit://routines"

    fun botChat(botId: String) = "allternit://bot/${Uri.encode(botId)}/chat"

    fun inboxItem(id: String) = "allternit://inbox/${Uri.encode(id)}"

    /** A VIEW intent for MainActivity (its `allternit` scheme filter in the nativeapp manifest). */
    fun intent(context: Context, url: String): Intent =
        Intent(Intent.ACTION_VIEW, Uri.parse(url))
            .setPackage(context.packageName)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
}
