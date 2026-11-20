package com.allternit.feature.bots

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibraryField
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.LibrarySpinner
import com.allternit.feature.artifacts.PillRow
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import com.allternit.core.design.R as DesignR

/** iOS BotFormData: the fixed choices of the bot form. */
object BotFormData {
    val createAccents = listOf("#B08D6E", "#6366f1", "#8b5cf6", "#ec4899", "#f97316", "#10b981", "#06b6d4", "#3b82f6", "#64748b")
    val editAccents = listOf("#B08D6E", "#6366f1", "#8b5cf6", "#ec4899", "#f43f5e", "#f97316", "#f59e0b", "#10b981", "#06b6d4", "#0ea5e9", "#3b82f6", "#64748b")
    val categories = listOf("research" to "Research", "code" to "Code", "writing" to "Writing", "data" to "Data", "sales" to "Sales", "design" to "Design", "ops" to "Operations", "custom" to "Custom")
    val tools = listOf(
        "web_search" to "Web search", "web_fetch" to "Web fetch", "file_read" to "File read", "file_write" to "File write",
        "code_execution" to "Code execution", "memory" to "Memory", "str_replace_editor" to "Editor", "computer" to "Computer",
    )
    data class DesktopPreset(val id: String, val label: String, val cpu: String, val memory: String, val disk: String) {
        val detail get() = "$cpu vCPU · ${memory.toInt() / 1024} GB RAM · ${disk.toInt() / 1024} GB disk"
    }
    val desktopPresets = listOf(
        DesktopPreset("small", "Small", "2", "4096", "20480"),
        DesktopPreset("medium", "Medium", "4", "8192", "40960"),
        DesktopPreset("large", "Large", "8", "16384", "81920"),
    )
    val harnessModes = listOf("cloud" to "Cloud", "byok" to "BYOK", "local" to "Local", "subprocess" to "Subprocess")
    val providers = listOf("openai" to "OpenAI", "anthropic" to "Anthropic", "google" to "Google", "local" to "Local", "custom" to "Custom")
    val nativeHarnesses = listOf(
        "codex" to "Codex", "claude" to "Claude", "kimi" to "Kimi", "grok" to "Grok", "hermes" to "Hermes", "kiro-cli" to "Kiro",
        "mcode" to "MiniMax Code", "qodercli" to "Qoder", "qoderclicn" to "Qoder CN", "qwenpaw" to "QwenPaw", "reasonix" to "Reasonix", "traecli" to "Trae",
    )
    data class BrainMode(val id: String, val label: String, val hint: String)
    val brainModes = listOf(
        BrainMode("allternit_cloud", "Allternit cloud", "Runs on Allternit cloud. Model is the one you pick below."),
        BrainMode("native_harness", "Native harness", "Resumes a local Codex, Claude, or Kimi session. Does not fall back to cloud."),
        BrainMode("uhp_harness", "UHP harness", "Binds a UHP harness id. Does not fall back to cloud."),
    )

    /** iOS BotFormModel.deriveHandle: lowercase, dashes, at most 64. */
    fun deriveHandle(name: String): String = BotModeStore.slugify(name).take(64).ifEmpty { "my-bot" }

    /** iOS BotSetupMaker.plainBody: a bot with no brain yet (it asks for one on its first turn). */
    fun plainBody(name: String, description: String, systemPrompt: String, tagline: String, welcome: String, color: String, makeMain: Boolean): JsonObject {
        var handle = BotModeStore.slugify(name)
        if (handle.length < 3) handle = "${handle.ifEmpty { "my" }}-bot"
        val profile = buildJsonObject {
            put("displayName", name); put("tagline", tagline); put("welcomeMessage", welcome); put("starterPrompts", JsonArray(emptyList()))
            put("accentColor", color); put("groupChatEnabled", true); put("botCategory", "custom")
            if (makeMain) put("chiefOfStaff", true)
        }
        return buildJsonObject {
            put("name", handle); put("description", description); put("type", "worker"); put("model", ""); put("provider", "")
            put("capabilities", JsonArray(emptyList())); put("tools", JsonArray(emptyList())); put("max_iterations", 10); put("temperature", 0.7)
            put("system_prompt", systemPrompt); put("trust_tier", "standard"); put("enabled_modes", JsonArray(listOf(JsonPrimitive("chat"))))
            put("is_bot", true); put("harness_config", buildJsonObject { put("mode", "cloud") }); put("bot_profile", profile)
            put("config", buildJsonObject { put("isBot", true); put("botProfile", profile) })
        }
    }

    /** iOS BotSetupMaker.needsMain: the first bot made becomes the main bot. */
    suspend fun needsMain(): Boolean = BotsApi.listAgents().orNull()?.none { it.isMainBot && it.isBot } ?: false
}

/** A model the pickers list (iOS ModelStore option). */
private data class ModelOption(val id: String, val name: String, val provider: String, val description: String?)

private suspend fun loadModels(host: BotsHost?): Pair<List<ModelOption>, String?> {
    val api = host?.backend?.api ?: return emptyList<ModelOption>() to null
    return when (val r = api.listModels()) {
        is ApiResult.Ok -> r.value.map { ModelOption(it.id, it.name, it.provider, it.description) } to null
        else -> emptyList<ModelOption>() to "Couldn't load models. Check your connection and try again."
    }
}

/** New bot (iOS NewBotSheet): the bot form in create mode, with vendor bots, templates and Import. */
@Composable
fun NewBotSheet(store: BotModeStore, host: BotsHost? = null, onBrowseTemplates: (() -> Unit)?, onDismiss: () -> Unit, onCreated: (List<String>) -> Unit) {
    BotFormSheet(editBotId = null, host = host, onBrowseTemplates = onBrowseTemplates, onDismiss = onDismiss, onDone = onCreated)
}

/** Edit bot (iOS EditBotSheet). */
@Composable
fun EditBotSheet(botId: String, onDismiss: () -> Unit, onSaved: () -> Unit) {
    BotFormSheet(editBotId = botId, host = null, onBrowseTemplates = null, onDismiss = onDismiss, onDone = { onSaved() })
}

