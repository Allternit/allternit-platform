package com.allternit.core.rfb

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiClients
import com.allternit.core.api.ApiConfig
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put
import okhttp3.HttpUrl.Companion.toHttpUrl
import java.net.URLEncoder
import java.util.UUID

/** One computer from `GET /api/v1/computers` (iOS ComputerRecord). */
data class ComputerRecord(
    val id: String,
    val kind: String,
    val status: String,
    val name: String,
    val botId: String?,
    val host: String?,
) {
    val isRunning: Boolean get() = status == "running"
    val kindLabel: String get() = when (kind) {
        "local" -> "This network"
        "byo_vps" -> "Your server"
        "managed" -> "Cloud"
        else -> kind.replaceFirstChar { it.uppercase() }
    }
}

/** Who holds a computer's control lease (iOS ComputerControlHolder). */
data class ComputerControlHolder(val kind: String, val id: String, val label: String?, val deviceId: String?) {
    val displayLabel: String get() = label?.takeIf { it.isNotEmpty() } ?: when (kind) {
        "agent" -> "An agent"
        "session" -> "A session"
        "bot" -> "A bot"
        else -> "Someone"
    }
}

/** The `/api/v1/computers` calls behind the screen viewer (iOS BotModeClient computer methods). */
object ComputersApi {
    private val config = ApiConfig()
    private val okHttp by lazy { ApiClients.okHttp(bearer = AllternitAuth.tokenProvider) }

    sealed interface Result<out T> {
        data class Ok<T>(val value: T) : Result<T>
        data class Failed(val message: String) : Result<Nothing>
    }

    /** A stable per-install id for the lease holder (iOS `allternit-bot-mode-device-id`). */
    fun deviceId(context: Context): String {
        val prefs = context.applicationContext.getSharedPreferences("allternit_shell", Context.MODE_PRIVATE)
        prefs.getString(KEY_DEVICE, null)?.let { return it }
        return UUID.randomUUID().toString().also { prefs.edit().putString(KEY_DEVICE, it).apply() }
    }

    suspend fun list(botId: String? = null): Result<List<ComputerRecord>> =
        call("GET", if (botId != null) "computers?bot_id=${esc(botId)}" else "computers") { j ->
            val o = j as? JsonObject
            val rows = (o?.get("computers") ?: j) as? kotlinx.serialization.json.JsonArray
            rows.orEmpty().mapNotNull { el ->
                val c = el as? JsonObject ?: return@mapNotNull null
                ComputerRecord(c.s("id") ?: return@mapNotNull null, c.s("kind") ?: "managed", c.s("status") ?: "unknown",
                    c.s("name") ?: "Computer", c.s("bot_id"), c.s("host"))
            }
        }

    suspend fun start(id: String): Result<Unit> = call("POST", "computers/${esc(id)}/start", "{}") { }

    /** `ws.vnc` of `GET computers/:id/status`. */
    suspend fun vncPath(id: String): Result<String> = call("GET", "computers/${esc(id)}/status") { j ->
        ((j as? JsonObject)?.get("ws") as? JsonObject)?.s("vnc").orEmpty()
    }

    /** The current lease holder, or null when nobody drives. */
    suspend fun control(id: String): Result<ComputerControlHolder?> = call("GET", "computers/${esc(id)}/control") { j ->
        val holder = ((j as? JsonObject)?.get("control") as? JsonObject)?.get("holder") as? JsonObject
        holder?.let { ComputerControlHolder(it.s("kind").orEmpty(), it.s("id").orEmpty(), it.s("label"), it.s("device_id")) }
    }

    suspend fun take(context: Context, id: String): Result<Unit> =
        call("POST", "computers/${esc(id)}/control/take", "{}", headers = mapOf(
            "x-allternit-device-id" to deviceId(context),
            "x-allternit-device-platform" to "android",
        )) { }

    suspend fun release(id: String): Result<Unit> = call("POST", "computers/${esc(id)}/control/release", "{}") { }

    /** A short-lived VNC token; full control only for the lease holder (server-enforced). */
    suspend fun vncToken(id: String, readOnly: Boolean): Result<String> {
        val body = buildJsonObject { put("purpose", "vnc"); put("read_only", readOnly) }.toString()
        return call("POST", "computers/${esc(id)}/ws-token", body) { j -> (j as? JsonObject)?.s("token").orEmpty() }
    }

    fun esc(s: String): String = URLEncoder.encode(s, "UTF-8").replace("+", "%20")

