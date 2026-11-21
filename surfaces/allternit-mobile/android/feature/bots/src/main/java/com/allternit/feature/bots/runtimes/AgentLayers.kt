package com.allternit.feature.bots.runtimes

import android.content.Context
import android.provider.Settings
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext
import com.allternit.feature.bots.BotRecord
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/*
 * The agent layers (Harness & Bots 2.0; web `useAgentLayers` in AgentCard.tsx, iOS BotBrainPickerSheet):
 * Runtime (how it works) · Brain (how it thinks) · Bot (who it is), over the Allternit control layer.
 * One state object feeds the bot form picker and the bot-chat composer chips, so both write the
 * same record: `config.botBrain.runtime/access/autoSwitchedFrom/modelRef` (legacy mode/harness kept
 * in step by [resolveRuntime]) and `config.agentView`. Unknown keys are never dropped.
 */

/** A brain the pickers can choose: a provider's model and how it is paid for. */
data class BrainOption(val providerId: String, val modelId: String, val name: String, val access: String = accessOfProvider(providerId)) {
    val key: String get() = "$providerId/$modelId"
}

/** A bot the Bot layer lists. */
data class BotChoice(val record: BotRecord, val connected: Boolean, val vendor: String?) {
    val id: String get() = record.id
}

/** One card or row in a layer. */
data class LayerChoice(
    val id: String,
    val name: String,
    val detail: String,
    val group: String? = null,
    /** Logo key: a runtime tool/id or a provider id (drawn as its initial on a neutral tile). */
    val mark: String,
    val badges: List<Pair<String, BadgeTone>> = emptyList(),
    val ok: Boolean = true,
    val selected: Boolean = false,
    val bot: BotRecord? = null,
)

enum class BadgeTone { Neutral, Live, Warn, Plan, Key, Local, Credits }

enum class AgentLayer(val label: String, val hint: String) { Runtime("Runtime", "How it works"), Brain("Brain", "How it thinks"), Bot("Bot", "Who it is") }

val CATEGORY_LABEL = mapOf(
    "native" to "Allternit native", "coding" to "Coding", "personal" to "Personal assistant",
    "general" to "General agent", "experimental" to "Experimental",
)
val ACCESS_LABEL = mapOf("plan" to "Plan", "key" to "API key", "local" to "Local", "credits" to "Credits")

private val LOCAL_PROVIDERS = setOf("ollama", "omlx", "allternit-sidecar", "local-brain", "allternit-devices", "ondevice")
private val PLAN_VENDORS = mapOf(
    "anthropic" to "Claude", "claude" to "Claude", "openai" to "ChatGPT", "codex" to "ChatGPT", "google" to "Gemini",
    "gemini" to "Gemini", "kimi" to "Kimi", "moonshot" to "Kimi", "xai" to "Grok", "grok" to "Grok",
)
private val PROVIDER_NAMES = mapOf(
    "allternit" to "Allternit Cloud", "anthropic" to "Anthropic", "claude" to "Anthropic", "openai" to "OpenAI", "codex" to "OpenAI",
    "google" to "Google", "gemini" to "Google", "moonshot" to "Moonshot", "kimi" to "Moonshot", "deepseek" to "DeepSeek", "xai" to "xAI",
    "grok" to "xAI", "mistral" to "Mistral", "groq" to "Groq", "openrouter" to "OpenRouter", "ollama" to "Ollama", "omlx" to "oMLX",
    "local-brain" to "Local Brain", "allternit-sidecar" to "Local Brain", "qwen" to "Qwen", "zai" to "ZAI", "meta" to "Meta",
    "ondevice" to "This phone", "claude-org" to "Claude organization",
)
private const val CLAUDE_ORG = "claude-org"

fun providerName(pid: String): String {
    val p = pid.removePrefix("subs-")
    PROVIDER_NAMES[p]?.let { return it }
    if (p.endsWith("-cli")) return cliName(p.removeSuffix("-cli"))
    return p.replaceFirstChar { it.uppercase() }
}

