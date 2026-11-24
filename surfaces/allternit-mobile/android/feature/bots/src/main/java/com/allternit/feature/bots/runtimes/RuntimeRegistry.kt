package com.allternit.feature.bots.runtimes

import android.content.Context
import com.allternit.feature.artifacts.LibraryNet
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put
import okhttp3.Request

/*
 * Runtime registry client (Harness & Bots 2.0, CONTRACT-harness-bots-2.md; plan D2/D3).
 *
 * The one list of runtimes a bot can run on comes from `GET <cloud-api>/api/v1/runtimes`
 * (`{version, runtimes}`, public, ETag, max-age 3600). The last good copy is kept on disk so the
 * picker opens instantly and works offline; a failed refresh with a saved copy is "stale", a failed
 * refresh with nothing saved is "error" and the picker shows only the built-in Allternit runtime.
 * Every field is read tolerantly: a new server field never breaks the screen (unknown ones ignored).
 */

/** One segment of a look line: text with {a} (argument), {r} (result) and {name} slots, and a role. */
data class LineSeg(val text: String, val role: String)
typealias LookLine = List<LineSeg>

/** Look tokens (contract "Tokens" + "Token fields added in P1"). Colors are hex strings. */
data class LookTokens(
    val kind: String,
    val bg: String, val fg: String, val mu: String, val ac: String,
    val ac2: String?, val ok: String?, val err: String?,
    val light: Map<String, String>,
    /** `dark: true` marks a native-dark look; an object gives the dark palette of a light look. */
    val darkFlag: Boolean?,
    val darkPalette: Map<String, String>,
    val banner: List<LookLine>,
    val bigText: String?, val bigGrad: List<String>, val bigParts: LookLine,
    val bannerBox: Boolean, val boxRole: String?,
    val userStyle: String, val userFill: String?,
    val prompt: String,
    val steps: Map<String, List<LookLine>>,
    val pre: List<LookLine>,
    val stepBox: Boolean,
    val answerGlyph: String?, val answerGlyphRole: String?, val answerRole: String?,
    val input: String, val inputGlyph: String?, val inputGlyphRole: String?, val inputBorderRole: String?,
    val placeholder: String,
    val footerL: LookLine?, val footerM: LookLine?, val footerR: LookLine?,
    val pillRole: String?,
    val header: String?, val status: String?,
    val radius: Int?, val sendSquare: Boolean,
) {
    val isApp: Boolean get() = kind == "app"
}

data class LiveUi(val label: String, val port: Int?)

data class RuntimeEntry(
    val id: String,
    val name: String,
    val vendor: String,
    val category: String,
    val tool: String?,
    val aliases: List<String>,
    /** Aliases of the runtime's CLI tool when the body carries them (the registry body has no `tools[]`). */
    val toolAliases: List<String>,
    val adapterKind: String,
    /** null = "any" brain. */
    val brains: List<String>?,
    val access: List<String>,
    val more: Boolean,
    val statusLabel: String?,
    val statusTone: String?,
    val lookDefault: String,
    val terminal: LookTokens?,
    val desktop: LookTokens?,
    val liveUi: LiveUi?,
    val legacyHarness: String?,
    val legacyUhp: List<String>,
    val docs: String?,
    /** Subscription plans allowed inside this runtime (registry `plans`); null on an older server, see [plansOf]. */
    val plans: List<String>? = null,
) {
    /** The tokens for [look] (falls back to the other look, like web `runtimeTokens`). */
    fun tokens(look: String? = null): LookTokens? {
        val want = look ?: lookDefault
        return (if (want == "desktop") desktop else terminal) ?: (if (want == "desktop") terminal else desktop)
    }
}