    private suspend fun <T> call(
        method: String,
        path: String,
        body: String? = null,
        headers: Map<String, String> = emptyMap(),
        map: (JsonElement) -> T,
    ): Result<T> {
        val url = (config.apiBaseUrl.toString().trimEnd('/') + "/" + path)
        val base = ApiCall.request(method, url.toHttpUrl(), body)
        val request = base.newBuilder().apply { headers.forEach { (k, v) -> header(k, v) } }.build()
        return when (val r = ApiCall.execute(okHttp, ApiJson.default, request, JsonElement.serializer())) {
            is ApiResult.Ok -> Result.Ok(map(r.value))
            is ApiResult.Decode -> Result.Ok(map(kotlinx.serialization.json.JsonNull))
            is ApiResult.HttpError -> Result.Failed(
                when (r.status) {
                    401, 403 -> "Sign in again to use this computer."
                    404 -> "This computer is gone. It may have been removed on another device."
                    502, 503, 504 -> "This computer isn't responding."
                    else -> r.message?.takeIf { it.isNotBlank() } ?: "Couldn't reach this computer. Try again."
                },
            )
            is ApiResult.Network -> Result.Failed("You're offline. Check your connection and try again.")
        }
    }

    private fun JsonObject.s(key: String): String? = (this[key] as? JsonPrimitive)?.contentOrNull
    private const val KEY_DEVICE = "allternit-bot-mode-device-id"
}

/**
 * A computer's live screen (iOS ComputerDriveView): the control bar (who drives, Take control /
 * Release), Start when it is off, the VNC canvas with a read-only or full token, a lease poll every
 * 5 s that reconnects when control moves, and a full-screen view.
 */
