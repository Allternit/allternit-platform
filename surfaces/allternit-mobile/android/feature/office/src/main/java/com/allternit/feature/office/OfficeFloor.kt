package com.allternit.feature.office

import android.annotation.SuppressLint
import android.content.Context
import android.view.ViewGroup
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.webkit.WebMessageCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.auth.AuthState
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.LibrarySpinner
import com.allternit.feature.bots.BotFloorEntry
import com.allternit.feature.bots.BotModeStore
import com.allternit.feature.bots.BotsResult
import com.allternit.feature.bots.floorEntries
import com.allternit.feature.bots.floorProjects
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.ChatSendRequest
import com.allternit.feature.chat.ChatViewModel
import com.allternit.feature.chat.SessionContext
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

private const val FACILITY_ORIGIN = "https://3dfacility.allternit.com"
private const val HOST_ORIGIN = "https://ai.allternit.com"

/** The live roster in the facility's `allternit:workspace` shape (iOS feedJSON, web workspace-feed.ts parseBots). */
internal fun workspaceFeed(entries: List<BotFloorEntry>, projects: List<com.allternit.feature.bots.BotFloorProject>, signedIn: Boolean): String {
    val obj = buildJsonObject {
        put("type", "allternit:workspace")
        put("bots", buildJsonArray {
            entries.forEach { b ->
                add(buildJsonObject {
                    put("id", b.id); put("name", b.name); put("role", b.role); put("color", b.colorHex)
                    put("status", b.status); put("task", b.task); put("chief", b.chief)
                    b.question?.let { put("question", it) }
                    b.avatarSheet?.let { sheet ->
                        put("avatar", buildJsonObject { put("sheet", sheet); if (b.avatarStill) put("layout", "still") })
                    }
                })
            }
        })
        if (signedIn && projects.isNotEmpty()) {
            put("projects", buildJsonArray { projects.forEach { p -> add(buildJsonObject { put("id", p.id); put("name", p.name); put("updatedAt", p.updatedAt) }) } })
        }
    }
    return obj.toString()
}

/**
 * The Office panel's thread (iOS OfficePanelThread): the picked bot's main chat, sent and streamed through the shared
 * chat stack ([ChatViewModel], the one the bot chat uses), posted to the facility as `allternit:workspace:thread`.
 * One view model per bot, so picking another bot never stops a reply in progress.
 */
private class OfficePanelThread(private val store: BotModeStore, private val backend: ChatBackend, private val context: Context) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val chats = mutableMapOf<String, ChatViewModel>()
    private var botId by mutableStateOf<String?>(null)
    private var sendError by mutableStateOf<String?>(null)
    var post: (String) -> Unit = {}
    private var watch: Job? = null

    init {
        watch = scope.launch {
            snapshotFlow { snapshot() }.collectLatest { json ->
                // At most about 4 posts a second; the trailing post carries the latest state.
                delay(250)
                if (json != null) post(json)
            }
        }
    }

    fun dispose() { watch?.cancel(); chats.values.forEach { it.dispose() }; chats.clear(); scope.coroutineContext[Job]?.cancel() }

    private suspend fun select(id: String): Boolean {
        val bot = store.bot(id) ?: return false
        if (botId != id) { botId = id; sendError = null }
        val chat = chats.getOrPut(id) {
            ChatViewModel(backend, context.contentResolver).also {
                it.sessionContext = SessionContext(originSurface = "chat", sessionMode = "agent", agentId = bot.id, agentName = bot.displayName)
            }
        }
        return when (val r = store.mainChatSessionId(bot)) {
            is BotsResult.Ok -> {
                // Fresh history each pick, unless a reply is still coming in.
                if (!chat.isStreaming && !chat.isCreatingSession) chat.loadSession(r.value)
                true
            }
            is BotsResult.Failed -> { if (botId == id) sendError = "Couldn't send. ${r.message}"; false }
        }
    }

    /** Sends a message from the panel into the bot's main chat, no navigation. */
    fun send(text: String, id: String) {
        scope.launch {
            var ready = botId == id && chats[id]?.currentSessionId != null
            if (!ready) ready = select(id)
            val chat = chats[id]
            if (!ready || chat == null) { if (botId == id && sendError == null) sendError = "Couldn't send. Try again."; return@launch }
            // Let the history land first so it can't replace the new message.
            var waited = 0
            while (waited < 50 && chat.messages.isEmpty() && chat.transientError == null) { delay(100); waited++ }
            if (chat.isStreaming || chat.isCreatingSession || chat.currentSessionId == null) { if (botId == id) sendError = "Couldn't send. Try again."; return@launch }
            sendError = null
            chat.send(ChatSendRequest(text = text))
        }
    }

    private fun snapshot(): String? {
        val id = botId ?: return null
        val chat = chats[id]
        val lines = (chat?.messages.orEmpty()).mapNotNull { m ->
            if (m.role != "user" && m.role != "assistant") return@mapNotNull null
            val t = m.content.trim()
            if (t.isEmpty()) return@mapNotNull null
            buildJsonObject {
                put("id", m.id); put("from", if (m.role == "user") "you" else "bot"); put("text", t.take(2000)); put("at", 0)
            }
        }.takeLast(20).toMutableList()
        (sendError ?: chat?.transientError)?.let { e ->
            lines.add(buildJsonObject { put("id", "office-error"); put("from", "bot"); put("text", e); put("at", System.currentTimeMillis()) })
        }
        return buildJsonObject {
            put("type", "allternit:workspace:thread"); put("botId", id)
            put("lines", JsonArray(lines)); put("working", chat?.isStreaming ?: false)
        }.toString()
    }
}