private fun cliName(tool: String): String = when (tool) {
    "claude" -> "Claude Code"
    "codex" -> "Codex"
    "kimi" -> "Kimi CLI"
    "gemini" -> "Gemini CLI"
    else -> "${tool.replaceFirstChar { it.uppercase() }} CLI"
}

/** How a brain is paid for (web `accessOfProvider`): Cloud = credits, local runtimes = local, plans/CLI sign-ins = plan, else key. */
fun accessOfProvider(pid: String): String = when {
    pid == "allternit" -> "credits"
    pid.startsWith("device-") || pid in LOCAL_PROVIDERS -> "local"
    pid == CLAUDE_ORG || pid.startsWith("subs-") || pid.endsWith("-cli") -> "plan"
    else -> "key"
}

/** The sentence's payment phrase (web `accessPhrase`). */
fun accessPhrase(access: String?, pid: String?): String = when (access) {
    "plan" -> when {
        pid == null -> "on your plan"
        pid == CLAUDE_ORG -> "billed to your Claude organization"
        pid.startsWith("subs-") -> PLAN_VENDORS[pid.removePrefix("subs-")]?.let { "on your $it plan" } ?: "on your plan"
        pid.endsWith("-cli") -> "with your ${cliName(pid.removeSuffix("-cli"))} sign-in"
        else -> "with your ${providerName(pid)} sign-in"
    }
    "key" -> "with your API key"
    "local" -> "on this computer"
    "credits" -> "with Allternit credits"
    else -> "with its own default model"
}

internal fun runtimeBrainsLabel(r: RuntimeEntry): String =
    r.brains?.let { list -> "${list.map(::providerName).distinct().joinToString(", ")} models" } ?: "Any brain"

internal fun matches(q: String, vararg parts: String?): Boolean =
    q.isBlank() || parts.filterNotNull().joinToString(" ").lowercase().contains(q.trim().lowercase())

/** "claude-sonnet-5" → "Claude Sonnet 5" (web `readableModelId`). */
fun readableModelId(id: String): String = id.substringAfterLast('/').split('-', '_').filter { it.isNotEmpty() }
    .joinToString(" ") { w -> if (w.lowercase() in setOf("gpt", "glm", "llm")) w.uppercase() else w.replaceFirstChar { it.uppercase() } }

/** The result of a pick: the full new botBrain and agentView, plus the auto-switch note when one fired. */
data class LayersChange(val layer: AgentLayer?, val botBrain: JsonObject, val agentView: JsonObject, val note: String? = null, val botId: String? = null)

private fun JsonObject.without(vararg keys: String) = JsonObject(this.filterKeys { it !in keys })
private fun JsonObject.with(vararg pairs: Pair<String, Any?>): JsonObject {
    val m = toMutableMap()
    for ((k, v) in pairs) when (v) {
        null -> m.remove(k)
        is String -> m[k] = JsonPrimitive(v)
        is JsonObject -> m[k] = v
    }
    return JsonObject(m)
}
private fun modelRefOf(providerId: String, modelId: String) = buildJsonObject { put("providerID", providerId); put("modelID", modelId) }

/**
 * The layers for one bot. [botBrain] / [agentView] are the stored `config` objects (or the form's
 * draft); [brains] is in the caller's order, default model first (the auto-switch takes the first fit).
 */