/** Where the registry stands (web `useRuntimes` status). */
sealed interface RegistryState {
    val runtimes: List<RuntimeEntry>
    data object Loading : RegistryState { override val runtimes: List<RuntimeEntry> get() = emptyList() }
    data class Ready(override val runtimes: List<RuntimeEntry>, val version: Int) : RegistryState
    /** A refresh failed; the saved copy is shown. */
    data class Stale(override val runtimes: List<RuntimeEntry>, val version: Int, val message: String) : RegistryState
    /** Nothing saved and the fetch failed: only the built-in Allternit runtime. */
    data class Error(val message: String) : RegistryState { override val runtimes: List<RuntimeEntry> get() = listOf(BUILT_IN_ALLTERNIT) }
}

/** The native runtime; always available, so a phone with no registry still has a working picker. */
val BUILT_IN_ALLTERNIT = RuntimeEntry(
    id = "allternit", name = "Allternit", vendor = "Allternit", category = "native", tool = null, aliases = emptyList(), toolAliases = emptyList(),
    adapterKind = "native", brains = null, access = listOf("plan", "key", "local", "credits"), more = false, statusLabel = null, statusTone = null,
    lookDefault = "terminal", terminal = null, desktop = null, liveUi = null, legacyHarness = null, legacyUhp = emptyList(), docs = null,
)

object RuntimeRegistry {
    private const val MAX_AGE_MS = 3_600_000L
    private val json = Json { ignoreUnknownKeys = true; isLenient = true }
    private val mutex = Mutex()
    private val _state = MutableStateFlow<RegistryState>(RegistryState.Loading)
    val state: StateFlow<RegistryState> = _state.asStateFlow()
    private var etag: String? = null
    private var fetchedAt = 0L
    private var diskRead = false

    val runtimes: List<RuntimeEntry> get() = _state.value.runtimes

    private fun cacheFile(context: Context) = File(context.filesDir, "runtimes/registry.json")

    /**
     * Loads the saved copy (first call), then refreshes from cloud-api when the copy is older than
     * max-age or [force] is set. Sends If-None-Match; a 304 keeps the saved copy and marks it fresh.
     */
    suspend fun load(context: Context, force: Boolean = false) = mutex.withLock {
        withContext(Dispatchers.IO) {
            if (!diskRead) {
                diskRead = true
                runCatching {
                    val f = cacheFile(context)
                    if (f.exists()) {
                        val o = json.parseToJsonElement(f.readText()) as? JsonObject
                        val body = o?.get("body") as? JsonObject
                        if (body != null) {
                            etag = o.s("etag")
                            fetchedAt = (o["fetchedAt"] as? JsonPrimitive)?.contentOrNull?.toLongOrNull() ?: 0L
                            _state.value = RegistryState.Ready(parseRuntimes(body), body.i("version") ?: 0)
                        }
                    }
                }
            }
            val now = System.currentTimeMillis()
            if (!force && _state.value is RegistryState.Ready && now - fetchedAt < MAX_AGE_MS) return@withContext
            if (_state.value is RegistryState.Error) _state.value = RegistryState.Loading
            val url = LibraryNet.config.cloudBaseUrl.newBuilder().addPathSegments("api/v1/runtimes").build()
            val req = Request.Builder().url(url).get().header("Accept", "application/json")
                .apply { etag?.takeIf { _state.value.runtimes.isNotEmpty() }?.let { header("If-None-Match", it) } }.build()
            val failure: String? = try {
                LibraryNet.okHttp.newCall(req).execute().use { resp ->
                    when {
                        resp.code == 304 -> { fetchedAt = now; saveStamp(context); null }
                        resp.isSuccessful -> {
                            val text = resp.body?.string().orEmpty()
                            val body = json.parseToJsonElement(text) as? JsonObject ?: error("bad body")
                            val list = parseRuntimes(body)
                            if (list.isEmpty()) error("empty registry")
                            etag = resp.header("ETag")
                            fetchedAt = now
                            _state.value = RegistryState.Ready(list, body.i("version") ?: 0)
                            save(context, body)
                            null
                        }
                        else -> "The runtime list answered ${resp.code}."
                    }
                }
            } catch (e: Exception) {
                "Couldn't reach Allternit. Check your connection."
            }
            if (failure != null) {
                val cur = _state.value
                _state.value = when (cur) {
                    is RegistryState.Ready -> RegistryState.Stale(cur.runtimes, cur.version, failure)
                    is RegistryState.Stale -> cur.copy(message = failure)
                    else -> RegistryState.Error(failure)
                }
            }
        }
    }

