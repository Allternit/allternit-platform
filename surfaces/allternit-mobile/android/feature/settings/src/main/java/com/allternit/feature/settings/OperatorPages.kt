package com.allternit.feature.settings

import android.content.Context
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import java.io.File
import java.util.UUID

/*
 * Settings part 2: the Infrastructure hub, Platform, Runtime operations, Prewarm, Replay, Meetings, Your twin,
 * Files, Self-hosted server and Brain. Every action here calls the same endpoint iOS does.
 */

/** iOS InfrastructureSettingsView: the hub for the cloud and runtime managers. */
@Composable
internal fun InfrastructurePage(back: () -> Unit, push: (String) -> Unit) {
    SPage("Infrastructure", tag = "settingsInfrastructure", onBack = back) {
        SNote("Where your bots run: your computers, cloud servers, and the runtime that schedules them.")
        SLabel("Servers")
        SCard {
            SRow("Cloud instances", "infra_cloudInstances", icon = DesignR.drawable.ph_cpu, onClick = { push("cloud-instances") })
            SDivider()
            SRow("Cloud deployments", "infra_cloudDeploy", icon = DesignR.drawable.ph_globe, onClick = { push("cloud-deploy") })
            SDivider()
            SRow("VPS servers", "infra_vps", icon = DesignR.drawable.ph_desktop, onClick = { push("vps-servers") })
            SDivider()
            SRow("Nodes", "infra_nodes", icon = DesignR.drawable.ph_link_simple, onClick = { push("nodes") })
            SDivider()
            SRow("Computer backups", "infra_backups", icon = DesignR.drawable.ph_floppy_disk, onClick = { push("cloud-backups") })
            SDivider()
            SRow("Enterprise BYOC", "infra_byoc", icon = DesignR.drawable.ph_seal_check, onClick = { push("byoc") })
        }
        SLabel("Runtime")
        SCard {
            SRow("Runtime operations", "infra_runtimeOps", icon = DesignR.drawable.ph_arrows_down_up, onClick = { push("runtime-ops") })
            SDivider()
            SRow("Prewarm", "infra_prewarm", icon = DesignR.drawable.ph_record_fill, onClick = { push("prewarm") })
            SDivider()
            SRow("Replay", "infra_replay", icon = DesignR.drawable.ph_play_fill, onClick = { push("replay") })
        }
        SNote("Security policies, audit logs and access controls are set per organization on the web dashboard.")
    }
}

/** iOS PlatformSettingsView: on-phone choices. Stored on this phone only. */
@Composable
internal fun PlatformPage(back: () -> Unit) {
    val context = LocalContext.current
    fun flag(key: String, default: Boolean) = mutableStateOf(SettingsPrefs.flag(context, key, default))
    var systemMessages by remember { flag("platform.systemMessages", false) }
    var telemetry by remember { flag("platform.telemetry", false) }
    var drafts by remember { mutableStateOf(SettingsPrefs.saveDrafts(context)) }
    var compact by remember { flag("platform.compact", false) }
    var sidebarLabels by remember { flag("platform.sidebarLabels", true) }
    var streaming by remember { flag("platform.streaming", true) }
    var dispatch by remember { flag("platform.dispatch", false) }
    SPage("Platform", tag = "settingsPlatform", onBack = back) {
        SLabel("General")
        SCard {
            SToggleRow("Show system messages", "platform_systemMessages", systemMessages, onChange = { systemMessages = it; SettingsPrefs.setFlag(context, "platform.systemMessages", it) })
            SDivider()
            SToggleRow("Telemetry", "platform_telemetry", telemetry, detail = "Send anonymized usage data to improve Allternit.", onChange = { telemetry = it; SettingsPrefs.setFlag(context, "platform.telemetry", it) })
            SDivider()
            SToggleRow("Auto-save drafts", "platform_autoSave", drafts, onChange = { drafts = it; SettingsPrefs.setSaveDrafts(context, it) })
        }
        SLabel("Appearance")
        SCard {
            SToggleRow("Compact density", "platform_compact", compact, onChange = { compact = it; SettingsPrefs.setFlag(context, "platform.compact", it) })
            SDivider()
            SToggleRow("Show sidebar labels", "platform_sidebarLabels", sidebarLabels, onChange = { sidebarLabels = it; SettingsPrefs.setFlag(context, "platform.sidebarLabels", it) })
        }
        SLabel("Models")
        SCard { SToggleRow("Streaming responses", "platform_streaming", streaming, onChange = { streaming = it; SettingsPrefs.setFlag(context, "platform.streaming", it) }) }
        SNote("Effort only applies to models that support it.")
        SLabel("Dispatch")
        SCard { SToggleRow("Enable dispatch", "platform_dispatch", dispatch, detail = "Route cowork tasks through Allternit's dispatch layer.", onChange = { dispatch = it; SettingsPrefs.setFlag(context, "platform.dispatch", it) }) }
        SLabel("API keys")
        SNote("Provider secrets, local models and tool credentials stay on the runtime that owns them. The cloud stores runtime identity and a pairing.")
        SLabel("Permissions")
        SNote("Android asks for the microphone, camera and notifications in context when a feature needs them. Change them any time in the phone's own Settings.")
    }
}

