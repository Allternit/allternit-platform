package com.allternit.feature.bots

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.rfb.BarIcon
import com.allternit.core.rfb.ComputerDriveScreen
import com.allternit.core.rfb.ComputerRecord
import com.allternit.core.rfb.ComputersApi
import com.allternit.core.rfb.DriveTopBar
import com.allternit.core.rfb.RemoteScreenCanvas
import com.allternit.core.rfb.RfbSession
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.launch

/** A bot's desktop sandbox (iOS BotDesktopStatus, `/api/v1/bots/:id/desktop`). */
data class BotDesktopStatus(val status: String, val controlState: String, val wsUrl: String?, val sandboxId: String) {
    val isRunning: Boolean get() = status == "running"
    /** Status-pill label, the web header copy. */
    val controlLabel: String get() = controlLabel(controlState)

    companion object {
        fun controlLabel(state: String): String = when (state) {
            "bot_controls" -> "Bot controls"
            "human_controls" -> "You control"
            "human_observing" -> "You're observing"
            else -> "Unknown"
        }
    }
}

/**
 * The bot desktop calls and their state per bot (iOS BotDesktopStore): status, provision, and the
 * observe / take-over / hand-back control actions, each followed by a status resync.
 */
object BotDesktopStore {
    class Entry(
        val status: BotDesktopStatus? = null,
        val sandboxId: String? = null,
        val loading: Boolean = false,
        val provisioning: Boolean = false,
        val actionInFlight: Boolean = false,
        val error: String? = null,
    ) {
        fun copy(
            status: BotDesktopStatus? = this.status, sandboxId: String? = this.sandboxId, loading: Boolean = this.loading,
            provisioning: Boolean = this.provisioning, actionInFlight: Boolean = this.actionInFlight, error: String? = this.error,
        ) = Entry(status, sandboxId, loading, provisioning, actionInFlight, error)
    }

    val entries = mutableStateMapOf<String, Entry>()

    private fun update(botId: String, f: (Entry) -> Entry) {
        entries[botId] = f(entries[botId] ?: Entry())
    }

    private fun base(botId: String) = "bots/${LibraryNet.escape(botId)}/desktop"
    private fun sandboxQuery(id: String?) = if (id.isNullOrEmpty()) "" else "?sandbox_id=${LibraryNet.escape(id)}"

    suspend fun refresh(botId: String) {
        update(botId) { it.copy(loading = true, error = null) }
        when (val r = LibraryNet.get(base(botId) + sandboxQuery(entries[botId]?.sandboxId))) {
            is ApiResult.Ok -> {
                val o = r.value.obj
                val status = BotDesktopStatus(
                    status = o.str("status") ?: "unknown",
                    controlState = o.str("control_state") ?: "unknown",
                    wsUrl = o.str("ws_url"),
                    sandboxId = o.str("sandbox_id").orEmpty(),
                )
                update(botId) { it.copy(status = status, sandboxId = status.sandboxId.ifEmpty { it.sandboxId }, loading = false) }
            }
            is ApiResult.HttpError -> if (r.status == 404) {
                // No desktop yet: the card offers Start a desktop.
                update(botId) { it.copy(status = BotDesktopStatus("off", "bot_controls", null, ""), loading = false) }
            } else update(botId) { it.copy(loading = false, error = r.friendly("load the desktop")) }
            else -> update(botId) { it.copy(loading = false, error = r.friendly("load the desktop")) }
        }
    }

    suspend fun provision(botId: String) {
        if (entries[botId]?.provisioning == true) return
        update(botId) { it.copy(provisioning = true, error = null) }
        when (val r = LibraryNet.send("POST", base(botId) + "/provision")) {
            is ApiResult.Ok -> {
                update(botId) { it.copy(sandboxId = r.value.obj.str("sandbox_id") ?: it.sandboxId, provisioning = false) }
                refresh(botId)
            }
            else -> update(botId) { it.copy(provisioning = false, error = (r as? ApiResult.HttpError)?.message ?: r.friendly("start the desktop")) }
        }
    }

    suspend fun observe(botId: String) = control(botId, "observe")
    suspend fun takeOver(botId: String) = control(botId, "take-over")
    suspend fun handBack(botId: String) = control(botId, "hand-back")