class AgentLayersModel(
    botBrain: JsonObject,
    private val agentView: JsonObject?,
    val runtimes: List<RuntimeEntry>,
    val brains: List<BrainOption>,
    val bots: List<BotChoice>,
    val botId: String?,
    val botName: String,
    /** Connected bot: Runtime and Brain lock to "Run by <vendor>". */
    val locked: Boolean,
    vendorName: String?,
    val query: String = "",
) {
    val vendor: String = vendorName?.takeIf { it.isNotBlank() } ?: "its vendor"
    val current: JsonObject = resolveRuntime(botBrain, runtimes)
    val runtimeId: String = current.s("runtime") ?: "allternit"
    val runtime: RuntimeEntry? = runtimes.runtime(runtimeId) ?: if (runtimeId == "allternit") BUILT_IN_ALLTERNIT else null
    val runtimeName: String = runtime?.name ?: runtimeId
    private val modelRef = current["modelRef"] as? JsonObject
    val brainProvider: String? = modelRef.s("providerID")
    private val brainModel: String? = modelRef.s("modelID")
    val brainKey: String? = if (brainProvider != null && brainModel != null) "$brainProvider/$brainModel" else null
    val currentBrain: BrainOption? = brains.firstOrNull { it.key == brainKey }
    val brainName: String = currentBrain?.name ?: brainModel?.let(::readableModelId) ?: "Default model"
    private val brainAccess: String? = currentBrain?.access ?: current.s("access") ?: brainProvider?.let(::accessOfProvider)
    val view: String = agentView.s("view") ?: "allternit"
    val look: String = agentView.s("look") ?: runtime?.lookDefault ?: "terminal"
    private fun viewObject(view: String = this.view, look: String = this.look): JsonObject =
        JsonObject((agentView ?: JsonObject(emptyMap())) + mapOf("view" to JsonPrimitive(view), "look" to JsonPrimitive(look)))

    val sentence: String = if (locked) "$botName is run by $vendor. Allternit shows its work and keeps the record."
    else "$botName works with $runtimeName and thinks with $brainName, ${accessPhrase(brainAccess, brainProvider)}."

    fun pickRuntime(r: RuntimeEntry): LayersChange {
        val resolved = resolveRuntime(current.without("runtime").with("runtime" to r.id), runtimes)
        var nextRef: JsonObject? = modelRef
        var access: String? = current.s("access")
        var autoSwitchedFrom: String? = null
        var note: String? = null
        if (brainProvider != null && brainKey != null && !fits(r, brainProvider)) {
            autoSwitchedFrom = brainKey
            val fb = brains.firstOrNull { fits(r, it.providerId) }
            if (fb != null) {
                nextRef = modelRefOf(fb.providerId, fb.modelId)
                access = fb.access
                val provider = if (r.brains == null) fb.providerId else r.brains.firstOrNull() ?: fb.providerId
                note = "${r.name} runs ${providerName(provider)} models only, so the brain changed to ${fb.name}."
            } else {
                nextRef = null
                access = null
                note = "${r.name} can't run $brainName, so it will use its own default model."
            }
        }
        val botBrain = resolved.without("modelRef", "access", "autoSwitchedFrom")
            .with("access" to access, "autoSwitchedFrom" to autoSwitchedFrom, "modelRef" to nextRef)
        return LayersChange(AgentLayer.Runtime, botBrain, viewObject(look = agentView.s("look") ?: r.lookDefault), note)
    }

    fun pickBrain(b: BrainOption): LayersChange {
        val botBrain = current.without("autoSwitchedFrom", "modelRef").with("access" to b.access, "modelRef" to modelRefOf(b.providerId, b.modelId))
        return LayersChange(AgentLayer.Brain, botBrain, viewObject())
    }

    fun pickView(v: String): LayersChange = LayersChange(null, current, viewObject(view = v))

    fun pickBot(id: String): LayersChange = LayersChange(AgentLayer.Bot, current, viewObject(), botId = id)

    // ── Choices ──
    val runtimeChoices: List<LayerChoice> = runtimes
        .filter { !it.more || it.id == runtimeId || query.isNotBlank() }
        .filter { matches(query, it.name, it.vendor, CATEGORY_LABEL[it.category]) }
        .map { r ->
            val badges = buildList {
                if (r.liveUi != null) add("Live app" to BadgeTone.Live)
                add(runtimeBrainsLabel(r) to BadgeTone.Neutral)
                if (r.statusTone == "warn" && r.statusLabel != null) add("Limited" to BadgeTone.Warn)
            }
            LayerChoice(
                id = r.id, name = r.name, detail = r.statusLabel ?: "${CATEGORY_LABEL[r.category] ?: r.category} · ${r.vendor}",
                group = CATEGORY_LABEL[r.category] ?: r.category, mark = r.tool ?: r.id, badges = badges, selected = r.id == runtimeId,
            )
        }

    private val brainMatches = brains.filter { matches(query, it.name, providerName(it.providerId), ACCESS_LABEL[it.access]) }
    val brainChoices: List<LayerChoice> = brainMatches.take(48).map { b ->
        val ok = runtime?.let { fits(it, b.providerId) } ?: true
        val phrase = accessPhrase(b.access, b.providerId)
        val tone = when (b.access) { "plan" -> BadgeTone.Plan; "local" -> BadgeTone.Local; "credits" -> BadgeTone.Credits; else -> BadgeTone.Key }
        LayerChoice(
            id = b.key, name = b.name, detail = if (ok) phrase.replaceFirstChar { it.uppercase() } else "Not with $runtimeName",
            mark = b.providerId, badges = listOf((ACCESS_LABEL[b.access] ?: b.access) to tone), ok = ok, selected = b.key == brainKey,
        )
    }

    val botChoices: List<LayerChoice> = bots
        .filter { matches(query, it.record.displayName, it.record.name, it.record.description) }
        .map { b ->
            LayerChoice(
                id = b.id, name = b.record.displayName,
                detail = if (b.connected) "Run by ${b.vendor ?: "its vendor"}" else b.record.profile.s("tagline") ?: b.record.description ?: "Allternit bot",
                mark = b.id, badges = listOf((if (b.connected) "Connected" else "Allternit") to BadgeTone.Neutral), selected = b.id == botId, bot = b.record,
            )
        }

    fun choices(layer: AgentLayer): List<LayerChoice> = when (layer) {
        AgentLayer.Runtime -> runtimeChoices
        AgentLayer.Brain -> brainChoices
        AgentLayer.Bot -> botChoices
    }

    /** The chip / row value for a layer. */
    fun chipText(layer: AgentLayer): String = when {
        locked && layer != AgentLayer.Bot -> "Run by $vendor"
        layer == AgentLayer.Runtime -> runtimeName
        layer == AgentLayer.Brain -> brainName
        else -> botName
    }
}