    private fun save(context: Context, body: JsonObject) = runCatching {
        val f = cacheFile(context)
        f.parentFile?.mkdirs()
        val wrapper = buildJsonObject {
            etag?.let { put("etag", it) }
            put("fetchedAt", fetchedAt.toString())
            put("body", body)
        }
        val tmp = File(f.parentFile, "registry.json.tmp")
        tmp.writeText(wrapper.toString())
        tmp.renameTo(f)
    }

    private fun saveStamp(context: Context) = runCatching {
        val f = cacheFile(context)
        val o = json.parseToJsonElement(f.readText()) as? JsonObject ?: return@runCatching
        val body = o["body"] as? JsonObject ?: return@runCatching
        save(context, body)
    }

    fun parseRuntimes(body: JsonObject): List<RuntimeEntry> =
        (body["runtimes"] as? JsonArray).orEmpty().mapNotNull { (it as? JsonObject)?.let(::parseEntry) }

    private fun parseEntry(o: JsonObject): RuntimeEntry? {
        val id = o.s("id") ?: return null
        val look = o["look"] as? JsonObject
        val brains = when (val b = o["brains"]) {
            is JsonArray -> b.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
            else -> null // "any" (or missing) fits every brain
        }
        val status = o["status"] as? JsonObject
        val live = o["liveUi"] as? JsonObject
        val legacy = o["legacy"] as? JsonObject
        return RuntimeEntry(
            id = id, name = o.s("name") ?: id, vendor = o.s("vendor") ?: "", category = o.s("category") ?: "general",
            tool = o.s("tool"), aliases = o.strs("aliases"), toolAliases = o.strs("toolAliases"),
            adapterKind = (o["adapter"] as? JsonObject).s("kind") ?: "unsupported",
            brains = brains, access = o.strs("access"), more = o.b("more") == true,
            statusLabel = status.s("label"), statusTone = status.s("tone"),
            lookDefault = look.s("default") ?: "terminal",
            terminal = (look?.get("terminal") as? JsonObject)?.let(::parseTokens),
            desktop = (look?.get("desktop") as? JsonObject)?.let(::parseTokens),
            liveUi = live?.let { LiveUi(it.s("label") ?: "${o.s("name") ?: id} web app", it.i("port")) },
            legacyHarness = legacy.s("harness"), legacyUhp = legacy.strs("uhp"), docs = o.s("docs"),
            plans = (o["plans"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull },
        )
    }

    private fun parseLine(e: JsonElement?): LookLine? = (e as? JsonArray)?.mapNotNull { seg ->
        val a = seg as? JsonArray ?: return@mapNotNull null
        val text = (a.getOrNull(0) as? JsonPrimitive)?.contentOrNull ?: return@mapNotNull null
        LineSeg(text, (a.getOrNull(1) as? JsonPrimitive)?.contentOrNull ?: "fg")
    }

    private fun parseLines(e: JsonElement?): List<LookLine> = (e as? JsonArray)?.mapNotNull { parseLine(it) }.orEmpty()

    private fun palette(e: JsonElement?): Map<String, String> =
        (e as? JsonObject)?.mapNotNull { (k, v) -> (v as? JsonPrimitive)?.contentOrNull?.let { k to it } }?.toMap().orEmpty()

    private fun parseTokens(t: JsonObject): LookTokens? {
        val steps = t["steps"] as? JsonObject ?: return null
        val big = t["big"] as? JsonObject
        val footer = t["footer"] as? JsonObject
        return LookTokens(
            kind = t.s("kind") ?: "tui",
            bg = t.s("bg") ?: "#16181d", fg = t.s("fg") ?: "#e5e7eb", mu = t.s("mu") ?: "#9ca3af", ac = t.s("ac") ?: "#e5e7eb",
            ac2 = t.s("ac2"), ok = t.s("ok"), err = t.s("err"),
            light = palette(t["light"]),
            darkFlag = (t["dark"] as? JsonPrimitive)?.booleanOrNull,
            darkPalette = palette(t["dark"]),
            banner = parseLines(t["banner"]),
            bigText = big.s("text"), bigGrad = big.strs("grad"), bigParts = parseLine(big?.get("parts")).orEmpty(),
            bannerBox = t.b("bannerBox") == true, boxRole = t.s("boxRole"),
            userStyle = t.s("userStyle") ?: "plain", userFill = t.s("userFill"),
            prompt = t.s("prompt") ?: "",
            steps = listOf("Read", "Edit", "Run").associateWith { parseLines(steps[it]) },
            pre = parseLines(t["pre"]),
            stepBox = t.b("stepBox") == true,
            answerGlyph = t.s("answerGlyph"),
            answerGlyphRole = t.s("agentGlyph") ?: t.s("answerGlyphRole"),
            answerRole = t.s("answerRole"),
            input = t.s("input") ?: "plain",
            inputGlyph = (t["inputGlyph"] as? JsonPrimitive)?.contentOrNull,
            inputGlyphRole = t.s("glyphRole"), inputBorderRole = t.s("inputRole"),
            placeholder = t.s("placeholder") ?: "",
            footerL = parseLine(footer?.get("l")), footerM = parseLine(footer?.get("m")), footerR = parseLine(footer?.get("r")),
            pillRole = t.s("pillRole"),
            header = t.s("header"), status = t.s("status"),
            radius = (t["radius"] as? JsonPrimitive)?.doubleOrNull?.toInt(), sendSquare = t.b("sendSquare") == true,
        )
    }
}

// ─── Shared rules (gizzi runtime-registry.ts is the reference; web compat.ts mirrors it) ─────────

/** A runtime by id or alias (case-insensitive). */
fun List<RuntimeEntry>.runtime(idOrAlias: String?): RuntimeEntry? {
    if (idOrAlias.isNullOrEmpty()) return null
    val key = idOrAlias.lowercase()
    return firstOrNull { it.id.lowercase() == key } ?: firstOrNull { r -> r.aliases.any { it.lowercase() == key } }
}

/** Tool aliases the registry body does not carry (web compat `toolAliasesOf` reads provider-registry CLI rows). */
private val KNOWN_TOOL_ALIASES = mapOf(
    "claude-cli" to listOf("claude"), "codex-cli" to listOf("codex"), "antigravity" to listOf("agy"),
    "gemini-cli" to listOf("gemini"), "kimi-cli" to listOf("kimi"), "qwen-cli" to listOf("qwen"),
)

/**
 * Does a brain from [providerId] fit [runtime]? `brains == any` fits every provider; the id may be a
 * vendor id (`anthropic`), a subscription id (`subs-anthropic`, prefix stripped) or a CLI provider id
 * (`claude-cli`): a runtime always runs its own CLI's models (its tool, id, aliases, the tool's aliases).
 */
fun fits(runtime: RuntimeEntry?, providerId: String): Boolean {
    val r = runtime ?: return false
    val brains = r.brains ?: return true
    val p = providerId.removePrefix("subs-")
    if (p in brains) return true
    val tool = r.tool ?: return false
    if (p == tool || p == r.id || p in r.aliases) return true
    return p in (r.toolAliases.ifEmpty { KNOWN_TOOL_ALIASES[tool].orEmpty() })
}

/**
 * Normalizer rule (contract), on a raw `botBrain` object; unknown keys are kept. With no `runtime`,
 * derive it: allternit_cloud -> allternit; native_harness + h -> the runtime whose legacy.harness or
 * id is h; uhp_harness -> the legacy.uhp match, else custom. With a runtime, recompute mode/harness.
 */
fun resolveRuntime(botBrain: JsonObject, runtimes: List<RuntimeEntry>): JsonObject {
    val m = botBrain.toMutableMap()
    val rid = botBrain.s("runtime")
    if (rid != null) {
        val r = runtimes.runtime(rid) ?: if (rid == "allternit") BUILT_IN_ALLTERNIT else null
        when {
            r == null && rid == "custom" -> Unit
            r == null -> { m["mode"] = JsonPrimitive("native_harness"); m["harness"] = JsonPrimitive(rid) }
            r.id == "allternit" -> { m.remove("harness"); m.remove("uhpHarnessId"); m["mode"] = JsonPrimitive("allternit_cloud") }
            r.adapterKind == "unsupported" -> { m["mode"] = JsonPrimitive("uhp_harness"); m["uhpHarnessId"] = JsonPrimitive(r.legacyUhp.firstOrNull() ?: r.id) }
            else -> { m["mode"] = JsonPrimitive("native_harness"); m["harness"] = JsonPrimitive(r.legacyHarness ?: r.id) }
        }
        m["runtime"] = JsonPrimitive(r?.id ?: rid)
        return JsonObject(m)
    }
    val mode = botBrain.s("mode")
    val harness = botBrain.s("harness")
    val derived = when {
        mode == "native_harness" && harness != null ->
            (runtimes.firstOrNull { it.legacyHarness == harness } ?: runtimes.runtime(harness))?.id ?: "custom"
        mode == "uhp_harness" -> {
            val u = botBrain.s("uhpHarnessId") ?: botBrain.s("harnessId")
            u?.let { id -> runtimes.firstOrNull { id in it.legacyUhp }?.id } ?: "custom"
        }
        else -> "allternit"
    }
    m["runtime"] = JsonPrimitive(derived)
    return JsonObject(m)
}

// ─── Small JSON readers (tolerant, like the iOS Codable structs) ───────────────────────────────

internal fun JsonObject?.s(key: String): String? = (this?.get(key) as? JsonPrimitive)?.takeIf { it !is JsonNull && it.isString }?.contentOrNull?.takeIf { it.isNotEmpty() }
private fun JsonObject?.i(key: String): Int? = (this?.get(key) as? JsonPrimitive)?.intOrNull
private fun JsonObject?.b(key: String): Boolean? = (this?.get(key) as? JsonPrimitive)?.booleanOrNull
private fun JsonObject?.strs(key: String): List<String> = (this?.get(key) as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }.orEmpty()

/** Plan display names (ALLOWED-RULE PLAN_NAMES). */
val PLAN_NAMES = mapOf("claude" to "Claude", "chatgpt" to "ChatGPT", "kimi" to "Kimi", "gemini" to "Gemini", "copilot" to "Copilot")

/** Built-in `plans` for a registry body without them (ALLOWED-RULE table). */
private val FALLBACK_PLANS = mapOf(
    "allternit" to listOf("claude", "chatgpt", "kimi", "gemini", "copilot"), "claude" to listOf("claude"), "claude-code" to listOf("claude"),
    "codex" to listOf("chatgpt"), "kimi" to listOf("kimi"), "antigravity" to listOf("gemini"), "copilot" to listOf("copilot"),
)

/** The plans a runtime allows: the registry's list, else the built-in table, else none. */
fun plansOf(r: RuntimeEntry): List<String> = r.plans ?: FALLBACK_PLANS[r.id] ?: FALLBACK_PLANS[r.tool?.removeSuffix("-cli")].orEmpty()