/** iOS RuntimeOperationsView: the runtime budget dashboard (iOS BudgetDashboardView), then Prewarm and Replay. */
@Composable
internal fun RuntimeOpsPage(back: () -> Unit, push: (String) -> Unit) {
    SPage("Runtime operations", tag = "settingsRuntimeOps", onBack = back) {
        SNote("How many credits your bots may use each hour, and the tools that keep the runtime fast.")
        BudgetDashboard()
        SCard(Modifier.padding(top = 18.dp)) {
            SRow("Prewarm", "runtime_prewarm", onClick = { push("prewarm") })
            SDivider()
            SRow("Replay", "runtime_replay", onClick = { push("replay") })
        }
    }
}

/** iOS PrewarmManagerView: the warm pool's size and a warm-up. */
@Composable
internal fun PrewarmPage(back: () -> Unit) {
    val scope = rememberCoroutineScope()
    val (load, reload) = rememberLoad("prewarm") { Net.get("runtime/prewarm/status").toLoad("load prewarm") }
    var size by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    SPage("Prewarm", tag = "settingsPrewarm", onBack = back) {
        SNote("A warm pool is runtimes kept ready, so a new chat starts at once.")
        LoadView(load, reload, "load prewarm") { body ->
            SCard {
                val facts = body.obj?.facts(emptySet()).orEmpty()
                if (facts.isEmpty()) SRow("Pool", "prewarm_none", value = "Off", chevron = false)
                facts.forEachIndexed { i, (k, v) -> if (i > 0) SDivider(); SRow(k, "prewarm_fact_$i", value = v, chevron = false) }
            }
        }
        SLabel("Pool size")
        SField("Runtimes to keep warm", size, "prewarm_size", { size = it.filter(Char::isDigit) })
        Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            SButton("Set size", "prewarm_setSize", enabled = size.toIntOrNull() != null) {
                scope.launch {
                    error = null
                    val r = Net.send("POST", "runtime/prewarm/pool", buildJsonObject { put("pool_size", size.toInt()) })
                    if (r is ApiResult.Ok) { message = "Pool size set."; reload() } else error = r.friendly("set the pool size")
                }
            }
            SButton("Warm up now", "prewarm_warmup") {
                scope.launch {
                    error = null
                    val r = Net.send("POST", "runtime/prewarm/warmup", buildJsonObject {})
                    if (r is ApiResult.Ok) { message = "Warming up."; reload() } else error = r.friendly("warm up")
                }
            }
        }
        message?.let { SNote(it) }
        SError(error)
    }
}

