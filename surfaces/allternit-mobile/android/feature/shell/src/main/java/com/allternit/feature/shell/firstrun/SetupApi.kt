package com.allternit.feature.shell.firstrun

import com.allternit.core.api.ApiConfig
import com.allternit.core.api.ApiJson
import com.allternit.feature.shell.ShellApi
import java.io.IOException
import java.time.Instant
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.put
import okhttp3.HttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody

/*
 * The calls behind the first-run setup (iOS Setup/SetupAPI.swift and Core/API/ConnectionsClient.swift):
 *
 *   Setup config (runtime, under /api, not /api/v1): GET/POST /api/onboarding/config
 *   Allternit Cloud catalog:  GET {cloud}/v1/models (public)
 *   Connect kit:              GET/POST {cloud}/api/v1/connections[/:id/connect|submit|login]?caps=runner
 *   Checklist:                GET setup/checks
 *   Connected view:           providers/auth/status, provisioned-instances, agents, gateway/provider-accounts,
 *                             memory/drive/mounts, bot-projects, mcp/connectors, subscriptions/gateway/v1/accounts,
 *                             agent-preferences
 *
 * `/api/v1` calls go to allternit-api with the Clerk bearer (cloud-api answers its own routes and
 * relays the rest to the person's computer, like iOS APIClient). Connections are session-auth
 * only, so they go straight to cloud-api, never through a runtime relay.
 */

/** A refused or failed setup call, already in plain words (iOS ConnectError). */
internal class SetupError(val status: Int?, val code: String?, override val message: String, val body: JsonObject? = null) :
    Exception(message)

