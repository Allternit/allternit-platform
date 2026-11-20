package com.allternit.feature.office

import android.annotation.SuppressLint
import android.content.Context
import android.view.ViewGroup
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
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
import androidx.compose.runtime.saveable.rememberSaveable
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
import com.allternit.feature.bots.BotsOfficeKit
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
/** Computers for the facility (web toWorkspaceComputers): name and state only, running first, at most 12. */
internal fun workspaceComputers(list: List<com.allternit.feature.bots.ComputerRow>): List<Triple<String, String, String>> {
    fun state(s: String): String? = when (s) { "running", "stopped", "error" -> s; "creating" -> "starting"; else -> null }
    val kept = list.filter { it.id.isNotEmpty() && it.name.isNotEmpty() && state(it.status) != null }
    // A stable sort keeps the server's order inside each group.
    return kept.sortedByDescending { it.isRunning }.take(12).map { Triple(it.id, it.name, state(it.status)!!) }
}

internal fun workspaceFeed(entries: List<BotFloorEntry>, projects: List<com.allternit.feature.bots.BotFloorProject>, signedIn: Boolean, computers: List<Triple<String, String, String>> = emptyList(), floor: FloorState? = null, v2: Boolean = false, now: Long = System.currentTimeMillis(), done: List<OfficeDoneBubble> = emptyList()): String {
    // Feed v2 only when the frame said v:2 and the server's floor state is v2.
    val f = if (v2) floor else null
    val obj = buildJsonObject {
        put("type", "allternit:workspace")
        if (f != null) put("v", 2)
        put("bots", buildJsonArray {
            entries.forEach { b ->
                add(buildJsonObject {
                    put("id", b.id); put("name", b.name); put("role", b.role); put("color", b.colorHex)
                    put("status", b.status); put("task", b.task); put("chief", if (f?.mainBotId != null) b.id == f.mainBotId else b.chief)
                    f?.bots?.get(b.id)?.let { fb ->
                        fb.pod?.let { put("pod", it) }
                        // One need per bot: approval, then question, then suggestion.
                        val (kind, count) = when {
                            fb.approval > 0 -> "approval" to fb.approval
                            fb.question > 0 -> "question" to fb.question
                            fb.suggestion > 0 -> "suggestion" to fb.suggestion
                            else -> "" to 0
                        }
                        if (count > 0) put("needs", buildJsonObject { put("kind", kind); put("count", count) })
                    }
                    b.question?.let { put("question", it) }
                    b.avatarSheet?.let { sheet ->
                        put("avatar", buildJsonObject { put("sheet", sheet); if (b.avatarStill) put("layout", "still") })
                    }
                })
            }
        })
        if (f != null) {
            put("pods", buildJsonArray { f.pods.forEach { p -> add(buildJsonObject { put("id", p.id); put("name", p.name); put("color", p.color); put("kind", p.kind) }) } })
            val ids = entries.map { it.id }.toSet()
            put("bubbles", buildJsonArray {
                // Done bubbles first: an approval from the sheet's card shows over the bot right away (10 minutes).
                done.filter { it.botId in ids && now - it.at in 0..600_000L }.takeLast(4)
                    .forEach { d -> add(buildJsonObject { put("id", d.id); put("botId", d.botId); put("kind", "done"); put("text", d.text.take(120)); put("at", d.at) }) }
                f.bots.entries.asSequence()
                    .filter { (id, b) -> id in ids && !b.resultText.isNullOrBlank() && now - b.resultAt in 0..600_000L }
                    .sortedByDescending { it.value.resultAt }.take(8)
                    .forEach { (id, b) -> add(buildJsonObject { put("id", "res-$id-${b.resultAt}"); put("botId", id); put("kind", "result"); put("text", b.resultText!!.trim().take(120)); put("at", b.resultAt) }) }
            })
            if (f.handoffs.isNotEmpty()) put("handoffs", buildJsonArray {
                f.handoffs.filter { it.from in ids && it.to in ids }.forEach { h -> add(buildJsonObject { put("id", h.id); put("from", h.from); put("to", h.to); put("label", h.label); put("what", h.label.ifEmpty { "work" }) }) }
            })
        }
        if (signedIn && projects.isNotEmpty()) {
            put("projects", buildJsonArray { projects.forEach { p -> add(buildJsonObject { put("id", p.id); put("name", p.name); put("updatedAt", p.updatedAt) }) } })
        }
        if (signedIn && computers.isNotEmpty()) {
            put("computers", buildJsonArray { computers.forEach { (id, name, status) -> add(buildJsonObject { put("id", id); put("name", name); put("status", status) }) } })
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

    /**
     * Sends a message from the panel (or the chief dock's mic, step 27) into the bot's main chat, no navigation.
     * [onResult] gets null once the message is on its way, else why it couldn't be sent.
     */
    fun send(text: String, id: String, onResult: (String?) -> Unit = {}) {
        scope.launch {
            var ready = botId == id && chats[id]?.currentSessionId != null
            if (!ready) ready = select(id)
            val chat = chats[id]
            if (!ready || chat == null) {
                if (botId == id && sendError == null) sendError = "Couldn't send. Try again."
                onResult(sendError?.takeIf { botId == id } ?: "Couldn't send. Try again."); return@launch
            }
            // Let the history land first so it can't replace the new message.
            var waited = 0
            while (waited < 50 && chat.messages.isEmpty() && chat.transientError == null) { delay(100); waited++ }
            if (chat.isStreaming || chat.isCreatingSession || chat.currentSessionId == null) {
                val why = if (chat.isStreaming) "Still answering. Try again in a moment." else "Couldn't send. Try again."
                if (botId == id) sendError = why
                onResult(why); return@launch
            }
            sendError = null
            val (modelId, effort) = OfficeFeature.selectedModel()
            chat.send(ChatSendRequest(text = text, runtimeModelId = modelId, effort = effort))
            onResult(null)
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

/** Where one bot sits in the frame, in frame CSS px (`allternit:workspace:anchors`, iOS WorkspaceLink). */
data class WorkspaceAnchor(val botId: String, val x: Double, val y: Double, val w: Double, val h: Double, val visible: Boolean)

/**
 * The native side of the Workspace frame (v2): the latest bot anchors the frame reported, the pod it last picked, and a
 * focus sender. Names match iOS: focus, anchors, pods.
 */
class WorkspaceLink {
    var anchors by mutableStateOf<List<WorkspaceAnchor>>(emptyList())
        internal set
    /** The pod last tapped in the frame (`action: "pod"`), or null. */
    var selectedPodId by mutableStateOf<String?>(null)
        internal set
    internal var send: (String) -> Unit = {}

    /** Asks the frame to frame a bot or a pod (both null clears). [inset] is what native UI covers, in CSS px. */
    fun focus(botId: String?, podId: String? = null, top: Int = 0, right: Int = 0, bottom: Int = 0, left: Int = 0) {
        send(buildJsonObject {
            put("type", "allternit:workspace:focus")
            put("botId", botId?.let { JsonPrimitive(it) } ?: JsonNull)
            put("podId", podId?.let { JsonPrimitive(it) } ?: JsonNull)
            put("inset", buildJsonObject { put("top", top); put("right", right); put("bottom", bottom); put("left", left) })
        }.toString())
    }

    internal fun parseAnchors(body: JsonObject) {
        anchors = (body["items"] as? JsonArray).orEmpty().mapNotNull { e ->
            val o = e as? JsonObject ?: return@mapNotNull null
            fun n(k: String) = (o[k] as? JsonPrimitive)?.content?.toDoubleOrNull()
            WorkspaceAnchor(
                (o["botId"] as? JsonPrimitive)?.content ?: return@mapNotNull null,
                n("x") ?: return@mapNotNull null, n("y") ?: return@mapNotNull null, n("w") ?: 0.0, n("h") ?: 0.0,
                (o["visible"] as? JsonPrimitive)?.content != "false",
            )
        }.take(80)
    }
}

private fun hostPage(dark: Boolean, full: Boolean): String {
    // The Workspace floor is the default; the full facility is the same frame in its other view.
    val view = if (full) "embed=facility&actions=chat" else "embed=workspace&chrome=none"
    val src = "$FACILITY_ORIGIN/?$view${if (dark) "&theme=dark" else ""}"
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
  if (d.type === 'allternit:facility:ready' && d.version === 1 && !connected) { connected = true; office.postMessage(JSON.stringify({ type: 'ready', v: 1 })); }
  else if (d.type === 'allternit:workspace:ready' && !connected) { connected = true; office.postMessage(JSON.stringify({ type: 'ready', v: d.v === 2 ? 2 : 1, caps: Array.isArray(d.caps) ? d.caps.filter((c) => typeof c === 'string').slice(0, 16) : [] })); }
  else if (d.type === 'allternit:workspace:anchors' && Array.isArray(d.items)) office.postMessage(JSON.stringify({ type: 'anchors', items: d.items.slice(0, 80) }));
  else if (d.type === 'allternit:facility:visitor' && d.version === 1) office.postMessage(JSON.stringify({ type: 'visitor', visitor: d.visitor }));
  else if (d.type === 'allternit:workspace:action') office.postMessage(JSON.stringify({ type: 'action', action: String(d.action || ''), botId: typeof d.botId === 'string' ? d.botId : null, podId: typeof d.pod === 'string' ? d.pod : (d.pod && typeof d.pod.podId === 'string' ? d.pod.podId : null), text: typeof d.text === 'string' ? d.text.slice(0, 4000) : null }));
});
</script></body></html>
""".trimIndent()
}

/**
 * The bots' Office floor (iOS OfficeFloorView): the facility from 3dfacility.allternit.com (`?embed=facility`), fed with
 * the person's real bots. Picking a bot or answering opens its main chat ([openBot]); a message written in the panel is
 * sent in the background and its reply streams into the panel. The 3D scene is the same web facility iOS frames, so
 * there is no 2D fallback to build; a failed or slow load shows the same "didn't load" state with Try again.
 * On the Workspace floor (step 26) with a [kit], tapping a bot opens [OfficeThreadSheet] over the floor instead, the
 * "N need you" pill ([OfficeNeedPill]) opens the first waiting bot with its [OfficeApprovalCard], and an approval adds
 * a done bubble to the feed.
 * Step 27 adds the chief dock ([ChiefDock]: the main bot, hold-to-talk, the Autopilot switch and its [AutopilotSheet],
 * the main bot's suggestion card) at the bottom of the floor, hidden while a thread sheet is open, and the
 * "While you were away" card ([OfficeDigestCard]) when the Office opens after 30 minutes or more away.
 */
@SuppressLint("SetJavaScriptEnabled")
@Composable
fun OfficeFloorScreen(store: BotModeStore, backend: ChatBackend, openBot: (String) -> Unit, link: WorkspaceLink = remember { WorkspaceLink() }, kit: BotsOfficeKit? = null) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val state by AllternitAuth.state.collectAsState()
    val userId = (state as? AuthState.SignedIn)?.account?.userId
    val dark = c.bgPrimary.luminance() < 0.5f
    var ready by remember { mutableStateOf(false) }
    var failed by remember { mutableStateOf(false) }
    var attempt by remember { mutableIntStateOf(0) }
    // Workspace floor by default; "Open the full facility" switches the frame to the facility view.
    var full by rememberSaveable { mutableStateOf(false) }
    var floor by remember { mutableStateOf<FloorState?>(null) }
    var frameV2 by remember(attempt, full) { mutableStateOf(false) }
    val thread = remember(store, backend) { OfficePanelThread(store, backend, context.applicationContext) }
    DisposableEffect(thread) { onDispose { thread.dispose() } }
    val webRef = remember { arrayOfNulls<WebView>(1) }
    var frameReady by remember(attempt, full) { mutableStateOf(false) }
    val visitorKey = "allternit.facility.visitor.${userId ?: "local"}"
    val prefs = remember { context.getSharedPreferences("allternit_office", Context.MODE_PRIVATE) }
    val currentOpenBot by androidx.compose.runtime.rememberUpdatedState(openBot)
    val scope = rememberCoroutineScope()
    // Step 26: the thread sheet's bot (null = closed), the client-side needs, and done bubbles for the feed.
    var sheetBot by rememberSaveable { mutableStateOf<String?>(null) }
    val needs = remember(store, backend) { OfficeNeeds(store, backend) }
    var done by remember { mutableStateOf<List<OfficeDoneBubble>>(emptyList()) }
    val sheetOn = kit != null
    // Step 27: Autopilot, the dock's send line, the digest, and the sheets the dock opens.
    val autopilot = remember(userId) { AutopilotModel() }
    var autopilotOpen by rememberSaveable { mutableStateOf(false) }
    var pickMainOpen by rememberSaveable { mutableStateOf(false) }
    var sendStatus by remember { mutableStateOf<String?>(null) }
    var digest by remember { mutableStateOf<FloorDigest?>(null) }
    // One `POST /floor/seen` per Office visit (kept across rotation, so the digest isn't lost to a second post).
    var seenPosted by rememberSaveable(userId) { mutableStateOf(false) }
    val mainBot = officeMainBot(store, floor)
    LaunchedEffect(autopilot) { if (userId != null && sheetOn) autopilot.load() }
    LaunchedEffect(userId, sheetOn) {
        if (userId == null || !sheetOn || seenPosted) return@LaunchedEffect
        val seen = FloorClient.seen() ?: return@LaunchedEffect
        seenPosted = true
        val previous = seen.previous
        if (previous != null && digestDue(seen)) FloorClient.digest(previous)?.takeIf { !it.isEmpty }?.let { digest = it }
    }
    LaunchedEffect(sendStatus) { if (sendStatus != null) { delay(6_000); sendStatus = null } }

    /** After a card's answer (sheet or dock): drop the need, add the done bubble, then re-read threads, needs and floor. */
    fun needDone(need: OfficeNeed, bubble: String?) {
        needs.markAnswered(need)
        if (bubble != null) {
            val now = System.currentTimeMillis()
            done = (done.filter { now - it.at < 600_000L } + OfficeDoneBubble("done-${need.id}-$now", need.botId, bubble, now)).takeLast(8)
        }
        scope.launch {
            store.refreshThreads()
            needs.refresh()
            if (frameV2 && userId != null) FloorClient.state()?.let { floor = it }
        }
    }

    // Names only for the Commons studios; a failed load just omits the field. Statuses and questions come from the
    // threads, so keep them fresh while the floor is open (iOS: every 15 s).
    LaunchedEffect(store) {
        if (store.projects.isEmpty()) store.refreshProjects()
        while (true) {
            store.refreshThreads()
            if (userId != null) needs.refresh()
            delay(15_000)
        }
    }
    var computers by remember { mutableStateOf<List<Triple<String, String, String>>>(emptyList()) }
    LaunchedEffect(userId) {
        if (userId == null) return@LaunchedEffect
        while (true) {
            (com.allternit.feature.bots.BotsApi.listAllComputers() as? com.allternit.feature.bots.BotsResult.Ok)?.let { computers = workspaceComputers(it.value) }
            delay(15_000)
        }
    }
    // Floor state v2 (pods, needs, results, handoffs), only worth fetching once the frame speaks v2.
    LaunchedEffect(userId, frameV2) {
        if (userId == null || !frameV2) { floor = null; return@LaunchedEffect }
        while (true) {
            floor = FloorClient.state()
            delay(15_000)
        }
    }
    // A cold 3D load on a slow phone can take a while (web: 25 s).
    LaunchedEffect(attempt, full) { delay(25_000); if (!ready) failed = true }
    // Push the roster whenever it changes, once the frame is up.
    LaunchedEffect(frameReady, attempt, full) {
        if (!frameReady) return@LaunchedEffect
        var last: String? = null
        snapshotFlow { workspaceFeed(store.floorEntries(), store.floorProjects(), userId != null, computers, floor?.withClientNeeds(needs), frameV2, done = done) }.collectLatest { feed ->
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
            androidx.compose.runtime.key(attempt, userId, full) {
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
                            link.send = { json -> if (frameReady) evaluateJavascript("window.feed($json)", null) }
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
                                            val v2 = (body["v"] as? JsonPrimitive)?.content == "2"
                                            if (full) {
                                                // Identity state is owned by the app, never the frame's partitioned storage.
                                                val visitor = prefs.getString(visitorKey, null) ?: "null"
                                                evaluateJavascript("window.feed({type:'allternit:facility:init',version:1,visitor:$visitor})", null)
                                            }
                                            frameV2 = v2
                                            frameReady = true
                                            ready = true
                                            // Replay the feed (v2 fields join as soon as the floor state lands, via the watcher).
                                            evaluateJavascript("window.feed(${workspaceFeed(store.floorEntries(), store.floorProjects(), userId != null, computers, floor?.withClientNeeds(needs), v2, done = done)})", null)
                                        }
                                        "anchors" -> link.parseAnchors(body)
                                        "action" -> if (str("action") == "pod") {
                                            link.selectedPodId = str("podId")
                                        } else {
                                            val id = str("botId")
                                            if (id != null && store.bot(id) != null) when (str("action")) {
                                                // The Workspace floor opens the bot's thread sheet over the floor (step 26);
                                                // the full facility opens real native bot work, then returns to the same world.
                                                "select", "answer", "open" -> if (sheetOn && !full) { sheetBot = id } else currentOpenBot(id)
                                                // Written in the panel: sent into the bot's main chat here, the reply streams back.
                                                "message" -> str("text")?.trim()?.takeIf { it.isNotEmpty() }?.let { thread.send(it, id) }
                                            }
                                        }
                                    }
                                }
                            }
                            loadDataWithBaseURL("$HOST_ORIGIN/office-host", hostPage(dark, full), "text/html", "utf-8", null)
                        }
                    },
                    onRelease = { webRef[0] = null; it.removeAllViews(); it.destroy() },
                )
            }
            Box(
                Modifier.align(Alignment.TopEnd).padding(12.dp).clip(RoundedCornerShape(20.dp)).background(c.bgSecondary)
                    .clickable { full = !full; ready = false; link.anchors = emptyList(); link.selectedPodId = null; sheetBot = null }
                    .padding(horizontal = 14.dp, vertical = 8.dp).testTag("officeViewSwitch"),
            ) { Text(if (full) "Back to Workspace" else "Open the full facility", color = c.textPrimary) }
            if (!ready) {
                Box(Modifier.align(Alignment.Center).clip(RoundedCornerShape(12.dp)).background(c.bgSecondary).padding(16.dp)) { LibrarySpinner("Opening the office…") }
            }
            // "N need you": opens the first waiting bot's sheet with its approval card on top.
            if (sheetOn && !full && ready && needs.items.isNotEmpty()) {
                val colorOf = store.floorEntries().associate { it.id to it.colorHex }
                OfficeNeedPill(
                    count = needs.items.size,
                    colors = needs.waitingBotIds().map { officeColor(colorOf[it]) },
                    onClick = { needs.first()?.let { sheetBot = it.botId } },
                    modifier = Modifier.align(Alignment.TopStart).padding(12.dp),
                )
            }
            // The chief dock; it hides while a thread sheet is open so the two never stack.
            if (kit != null && !full && sheetBot == null) {
                val main = mainBot
                val suggestion = main?.let { m -> needs.items.firstOrNull { it is OfficeNeed.Suggestion && it.botId == m.id } as? OfficeNeed.Suggestion }
                ChiefDock(
                    mainBot = main, signedIn = userId != null, store = store, backend = kit.backend, autopilot = autopilot,
                    suggestion = suggestion, sendStatus = sendStatus,
                    onTalk = { text ->
                        if (main != null) {
                            sendStatus = "Sending to ${main.displayName}…"
                            thread.send(text, main.id) { err -> sendStatus = err ?: "Sent to ${main.displayName}. Tap to open the thread." }
                        }
                    },
                    onOpenMain = { main?.let { sheetBot = it.id } },
                    onOpenAutopilot = { autopilotOpen = true },
                    onPickMain = { pickMainOpen = true },
                    onSignIn = { kit.openSignIn() },
                    onSuggestionDone = { n, b -> needDone(n, b) },
                    modifier = Modifier.align(Alignment.BottomCenter),
                )
            }
            // "While you were away": once per visit after 30+ minutes away; a line opens that bot's sheet.
            val away = digest
            if (kit != null && away != null && !full && sheetBot == null) {
                OfficeDigestCard(
                    away, store, mainBot?.displayName,
                    onOpenBot = { id -> digest = null; sheetBot = id },
                    onDismiss = { digest = null },
                    modifier = Modifier.align(Alignment.TopCenter).padding(start = 12.dp, end = 12.dp, top = 58.dp),
                )
            }
        }
    }

    val openId = sheetBot
    if (kit != null && openId != null && !full) {
        if (store.bot(openId) == null) {
            LaunchedEffect(openId) { sheetBot = null; link.focus(null) }
        } else {
            val mainName = mainBot?.displayName
            OfficeThreadSheet(
                botId = openId, store = store, kit = kit, needs = needs, mainBotName = mainName,
                onInset = { bottom -> link.focus(openId, bottom = bottom) },
                onDone = { need, bubble -> needDone(need, bubble) },
                onDismiss = { sheetBot = null; link.focus(null) },
            )
        }
    }

    if (kit != null && autopilotOpen && !full) {
        AutopilotSheet(
            model = autopilot, mainBot = mainBot, bots = store.bots, pods = floor?.pods.orEmpty(),
            onOpenBotSettings = { id -> kit.openBotSettings(id) },
            onDismiss = { autopilotOpen = false },
        )
    }
    if (kit != null && pickMainOpen) {
        kit.MainBotSetup(
            onDismiss = { pickMainOpen = false },
            onDone = {
                pickMainOpen = false
                scope.launch {
                    if (frameV2 && userId != null) FloorClient.state()?.let { floor = it }
                    autopilot.load()
                }
            },
        )
    }
}