/**
 * The bot form (iOS BotFormSheet + BotFormModel): name, accent, then "More options" (role,
 * job & tools, computer & runtime, intelligence, advanced) when creating; the editable fields when
 * editing. Create sends the full agent body (createBody); Save sends profile, config and prompt.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun BotFormSheet(editBotId: String?, host: BotsHost?, onBrowseTemplates: (() -> Unit)?, onDismiss: () -> Unit, onDone: (List<String>) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val editing = editBotId != null
    var displayName by remember { mutableStateOf("") }
    var handle by remember { mutableStateOf("") }
    var tagline by remember { mutableStateOf("") }
    var description by remember { mutableStateOf("") }
    var systemPrompt by remember { mutableStateOf("") }
    var welcome by remember { mutableStateOf("") }
    var accent by remember { mutableStateOf("#B08D6E") }
    var category by remember { mutableStateOf("custom") }
    val tools = remember { mutableStateListOf<String>() }
    var desktopEnabled by remember { mutableStateOf(false) }
    var desktopPreset by remember { mutableStateOf("small") }
    var brainMode by remember { mutableStateOf("allternit_cloud") }
    var harnessId by remember { mutableStateOf("codex") }
    var nativeSessionId by remember { mutableStateOf("") }
    var uhpHarnessId by remember { mutableStateOf("") }
    var modelId by remember { mutableStateOf("") }
    var provider by remember { mutableStateOf("anthropic") }
    var harnessMode by remember { mutableStateOf("cloud") }
    var maxIterations by remember { mutableStateOf(10) }
    var temperature by remember { mutableStateOf(0.7f) }
    var knowledgeBrains by remember { mutableStateOf<List<Pair<String, String>>>(emptyList()) }
    var knowledgeBrainId by remember { mutableStateOf("") }
    var quotaLine by remember { mutableStateOf<String?>(null) }
    var storedProfile by remember { mutableStateOf(JsonObject(emptyMap())) }
    var storedConfig by remember { mutableStateOf(JsonObject(emptyMap())) }
    var loading by remember { mutableStateOf(editing) }
    var loadError by remember { mutableStateOf<String?>(null) }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var moreOpen by remember { mutableStateOf(false) }
    var advancedOpen by remember { mutableStateOf(false) }
    var modelPickerOpen by remember { mutableStateOf(false) }
    var vendorOpen by remember { mutableStateOf(false) }
    var importOpen by remember { mutableStateOf(false) }
    val look = remember { BotLookModel() }

    suspend fun loadBot() {
        val id = editBotId ?: return
        loading = true
        loadError = null
        when (val r = BotsApi.getAgent(id)) {
            is BotsResult.Ok -> {
                val b = r.value
                storedProfile = b.profile ?: JsonObject(emptyMap())
                storedConfig = b.config
                handle = b.name
                displayName = b.displayName
                tagline = storedProfile.str("tagline").orEmpty()
                category = storedProfile.str("botCategory") ?: "custom"
                accent = storedProfile.str("accentColor") ?: "#B08D6E"
                welcome = storedProfile.str("welcomeMessage").orEmpty()
                description = b.description.orEmpty()
                systemPrompt = b.systemPrompt.orEmpty()
                tools.clear()
                val allowed = (b.raw["allowed_tools"] as? JsonArray ?: b.raw["tools"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content }.orEmpty()
                tools.addAll(allowed)
                // The Look editor opens on what the bot has saved (iOS formFromBot).
                look.configure(b.config["avatar"], accent, displayName, VendorGroupTurn.binding(id)?.takeIf { it.isVendor }?.vendor)
            }
            is BotsResult.Failed -> loadError = r.message
        }
        loading = false
    }
    LaunchedEffect(editBotId) {
        if (editing) loadBot() else {
            look.configure(null, accent, "", null)
            // The wizard's load: knowledge brains and the desktop quota (failures leave them empty).
            (LibraryNet.get("brains") as? ApiResult.Ok)?.value?.let { j ->
                knowledgeBrains = j.listUnder("brains").mapNotNull { o ->
                    val id = o.str("brain_id") ?: return@mapNotNull null
                    val clone = o.str("clone_url")?.replace(".git", "")?.split("/")?.takeLast(2)
                    id to (if (clone != null && clone.size == 2) clone.joinToString("/") else if (id.length <= 16) id else "${id.take(8)}…${id.takeLast(8)}")
                }
                if (knowledgeBrainId.isEmpty()) knowledgeBrains.firstOrNull()?.let { knowledgeBrainId = it.first }
            }
            (LibraryNet.get("computers/quota") as? ApiResult.Ok)?.value?.obj?.let { q ->
                quotaLine = if (q.bool("allowed") == false) q.str("reason") ?: "Desktop quota reached — this bot may not be able to provision."
                else (q["active_limit"] as? JsonPrimitive)?.content?.let { limit -> "Your desktop quota: ${(q["active"] as? JsonPrimitive)?.content ?: "0"} of $limit desktops in use" }
            }
        }
    }

    val nameValid = displayName.trim().length >= 2
    val brainMissing = brainMode == "allternit_cloud" && modelId.isEmpty() && !editing
    val uhpMissing = brainMode == "uhp_harness" && uhpHarnessId.isBlank()
    val canSave = host?.signedIn != false && nameValid && !saving && !look.busy && !look.blocked && (editing || (!brainMissing && !uhpMissing))

    fun createBody(makeMain: Boolean): JsonObject {
        val name = displayName.trim()
        val base = BotFormData.deriveHandle(name)
        val h = if (base.length >= 3) base else "$base-bot"
        val about = description.trim()
        val profile = buildJsonObject {
            put("displayName", name); put("tagline", tagline.trim()); put("welcomeMessage", welcome); put("starterPrompts", JsonArray(emptyList()))
            put("accentColor", accent); put("groupChatEnabled", true); put("botCategory", category)
            if (makeMain) put("chiefOfStaff", true)
        }
        val botBrain = buildJsonObject {
            put("mode", brainMode)
            if (brainMode == "native_harness") { put("harness", harnessId); if (nativeSessionId.isNotBlank()) put("sessionId", nativeSessionId.trim()) }
            if (brainMode == "uhp_harness") put("harnessId", uhpHarnessId.trim())
        }
        val preset = BotFormData.desktopPresets.first { it.id == desktopPreset }
        val vm = buildJsonObject {
            put("enabled", desktopEnabled)
            if (desktopEnabled) { put("cpu", preset.cpu); put("memory", preset.memory); put("disk", preset.disk); put("persistent", true) }
        }
        val config = buildJsonObject {
            // The Look editor's avatar; the wizard's Gizzi default when the looks couldn't load.
            put("avatar", look.avatar ?: buildJsonObject { put("type", "mascot"); put("mascotTemplate", "gizzi") })
            put("isBot", true); put("botProfile", profile); put("botBrain", botBrain); put("vmOperator", vm)
            if (knowledgeBrainId.isNotEmpty()) put("brainId", knowledgeBrainId)
        }
        return buildJsonObject {
            put("name", h)
            put("description", tagline.trim().ifEmpty { about.ifEmpty { "$name is a custom Allternit bot." } })
            put("type", "worker"); put("model", modelId); put("provider", provider)
            put("capabilities", JsonArray(emptyList())); put("system_prompt", systemPrompt); put("tools", JsonArray(emptyList()))
            put("max_iterations", maxIterations); put("temperature", (temperature * 10).toInt() / 10.0)
            put("config", config); put("trust_tier", "standard")
            put("harness_config", buildJsonObject { put("mode", harnessMode) })
            put("enabled_modes", JsonArray(listOf(JsonPrimitive("chat")))); put("allowed_skills", JsonArray(emptyList()))
            put("allowed_tools", JsonArray(tools.map { JsonPrimitive(it) })); put("category", "general"); put("tags", JsonArray(emptyList()))
            put("data_classification", "internal"); put("write_scope", "workspace"); put("is_bot", true); put("bot_profile", profile); put("vm_operator", vm)
        }
    }

    fun submit() {
        if (!canSave) {
            error = when {
                !nameValid -> "Add a display name (at least 2 characters) to continue."
                uhpMissing -> "Add the UHP harness id, or pick another brain."
                brainMissing -> "Pick a brain first."
                else -> null
            }
            return
        }
        saving = true
        error = null
        scope.launch {
            if (editBotId != null) {
                val profile = JsonObject(storedProfile + mapOf(
                    "displayName" to JsonPrimitive(displayName.trim()), "tagline" to JsonPrimitive(tagline), "welcomeMessage" to JsonPrimitive(welcome),
                    "accentColor" to JsonPrimitive(accent), "botCategory" to JsonPrimitive(category),
                ))
                val body = buildJsonObject {
                    put("system_prompt", systemPrompt); put("allowed_tools", JsonArray(tools.map { JsonPrimitive(it) }))
                    // An untouched look stays as saved.
                    val lookAvatar = look.avatar?.takeIf { look.dirty }
                    put("bot_profile", profile); put("config", JsonObject(storedConfig + ("botProfile" to profile) + (if (lookAvatar != null) mapOf("avatar" to lookAvatar) else emptyMap())))
                    (tagline.trim().ifEmpty { description.trim() }).takeIf { it.isNotEmpty() }?.let { put("description", it) }
                }
                when (val r = BotsApi.updateAgent(editBotId, body)) {
                    is BotsResult.Ok -> onDone(listOf(editBotId))
                    is BotsResult.Failed -> error = "Failed to update bot. ${r.message}"
                }
            } else {
                val makeMain = BotFormData.needsMain()
                when (val r = BotsApi.createAgent(createBody(makeMain))) {
                    is BotsResult.Ok -> { look.syncAsset(r.value); onDone(listOf(r.value)) }
                    is BotsResult.Failed -> error = "Couldn’t create the bot: ${r.message}"
                }
            }
            saving = false
        }
    }

    LibrarySheet(
        title = if (editing) "Edit bot" else "Create bot", testTag = if (editing) "editBotSheet" else "newBotSheet", onDismiss = onDismiss,
        leading = LibrarySheetAction("Cancel", enabled = !saving, testTag = "botFormCancel", onClick = onDismiss),
        trailing = LibrarySheetAction(if (editing) (if (saving) "Saving…" else "Save bot") else (if (saving) "Creating…" else "Create"), enabled = canSave || !saving, bold = true, testTag = "botFormSave", onClick = ::submit),
    ) {
        Column(Modifier.fillMaxWidth().heightIn(max = 680.dp).verticalScroll(rememberScrollState()).imePadding().padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            if (host?.signedIn == false) Text(if (editing) "Sign in to edit this bot." else "Sign in to make a bot.", color = c.textSecondary, fontSize = 14.sp)
            (error ?: look.error)?.let { ErrorLine(it, Modifier.padding(horizontal = 0.dp)) }
            when {
                loading -> LibrarySpinner("Loading the bot")
                loadError != null -> ListState(false, loadError, true, "Couldn't load this bot", "") { scope.launch { loadBot() } }
                else -> {
                    LibraryField("", displayName, "Name your bot", "botFormName") { displayName = it; look.push(displayName = it) }
                    if (editing && handle.isNotEmpty()) Text("Handle: @$handle", color = c.textTertiary, fontSize = 12.sp)
                    AccentRow(if (editing) BotFormData.editAccents else BotFormData.createAccents, accent) { accent = it; look.push(accent = it) }
                    Label("Look")
                    BotLookEditor(look)
                    if (editing) {
                        LibraryField("Tagline", tagline, "What it does, in a line", "botFormTagline") { tagline = it }
                        LibraryField("Welcome message", welcome, "Shown at the top of a fresh chat", "botFormWelcome", singleLine = false, minHeight = 64) { welcome = it }
                        LibraryField("Job instructions", systemPrompt, "Your job: …", "botFormPrompt", singleLine = false, minHeight = 140) { systemPrompt = it }
                        ToolsChips(tools)
                    } else {
                        Row(Modifier.clickable { moreOpen = !moreOpen }.testTag("botFormMore").padding(vertical = 6.dp)) {
                            Text("More options", color = c.textSecondary, fontSize = 14.sp)
                            Text(" · accent, role, brain, computer", color = c.textTertiary, fontSize = 14.sp)
                        }
                        if (moreOpen) Column(verticalArrangement = Arrangement.spacedBy(18.dp)) {
                            Label("What it does")
                            PillRow(BotFormData.categories, BotFormData.categories.first { it.first == category }, { it.second }, { "botFormCategory${it.first}" }) { category = it.first }
                            LibraryField("", description, "e.g. Works out unit economics and pricing, and runs the monthly close. Reads Stripe and our sheets. Never moves money or emails customers.", "botFormDescription", singleLine = false, minHeight = 80) { description = it }
                            Note("We draft its instructions and tools from this. You can edit both later.")
                            SectionTitle("Job & Tools", "What this bot does, and what it is allowed to use. Saved with the bot when you create it.")
                            LibraryField("Job instructions", systemPrompt, "Your job: …\n\nYou do:\n- …\n\nYou do not:\n- …", "botFormPrompt", singleLine = false, minHeight = 160) { systemPrompt = it }
                            Note("This becomes the bot's system prompt. State the job plainly — what it does and what it does not do. Optional: the platform adds identity and computer context at runtime.")
                            ToolsChips(tools)
                            SectionTitle("Computer & Runtime", "Your bot gets its own always-on desktop in Computer Cloud. Files, tools, and browser state persist between sessions.")
                            ToggleRow("Persistent desktop", "Provisioned when you create the bot, bound to it by id. The same computer is reused every time you open the bot.", desktopEnabled, "botFormDesktop") { desktopEnabled = it }
                            if (desktopEnabled) {
                                Label("Size")
                                Note("Bigger desktops cost more disk on the host — each bot's allocation counts against how many bots the machine can hold. Start small and resize later if the bot outgrows it.")
                                BotFormData.desktopPresets.forEach { p -> ChoiceRow(p.label, p.detail, desktopPreset == p.id, "botFormDesktop${p.id}") { desktopPreset = p.id } }
                                quotaLine?.let { Note(it) }
                                Note("The desktop starts provisioning when you hit Create bot. You can watch its status here and on the bot card.")
                            }
                            SectionTitle("Intelligence", "Pick how this bot runs and the model it uses.")
                            Label("Brain")
                            Note("How this bot runs. Separate from the Gizzi knowledge brain.")
                            BotFormData.brainModes.forEach { m -> ChoiceRow(m.label, m.hint, brainMode == m.id, "botFormBrain${m.id}") { brainMode = m.id } }
                            if (brainMissing) ErrorLine("Pick a model below to create this bot", Modifier.padding(0.dp))
                            if (brainMode == "native_harness") {
                                Label("Native harness")
                                PillRow(BotFormData.nativeHarnesses, BotFormData.nativeHarnesses.first { it.first == harnessId }, { it.second }, { "botFormHarness${it.first}" }) { harnessId = it.first }
                                LibraryField("Native session id", nativeSessionId, "Leave empty to bind on first start", "botFormNativeSession") { nativeSessionId = it }
                            }
                            if (brainMode == "uhp_harness") {
                                LibraryField("UHP harness id", uhpHarnessId, "e.g. uhp-claude-desktop-01", "botFormUhp") { uhpHarnessId = it }
                                if (uhpMissing) ErrorLine("Required to start this bot", Modifier.padding(0.dp)) else Note("Bound when you save. Does not fall back to cloud.")
                            }
                            Label("Gizzi knowledge")
                            if (knowledgeBrains.isEmpty()) Note("No Gizzi knowledge brains found. Select a platform model below.")
                            else PillRow(listOf("" to "Platform model") + knowledgeBrains, (listOf("" to "Platform model") + knowledgeBrains).first { it.first == knowledgeBrainId }, { it.second }, { "botFormKnowledge${it.first}" }) { knowledgeBrainId = it.first }
                            Label("Model")
                            ChoiceRow(if (modelId.isEmpty()) "Pick a brain" else "$provider/$modelId", null, false, "botFormModel") { modelPickerOpen = true }
                            Label("Provider")
                            PillRow(BotFormData.providers, BotFormData.providers.firstOrNull { it.first == provider } ?: BotFormData.providers.first(), { it.second }, { "botFormProvider${it.first}" }) { provider = it.first }
                            Label("Harness")
                            PillRow(BotFormData.harnessModes, BotFormData.harnessModes.first { it.first == harnessMode }, { it.second }, { "botFormHarnessMode${it.first}" }) { harnessMode = it.first }
                            Text(if (advancedOpen) "▾ Advanced" else "▸ Advanced", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium, modifier = Modifier.clickable { advancedOpen = !advancedOpen }.testTag("botFormAdvanced"))
                            if (advancedOpen) {
                                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    Text("Max iterations: $maxIterations", color = c.textPrimary, fontSize = 14.sp, modifier = Modifier.weight(1f))
                                    Pill("−", "botFormIterLess", enabled = maxIterations > 1) { maxIterations-- }
                                    Pill("+", "botFormIterMore", enabled = maxIterations < 50) { maxIterations++ }
                                }
                                Note("Tool calls a bot may take on one task.")
                                Text("Temperature: ${String.format(java.util.Locale.US, "%.1f", temperature)}", color = c.textPrimary, fontSize = 14.sp)
                                Slider(temperature, { temperature = (it * 10).toInt() / 10f }, valueRange = 0f..2f, steps = 19, modifier = Modifier.testTag("botFormTemperature").semantics { contentDescription = "Temperature" })
                            }
                            RowGroup(Modifier.padding(horizontal = 0.dp)) {
                                NavRow("Bring a vendor’s bot instead", DesignR.drawable.ph_plugs_connected, null, "botFormVendor", last = false) { vendorOpen = true }
                                NavRow("Import a bot", DesignR.drawable.ph_upload_simple, null, "botFormImport", last = onBrowseTemplates == null) { importOpen = true }
                                if (onBrowseTemplates != null) NavRow("Browse team templates", R.drawable.bots_grid, null, "botFormTemplates") { onBrowseTemplates() }
                            }
                        }
                    }
                }
            }
            Gap(12.dp)
        }
    }
    if (modelPickerOpen) ModelPickerSheet(host, "Model", "$provider/$modelId", onDismiss = { modelPickerOpen = false }) { m ->
        val parts = m.id.split("/", limit = 2)
        modelId = if (parts.size == 2) parts[1] else m.id
        provider = if (parts.size == 2) parts[0] else m.provider
        modelPickerOpen = false
    }
    if (vendorOpen) VendorConnectSheet(offerMain = false, onDismiss = { vendorOpen = false }) { first, main ->
        vendorOpen = false
        onDone(listOfNotNull(main ?: first))
    }
    if (importOpen) ImportBotSheet(onDismiss = { importOpen = false }, backend = host?.backend) { ids -> importOpen = false; onDone(ids) }
}

@Composable private fun Label(t: String) = Text(t, color = AllternitTheme.colors.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
@Composable private fun Note(t: String) = Text(t, color = AllternitTheme.colors.textTertiary, fontSize = 12.sp)

@Composable
private fun SectionTitle(title: String, sub: String) = Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
    Text(title, color = AllternitTheme.colors.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
    Text(sub, color = AllternitTheme.colors.textSecondary, fontSize = 12.5.sp)
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun AccentRow(colors: List<String>, selected: String, onPick: (String) -> Unit) {
    val c = AllternitTheme.colors
    FlowRow(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        colors.forEach { hex ->
            val on = hex.equals(selected, ignoreCase = true)
            Box(
                Modifier.size(30.dp).clip(CircleShape).background(hexColor(hex)).border(if (on) 3.dp else 0.dp, c.textPrimary, CircleShape)
                    .testTag("botFormAccent$hex").clickable(role = Role.RadioButton) { onPick(hex) }.semantics { contentDescription = "Accent $hex" + if (on) ", selected" else "" },
            )
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ToolsChips(tools: MutableList<String>) {
    val c = AllternitTheme.colors
    Label("Tools")
    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        BotFormData.tools.forEach { (id, label) ->
            val on = id in tools
            Text(
                (if (on) "✓ " else "") + label, color = if (on) c.bgPrimary else c.textPrimary, fontSize = 13.sp,
                modifier = Modifier.clip(CircleShape).background(if (on) c.textPrimary else c.fillNeutral).testTag("botFormTool$id")
                    .clickable(role = Role.Checkbox) { if (on) tools.remove(id) else tools.add(id) }.padding(horizontal = 12.dp, vertical = 7.dp),
            )
        }
    }
    val extra = tools.count { id -> BotFormData.tools.none { it.first == id } }
    Note(when {
        tools.isEmpty() -> "No tools — chat only"
        extra > 0 -> "Also on from the template: $extra more tool${if (extra == 1) "" else "s"}."
        else -> "What this bot is allowed to use. Saved with the bot when you create it."
    })
}

@Composable
private fun ChoiceRow(title: String, detail: String?, on: Boolean, testTag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPanel).border(BorderStroke(if (on) 1.5.dp else 1.dp, if (on) c.textPrimary else c.borderSubtle), RoundedCornerShape(12.dp))
            .testTag(testTag).clickable(role = Role.RadioButton, onClick = onClick).padding(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
            detail?.let { Text(it, color = c.textSecondary, fontSize = 12.sp) }
        }
        if (on) Text("✓", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Bold)
    }
}

/** The model list grouped by provider (iOS BotFormSheet modelPicker / BotBrainPickerSheet list). */
@Composable
private fun ModelPickerSheet(host: BotsHost?, title: String, currentId: String?, saving: Boolean = false, saveError: String? = null, onSave: ((ModelOption) -> Unit)? = null, onDismiss: () -> Unit, onPick: (ModelOption) -> Unit) {
    val c = AllternitTheme.colors
    var models by remember { mutableStateOf<List<ModelOption>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var selection by remember { mutableStateOf(currentId) }
    LaunchedEffect(Unit) {
        val (m, e) = loadModels(host)
        models = m; error = e; loading = false
    }
    LibrarySheet(
        title = title, testTag = "botModelPicker", onDismiss = onDismiss,
        leading = LibrarySheetAction(if (onSave != null) "Cancel" else "Close", onClick = onDismiss),
        trailing = onSave?.let { save -> LibrarySheetAction(if (saving) "Saving…" else "Save", enabled = !saving && selection != currentId && models.any { it.id == selection }, bold = true, testTag = "botModelSave") { models.firstOrNull { it.id == selection }?.let(save) } },
    ) {
        Column(Modifier.fillMaxWidth().heightIn(max = 600.dp).verticalScroll(rememberScrollState()).padding(bottom = 20.dp)) {
            when {
                loading -> LibrarySpinner("Loading models")
                error != null && models.isEmpty() -> Text(error ?: "", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(20.dp))
                models.isEmpty() -> Text("No models yet. Connect a model provider in Settings.", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(20.dp))
                else -> models.groupBy { it.provider }.forEach { (prov, list) ->
                    SectionHeader(prov)
                    RowGroup {
                        list.forEachIndexed { i, m ->
                            Row(
                                Modifier.fillMaxWidth().testTag("botModel${m.id}").clickable { selection = m.id; if (onSave == null) onPick(m) }.padding(horizontal = 16.dp, vertical = 10.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Column(Modifier.weight(1f)) {
                                    Text(m.name, color = c.textPrimary, fontSize = 15.sp)
                                    m.description?.let { Text(it, color = c.textSecondary, fontSize = 12.sp, maxLines = 2) }
                                }
                                if (selection == m.id) Text("✓", color = c.textPrimary, fontWeight = FontWeight.Bold)
                            }
                            if (i < list.lastIndex) Hairline()
                        }
                    }
                }
            }
            saveError?.let { ErrorLine(it) }
        }
    }
}

/** Pick a Brain (iOS BotBrainPickerSheet): saves the bot's model and provider. */
@Composable
fun BotBrainPickerSheet(bot: BotRecord, host: BotsHost?, onDismiss: () -> Unit, onSaved: () -> Unit) {
    val scope = rememberCoroutineScope()
    var saving by remember { mutableStateOf(false) }
    var saveError by remember { mutableStateOf<String?>(null) }
    val current = if (bot.model.isNotEmpty() && bot.provider.isNotEmpty()) "${bot.provider}/${bot.model}" else null
    ModelPickerSheet(host, "Pick a Brain", current, saving, saveError, onSave = { m ->
        saving = true
        saveError = null
        scope.launch {
            val short = m.id.substringAfter('/')
            when (val r = BotsApi.updateAgent(bot.id, buildJsonObject { put("model", short); put("provider", m.provider) })) {
                is BotsResult.Ok -> onSaved()
                is BotsResult.Failed -> saveError = r.message
            }
            saving = false
        }
    }, onDismiss = onDismiss, onPick = {})
}

/** New from Template (iOS AgentTemplateSheet): pick one, confirm, create with the default model. */
@Composable
fun AgentTemplateSheet(onDismiss: () -> Unit, onConfirm: (AgentTemplate) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var templates by remember { mutableStateOf<List<AgentTemplate>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var pending by remember { mutableStateOf<AgentTemplate?>(null) }
    suspend fun load() {
        loading = true
        when (val r = BotsApi.listTemplates()) {
            is BotsResult.Ok -> { templates = r.value; error = null }
            is BotsResult.Failed -> error = r.message
        }
        loading = false
    }
    LaunchedEffect(Unit) { load() }
    LibrarySheet(title = "New from Template", testTag = "agentTemplateSheet", onDismiss = onDismiss, leading = LibrarySheetAction("Close", onClick = onDismiss)) {
        Column(Modifier.fillMaxWidth().heightIn(max = 600.dp).verticalScroll(rememberScrollState()).padding(vertical = 8.dp)) {
            if (!ListState(loading, error, templates.isEmpty(), "Couldn't load templates", "No templates yet.") { scope.launch { load() } }) {
                RowGroup {
                    templates.forEachIndexed { i, t ->
                        Column(Modifier.fillMaxWidth().testTag("agentTemplate${t.id}").clickable { pending = t }.padding(horizontal = 16.dp, vertical = 10.dp)) {
                            Text(t.name, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                            if (t.description.isNotEmpty()) Text(t.description, color = c.textSecondary, fontSize = 12.5.sp, maxLines = 2)
                        }
                        if (i < templates.lastIndex) Hairline()
                    }
                }
            }
        }
    }
    pending?.let { t ->
        LibrarySheet(title = "Create ${t.name}?", testTag = "agentTemplateConfirm", onDismiss = { pending = null }, leading = LibrarySheetAction("Cancel") { pending = null }, trailing = LibrarySheetAction("Create", bold = true, testTag = "agentTemplateCreate") { pending = null; onConfirm(t) }) {
            Text("This creates a new agent from the template with your default model.", color = c.textSecondary, fontSize = 15.sp, modifier = Modifier.padding(20.dp))
        }
    }
}

/** A vendor product (iOS ConnectProduct, `gateway/connect`). */
private data class ConnectProduct(val product: String, val name: String, val offers: String, val connected: Boolean, val bots: List<Pair<String, String>>, val signInText: String, val terms: String?, val verifiedLive: Boolean)

private fun connectProduct(o: JsonObject): ConnectProduct? {
    val id = o.str("product") ?: o.str("id") ?: return null
    return ConnectProduct(
        id, o.str("name") ?: id, o.str("offers").orEmpty(), o.str("status") == "ready" || o.bool("connected") == true,
        o.list("bots").mapNotNull { b -> (b.str("botId") ?: return@mapNotNull null) to (b.str("name") ?: "Bot") },
        o.str("signIn") ?: o.str("signInText") ?: "Sign in on your computer", o.str("terms"), o.bool("verifiedLive") == true,
    )
}

/**
 * Bring a vendor's bot (iOS VendorConnectView + VendorProductView): the products your computer can
 * connect, then one-step connect with sign-in polling, consent, extra fields and "Make it my main bot".
 */
@Composable
fun VendorConnectSheet(offerMain: Boolean, onDismiss: () -> Unit, onConnected: (firstBotId: String?, mainBotId: String?) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var products by remember { mutableStateOf<List<ConnectProduct>?>(null) }
    var loadError by remember { mutableStateOf<String?>(null) }
    var unsupported by remember { mutableStateOf(false) }
    var open by remember { mutableStateOf<ConnectProduct?>(null) }
    suspend fun load() {
        loadError = null
        when (val r = LibraryNet.get("gateway/connect")) {
            is ApiResult.Ok -> products = r.value.obj.list("products").mapNotNull(::connectProduct)
            is ApiResult.HttpError -> if (r.status == 404) unsupported = true else loadError = r.friendly("connect a bot")
            else -> loadError = r.friendly("connect a bot")
        }
    }
    LaunchedEffect(Unit) { load() }
    val p = open
    if (p != null) { VendorProductSheet(p, offerMain, onDismiss = { open = null }, onConnected = onConnected); return }
    LibrarySheet(title = "Connect a vendor bot", testTag = "vendorConnectSheet", onDismiss = onDismiss, leading = LibrarySheetAction("Close", onClick = onDismiss)) {
        Column(Modifier.fillMaxWidth().heightIn(max = 620.dp).verticalScroll(rememberScrollState()).padding(vertical = 8.dp)) {
            val list = products
            when {
                unsupported -> Text("One-step connect needs a newer Allternit on your computer. Update it, then try again.", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(20.dp))
                loadError != null -> ListState(false, "Vendor bots connect through it. $loadError", true, "Couldn’t reach your computer", "") { scope.launch { load() } }
                list == null -> LibrarySpinner("Loading")
                else -> {
                    RowGroup {
                        list.forEachIndexed { i, prod ->
                            Column(Modifier.fillMaxWidth().testTag("vendor${prod.product}").clickable { open = prod }.padding(horizontal = 16.dp, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                                Text(prod.name, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                                Text(prod.offers, color = c.textSecondary, fontSize = 12.5.sp, maxLines = 2)
                                Text(if (prod.connected) "Connected · ${prod.bots.size} bot${if (prod.bots.size == 1) "" else "s"}" else prod.signInText, color = c.textTertiary, fontSize = 12.sp)
                            }
                            if (i < list.lastIndex) Hairline()
                        }
                    }
                    Text("Use a subscription you already pay for. Connect once and every chat, GPT, Project, Gem or bot you have there becomes an Allternit bot.", color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.padding(20.dp))
                }
            }
        }
    }
}

@Composable
private fun VendorProductSheet(p: ConnectProduct, offerMain: Boolean, onDismiss: () -> Unit, onConnected: (String?, String?) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var result by remember { mutableStateOf<JsonObject?>(null) }
    var busy by remember { mutableStateOf(false) }
    var waiting by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var makeMain by remember { mutableStateOf(offerMain) }
    val fields = remember { mutableStateOf(mapOf<String, String>()) }
    val status = result.str("status")
    val extras: List<Triple<String, String, Boolean>> = when {
        p.product == "claude-managed-agents" && status != "ready" -> listOf(Triple("apiKey", "Anthropic API key", true))
        p.product == "muse" && status != "ready" -> listOf(Triple("agentAddress", "Muse WhatsApp number", false), Triple("phoneNumberId", "WhatsApp phone number ID", false), Triple("accessToken", "WhatsApp access token", true), Triple("appSecret", "Meta app secret", true))
        else -> emptyList()
    }
    val bots = result.list("bots").mapNotNull { b -> (b.str("botId") ?: return@mapNotNull null) to (b.str("name") ?: "Bot") }.ifEmpty { p.bots }
    fun run(acceptTerms: Boolean = false, polls: Int = 0) {
        busy = true
        error = null
        scope.launch {
            val body = buildJsonObject {
                put("product", p.product)
                if (acceptTerms) put("acceptTerms", true)
                if (makeMain) put("makeMain", true)
                fields.value.filterValues { it.isNotBlank() }.takeIf { it.isNotEmpty() }?.let { f -> put("fields", buildJsonObject { f.forEach { (k, v) -> put(k, v) } }) }
            }
            when (val r = LibraryNet.send("POST", "gateway/connect", body)) {
                is ApiResult.Ok -> {
                    val o = r.value.obj
                    result = o
                    when (o.str("status")) {
                        "ready" -> { waiting = false; val ids = o.list("bots").mapNotNull { it.str("botId") }; onConnected(ids.firstOrNull(), if (makeMain) o.str("mainBotId") ?: ids.firstOrNull() else null) }
                        "needs_sign_in" -> if (polls < 100) { waiting = true; busy = false; delay(3_000); run(polls = polls + 1); return@launch }
                    }
                }
                else -> error = r.friendly("connect ${p.name}")
            }
            busy = false
        }
    }
    LibrarySheet(
        title = p.name, testTag = "vendorProductSheet", onDismiss = onDismiss, leading = LibrarySheetAction("Close", onClick = onDismiss),
        trailing = LibrarySheetAction(if (status == "needs_consent") "Agree & connect" else if (busy) "Connecting…" else "Connect", enabled = !busy && status != "ready", bold = true, testTag = "vendorConnect") { run(acceptTerms = status == "needs_consent") },
    ) {
        Column(Modifier.fillMaxWidth().heightIn(max = 620.dp).verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text(p.offers, color = c.textSecondary, fontSize = 14.sp)
            if (!p.verifiedLive) Note("Not yet verified live — no real account has been run through this connection yet.")
            result?.let { r ->
                Label(r.str("statusText") ?: (status ?: "").replace('_', ' ').replaceFirstChar { it.uppercase() })
                val msg = if (status == "needs_sign_in") "A ${p.name} window is open on your computer. Sign in there; this checks again on its own." else r.str("message").orEmpty()
                if (msg.isNotEmpty()) Text(msg, color = c.textSecondary, fontSize = 13.sp)
                if (status != "ready") (r["steps"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content }?.forEachIndexed { i, s -> Note("${i + 1}. $s") }
                if (waiting) LibrarySpinner("Waiting for sign-in")
            }
            if (status == "needs_consent") (result.str("terms") ?: p.terms)?.let { Label("Before you connect"); Note(it) }
            extras.forEach { (key, label, _) -> LibraryField(label, fields.value[key].orEmpty(), label, "vendorField$key") { fields.value = fields.value + (key to it) } }
            if (bots.isNotEmpty()) { Label("Bots"); bots.forEach { Text("• ${it.second}", color = c.textPrimary, fontSize = 14.sp) } }
            if (offerMain && status != "ready") ToggleRow("Make it my main bot", null, makeMain, "vendorMakeMain") { makeMain = it }
            error?.let { ErrorLine(it, Modifier.padding(0.dp)) }
        }
    }
}

/** A bot from a team file (iOS ImportedBotSpec). */
private data class ImportedBot(val name: String, val description: String, val systemPrompt: String, val tagline: String, val welcome: String, val color: String)

/** iOS TeamFileParser.parse: openmaus package / team, `{bots}`, or a single bot. */
private fun parseTeam(text: String): Pair<String, List<ImportedBot>> {
    val json = runCatching { com.allternit.core.api.ApiJson.default.parseToJsonElement(text) as? JsonObject }.getOrNull() ?: throw IllegalArgumentException("This isn't a team file or bot package.")
    fun s(o: JsonObject?, k: String) = o.str(k)?.trim().orEmpty()
    fun member(m: JsonObject): ImportedBot? {
        val name = s(m, "displayName").ifEmpty { s(m, "name") }.ifEmpty { return null }
        val title = s(m, "title")
        val color = s(m.child("appearance"), "color").ifEmpty { s(m, "accentColor").ifEmpty { "#B08D6E" } }
        return ImportedBot(name, s(m, "description").ifEmpty { title }, s(m, "systemPrompt"), s(m, "tagline").ifEmpty { title }, s(m, "welcomeMessage"), color)
    }
    val format = s(json, "format")
    val (name, bots) = when {
        format == "openmaus.package" && json.child("package") != null -> s(json.child("package"), "name") to json.child("package").list("agents").mapNotNull(::member)
        format == "openmaus.team" && json.child("team") != null -> s(json.child("team"), "name") to json.child("team").list("members").mapNotNull(::member)
        json["bots"] is JsonArray -> s(json, "name") to json.list("bots").mapNotNull(::member)
        else -> member(json)?.let { it.name to listOf(it) } ?: throw IllegalArgumentException("This file has no bots in it.")
    }
    if (bots.isEmpty()) throw IllegalArgumentException("This team file has no bots in it.")
    if (bots.size > 200) throw IllegalArgumentException("A team file can have at most 200 bots.")
    return name.ifEmpty { "Imported team" } to bots
}

/**
 * Import a bot (iOS ImportBotSheet): an Allternit team file (.json) or a bot package (.zip), from a
 * file or a link. Team bots are created like plain bots; a package's memory, skills, tasks and docs
 * are copied to the computer (BotZip). The first bot becomes the main bot when the account has none.
 */
@Composable
fun ImportBotSheet(onDismiss: () -> Unit, backend: com.allternit.feature.chat.ChatBackend? = null, onImported: (List<String>) -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var source by remember { mutableStateOf("file") }
    var link by remember { mutableStateOf("") }
    var team by remember { mutableStateOf<Pair<String, List<ImportedBot>>?>(null) }
    var pkg by remember { mutableStateOf<BotZip.Package?>(null) }
    var pkgWarnings by remember { mutableStateOf<List<String>>(emptyList()) }
    var reading by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val made = remember { mutableStateListOf<String>() }
    val failures = remember { mutableStateListOf<String>() }

    fun read(bytes: ByteArray?, name: String) {
        team = null; pkg = null; pkgWarnings = emptyList()
        if (bytes == null) { error = "Couldn't read the file."; return }
        if (BotZip.looksLikeZip(bytes) || name.endsWith(".zip", ignoreCase = true)) {
            try { pkg = BotZip.load(bytes); error = null } catch (e: BotZipException) { error = e.message }
            return
        }
        try { team = parseTeam(bytes.decodeToString()); error = null } catch (e: IllegalArgumentException) { error = e.message }
    }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        reading = true
        scope.launch {
            val bytes = kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) {
                runCatching { context.contentResolver.openInputStream(uri)?.use { it.readBytes() } }.getOrNull()
            }
            read(bytes, uri.lastPathSegment.orEmpty())
            reading = false
        }
    }
    fun fetchLink() {
        val url = link.trim()
        reading = true
        scope.launch {
            val bytes = kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) {
                runCatching { okhttp3.OkHttpClient().newCall(okhttp3.Request.Builder().url(url).build()).execute().use { if (it.isSuccessful) it.body?.bytes() else null } }.getOrNull()
            }
            if (bytes == null) error = "Couldn't download that file. Check the link and try again." else read(bytes, url.substringBefore('?'))
            reading = false
        }
    }
    fun runImport() {
        busy = true
        error = null
        failures.clear()
        val p = pkg
        if (p != null) {
            scope.launch {
                try {
                    val (id, warnings) = BotZip.importPackage(p, backend, BotFormData.needsMain())
                    pkgWarnings = warnings
                    busy = false
                    if (warnings.isEmpty()) onImported(listOf(id)) else { made.add(id) }
                } catch (e: BotZipException) {
                    error = e.message
                    busy = false
                }
            }
            return
        }
        val t = team ?: run { busy = false; return }
        scope.launch {
            val ids = mutableListOf<String>()
            var needsMain = BotFormData.needsMain()
            for (b in t.second) {
                if (b.name in made) continue
                when (val r = BotsApi.createAgent(BotFormData.plainBody(b.name, b.description, b.systemPrompt, b.tagline, b.welcome, b.color, needsMain))) {
                    is BotsResult.Ok -> { ids.add(r.value); made.add(b.name); needsMain = false }
                    is BotsResult.Failed -> failures.add("${b.name}: ${r.message}")
                }
            }
            busy = false
            if (ids.isNotEmpty()) onImported(ids) else if (failures.isEmpty()) onDismiss()
        }
    }
    val pkgDone = pkg != null && made.isNotEmpty()
    LibrarySheet(
        title = "Import a bot", testTag = "importBotSheet", onDismiss = onDismiss,
        leading = LibrarySheetAction("Cancel", enabled = !busy, onClick = onDismiss),
        trailing = when {
            pkgDone -> LibrarySheetAction("Done", bold = true, testTag = "importBotDone") { onImported(made.toList()) }
            team != null || pkg != null -> LibrarySheetAction(if (busy) "Importing…" else if (failures.isEmpty()) "Import" else "Retry", enabled = !busy, bold = true, testTag = "importBotRun", onClick = ::runImport)
            else -> null
        },
    ) {
        Column(Modifier.fillMaxWidth().heightIn(max = 620.dp).verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            PillRow(listOf("file" to "From a file", "url" to "From a link"), if (source == "file") "file" to "From a file" else "url" to "From a link", { it.second }, { "importSource${it.first}" }) { source = it.first }
            if (source == "file") Pill("Choose a team file or bot package", "importChooseFile", enabled = !busy && !reading) { picker.launch(arrayOf("application/json", "text/plain", "application/zip")) }
            else {
                LibraryField("", link, "https://github.com/…/team.json or bot.zip", "importLink") { link = it }
                Pill("Read the file", "importReadLink", enabled = !busy && link.trim().startsWith("http")) { fetchLink() }
            }
            if (reading) LibrarySpinner("Reading the file…")
            Note("Bring in bots you already have: an Allternit team file (.json), or a bot package (.zip) exported from Allternit.")
            team?.let { (name, bots) ->
                Label("$name · ${bots.size} bot${if (bots.size == 1) "" else "s"}")
                bots.forEach { b ->
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(b.name, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                            if (b.tagline.isNotEmpty()) Text(b.tagline, color = c.textSecondary, fontSize = 12.sp, maxLines = 2)
                        }
                        if (b.name in made) Text("✓", color = com.allternit.core.design.AllternitStatic.StatusSuccess, fontWeight = FontWeight.Bold)
                        else if (failures.any { it.startsWith(b.name + ":") }) Text("!", color = com.allternit.core.design.AllternitStatic.StatusError, fontWeight = FontWeight.Bold)
                    }
                }
            }
            pkg?.let { p ->
                val pv = p.preview
                Column(Modifier.fillMaxWidth().testTag("importPackagePreview"), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Label(pv.name.ifEmpty { "Bot package" } + (pv.category?.let { " · $it" } ?: ""))
                    if (pv.description.isNotEmpty()) Text(pv.description, color = c.textSecondary, fontSize = 13.sp, maxLines = 4)
                    if (pv.contents.isNotEmpty()) Note(pv.contents.joinToString(" · "))
                    Note("${pv.fileCount} file${if (pv.fileCount == 1) "" else "s"} go to your computer")
                    pv.warnings.forEach { Note(it) }
                    if (pkgDone) Text("✓ Imported", color = com.allternit.core.design.AllternitStatic.StatusSuccess, fontWeight = FontWeight.Bold)
                }
            }
            if (pkgWarnings.isNotEmpty()) { Label("Imported with problems"); pkgWarnings.forEach { ErrorLine(it, Modifier.padding(0.dp)) } }
            if (failures.isNotEmpty()) { Label("Not imported"); failures.forEach { ErrorLine(it, Modifier.padding(0.dp)) } }
            error?.let { ErrorLine(it, Modifier.padding(0.dp)) }
        }
    }
}

@Suppress("unused") private fun JsonElement?.asObj() = this as? JsonObject