private fun hostPage(dark: Boolean): String {
    val src = "$FACILITY_ORIGIN/?embed=facility&actions=chat${if (dark) "&theme=dark" else ""}"
    // The facility only talks to a parent frame, so this host page (served as ai.allternit.com, an origin the facility
    // accepts) frames it and relays: the app -> feed(json) -> the frame; the frame's actions -> the `office` handler.
    return """
<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1,maximum-scale=1">
<style>html,body{margin:0;height:100%;background:transparent}iframe{border:0;width:100%;height:100%;display:block}</style></head>
<body><iframe id="f" src="$src" allow="autoplay"></iframe><script>
const f = document.getElementById('f'), O = '$FACILITY_ORIGIN';
window.feed = (m) => f.contentWindow && f.contentWindow.postMessage(m, O);
let connected = false;
addEventListener('message', (e) => {
  if (e.origin !== O || e.source !== f.contentWindow || !e.data) return;
  const d = e.data;
  if (d.type === 'allternit:facility:ready' && d.version === 1 && !connected) { connected = true; office.postMessage(JSON.stringify({ type: 'ready' })); }
  else if (d.type === 'allternit:facility:visitor' && d.version === 1) office.postMessage(JSON.stringify({ type: 'visitor', visitor: d.visitor }));
  else if (d.type === 'allternit:workspace:action') office.postMessage(JSON.stringify({ type: 'action', action: String(d.action || ''), botId: typeof d.botId === 'string' ? d.botId : null, text: typeof d.text === 'string' ? d.text.slice(0, 4000) : null }));
});
</script></body></html>
""".trimIndent()
}

/**
 * The bots' Office floor (iOS OfficeFloorView): the facility from 3dfacility.allternit.com (`?embed=facility`), fed with
 * the person's real bots. Picking a bot or answering opens its main chat ([openBot]); a message written in the panel is
 * sent in the background and its reply streams into the panel. The 3D scene is the same web facility iOS frames, so
 * there is no 2D fallback to build; a failed or slow load shows the same "didn't load" state with Try again.
 */
