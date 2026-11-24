package com.allternit.feature.bots.runtimes

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.feature.bots.BotAvatar
import com.allternit.feature.bots.BotRecord
import com.allternit.feature.bots.BotsApi
import com.allternit.feature.bots.BotsResult
import com.allternit.feature.bots.VendorGroupTurn
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import com.allternit.core.design.R as DesignR

/**
 * A bot chat's agent state (web ComposerAgentChips + AgentCard on the bot page): the bot's stored
 * `config` with botBrain and agentView, shared by the composer chips and the View switch. A pick
 * saves the whole config back (`PATCH agents/:id`, every other key kept); a brain pick also sets the
 * agent's top-level model/provider like BotBrainPickerSheet. A failed save puts the old config back.
 */
class BotAgentState internal constructor(val bot: BotRecord, private val scope: CoroutineScope, val onOpenBot: (String) -> Unit) {
    var config by mutableStateOf(bot.config)
        private set
    var note by mutableStateOf<String?>(null)
    var error by mutableStateOf<String?>(null)
    var vendor by mutableStateOf(bot.vendorName())
        private set
    var connected by mutableStateOf(bot.isConnected)
        private set
    var bots by mutableStateOf<List<BotChoice>>(emptyList())
        private set

    val botBrain: JsonObject get() = config["botBrain"] as? JsonObject ?: JsonObject(emptyMap())
    val agentView: JsonObject? get() = config["agentView"] as? JsonObject
    val view: String get() = agentView.s("view") ?: "allternit"

    internal suspend fun load() {
        VendorGroupTurn.binding(bot.id)?.let { b -> if (b.isVendor) { connected = true; vendor = bot.vendorName(b.vendor) } }
        val r = BotsApi.listAgents()
        if (r is BotsResult.Ok) bots = r.value.filter { it.isBot }.map { BotChoice(it, it.isConnected, it.vendorName()) }
    }

    fun apply(change: LayersChange) {
        val other = change.botId
        if (other != null) { if (other != bot.id) onOpenBot(other); return }
        val old = config
        config = mergedConfig(old, change)
        note = change.note
        error = null
        val ref = change.botBrain["modelRef"] as? JsonObject
        val body = buildJsonObject {
            put("config", config)
            if (change.layer != null && ref != null) { ref.s("modelID")?.let { put("model", it) }; ref.s("providerID")?.let { put("provider", it) } }
        }
        scope.launch {
            when (val r = BotsApi.updateAgent(bot.id, body)) {
                is BotsResult.Ok -> Unit
                is BotsResult.Failed -> { config = old; note = null; error = "Couldn't save the change. ${r.message}" }
            }
        }
    }
}

@Composable
fun rememberBotAgentState(bot: BotRecord, onOpenBot: (String) -> Unit): BotAgentState {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val state = remember(bot.id) { BotAgentState(bot, scope, onOpenBot) }
    LaunchedEffect(bot.id) {
        state.load()
        RuntimeRegistry.load(context)
    }
    return state
}

/**
 * Composer chips in a bot chat (mockup a2): runtime (mark + name), brain (mark + name), bot (face +
 * name), each opening that layer's choices with the sentence at the foot. A Connected bot shows
 * locked "Run by <Vendor>" chips. [brains] come from the shell's model catalog, default first.
 */
@Composable
fun AgentChipsRow(
    agent: BotAgentState,
    brains: List<BrainOption>,
    brainsLoading: Boolean,
    onOpenConnections: (() -> Unit)?,
    modifier: Modifier = Modifier,
    iconOnly: Boolean = false,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val registry by RuntimeRegistry.state.collectAsState()
    var open by remember { mutableStateOf<AgentLayer?>(null) }
    val model = AgentLayersModel(
        botBrain = agent.botBrain, agentView = agent.agentView, runtimes = registry.runtimes.ifEmpty { listOf(BUILT_IN_ALLTERNIT) }, brains = brains,
        bots = agent.bots, botId = agent.bot.id, botName = agent.bot.displayName, locked = agent.connected, vendorName = agent.vendor,
    )
    val notice = when (val s = registry) {
        is RegistryState.Stale -> RegistryNotice("Couldn't load the latest runtimes. Showing the saved list. ${s.message}") { scope.launch { RuntimeRegistry.load(context, force = true) } }
        is RegistryState.Error -> RegistryNotice("Couldn't load the runtimes. ${s.message}") { scope.launch { RuntimeRegistry.load(context, force = true) } }
        else -> null
    }
    Row(modifier.testTag("composerAgentChips"), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(2.dp)) {
        AgentLayer.entries.forEach { layer ->
            val locked = model.locked && layer != AgentLayer.Bot
            AgentChip(
                layer = layer, text = model.chipText(layer), locked = locked, iconOnly = iconOnly,
                mark = when (layer) { AgentLayer.Runtime -> model.runtime?.let { it.tool ?: it.id } ?: model.runtimeId; AgentLayer.Brain -> model.brainProvider ?: "allternit"; AgentLayer.Bot -> "" },
                bot = if (layer == AgentLayer.Bot) agent.bot else null,
            ) { open = layer }
        }
    }
    open?.let { layer ->
        AgentLayerSheet(
            model, layer, agent.note, onDismiss = { open = null },
            // An auto-switch keeps the sheet open so its note is read; any other pick closes it.
            onChange = { ch -> agent.apply(ch); if (ch.note == null) open = null },
            registryNotice = notice, brainsLoading = brainsLoading, onOpenConnections = onOpenConnections,
        )
    }
}

@Composable
private fun AgentChip(layer: AgentLayer, text: String, locked: Boolean, iconOnly: Boolean, mark: String, bot: BotRecord?, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        Modifier.heightIn(min = 44.dp).widthIn(min = if (iconOnly) 44.dp else 0.dp).clip(RoundedCornerShape(10.dp))
            .background(if (locked) c.fillNeutral.copy(alpha = c.fillNeutral.alpha * 0.6f) else Color.Transparent)
            // A locked chip (Connected bot) is read, not opened: the vendor runs the runtime and brain.
            .clickable(enabled = !locked, role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .semantics(mergeDescendants = true) { contentDescription = "${layer.label}, ${layer.hint}: $text${if (locked) ", locked" else ""}" }
            .testTag("composerAgentChip${layer.name}")
            .padding(horizontal = 6.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp, Alignment.CenterHorizontally),
    ) {
        if (locked) Icon(painterResource(DesignR.drawable.ph_lock), null, tint = c.textSecondary, modifier = Modifier.size(13.dp))
        if (bot != null) Box(Modifier.size(20.dp)) { BotAvatar(bot, 20.dp) } else if (!locked) Mark(mark, 20.dp)
        // Full accessible labels remain on icon-only chips, including vendor-locked layers.
        if (!iconOnly) Text(text, color = c.textSecondary, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 120.dp))
        if (!locked && !iconOnly) Icon(painterResource(DesignR.drawable.ph_caret_down), null, tint = c.textSecondary.copy(alpha = 0.8f), modifier = Modifier.size(10.dp))
    }
}