internal val REPLAY = ResourceSpec(
    title = "Replay", tag = "replay", list = "runtime/replay/sessions", listKeys = listOf("sessions", "manifests", "items"),
    idKeys = listOf("run_id", "id"), titleKeys = listOf("title", "name", "run_id", "id"), detailKeys = listOf("status", "created_at"),
    note = "Recorded runs you can play back to see what a bot did.",
    empty = "No recorded runs yet.",
    actions = listOf(RAction("Replay this run", "replay_execute", "POST", { "runtime/replay/sessions/${LibraryNet.escape(it.str("run_id") ?: it.str("id") ?: "")}/execute" }, confirm = "The run is played again on your runtime.")),
)

/** iOS MeetingsSettingsView: when Gizzi joins, how it replies, the recap email, and the connected calendars. */
@Composable
internal fun MeetingsPage(back: () -> Unit) {
    val scope = rememberCoroutineScope()
    val (load, reload) = rememberLoad("meetingSettings") { Net.cloud("GET", "api/v1/me/meeting-settings").toLoad("load meeting settings") }
    val (cals, calsReload) = rememberLoad("meetingCalendars") { Net.cloud("GET", "api/v1/calendar/connections").toLoad("load your calendars") }
    var error by remember { mutableStateOf<String?>(null) }
    suspend fun update(body: JsonObject) {
        error = null
        val r = Net.cloud("PUT", "api/v1/me/meeting-settings", body)
        if (r is ApiResult.Ok) reload() else error = r.friendly("save")
    }
    SPage("Meetings", tag = "settingsMeetings", onBack = back) {
        SNote("Gizzi can join your calls, take notes and send a recap.")
        LoadView(load, reload, "load meeting settings") { body ->
            val s = body.obj
            val autoJoin = s.str("autoJoin") ?: s.str("auto_join") ?: "all_except_1on1"
            val voice = (s.str("defaultReplyMode") ?: s.str("default_reply_mode")) == "voice"
            val recap = s.bool("recapEmail") ?: s.bool("recap_email") ?: true
            SLabel("Join calls")
            SCard {
                listOf("all" to "Every call", "all_except_1on1" to "All except one-on-ones", "none" to "Only when I ask").forEachIndexed { i, (v, label) ->
                    if (i > 0) SDivider()
                    SRow(label, "meetings_join_$v", chevron = false, onClick = { scope.launch { update(buildJsonObject { put("auto_join", v) }) } }, trailing = { if (autoJoin == v) CheckMark() })
                }
            }
            SLabel("Replies and recap")
            SCard {
                SToggleRow("Speak replies", "meetings_voice", voice, detail = "Gizzi answers out loud instead of in the call chat.", onChange = { scope.launch { update(buildJsonObject { put("default_reply_mode", if (it) "voice" else "chat") }) } })
                SDivider()
                SToggleRow("Recap email", "meetings_recap", recap, onChange = { scope.launch { update(buildJsonObject { put("recap_email", it) }) } })
            }
        }
        SLabel("Calendars")
        LoadView(cals, calsReload, "load your calendars") { body ->
            val rows = body.listUnder("connections", "items")
            if (rows.isEmpty()) SNote("No calendar is connected. Connect Google or Microsoft on the web to let Gizzi see your meetings.") else SCard {
                rows.forEachIndexed { i, c ->
                    if (i > 0) SDivider()
                    SRow(c.str("label") ?: c.str("accountEmail") ?: c.str("account_email") ?: "Calendar", "meetings_cal_$i", detail = c.str("provider"), value = c.str("status"), chevron = false)
                }
            }
        }
        SCard(Modifier.padding(top = 18.dp)) {
            SRow("Disconnect Zoom", "meetings_zoomDisconnect", destructive = true, chevron = false, onClick = {
                scope.launch {
                    val r = Net.cloud("DELETE", "api/v1/zoom/connection")
                    if (r is ApiResult.Ok) calsReload() else error = r.friendly("disconnect Zoom")
                }
            })
        }
        SError(error)
    }
}