@SuppressLint("SetJavaScriptEnabled")
@Composable
fun OfficeFloorScreen(store: BotModeStore, backend: ChatBackend, openBot: (String) -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val state by AllternitAuth.state.collectAsState()
    val userId = (state as? AuthState.SignedIn)?.account?.userId
    val dark = c.bgPrimary.luminance() < 0.5f
    var ready by remember { mutableStateOf(false) }
    var failed by remember { mutableStateOf(false) }
    var attempt by remember { mutableIntStateOf(0) }
    val thread = remember(store, backend) { OfficePanelThread(store, backend, context.applicationContext) }
    DisposableEffect(thread) { onDispose { thread.dispose() } }
    val webRef = remember { arrayOfNulls<WebView>(1) }
    var frameReady by remember(attempt) { mutableStateOf(false) }
    val visitorKey = "allternit.facility.visitor.${userId ?: "local"}"
    val prefs = remember { context.getSharedPreferences("allternit_office", Context.MODE_PRIVATE) }
    val currentOpenBot by androidx.compose.runtime.rememberUpdatedState(openBot)

    // Names only for the Commons studios; a failed load just omits the field. Statuses and questions come from the
    // threads, so keep them fresh while the floor is open (iOS: every 15 s).
    LaunchedEffect(store) {
        if (store.projects.isEmpty()) store.refreshProjects()
        while (true) { store.refreshThreads(); delay(15_000) }
    }
    // A cold 3D load on a slow phone can take a while (web: 25 s).
    LaunchedEffect(attempt) { delay(25_000); if (!ready) failed = true }
    // Push the roster whenever it changes, once the frame is up.
    LaunchedEffect(frameReady, attempt) {
        if (!frameReady) return@LaunchedEffect
        var last: String? = null
        snapshotFlow { workspaceFeed(store.floorEntries(), store.floorProjects(), userId != null) }.collectLatest { feed ->
            if (feed != last) { last = feed; webRef[0]?.evaluateJavascript("window.feed($feed)", null) }
        }
    }

    Box(Modifier.fillMaxSize().background(c.bgPrimary).testTag("officeFloor")) {
        if (failed) {
            FriendlyState(
                FriendlyStyle.Offline, "The office didn't load", "Check your connection and try again.",
                actionTitle = "Try again", onAction = { failed = false; ready = false; attempt += 1 },
            ) { Icon(painterResource(DesignR.drawable.ph_warning), null, tint = c.textSecondary) }
        } else {
            androidx.compose.runtime.key(attempt, userId) {
                AndroidView(
                    modifier = Modifier.fillMaxSize(),
                    factory = { ctx ->
                        WebView(ctx).apply {
                            settings.javaScriptEnabled = true
                            settings.domStorageEnabled = true
                            settings.allowFileAccess = false
                            settings.allowContentAccess = false
                            settings.mediaPlaybackRequiresUserGesture = false
                            settings.mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
                            setBackgroundColor(android.graphics.Color.TRANSPARENT)
                            layoutParams = ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
                            webViewClient = WebViewClient()
                            webRef[0] = this
                            thread.post = { json -> if (frameReady) evaluateJavascript("window.feed($json)", null) }
                            if (WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) {
                                WebViewCompat.addWebMessageListener(this, "office", setOf(HOST_ORIGIN)) { _, msg: WebMessageCompat, _, isMainFrame, _ ->
                                    // Only the validated top-level relay may reach native actions or persistence.
                                    if (!isMainFrame) return@addWebMessageListener
                                    val body = runCatching { kotlinx.serialization.json.Json.parseToJsonElement(msg.data ?: "") as? JsonObject }.getOrNull() ?: return@addWebMessageListener
                                    fun str(k: String) = (body[k] as? JsonPrimitive)?.takeIf { it !is JsonNull }?.content
                                    when (str("type")) {
                                        "visitor" -> {
                                            val v = body["visitor"] as? JsonObject
                                            val name = (v?.get("config") as? JsonObject)?.get("name") as? JsonPrimitive
                                            val text = v?.toString()
                                            if (frameReady && name != null && text != null && text.length < 3_200_000) prefs.edit().putString(visitorKey, text).apply()
                                        }
                                        "ready" -> {
                                            // Identity state is owned by the app, never the frame's partitioned storage.
                                            val visitor = prefs.getString(visitorKey, null) ?: "null"
                                            evaluateJavascript("window.feed({type:'allternit:facility:init',version:1,visitor:$visitor})", null)
                                            frameReady = true
                                            ready = true
                                            evaluateJavascript("window.feed(${workspaceFeed(store.floorEntries(), store.floorProjects(), userId != null)})", null)
                                        }
                                        "action" -> {
                                            val id = str("botId")
                                            if (id != null && store.bot(id) != null) when (str("action")) {
                                                // The full facility opens real native bot work, then returns to the same world.
                                                "select", "answer", "open" -> currentOpenBot(id)
                                                // Written in the panel: sent into the bot's main chat here, the reply streams back.
                                                "message" -> str("text")?.trim()?.takeIf { it.isNotEmpty() }?.let { thread.send(it, id) }
                                            }
                                        }
                                    }
                                }
                            }
                            loadDataWithBaseURL("$HOST_ORIGIN/office-host", hostPage(dark), "text/html", "utf-8", null)
                        }
                    },
                    onRelease = { webRef[0] = null; it.removeAllViews(); it.destroy() },
                )
            }
            if (!ready) {
                Box(Modifier.align(Alignment.Center).clip(RoundedCornerShape(12.dp)).background(c.bgSecondary).padding(16.dp)) { LibrarySpinner("Opening the office…") }
            }
        }
    }
}