internal fun JsonElement?.obj(): JsonObject? = this as? JsonObject
internal fun JsonElement?.arr(): List<JsonElement> = (this as? JsonArray).orEmpty()
internal fun JsonElement?.str(): String? = (this as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull
internal fun JsonElement?.bool(): Boolean? = (this as? JsonPrimitive)?.booleanOrNull
internal fun JsonElement?.num(): Double? = (this as? JsonPrimitive)?.doubleOrNull
internal operator fun JsonElement?.get(key: String): JsonElement? = (this as? JsonObject)?.get(key)

internal object SetupHttp {
    private val JSON = "application/json; charset=utf-8".toMediaType()
    private val config = ApiConfig()

    /** `/api/v1/<path>`. */
    suspend fun v1(method: String, path: String, body: JsonElement? = null, query: Map<String, String> = emptyMap()): JsonElement =
        send(method, config.apiBaseUrl.newBuilder().addPathSegments(path).apply { query.forEach { (k, v) -> addQueryParameter(k, v) } }.build(), body)

    /** `/api/<path>` on the runtime (the onboarding router is not under /api/v1; iOS drops a trailing v1). */
    suspend fun api(method: String, path: String, body: JsonElement? = null): JsonElement {
        val segments = config.apiBaseUrl.pathSegments.filter { it.isNotEmpty() }
        val root = if (segments.lastOrNull() == "v1") segments.dropLast(1) else segments
        val url = config.apiBaseUrl.newBuilder().encodedPath("/")
            .apply { root.forEach { addPathSegment(it) } }
            .addPathSegments(path).build()
        return send(method, url, body)
    }

    /** Straight to cloud-api (`{cloud}/<path>`). */
    suspend fun cloud(method: String, path: String, body: JsonElement? = null, query: Map<String, String> = emptyMap()): JsonElement {
        val url = config.cloudBaseUrl.newBuilder().addPathSegments(path)
            .apply { query.forEach { (k, v) -> addQueryParameter(k, v) } }.build()
        return send(method, url, body)
    }

    val cloudBase: HttpUrl get() = config.cloudBaseUrl

    /**
     * A REST call on the person's computer through cloud-api's runtime relay (iOS
     * RuntimeRelayClient): the request goes as a `{ method, path, headers, body, bodyEncoding }`
     * envelope to `{cloud}/api/v1/runtime-devices/:id/proxy`, and the relay answers with the
     * computer's own response.
     */
    suspend fun relay(runtimeId: String, method: String, path: String, body: JsonElement? = null): JsonElement {
        val envelope = buildJsonObject {
            put("method", method)
            put("path", path)
            put("headers", buildJsonObject {
                put("accept", "application/json")
                put("content-type", "application/json")
            })
            if (body != null) put("body", body.toString())
            put("bodyEncoding", "utf8")
        }
        val url = config.cloudBaseUrl.newBuilder().addPathSegments("api/v1/runtime-devices").addPathSegment(runtimeId).addPathSegment("proxy").build()
        return send("POST", url, envelope)
    }

    suspend fun send(method: String, url: HttpUrl, body: JsonElement?, authed: Boolean = true): JsonElement =
        withContext(Dispatchers.IO) {
            val payload = when {
                body != null -> body.toString().toRequestBody(JSON)
                method == "POST" || method == "PUT" -> "{}".toRequestBody(JSON)
                else -> null
            }
            val request = Request.Builder().url(url).header("Accept", "application/json").method(method, payload).build()
            // The shared client never times out a read (SSE); setup calls give up after 20 s, like iOS's request timeouts.
            val client = (if (authed) ShellApi.okHttp else publicHttp).newBuilder().callTimeout(20, TimeUnit.SECONDS).build()
            val response = try {
                client.newCall(request).execute()
            } catch (e: IOException) {
                throw SetupError(null, "network", "No connection. Try again.")
            }
            response.use { r ->
                val text = r.body?.string().orEmpty()
                val json = runCatching { ApiJson.default.parseToJsonElement(text) }.getOrDefault(JsonNull)
                if (!r.isSuccessful) {
                    val err = json["error"]
                    val code = err.str() ?: err["code"].str()
                    val message = json["message"].str() ?: err["message"].str() ?: code
                        ?: if (r.code == 401 || r.code == 403) "Sign in again, then try again." else "Something went wrong. Try again."
                    throw SetupError(r.code, code, message, json.obj())
                }
                json
            }
        }

    private val publicHttp by lazy { okhttp3.OkHttpClient.Builder().build() }
}

// Rows

internal data class SetupModelRow(val id: String, val name: String)

internal data class SetupProviderAuth(val id: String, val authenticated: Boolean)

internal data class SetupCloudComputer(val id: String, val status: String, val cpuCores: Int?, val memoryMb: Int?) {
    val specs: String?
        get() = if (cpuCores != null && memoryMb != null && cpuCores > 0 && memoryMb > 0)
            "$cpuCores vCPU · ${Math.round(memoryMb / 1024.0)} GB" else null
    val isRunning: Boolean get() = Regex("running|active|ready|online", RegexOption.IGNORE_CASE).containsMatchIn(status)
}

internal enum class SetupCheckStatus(val raw: String, val label: String) {
    Ready("ready", "Ready"), NotChecked("not_checked", "Not checked"), NeedsSetup("needs_setup", "Needs setup"),
}

internal data class SetupCheckRow(
    val id: String, val title: String, val required: Boolean, val status: SetupCheckStatus,
    val detail: String, val fixLabel: String?, val fixSection: String?,
)

internal data class SetupNamed(val id: String, val name: String, val connected: Boolean, val detail: String? = null, val isMain: Boolean = false)

internal object SetupApi {
    /** `user.onboardingComplete` from the server's setup config (authoritative). */
    suspend fun onboardingComplete(): Boolean =
        SetupHttp.api("GET", "onboarding/config")["user"]["onboardingComplete"].bool() == true

    /** Persists the default model and the wizard-completion flag (web completeFirstRun). */
    suspend fun saveConfig(defaultModel: String?, mode: String, appVersion: String) {
        val body = buildJsonObject {
            put("onboardingComplete", true)
            put("wizard", buildJsonObject {
                put("lastRunAt", Instant.now().toString())
                put("lastRunVersion", appVersion)
                put("lastRunCommand", "onboard")
                put("lastRunMode", mode)
            })
            if (defaultModel != null) put("defaultModel", defaultModel)
        }
        SetupHttp.api("POST", "onboarding/config", body)
    }

    /** The live Allternit Cloud catalog (`{cloud}/v1/models`, public, OpenAI-style `data`). */
    suspend fun cloudModelCatalog(): List<SetupModelRow> = runCatching {
        val url = SetupHttp.cloudBase.newBuilder().addPathSegments("v1/models").build()
        SetupHttp.send("GET", url, null, authed = false)["data"].arr().mapNotNull { item ->
            val id = item["id"].str()?.trim()?.takeIf { it.isNotEmpty() } ?: return@mapNotNull null
            if (item["hidden"].bool() == true) return@mapNotNull null
            val name = item["name"].str() ?: item["extra"]["name"].str() ?: id
            SetupModelRow(id, name.ifEmpty { id })
        }
    }.getOrDefault(emptyList())

    suspend fun providerAuthStatus(): List<SetupProviderAuth> =
        SetupHttp.v1("GET", "providers/auth/status")["providers"].arr().mapNotNull { row ->
            val id = row["provider_id"].str() ?: return@mapNotNull null
            SetupProviderAuth(id, row["authenticated"].bool() == true)
        }

    suspend fun cloudComputers(): List<SetupCloudComputer> {
        val json = SetupHttp.v1("GET", "provisioned-instances")
        val list = if (json is JsonObject) json["instances"].arr() else json.arr()
        return list.mapNotNull { c ->
            val id = c["id"].str() ?: return@mapNotNull null
            val status = c["status"].str() ?: "unknown"
            if (status == "deleted") return@mapNotNull null
            SetupCloudComputer(id, status, c["cpuCores"].num()?.toInt(), c["memoryMb"].num()?.toInt())
        }
    }

    /** The account's bots (iOS MeetAgentsAPI.roster): agents with a bot profile. */
    suspend fun roster(): List<SetupNamed> {
        val json = SetupHttp.v1("GET", "agents")
        val rows = if (json is JsonObject) json["agents"].arr() else json.arr()
        return rows.mapNotNull { row ->
            val id = row["id"].str() ?: return@mapNotNull null
            val config = row["config"].obj()
            val profile = row["bot_profile"].obj() ?: config["botProfile"].obj()
            val isBot = profile != null || row["is_bot"].bool() == true || config["isBot"].bool() == true
            if (!isBot) return@mapNotNull null
            val display = profile["displayName"].str()?.takeIf { it.isNotBlank() } ?: row["name"].str() ?: id
            val main = profile["chiefOfStaff"].bool() == true || config["botProfile"]["chiefOfStaff"].bool() == true
            SetupNamed(id, display, connected = true, isMain = main)
        }
    }

    suspend fun channelAccounts(): List<JsonElement> = SetupHttp.v1("GET", "gateway/provider-accounts")["accounts"].arr()

    suspend fun memoryMounts(): List<SetupNamed> = SetupHttp.v1("GET", "memory/drive/mounts")["mounts"].arr().mapNotNull { m ->
        val ref = m["ref"].str() ?: return@mapNotNull null
        SetupNamed(ref, m["name"].str() ?: ref, connected = m["revision"] != null && m["revision"] !is JsonNull)
    }

    suspend fun botProjects(): List<SetupNamed> = SetupHttp.v1("GET", "bot-projects")["projects"].arr().mapNotNull { p ->
        val id = p["id"].str() ?: return@mapNotNull null
        SetupNamed(id, p["title"].str() ?: id, connected = true)
    }

    suspend fun mcpConnectors(): List<SetupNamed> = SetupHttp.v1("GET", "mcp/connectors")["connectors"].arr().mapNotNull { c ->
        val id = c["id"].str() ?: return@mapNotNull null
        val enabled = c["enabled"].bool() != false
        val reauth = c["needs_reauth"].bool() == true
        SetupNamed(id, c["name"].str() ?: id, connected = enabled && !reauth)
    }

    /** Subscription Fabric logins on the Sessions computer: provider + ready. */
    suspend fun fabricAccounts(): List<Pair<String, Boolean>> =
        SetupHttp.v1("GET", "subscriptions/gateway/v1/accounts").arr().mapNotNull { a ->
            val provider = a["provider"].str() ?: return@mapNotNull null
            val health = a["session_health"].str() ?: "unknown"
            provider to (a["enabled"].bool() == true && (health == "ready" || health == "degraded"))
        }

    private val checkOrder = listOf("account", "brain", "google", "accounts", "notifications", "computers", "channels")

    /** Server rows, well-formed ones only (the device's notifications row is added by the caller). */
    suspend fun setupChecks(): List<SetupCheckRow> = SetupHttp.v1("GET", "setup/checks")["rows"].arr().mapNotNull { r ->
        val id = r["id"].str() ?: return@mapNotNull null
        val title = r["title"].str() ?: return@mapNotNull null
        if (id == "notifications") return@mapNotNull null
        val label = r["fix"]["label"].str()
        val section = r["fix"]["section"].str()
        val both = label != null && section != null
        SetupCheckRow(
            id, title, r["required"].bool() == true,
            SetupCheckStatus.entries.firstOrNull { it.raw == r["status"].str() } ?: SetupCheckStatus.NotChecked,
            r["detail"].str() ?: "", if (both) label else null, if (both) section else null,
        )
    }

    fun sortChecks(rows: List<SetupCheckRow>): List<SetupCheckRow> {
        fun rank(id: String) = checkOrder.indexOf(id).let { if (it < 0) checkOrder.size else it }
        return rows.withIndex().sortedWith(compareBy({ rank(it.value.id) }, { it.index })).map { it.value }
    }

    /** The name the person gave us (Settings → Profile), as typed. */
    suspend fun preferredName(): String? = runCatching {
        val json = SetupHttp.v1("GET", "agent-preferences")
        val prefs = json["preferences"] ?: json
        prefs["preferred_name"].str()?.takeIf { it.isNotBlank() }
            ?: prefs["full_name"].str()?.split(" ")?.firstOrNull()
    }.getOrNull()
}

// Connect kit (CONTRACT-connections-2026-11.md)

/** The only four status words. */
internal enum class ConnState { Off, Connecting, On, Needs;
    companion object {
        fun of(raw: String?) = when (raw) { "on" -> On; "needs" -> Needs; "connecting" -> Connecting; else -> Off }
    }
}

/** One row of `GET /api/v1/connections`; everything but `id` is optional. */
internal data class ConnectionWire(
    val id: String,
    val state: ConnState = ConnState.Off,
    val found: String? = null,
    val reason: String? = null,
    val reasonCode: String? = null,
    val runsOn: String? = null,
    val available: Boolean = true,
    val unavailableReason: String? = null,
    val fallbacks: List<String> = emptyList(),
    val source: String? = null,
    val personOnly: Boolean = false,
) {
    val runtimeId: String? get() = source?.takeIf { it.startsWith("runtime:") }?.removePrefix("runtime:")

    companion object {
        fun parse(v: JsonElement): ConnectionWire? {
            val id = v["id"].str() ?: return null
            return ConnectionWire(
                id = id, state = ConnState.of(v["state"].str()), found = v["found"].str(), reason = v["reason"].str(),
                reasonCode = v["reason_code"].str(), runsOn = v["runs_on"].str(), available = v["available"].bool() ?: true,
                unavailableReason = v["unavailable_reason"].str(), fallbacks = v["fallbacks"].arr().mapNotNull { it.str() },
                source = v["source"].str(), personOnly = v["person_only"].bool() == true,
            )
        }
    }
}

/** The disclosure text the server sends with `409 disclosure_required` (shown exactly as given). */
internal data class ConnDisclosure(val version: String, val title: String, val body: List<String>, val acceptLabel: String, val declineLabel: String)

/** `{login}` from `/connect`: what the sheet shows next. */
internal data class ConnLogin(val kind: String, val code: String?, val url: String?, val urlWithCode: String?, val expiresAt: String?, val poll: Boolean) {
    val openUrl: String? get() = urlWithCode ?: url
    val expiry: Instant? get() = expiresAt?.let { runCatching { Instant.parse(it) }.getOrNull() }
}

internal class DisclosureRequired(val disclosure: ConnDisclosure) : Exception("Read and accept the note first.")

internal object ConnectionsClient {
    /** Every cloud call says `caps=runner`: this app knows the disclosure step (contract §9.1). */
    private val caps = mapOf("caps" to "runner")

    /**
     * [target] is the computer a row lives on (its runtime id), or null for cloud-api. Computer
     * rows (CLI sign-ins, the local brain) are read and connected on that computer, like iOS.
     */
    private suspend fun send(rest: String, target: String?, method: String = "GET", body: JsonElement? = null): JsonElement = try {
        val path = "api/v1/connections" + if (rest.isEmpty()) "" else "/$rest"
        if (target != null) SetupHttp.relay(target, method, "/$path", body) else SetupHttp.cloud(method, path, body, caps)
    } catch (e: SetupError) {
        val inner = e.body["error"]
        if (inner["code"].str() == "disclosure_required") {
            val d = inner["disclosure"]
            val version = d["version"].str()
            if (version != null) {
                throw DisclosureRequired(ConnDisclosure(
                    version, d["title"].str() ?: "", d["body"].arr().mapNotNull { it.str() },
                    d["accept_label"].str() ?: "I understand, connect", d["decline_label"].str() ?: "Use a key instead",
                ))
            }
        }
        throw e
    }

    suspend fun list(target: String? = null): List<ConnectionWire> = send("", target)["connections"].arr().mapNotNull(ConnectionWire::parse)

    /**
     * Returns the login to show next, or null when the row is already on. [way] is `code` for plan
     * sign-ins and `oauth` for the Claude organization link (a redirect sign-in the server starts and polls).
     */
    suspend fun connect(id: String, target: String?, acceptDisclosure: String? = null, way: String = "code"): Pair<ConnState?, ConnLogin?> {
        val json = send("$id/connect", target, "POST", buildJsonObject {
            put("way", way)
            if (acceptDisclosure != null) put("accept_disclosure", acceptDisclosure)
        })
        val l = json["login"]
        val login = l.obj()?.let {
            ConnLogin(l["kind"].str() ?: "unknown", l["code"].str(), l["url"].str(), l["url_with_code"].str(), l["expires_at"].str(), l["poll"].bool() ?: true)
        }
        return json["state"].str()?.let(ConnState::of) to login
    }

    /** iOS ConnectionsClient.disconnect: `DELETE /api/v1/connections/<id>`. */
    suspend fun disconnect(id: String, target: String?) {
        send(id, target, "DELETE")
    }

    suspend fun submit(id: String, target: String?, value: String): ConnectionWire? =
        ConnectionWire.parse(send("$id/submit", target, "POST", buildJsonObject { put("value", value) }))

    /** `pending`, `done`, `expired` or `failed`, with the fresh row when done. */
    suspend fun login(id: String, target: String?): Pair<String, ConnectionWire?> {
        val json = send("$id/login", target)
        return (json["status"].str() ?: "failed") to json["row"]?.let(ConnectionWire::parse)
    }

    /** A short plain sentence for a failed call (iOS ConnectionsClient.describe). */
    fun describe(e: Throwable): String = when (e) {
        is DisclosureRequired -> "Read and accept the note first."
        is SetupError -> when (e.code) {
            "runner_busy" -> "Busy. Try again"
            "could_not_start" -> "Could not start. Try again"
            "update_app" -> "Update the app to connect this plan."
            "not_found" -> "This isn't offered on your account right now. Use a key for now."
            "login_expired" -> "Sign-in expired. Sign in again."
            "vendor_limited" -> e.message.ifEmpty { "This plan isn't active or its limit is used up. Check the subscription, or pick another brain." }
            "network" -> "No connection. Try again."
            else -> when (e.status) {
                401 -> "Sign in to Allternit on this phone, then try again."
                403 -> "Your Allternit session can't use this. Sign in again."
                404 -> "This isn't ready on the server yet. Try again later."
                else -> e.message.takeIf { it.isNotEmpty() && e.code != null } ?: "The server answered ${e.status ?: 0}. Try again."
            }
        }
        else -> "Could not start. Try again."
    }
}

/** Where a row runs (iOS ConnRunsOn). */
internal enum class ConnRunsOn { Server, Runner, Device, Computer }

internal enum class ConnWay { OAuth, Code, Paste, Install, Included }

/** The parts of a connection that don't change per account (iOS ConnectionsCatalog). */
internal data class ConnEntry(
    val id: String,
    val name: String,
    val mono: String,
    val blurb: String? = null,
    val way: ConnWay,
    val runsOn: ConnRunsOn = ConnRunsOn.Server,
    val isNew: Boolean = false,
    /** The Settings > Connections group: brains, computers, phones, tools, voice, apps or channels (iOS ConnGroup). */
    val group: String = "brains",
)

internal data class ConnKeyProvider(val id: String, val name: String, val keyPage: String)

internal object ConnectionsCatalog {
    val entries = listOf(
        ConnEntry("allternit", "Allternit Cloud", "A", way = ConnWay.Included),
        ConnEntry("chatgpt", "ChatGPT", "G", "Plus or Pro plan", ConnWay.OAuth, ConnRunsOn.Runner),
        ConnEntry("claude", "Claude", "C", "Pro or Max plan", ConnWay.Code, ConnRunsOn.Runner),
        // Listed only once cloud-api sends the row (Cloud flag ALLTERNIT_CLAUDE_LINK; ConnectionsStore.isListed).
        ConnEntry("claude-org", "Claude organization", "C", "Bill your own Claude organization", ConnWay.OAuth, isNew = true),
        ConnEntry("gemini", "Gemini", "G", "Free Google AI Studio key", ConnWay.Paste, isNew = true),
        ConnEntry("copilot", "GitHub Copilot", "H", "Copilot Pro or Business", ConnWay.OAuth, isNew = true),
        ConnEntry("grok", "Grok", "X", "SuperGrok plan", ConnWay.Code, ConnRunsOn.Runner, isNew = true),
        ConnEntry("kimi", "Kimi", "K", "Kimi plan", ConnWay.Code, ConnRunsOn.Runner),
        ConnEntry("api-key", "An API key", "⚿", "OpenAI, Anthropic, Google, xAI, OpenRouter and more", ConnWay.Paste),
        ConnEntry("local", "Local brain", "L", "Runs offline on your own computer", ConnWay.Install, ConnRunsOn.Computer),
        ConnEntry("this-mac", "Your Mac or PC", "C", "Lets Allternit use the apps on it", ConnWay.Install, ConnRunsOn.Computer, group = "computers"),
        // Settings > Connections groups the setup flow does not show (iOS ConnectionsCatalog).
        ConnEntry("cloud-computer", "Allternit Cloud computer", "C", way = ConnWay.Included, group = "computers"),
        ConnEntry("server", "Your own server", "S", "A Linux machine you own", ConnWay.Install, ConnRunsOn.Computer, group = "computers"),
        ConnEntry("phone", "Your phone", "P", "iPhone or Android", ConnWay.Install, ConnRunsOn.Device, group = "phones"),
        ConnEntry("android-screen", "Android phone (screen share)", "A", "See and tap your Android from any device", ConnWay.Install, isNew = true, group = "phones"),
        ConnEntry("phone-control", "Let your bots use this phone", "P", "Taps and types in your phone apps for you", ConnWay.Install, ConnRunsOn.Device, group = "phones"),
        ConnEntry("gizzi", "Gizzi Code", "G", way = ConnWay.Install, runsOn = ConnRunsOn.Computer, group = "tools"),
        ConnEntry("claude-code", "Claude Code", "C", way = ConnWay.Code, runsOn = ConnRunsOn.Computer, group = "tools"),
        ConnEntry("codex", "Codex", "X", way = ConnWay.Code, runsOn = ConnRunsOn.Computer, group = "tools"),
        ConnEntry("gemini-cli", "Gemini CLI", "G", way = ConnWay.Code, runsOn = ConnRunsOn.Computer, group = "tools"),
        ConnEntry("cursor", "Cursor Agent", "C", way = ConnWay.Code, runsOn = ConnRunsOn.Computer, group = "tools"),
        ConnEntry("grok-cli", "Grok CLI", "X", way = ConnWay.Code, runsOn = ConnRunsOn.Computer, group = "tools"),
        ConnEntry("kimi-cli", "Kimi CLI", "K", way = ConnWay.Code, runsOn = ConnRunsOn.Computer, group = "tools"),
        ConnEntry("openai-voice", "OpenAI", "O", "Realtime voice", ConnWay.Paste, ConnRunsOn.Device, group = "voice"),
        ConnEntry("elevenlabs", "ElevenLabs", "E", "Studio voices", ConnWay.Paste, ConnRunsOn.Device, group = "voice"),
        ConnEntry("gmail", "Gmail", "G", way = ConnWay.OAuth, group = "apps"),
        ConnEntry("drive", "Google Drive", "D", way = ConnWay.OAuth, group = "apps"),
        ConnEntry("slack", "Slack", "S", way = ConnWay.OAuth, group = "apps"),
        ConnEntry("github", "GitHub", "H", way = ConnWay.OAuth, group = "apps"),
        ConnEntry("notion", "Notion", "N", way = ConnWay.OAuth, group = "apps"),
        ConnEntry("telegram", "Telegram", "T", way = ConnWay.OAuth, group = "channels"),
        ConnEntry("whatsapp", "WhatsApp", "W", way = ConnWay.OAuth, group = "channels"),
        ConnEntry("discord", "Discord", "D", way = ConnWay.OAuth, group = "channels"),
    )

    /** The bundled logo (assets/logos) iOS shows for a row; null where iOS draws an SF symbol. */
    fun logoKey(id: String): String? = when (id) {
        "allternit", "gizzi" -> "allternit"
        "chatgpt", "openai-voice" -> "openai"
        "claude", "claude-code", "claude-org" -> "claude"
        "gemini" -> "googlegemini"
        "gemini-cli" -> "gemini"
        "copilot", "github" -> "github"
        "grok", "grok-cli" -> "grok"
        "kimi", "kimi-cli" -> "kimi"
        "local" -> "ollama"
        "codex", "cursor", "elevenlabs", "gmail", "slack", "notion", "telegram", "whatsapp", "discord" -> id
        "drive" -> "googledrive"
        else -> null
    }

    val keyProviders = listOf(
        ConnKeyProvider("openai-key", "OpenAI", "https://platform.openai.com/api-keys"),
        ConnKeyProvider("anthropic-key", "Anthropic", "https://console.anthropic.com/settings/keys"),
        ConnKeyProvider("google-key", "Google AI Studio", "https://aistudio.google.com/apikey"),
        ConnKeyProvider("xai-key", "xAI", "https://console.x.ai"),
        ConnKeyProvider("openrouter-key", "OpenRouter", "https://openrouter.ai/settings/keys"),
        ConnKeyProvider("groq-key", "Groq", "https://console.groq.com/keys"),
        ConnKeyProvider("deepseek-key", "DeepSeek", "https://platform.deepseek.com/api_keys"),
        ConnKeyProvider("kimi-key", "Kimi (Moonshot)", "https://platform.moonshot.ai/console/api-keys"),
        ConnKeyProvider("together-key", "Together", "https://api.together.ai/settings/api-keys"),
        ConnKeyProvider("fireworks-key", "Fireworks", "https://fireworks.ai/account/api-keys"),
        ConnKeyProvider("deepinfra-key", "DeepInfra", "https://deepinfra.com/dash/api_keys"),
    )

    fun entry(id: String) = entries.firstOrNull { it.id == id }
    fun keyProvider(id: String) = keyProviders.firstOrNull { it.id == id }

    val knownIds: Set<String> = entries.map { it.id }.toSet() + keyProviders.map { it.id }
    val runnerIds = setOf("claude", "grok", "kimi")

    /** The coding tools whose sign-in runs on a computer (contract §4, runtime CLI logins). */
    val cliLoginIds = setOf("claude-code", "codex", "gemini-cli", "cursor", "grok-cli", "kimi-cli")

    /** The key sheet "Use a key instead" opens for a plan row. */
    fun keyFallback(id: String) = when (id) {
        "claude" -> "anthropic-key"; "chatgpt" -> "openai-key"; "grok" -> "xai-key"; "kimi" -> "kimi-key"; else -> "api-key"
    }

    /**
     * What a row shows when the server sent none (contract §3). Rows that can never work
     * without the server's say-so start unavailable. iOS signs ChatGPT in on the phone itself
     * (§9.9); Android has no on-device ChatGPT sign-in, so it waits for the server's runner row
     * and falls back to an OpenAI key, like the other plan rows.
     */
    fun fallbackRow(id: String, computerOnline: Boolean): ConnectionWire = when (id) {
        "claude", "grok", "kimi", "chatgpt" -> ConnectionWire(id, reasonCode = "not_available_yet", available = false,
            unavailableReason = "Not available yet. Use a key for now.", fallbacks = listOf(keyFallback(id)))
        "copilot" -> ConnectionWire(id, available = false, unavailableReason = "Coming soon.", fallbacks = listOf("github"))
        "claude-org" -> ConnectionWire(id, reasonCode = "not_available_yet", available = false, unavailableReason = "Not available yet.")
        "local" -> if (computerOnline) ConnectionWire(id)
        else ConnectionWire(id, reasonCode = "no_computer", available = false, unavailableReason = "Runs on a computer.", fallbacks = listOf("this-mac"))
        else -> ConnectionWire(id)
    }

    /** Contract §5: the plain text for each reason code. */
    fun reasonText(code: String?, name: String): String? = when (code) {
        "login_expired" -> "Sign-in expired"
        "key_rejected" -> "The key was rejected"
        "vendor_limited" -> "$name plan not active or limit used up"
        "no_computer" -> "Needs your own computer for now"
        "not_available_yet" -> "Not available yet"
        "org_not_eligible" -> "Your Claude organization can't be billed. It must pay Anthropic directly."
        "spend_limit_reached" -> "Your Claude organization's spend limit is reached"
        "disconnected" -> "Your Claude organization is disconnected"
        "relink_needed" -> "Link your Claude organization again"
        "computer_offline" -> "Your computer is offline"
        "could_not_start" -> "Could not start. Try again"
        "update_app" -> "Update the app to connect this plan."
        "runner_busy" -> "Busy. Try again"
        "usage_limit" -> "$name limit reached"
        else -> null
    }

    /** Where a person raises their organization's spend limit at Anthropic. */
    const val CLAUDE_LINKED_PLATFORMS_URL = "https://platform.claude.com/linked-platforms"

    /** The one-line note under the unlinked Claude organization row. */
    const val CLAUDE_ORG_NOTE = "Needs the Admin or Billing role. Your org must pay Anthropic directly."

    fun fallbackLabel(id: String): String = when {
        id == "this-mac" -> "Add a computer"
        id == "api-key" || id.endsWith("-key") -> "Use a key"
        id == "github" -> "Use GitHub"
        else -> entry(id)?.let { "Use ${it.name}" } ?: "Other way"
    }
}

// The "More" list (contract §11): GET {cloud}/api/v1/connections/catalog

/** One entry of the catalog; the field names mirror the wire (iOS ConnCatalogEntry). */
internal data class CatalogEntry(
    val id: String,
    val group: String,
    val name: String,
    val blurb: String?,
    val category: String?,
    val ways: List<String>,
    val runsOn: String?,
    val connectorId: String?,
    val keyPage: String?,
    val logo: String?,
    val available: Boolean,
    val unavailableReason: String?,
    val fallbacks: List<String>,
) {
    /** The ways this app can run today: a key paste (key sheet or connector key) and a browser sign-in. */
    val runnable: Boolean get() = ways.firstOrNull() in setOf("paste", "oauth")

    /** The row the page draws for this entry until the server has a live one. */
    fun wire() = ConnectionWire(id, runsOn = runsOn, available = available, unavailableReason = unavailableReason, fallbacks = fallbacks)

    fun entry() = ConnEntry(
        id = id, name = name, mono = name.firstOrNull()?.uppercase() ?: "?",
        blurb = blurb?.takeIf { it.isNotBlank() } ?: category,
        way = if (ways.firstOrNull() == "paste") ConnWay.Paste else ConnWay.OAuth,
        runsOn = when (runsOn) { "computer" -> ConnRunsOn.Computer; "device" -> ConnRunsOn.Device; "runner" -> ConnRunsOn.Runner; else -> ConnRunsOn.Server },
        group = group,
    )

    companion object {
        fun parse(v: JsonElement): CatalogEntry? {
            val id = v["id"].str() ?: return null
            return CatalogEntry(
                id = id, group = v["group"].str() ?: "apps", name = v["name"].str() ?: id, blurb = v["blurb"].str(),
                category = v["category"].str(), ways = v["ways"].arr().mapNotNull { it.str() }, runsOn = v["runs_on"].str(),
                connectorId = v["connector_id"].str(), keyPage = v["key_page"].str(), logo = v["logo"].str(),
                available = v["available"].bool() ?: true, unavailableReason = v["unavailable_reason"].str(),
                fallbacks = v["fallbacks"].arr().mapNotNull { it.str() },
            )
        }
    }
}

/** One page of the catalog: `counts` follow `q` but not `group`; `nextCursor` is opaque. */
internal data class CatalogPage(val total: Int, val nextCursor: String?, val counts: Map<String, Int>, val entries: List<CatalogEntry>)

internal object ConnectionsCatalogClient {
    const val PAGE = 24

    /** [limit] 0 returns only the per-group counts (the More tiles read that). */
    suspend fun page(group: String? = null, q: String = "", cursor: String? = null, limit: Int = PAGE): CatalogPage {
        val query = buildMap {
            if (group != null) put("group", group)
            q.trim().takeIf { it.isNotEmpty() }?.let { put("q", it.take(64)) }
            if (cursor != null) put("cursor", cursor)
            put("limit", limit.toString())
        }
        val json = SetupHttp.cloud("GET", "api/v1/connections/catalog", null, query)
        val counts = json["counts"].obj().orEmpty().mapNotNull { (k, v) -> v.num()?.toInt()?.let { k to it } }.toMap()
        return CatalogPage(
            total = json["total"].num()?.toInt() ?: 0,
            nextCursor = json["next_cursor"].str()?.takeIf { it.isNotEmpty() },
            counts = counts,
            entries = json["entries"].arr().mapNotNull(CatalogEntry::parse),
        )
    }
}

/** An app connected through the computer's connector gateway (iOS ConnectorsClient, the part More needs). */
internal data class ConnectedApp(val id: String, val name: String, val detail: String?, val logo: String?)

/**
 * The connector calls More uses for apps (the same routes as Settings › Customize › Connectors):
 * `GET connectors`, `POST connectors/<id>/connect`, `DELETE connectors/<id>/disconnect`. They go
 * to the person's computer through cloud-api, so they fail when no computer answers.
 */
internal object AppConnectorsClient {
    suspend fun connected(): List<ConnectedApp> {
        val json = SetupHttp.v1("GET", "connectors")
        val list = (json["connectors"] ?: json["items"]).arr().ifEmpty { json.arr() }
        return list.mapNotNull { c ->
            val id = c["id"].str() ?: return@mapNotNull null
            if (c["connection"]["status"].str() != "connected") return@mapNotNull null
            val detail = listOfNotNull(c["connection"]["account"].str(), c["category"].str()).firstOrNull()
            ConnectedApp(id, c["name"].str() ?: id, detail, c["logo"].str())
        }
    }

    /** Returns the sign-in page to open, when the connector needs one. */
    suspend fun connect(id: String, apiKey: String? = null): String? {
        val body = buildJsonObject { if (apiKey != null) put("api_key", apiKey) }
        return SetupHttp.v1("POST", "connectors/$id/connect", body)["authorize_url"].str()
    }

    suspend fun disconnect(id: String) {
        SetupHttp.v1("DELETE", "connectors/$id/disconnect")
    }
}