    private suspend fun control(botId: String, action: String) {
        val e = entries[botId]
        val sandbox = e?.sandboxId
        if (sandbox.isNullOrEmpty() || e.actionInFlight) return
        update(botId) { it.copy(actionInFlight = true, error = null) }
        val r = LibraryNet.send("POST", base(botId) + "/" + action + sandboxQuery(sandbox))
        update(botId) { it.copy(actionInFlight = false, error = if (r is ApiResult.Ok) null else r.friendly("change who controls the desktop")) }
        if (r is ApiResult.Ok) refresh(botId)
    }
}

/**
 * A bot's own computer from its page (iOS BotComputerView): its desktop sandbox (observe / take over
 * / hand back, View desktop opens the native viewer) and the computers assigned to it, each opening
 * its live screen.
 */
@Composable
internal fun BotComputerScreen(bot: BotRecord, nav: BotNavigator) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val entry = BotDesktopStore.entries[bot.id]
    var computers by remember { mutableStateOf<List<ComputerRecord>>(emptyList()) }
    var viewing by remember { mutableStateOf<BotDesktopStatus?>(null) }
    var driving by remember { mutableStateOf<ComputerRecord?>(null) }
    suspend fun reload() {
        BotDesktopStore.refresh(bot.id)
        computers = (ComputersApi.list(bot.id) as? ComputersApi.Result.Ok)?.value.orEmpty()
    }
    LaunchedEffect(bot.id) { reload() }
    BotScreen("Computer", nav, testTag = "botComputerScreen") {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(vertical = 8.dp)) {
            Text("${bot.displayName}'s desktop", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(start = 20.dp, bottom = 6.dp, top = 8.dp))
            RowGroup {
                val status = entry?.status
                val busy = entry?.actionInFlight == true
                when {
                    entry?.loading == true && status == null -> Box(Modifier.fillMaxWidth().padding(16.dp), contentAlignment = Alignment.Center) {
                        CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp)
                    }
                    status != null && status.isRunning -> {
                        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Dot(AllternitStatic.StatusSuccess)
                            Text("Running", color = AllternitStatic.StatusSuccess, fontSize = 15.sp, modifier = Modifier.weight(1f))
                            Text(status.controlLabel, color = c.textSecondary, fontSize = 12.5.sp)
                        }
                        if (status.wsUrl != null) {
                            Hairline()
                            NavRow("View desktop", com.allternit.core.design.R.drawable.ph_desktop, null, "botViewDesktop", last = false) { viewing = status }
                        }
                        Hairline()
                        if (status.controlState == "human_controls") {
                            NavRow("Hand back to ${bot.displayName}", com.allternit.core.design.R.drawable.ph_hand, null, "botDesktopHandBack", last = true) {
                                if (!busy) scope.launch { BotDesktopStore.handBack(bot.id) }
                            }
                        } else {
                            NavRow("Take over", com.allternit.core.design.R.drawable.ph_hand, null, "botDesktopTakeOver", last = status.controlState == "human_observing") {
                                if (!busy) scope.launch { BotDesktopStore.takeOver(bot.id) }
                            }
                            if (status.controlState != "human_observing") {
                                Hairline()
                                NavRow("Watch without pausing", com.allternit.core.design.R.drawable.ph_eye, null, "botDesktopObserve", last = true) {
                                    if (!busy) scope.launch { BotDesktopStore.observe(bot.id) }
                                }
                            }
                        }
                    }
                    else -> {
                        Text("${bot.displayName} has no desktop running.", color = c.textSecondary, fontSize = 15.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp))
                        Hairline()
                        val provisioning = entry?.provisioning == true
                        NavRow(if (provisioning) "Starting…" else "Start a desktop", com.allternit.core.design.R.drawable.ph_desktop, null, "botDesktopProvision", last = true) {
                            if (!provisioning) scope.launch { BotDesktopStore.provision(bot.id) }
                        }
                    }
                }
                entry?.error?.let {
                    Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp))
                }
            }
            if (computers.isNotEmpty()) {
                Text("Assigned computers", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(start = 20.dp, bottom = 6.dp, top = 20.dp))
                RowGroup {
                    computers.forEachIndexed { i, r ->
                        Row(
                            Modifier.fillMaxWidth().clickable { driving = r }.padding(horizontal = 16.dp, vertical = 12.dp).testTag("botComputerRow"),
                            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            Dot(if (r.isRunning) AllternitStatic.StatusSuccess else c.textTertiary)
                            Column(Modifier.weight(1f)) {
                                Text(r.name, color = c.textPrimary, fontSize = 15.sp)
                                Text("${r.kindLabel} · ${r.status.replaceFirstChar { it.uppercase() }}", color = c.textSecondary, fontSize = 12.5.sp)
                            }
                            Icon(painterResource(com.allternit.core.design.R.drawable.ph_caret_right), null, tint = c.textTertiary, modifier = Modifier.size(14.dp))
                        }
                        if (i < computers.lastIndex) Hairline()
                    }
                }
            }
        }
    }
    viewing?.let { status ->
        Dialog(onDismissRequest = { viewing = null }, properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
            BotDesktopView(bot, status, onClose = { viewing = null })
        }
    }
    driving?.let { computer ->
        Dialog(onDismissRequest = { driving = null }, properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
            ComputerDriveScreen(computer, backTitle = "Computer", onBack = { driving = null })
        }
    }
}

