package com.allternit.feature.system

import android.app.PendingIntent
import android.content.Intent
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import com.allternit.core.auth.AllternitAuth
import com.allternit.feature.chat.ChatBackends
import com.allternit.feature.chat.RuntimeCall
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import org.json.JSONObject

/**
 * The Quick Settings tiles, one per iOS control (`AllternitControls`): Call main bot, Ask a bot, Hand off a task,
 * Show what needs me, Run routine and Pause all bots. Five open the app on the iOS link; Pause is a toggle.
 */
abstract class LinkTile(private val url: String, private val title: String) : TileService() {
    override fun onStartListening() {
        qsTile?.apply {
            state = Tile.STATE_INACTIVE
            label = title
            updateTile()
        }
    }

    override fun onClick() {
        val intent = SystemLinks.intent(this, url)
        if (Build.VERSION.SDK_INT >= 34) {
            val pending = PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            startActivityAndCollapse(pending)
        } else {
            @Suppress("DEPRECATION")
            startActivityAndCollapse(intent)
        }
    }
}

class CallMainBotTile : LinkTile(SystemLinks.CALL_MAIN, "Call main bot")
class AskBotTile : LinkTile(SystemLinks.ASK_MAIN, "Ask a bot")
class HandOffTaskTile : LinkTile(SystemLinks.HAND_OFF, "Hand off a task")
class RunRoutineTile : LinkTile(SystemLinks.ROUTINES, "Run a routine")

/** Show what needs me: shows the count when something waits (iOS shows the label only). */
class NeedsMeTile : TileService() {
    override fun onStartListening() {
        val needs = StatusSync.snapshot(this)?.needsCount ?: 0
        qsTile?.apply {
            label = "Show what needs me"
            state = if (needs > 0) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
            if (Build.VERSION.SDK_INT >= 29) subtitle = if (needs > 0) "$needs waiting" else null
            updateTile()
        }
        StatusSync.refreshAsync(this)
    }

    override fun onClick() {
        val intent = SystemLinks.intent(this, SystemLinks.INBOX)
        if (Build.VERSION.SDK_INT >= 34) {
            startActivityAndCollapse(PendingIntent.getActivity(this, 1, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT))
        } else {
            @Suppress("DEPRECATION")
            startActivityAndCollapse(intent)
        }
    }
}

/**
 * Pause all bots (iOS `PauseBotsControl`, a toggle): `GET/PUT /api/v1/bots/pause` on the computer, the same call Bots mode
 * makes. The state follows the stored snapshot; a tap flips it in place and redraws (iOS opens the app to run the intent).
 */
class PauseBotsTile : TileService() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    override fun onStartListening() {
        draw(StatusSync.snapshot(this)?.paused ?: false, null)
        scope.launch { StatusSync.refresh(applicationContext)?.let { draw(it.paused, null) } }
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    override fun onClick() {
        val paused = qsTile?.state == Tile.STATE_ACTIVE
        draw(!paused, "Working")
        scope.launch {
            AllternitAuth.start(applicationContext)
            val result = kotlinx.coroutines.withContext(Dispatchers.IO) {
                if (AllternitAuth.tokenProvider.bearerToken().isNullOrBlank()) return@withContext null
                ChatBackends.create(applicationContext)
                    .runtimeJson("PUT", "/api/v1/bots/pause", "{\"paused\":${!paused},\"by\":\"android\"}")
            }
            when (result) {
                is RuntimeCall.Ok -> {
                    val now = (result.body as? kotlinx.serialization.json.JsonObject)?.get("paused")
                        ?.let { (it as? kotlinx.serialization.json.JsonPrimitive)?.content?.toBooleanStrictOrNull() } ?: !paused
                    draw(now, null)
                    StatusSync.refresh(applicationContext)
                }
                RuntimeCall.NoComputer -> draw(paused, "No computer")
                null -> draw(paused, "Sign in")
                else -> draw(paused, "Try again")
            }
        }
    }

    private fun draw(paused: Boolean, subtitleText: String?) {
        qsTile?.apply {
            label = if (paused) "Bots paused" else "Pause all bots"
            state = if (paused) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
            if (Build.VERSION.SDK_INT >= 29) subtitle = subtitleText ?: if (paused) "Tap to resume" else null
            updateTile()
        }
    }
}