private val TWIN_KINDS = listOf("fact" to "Fact", "preference" to "Preference", "schedule_rule" to "Schedule rule", "person" to "Person", "decision" to "Decision")

/** iOS TwinSettingsView: how your bots speak for you, and what they remember about you. Proposed memories wait for your OK. */
@Composable
internal fun TwinPage(back: () -> Unit) {
    val scope = rememberCoroutineScope()
    val (profile, profileReload) = rememberLoad("twinProfile") { Net.get("twin/profile").toLoad("load your twin") }
    val (memory, memoryReload) = rememberLoad("twinMemory") { Net.get("twin/memory").toLoad("load memories") }
    var adding by remember { mutableStateOf(false) }
    var kind by remember { mutableStateOf("fact") }
    var subject by remember { mutableStateOf("") }
    var content by remember { mutableStateOf("") }
    var displayName by remember { mutableStateOf<String?>(null) }
    var style by remember { mutableStateOf<String?>(null) }
    var signature by remember { mutableStateOf<String?>(null) }
    var disclosure by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var saved by remember { mutableStateOf(false) }
    SPage("Your twin", tag = "settingsTwin", onBack = back) {
        SNote("Your twin is how your bots speak for you. It only uses what you approve.")
        LoadView(profile, profileReload, "load your twin") { body ->
            val p = body.obj?.get("profile")?.obj ?: body.obj
            SField("Name", displayName ?: p.str("displayName") ?: p.str("display_name") ?: "", "twin_name", { displayName = it; saved = false })
            SField("Speaking style", style ?: p.str("speakingStyle") ?: p.str("speaking_style") ?: "", "twin_style", { style = it; saved = false })
            SField("Signature", signature ?: p.str("signature") ?: "", "twin_signature", { signature = it; saved = false })
            SField("Tell people it's a bot", disclosure ?: p.str("ownerDisclosure") ?: p.str("owner_disclosure") ?: "", "twin_disclosure", { disclosure = it; saved = false }, singleLine = false)
            Row(Modifier.padding(top = 8.dp)) {
                SButton(if (saved) "Saved" else "Save", "twin_save") {
                    scope.launch {
                        error = null
                        val r = Net.send("PUT", "twin/profile", buildJsonObject {
                            put("displayName", displayName ?: p.str("displayName") ?: ""); put("speakingStyle", style ?: p.str("speakingStyle") ?: "")
                            put("signature", signature ?: p.str("signature") ?: ""); put("ownerDisclosure", disclosure ?: p.str("ownerDisclosure") ?: "")
                        })
                        if (r is ApiResult.Ok) saved = true else error = r.friendly("save your twin")
                    }
                }
            }
        }
        SLabel("Memories")
        LoadView(memory, memoryReload, "load memories") { body ->
            val rows = body.listUnder("memory", "memories", "items")
            if (rows.isEmpty()) SNote("Nothing remembered yet.") else SCard {
                rows.forEachIndexed { i, m ->
                    val id = m.str("id") ?: return@forEachIndexed
                    val proposed = m.str("status") == "proposed"
                    if (i > 0) SDivider()
                    SRow(m.str("subject") ?: "Memory", "twin_memory_$id", detail = m.str("content"), value = if (proposed) "Waiting" else null, chevron = false, onClick = {
                        if (proposed) scope.launch {
                            val r = Net.send("POST", "twin/memory/${LibraryNet.escape(id)}/accept")
                            if (r is ApiResult.Ok) memoryReload() else error = r.friendly("accept it")
                        }
                    }, trailing = {
                        SButton(if (proposed) "Accept" else "Forget", "twin_memory_${id}_action", destructive = !proposed) {
                            scope.launch {
                                val r = if (proposed) Net.send("POST", "twin/memory/${LibraryNet.escape(id)}/accept") else Net.send("DELETE", "twin/memory/${LibraryNet.escape(id)}")
                                if (r is ApiResult.Ok) memoryReload() else error = r.friendly("change it")
                            }
                        }
                    })
                }
            }
        }
        if (adding) {
            SLabel("Add a memory")
            SCard {
                TWIN_KINDS.forEachIndexed { i, (v, label) ->
                    if (i > 0) SDivider()
                    SRow(label, "twin_kind_$v", chevron = false, onClick = { kind = v }, trailing = { if (kind == v) CheckMark() })
                }
            }
            SField("About", subject, "twin_subject", { subject = it })
            SField("What to remember", content, "twin_content", { content = it }, singleLine = false)
            Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                SButton("Add", "twin_add", enabled = subject.isNotBlank() && content.isNotBlank()) {
                    scope.launch {
                        val r = Net.send("POST", "twin/memory", buildJsonObject { put("kind", kind); put("subject", subject.trim()); put("content", content.trim()); put("visibility", "private") })
                        if (r is ApiResult.Ok) { adding = false; subject = ""; content = ""; memoryReload() } else error = r.friendly("add it")
                    }
                }
                SButton("Cancel", "twin_addCancel") { adding = false }
            }
        } else SCard(Modifier.padding(top = 18.dp)) { SRow("Add a memory", "twin_new", chevron = false, onClick = { adding = true }) }
        SError(error)
    }
}