@Composable
fun ComputerDriveScreen(computer: ComputerRecord, backTitle: String, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val myDevice = remember { ComputersApi.deviceId(context) }
    var running by remember { mutableStateOf(computer.isRunning) }
    var holder by remember { mutableStateOf<ComputerControlHolder?>(null) }
    var wsPath by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var fullScreen by remember { mutableStateOf(false) }
    var keyboard by remember { mutableStateOf(false) }
    val holdsControl = holder?.let { it.kind == "user" && it.deviceId == myDevice } == true

    suspend fun connect(refreshLease: Boolean = true) {
        if (!running) return
        error = null
        if (refreshLease) (ComputersApi.control(computer.id) as? ComputersApi.Result.Ok)?.let { holder = it.value }
        val holds = holder?.let { it.kind == "user" && it.deviceId == myDevice } == true
        val path = when (val r = ComputersApi.vncPath(computer.id)) {
            is ComputersApi.Result.Ok -> r.value
            is ComputersApi.Result.Failed -> { wsPath = null; error = r.message; return }
        }
        var token = ComputersApi.vncToken(computer.id, readOnly = !holds)
        // Control moved between the lease read and now: watch instead.
        if (token is ComputersApi.Result.Failed && holds) token = ComputersApi.vncToken(computer.id, readOnly = true)
        when (token) {
            is ComputersApi.Result.Ok -> {
                val sep = if (path.contains("?")) "&" else "?"
                wsPath = "$path${sep}token=${ComputersApi.esc(token.value)}"
            }
            is ComputersApi.Result.Failed -> { wsPath = null; error = token.message }
        }
    }

    LaunchedEffect(computer.id) { connect() }
    LaunchedEffect(computer.id, running) {
        while (running) {
            delay(5_000)
            val r = ComputersApi.control(computer.id) as? ComputersApi.Result.Ok ?: continue
            if (r.value != holder) {
                holder = r.value
                connect(refreshLease = false)
            }
        }
    }

    val controlLabel = when {
        !running -> "Off"
        holder == null -> "Nobody is driving — view only"
        holdsControl -> "You're driving"
        else -> "${holder?.displayLabel} is driving — view only"
    }

    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().testTag("computerDrive")) {
        DriveTopBar(backTitle = backTitle, title = computer.name, onBack = onBack, trailing = {
            if (holdsControl && wsPath != null) {
                BarIcon(com.allternit.core.design.R.drawable.ph_keyboard, if (keyboard) "Hide keyboard" else "Show keyboard", "computerKeyboard") { keyboard = !keyboard }
            }
            BarIcon(com.allternit.core.design.R.drawable.ph_corners_out, "Full screen", "computerFullScreen", enabled = wsPath != null) { fullScreen = true }
        })
        Row(
            Modifier.fillMaxWidth().background(c.bgSecondary).padding(horizontal = 14.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Icon(painterResource(if (holdsControl) com.allternit.core.design.R.drawable.ph_hand else com.allternit.core.design.R.drawable.ph_eye), null,
                tint = if (holdsControl) c.accentPrimary else c.textSecondary, modifier = Modifier.size(18.dp))
            Text(controlLabel, color = c.textSecondary, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            if (running) {
                val label = if (holdsControl) "Release" else "Take control"
                Text(
                    label,
                    color = if (holdsControl) c.textPrimary else c.onAccent,
                    fontSize = 14.sp, fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.clip(RoundedCornerShape(8.dp))
                        .background(if (holdsControl) c.fillNeutral else c.accentPrimary)
                        .clickable(enabled = !busy) {
                            busy = true
                            scope.launch {
                                if (holdsControl) {
                                    ComputersApi.release(computer.id)
                                    keyboard = false
                                } else {
                                    (ComputersApi.take(context, computer.id) as? ComputersApi.Result.Failed)?.let { error = it.message }
                                }
                                connect()
                                busy = false
                            }
                        }
                        .padding(horizontal = 14.dp, vertical = 7.dp)
                        .testTag(if (holdsControl) "computerRelease" else "computerTakeControl"),
                )
            }
        }
        Box(Modifier.weight(1f).fillMaxWidth().background(Color.Black), contentAlignment = Alignment.Center) {
            val path = wsPath
            when {
                !running -> FriendlyState(
                    style = FriendlyStyle.Offline,
                    title = "${computer.name} is off",
                    message = "Start it to see its screen.",
                    actionTitle = if (busy) null else "Start",
                    onAction = if (busy) null else ({
                        busy = true
                        scope.launch {
                            when (val r = ComputersApi.start(computer.id)) {
                                is ComputersApi.Result.Ok -> { running = true; connect() }
                                is ComputersApi.Result.Failed -> error = r.message
                            }
                            busy = false
                        }
                    }),
                    modifier = Modifier.testTag("computerOff"),
                ) { Icon(painterResource(com.allternit.core.design.R.drawable.ph_x), null, tint = c.textSecondary, modifier = Modifier.size(28.dp)) }
                path != null && !fullScreen -> key(path) { ScreenFor(path, holdsControl, keyboard) }
                error != null -> FriendlyState(
                    style = FriendlyStyle.Error,
                    title = "Couldn't connect",
                    message = error.orEmpty(),
                    actionTitle = "Try again",
                    onAction = { scope.launch { connect() } },
                ) { Icon(painterResource(com.allternit.core.design.R.drawable.ph_x), null, tint = AllternitStatic.StatusError, modifier = Modifier.size(28.dp)) }
                path == null -> CircularProgressIndicator(color = Color.White, modifier = Modifier.size(28.dp), strokeWidth = 2.dp)
            }
        }
    }
    val fullPath = wsPath
    if (fullScreen && fullPath != null) {
        Dialog(onDismissRequest = { fullScreen = false }, properties = DialogProperties(usePlatformDefaultWidth = false)) {
            Column(Modifier.fillMaxSize().background(Color.Black)) {
                DriveTopBar(backTitle = "Close", title = computer.name, onBack = { fullScreen = false }, dark = true)
                Box(Modifier.weight(1f)) { key(fullPath) { ScreenFor(fullPath, holdsControl, keyboard) } }
            }
        }
    }
}

/** One RFB session per ws path, input allowed only while this device holds the lease. */
@Composable
private fun ScreenFor(wsPath: String, controls: Boolean, keyboard: Boolean) {
    val session = remember(wsPath) { RfbSession(wsPath, controls) }
    session.allowsInput = controls
    DisposableEffect(session) {
        session.start()
        onDispose { session.dispose() }
    }
    RemoteScreenCanvas(session, keyboardVisible = keyboard)
}

/** The "‹ Back · title · actions" bar the screen viewers share. */
@Composable
fun DriveTopBar(backTitle: String, title: String, onBack: () -> Unit, dark: Boolean = false, center: String? = null, trailing: @Composable () -> Unit = {}) {
    val c = AllternitTheme.colors
    val fg = if (dark) Color.White else c.textPrimary
    Row(Modifier.fillMaxWidth().height(52.dp).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(
            "‹ $backTitle", color = fg, fontSize = 16.sp,
            modifier = Modifier.clip(RoundedCornerShape(8.dp)).clickable(onClickLabel = backTitle) { onBack() }
                .padding(horizontal = 8.dp, vertical = 10.dp).testTag("screenBack"),
        )
        Column(Modifier.weight(1f), horizontalAlignment = Alignment.CenterHorizontally) {
            Text(title, color = fg, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (center != null) Text(center, color = if (dark) Color.White.copy(alpha = 0.6f) else c.textSecondary, fontSize = 11.sp, maxLines = 1)
        }
        Row(horizontalArrangement = Arrangement.spacedBy(2.dp), verticalAlignment = Alignment.CenterVertically) { trailing() }
    }
}

@Composable
fun BarIcon(icon: Int, label: String, testTag: String, enabled: Boolean = true, tint: Color? = null, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        Modifier.size(40.dp).clip(CircleShape).clickable(enabled = enabled, onClickLabel = label) { onClick() }.testTag(testTag),
        contentAlignment = Alignment.Center,
    ) {
        Icon(painterResource(icon), label, tint = (tint ?: c.textPrimary).copy(alpha = if (enabled) 1f else 0.35f), modifier = Modifier.size(20.dp))
    }
}