/**
 * Full-screen native VNC viewer for a bot's desktop sandbox (iOS BotDesktopView): the RFB canvas,
 * Close, the connection line, the keyboard while you control, the control badge, and "Hand back to
 * bot" or the view-only banner. Input only leaves while the store says `human_controls`.
 */
@Composable
internal fun BotDesktopView(bot: BotRecord, status: BotDesktopStatus, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val session = remember(status.sandboxId) { RfbSession(status.wsUrl.orEmpty(), status.controlState == "human_controls") }
    val live = BotDesktopStore.entries[bot.id]?.status?.controlState ?: status.controlState
    session.allowsInput = live == "human_controls"
    var keyboard by remember { mutableStateOf(false) }
    DisposableEffect(session) {
        session.start()
        onDispose { session.dispose() }
    }
    val connection = when (val s = session.state) {
        RfbSession.State.Connecting -> "Connecting…"
        RfbSession.State.Live -> "Live"
        is RfbSession.State.Reconnecting -> "Reconnecting (${s.attempt}/${RfbSession.MAX_RECONNECT_ATTEMPTS})…"
        is RfbSession.State.Failed -> "Connection failed"
        is RfbSession.State.NeedsCredentials -> "Sign-in needed"
    }
    val badge = if (live == "human_controls") AllternitStatic.StatusSuccess else AllternitStatic.StatusInfo
    Column(Modifier.fillMaxSize().background(Color.Black).statusBarsPadding().testTag("botDesktopView")) {
        DriveTopBar(
            backTitle = "Close",
            title = session.desktopName.ifEmpty { "${bot.displayName} desktop" },
            center = connection,
            onBack = onClose,
            dark = true,
            trailing = {
                if (session.allowsInput) {
                    BarIcon(com.allternit.core.design.R.drawable.ph_keyboard, if (keyboard) "Hide keyboard" else "Show keyboard", "botDesktopKeyboard", tint = Color.White) { keyboard = !keyboard }
                }
                Text(
                    BotDesktopStatus.controlLabel(live), color = badge, fontSize = 12.sp,
                    modifier = Modifier.clip(CircleShape).background(badge.copy(alpha = 0.2f)).padding(horizontal = 8.dp, vertical = 4.dp),
                )
            },
        )
        Box(Modifier.weight(1f)) {
            RemoteScreenCanvas(session, keyboardVisible = keyboard, bottomOverlay = {
                if (live == "human_controls") {
                    Row(
                        Modifier.clip(CircleShape).background(Color.White.copy(alpha = 0.18f))
                            .clickable { scope.launch { BotDesktopStore.handBack(bot.id) } }
                            .padding(horizontal = 14.dp, vertical = 8.dp).testTag("botDesktopHandBackPill"),
                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        Icon(painterResource(com.allternit.core.design.R.drawable.ph_hand), null, tint = Color.White, modifier = Modifier.size(16.dp))
                        Text("Hand back to bot", color = Color.White, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                    }
                } else {
                    Row(
                        Modifier.clip(CircleShape).background(Color.White.copy(alpha = 0.18f)).padding(horizontal = 12.dp, vertical = 6.dp),
                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        Icon(painterResource(com.allternit.core.design.R.drawable.ph_eye), null, tint = Color.White, modifier = Modifier.size(14.dp))
                        Text(if (live == "human_observing") "You're observing — view only" else "Bot is driving — view only", color = Color.White, fontSize = 12.sp)
                    }
                }
            })
        }
    }
}