/** iOS FilesDashboardView and FilesKeysView: Files state and the key devices. */
@Composable
internal fun FilesPage(back: () -> Unit, push: (String) -> Unit) {
    val context = LocalContext.current
    var enabled by remember { mutableStateOf(SettingsPrefs.flag(context, "files.enabled", false)) }
    SPage("Files", tag = "settingsFiles", onBack = back) {
        SNote("Files keeps your files in sync between your devices and computers, encrypted with a key only your devices hold.")
        SCard {
            SToggleRow("Files on this phone", "files_enabled", enabled, onChange = { enabled = it; SettingsPrefs.setFlag(context, "files.enabled", it) })
            SDivider()
            SRow("This phone", "files_device", value = android.os.Build.MODEL ?: "Android", chevron = false)
        }
        if (!enabled) SNote("Files is off. Turn it on to see your key, shares and guests.") else {
            SCard(Modifier.padding(top = 18.dp)) { SRow("Files key", "files_keysRow", detail = "The key that opens your files on each device.", onClick = { push("files-keys") }) }
        }
    }
}

@Composable
internal fun FilesKeysPage(back: () -> Unit) {
    val (load, reload) = rememberLoad("filesKeys") { Net.cloud("GET", "api/v1/files-keys").toLoad("load your Files key") }
    SPage("Files key", tag = "settingsFilesKeys", onBack = back) {
        SNote("Your Files key never leaves your devices unlocked. A new device asks one you already trust.")
        LoadView(load, reload, "load your Files key") { body ->
            val o = body.obj
            val key = o?.get("key")?.obj ?: o
            if (key == null || key.isEmpty()) StateBox("No Files key yet", "Set up Files on iPhone or on a computer first. This phone then asks for access from there.", null, null) else {
                SCard {
                    key.facts(emptySet()).forEachIndexed { i, (k, v) -> if (i > 0) SDivider(); SRow(k, "filesKeys_fact_$i", value = v, chevron = false) }
                }
                val devices = (key["devices"] ?: o?.get("devices")).listUnder("devices", "items")
                if (devices.isNotEmpty()) {
                    SLabel("Devices")
                    SCard {
                        devices.forEachIndexed { i, d ->
                            if (i > 0) SDivider()
                            SRow(d.str("name") ?: d.str("device_name") ?: d.str("id") ?: "Device", "filesKeys_device_$i", detail = d.str("added_at") ?: d.str("created_at"), chevron = false)
                        }
                    }
                }
            }
        }
        SNote("Approving, declining and replacing keys needs the Files encryption engine, which isn't on Android yet. Do those on iPhone or a computer.")
    }
}