/** A Connected bot (contract: `botKind` from GET /agents/:id, else a vendor pack on the profile). */
val BotRecord.isConnected: Boolean get() = raw.s("botKind") == "connected" || vendorPackId != null

/** The vendor name shown in "Run by <vendor>": the binding's vendor, else the pack id, readable. */
fun BotRecord.vendorName(bindingVendor: String? = null): String? =
    bindingVendor?.takeIf { it.isNotBlank() } ?: vendorPackId?.split('-', '_')?.joinToString(" ") { w -> w.replaceFirstChar { it.uppercase() } }

/**
 * The new `config` for a bot after a pick: the stored config with botBrain and agentView replaced and
 * every other key kept. The change's botBrain is built from the stored one, so its unknown keys
 * (nativeSessionId, effort, keys from newer clients) are already inside it.
 */
fun mergedConfig(stored: JsonObject, change: LayersChange): JsonObject =
    JsonObject(stored + mapOf("botBrain" to change.botBrain, "agentView" to change.agentView))

/** Layout changes and the layer open/close snap with the system animator off or the in-app Reduce motion on. */
@Composable
internal fun rememberReduceMotion(): Boolean {
    val context = LocalContext.current
    return remember(context) {
        runCatching { Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }.getOrDefault(false) ||
            runCatching { context.applicationContext.getSharedPreferences("allternit_settings", Context.MODE_PRIVATE).getBoolean("allternit.reduceMotion", false) }.getOrDefault(false)
    }
}