/** The self-hosted server this phone talks to (iOS SelfHostedServerStore). The key stays in this app's private storage. */
internal object SelfHosted {
    private const val FILE = "allternit_selfhosted"
    private val client = OkHttpClient()
    private val json = Json { ignoreUnknownKeys = true }

    fun url(context: Context): String = context.getSharedPreferences(FILE, Context.MODE_PRIVATE).getString("url", "") ?: ""
    fun key(context: Context): String = context.getSharedPreferences(FILE, Context.MODE_PRIVATE).getString("key", "") ?: ""
    fun save(context: Context, url: String, key: String) = context.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit().putString("url", url).putString("key", key).apply()
    fun clear(context: Context) = context.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit().clear().apply()

    /** https only, no trailing slash (iOS SelfHostedClient.normalize). */
    fun normalize(input: String): String? {
        val t = input.trim().trimEnd('/')
        return if (t.startsWith("https://") && t.length > 8) t else null
    }

    suspend fun call(base: String, key: String?, method: String, path: String, body: JsonObject? = null, idempotency: String? = null): Load<JsonElement> = withContext(Dispatchers.IO) {
        try {
            val b = Request.Builder().url(base + path)
            if (key != null) b.header("Authorization", "Bearer $key")
            if (idempotency != null) b.header("Idempotency-Key", idempotency)
            b.method(method, if (method == "GET") null else (body?.toString() ?: "{}").toRequestBody("application/json".toMediaType()))
            client.newCall(b.build()).execute().use { r ->
                val text = r.body?.string().orEmpty()
                when {
                    r.code == 401 || r.code == 403 -> Load.Failed("The server didn't accept this key.")
                    !r.isSuccessful -> Load.Failed("The server answered ${r.code}.")
                    else -> Load.Ready(if (text.isBlank()) JsonObject(emptyMap()) else json.parseToJsonElement(text))
                }
            }
        } catch (e: Exception) {
            Load.Failed("Couldn't reach the server. Check the address and your connection.")
        }
    }
}

/** iOS SelfHostedSettingsView: connect a server by https address and key, see its agents and environments, open a session. */
@Composable
internal fun SelfHostedPage(back: () -> Unit, push: (String) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var url by remember { mutableStateOf(SelfHosted.url(context)) }
    var key by remember { mutableStateOf(SelfHosted.key(context)) }
    var connected by remember { mutableStateOf(SelfHosted.url(context).isNotBlank()) }
    var info by remember { mutableStateOf<Load<JsonElement>>(Load.Loading) }
    var error by remember { mutableStateOf<String?>(null) }
    var nonce by remember { mutableStateOf(0) }
    androidx.compose.runtime.LaunchedEffect(connected, nonce) {
        if (connected) info = SelfHosted.call(SelfHosted.url(context), null, "GET", "/v1/server/info")
    }
    SPage("Self-hosted server", tag = "settingsSelfHosted", onBack = back) {
        SNote("Run your own Allternit server and use it from this phone. Only https addresses work.")
        if (!connected) {
            SField("Server address", url, "selfHosted_url", { url = it })
            SField("Access key", key, "selfHosted_key", { key = it })
            Row(Modifier.padding(top = 8.dp)) {
                SButton("Connect", "selfHosted_connect", enabled = url.isNotBlank() && key.isNotBlank()) {
                    val base = SelfHosted.normalize(url)
                    if (base == null) { error = "Use an https:// address."; return@SButton }
                    scope.launch {
                        error = null
                        val r = SelfHosted.call(base, key.trim(), "GET", "/v1/agents")
                        if (r is Load.Ready) { SelfHosted.save(context, base, key.trim()); url = base; connected = true } else error = (r as? Load.Failed)?.message
                    }
                }
            }
            SError(error)
        } else {
            LoadView(info, { nonce += 1 }, "reach the server") { body ->
                SCard {
                    SRow("Address", "selfHosted_address", value = SelfHosted.url(context), chevron = false)
                    body.obj?.facts(emptySet()).orEmpty().forEachIndexed { i, (k, v) -> SDivider(); SRow(k, "selfHosted_fact_$i", value = v, chevron = false) }
                }
            }
            SCard(Modifier.padding(top = 18.dp)) {
                SRow("New session", "selfHosted_session", onClick = { push("self-hosted-session") })
                SDivider()
                SRow("Disconnect", "selfHosted_disconnect", destructive = true, chevron = false, onClick = { SelfHosted.clear(context); connected = false; key = ""; })
            }
        }
    }
}

/** iOS SelfHostedSessionView: pick an agent and environment, open a session and talk to it. */
@Composable
internal fun SelfHostedSessionPage(back: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val base = remember { SelfHosted.url(context) }
    val key = remember { SelfHosted.key(context) }
    val (agents, agentsReload) = rememberLoad("shAgents") { SelfHosted.call(base, key, "GET", "/v1/agents") }
    val (envs, _) = rememberLoad("shEnvs") { SelfHosted.call(base, key, "GET", "/v1/environments") }
    var agent by remember { mutableStateOf<String?>(null) }
    var env by remember { mutableStateOf<String?>(null) }
    var sessionId by remember { mutableStateOf<String?>(null) }
    var draft by remember { mutableStateOf("") }
    var sent by remember { mutableStateOf(listOf<String>()) }
    var error by remember { mutableStateOf<String?>(null) }
    SPage("Session", tag = "settingsSelfHostedSession", onBack = back) {
        if (base.isBlank()) { StateBox("No server connected", "Connect a server first.", null, null); return@SPage }
        if (sessionId == null) {
            SLabel("Agent")
            LoadView(agents, agentsReload, "load agents") { body ->
                val rows = body.listUnder("agents", "data", "items")
                if (rows.isEmpty()) SNote("This server has no agents.") else SCard {
                    rows.forEachIndexed { i, a ->
                        val id = a.str("id") ?: return@forEachIndexed
                        if (i > 0) SDivider()
                        SRow(a.str("name") ?: id, "selfHosted_agent_$id", chevron = false, onClick = { agent = id }, trailing = { if (agent == id) CheckMark() })
                    }
                }
            }
            SLabel("Environment")
            LoadView(envs, {}, "load environments") { body ->
                val rows = body.listUnder("environments", "data", "items")
                if (rows.isEmpty()) SNote("This server has no environments.") else SCard {
                    rows.forEachIndexed { i, e ->
                        val id = e.str("id") ?: return@forEachIndexed
                        if (i > 0) SDivider()
                        SRow(e.str("name") ?: id, "selfHosted_env_$id", chevron = false, onClick = { env = id }, trailing = { if (env == id) CheckMark() })
                    }
                }
            }
            Row(Modifier.padding(top = 12.dp)) {
                SButton("Start session", "selfHosted_start", enabled = agent != null && env != null) {
                    scope.launch {
                        error = null
                        val r = SelfHosted.call(base, key, "POST", "/v1/sessions", buildJsonObject { put("agent_id", agent ?: ""); put("environment_id", env ?: "") }, UUID.randomUUID().toString())
                        if (r is Load.Ready) sessionId = r.value.obj.str("id") else error = (r as? Load.Failed)?.message
                    }
                }
            }
        } else {
            val sid = sessionId ?: ""
            sent.forEach { SNote("You: $it") }
            SField("Message", draft, "selfHosted_message", { draft = it }, singleLine = false)
            Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                SButton("Send", "selfHosted_send", enabled = draft.isNotBlank()) {
                    val text = draft.trim()
                    scope.launch {
                        error = null
                        val r = SelfHosted.call(base, key, "POST", "/v1/sessions/${LibraryNet.escape(sid)}/messages", buildJsonObject { put("content", text) }, UUID.randomUUID().toString())
                        if (r is Load.Ready) { sent = sent + text; draft = "" } else error = (r as? Load.Failed)?.message
                    }
                }
                SButton("Stop", "selfHosted_interrupt") {
                    scope.launch { SelfHosted.call(base, key, "POST", "/v1/sessions/${LibraryNet.escape(sid)}/interrupt", null) }
                }
            }
            SNote("Replies stream in the Allternit app on the server's own screen. The live stream view isn't on Android yet.")
        }
        SError(error)
    }
}

/** iOS Brain: notes captured on this phone go to a folder first, so capture works offline. Pushing to the brain's git remote is the next step. */
internal object BrainStore {
    fun dir(context: Context) = File(context.filesDir, "brain/inbox")

    fun pending(context: Context): Int = dir(context).listFiles()?.size ?: 0

    fun capture(context: Context, text: String, type: String): String {
        val trimmed = text.trim()
        require(trimmed.isNotEmpty()) { "Write something first." }
        val title = trimmed.lines().first().trim().take(80).ifBlank { "Note" }
        val slug = title.lowercase().replace(Regex("[^a-z0-9]+"), "-").trim('-').take(48).ifBlank { "note" }
        val stamp = java.text.SimpleDateFormat("yyyyMMdd-HHmmss", java.util.Locale.US).format(java.util.Date())
        val file = File(dir(context), "$stamp-$slug.md")
        file.parentFile?.mkdirs()
        file.writeText("---\ntitle: ${title.replace("\n", " ")}\ntype: $type\ncaptured: ${java.time.Instant.now()}\n---\n\n$trimmed\n")
        return file.name
    }
}

/** iOS BrainCaptureSheet: pick Idea or Pain, write the note, save. */
@Composable
internal fun BrainCapturePage(back: () -> Unit) {
    val context = LocalContext.current
    var type by remember { mutableStateOf("idea") }
    var text by remember { mutableStateOf("") }
    var message by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    SPage("Capture to brain", tag = "brainCapture", onBack = back) {
        SCard {
            listOf("idea" to "Idea", "pain" to "Pain").forEachIndexed { i, (v, label) ->
                if (i > 0) SDivider()
                SRow(label, "brainCapture_type_$v", chevron = false, onClick = { type = v }, trailing = { if (type == v) CheckMark() })
            }
        }
        SField("An idea, a pain point, a note to future you", text, "brainCapture_text", { text = it; message = null }, singleLine = false)
        Row(Modifier.padding(top = 8.dp)) {
            SButton("Save to brain", "brainCapture_save", enabled = text.isNotBlank()) {
                try {
                    BrainStore.capture(context, text, type)
                    text = ""; error = null; message = "Saved on this phone."
                } catch (e: Exception) { error = e.message ?: "Couldn't save." }
            }
        }
        message?.let { SNote(it) }
        SError(error)
    }
}

/** iOS BrainSpikeView: what is waiting on this phone to be pushed, and the capture sheet. */
@Composable
internal fun BrainPage(back: () -> Unit, push: (String) -> Unit) {
    val context = LocalContext.current
    SPage("Brain", tag = "settingsBrain", onBack = back) {
        SNote("Notes you capture are saved on this phone first, so capture works with no signal.")
        SCard {
            SRow("Capture to brain", "brain_capture", onClick = { push("brain-capture") })
            SDivider()
            SRow("Waiting on this phone", "brain_pending", value = "${BrainStore.pending(context)} notes", chevron = false)
        }
        SNote("Pushing notes to the brain's git remote runs from iPhone or a computer for now.")
    }
}
