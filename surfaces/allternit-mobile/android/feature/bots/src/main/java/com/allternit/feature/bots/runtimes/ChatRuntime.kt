package com.allternit.feature.bots.runtimes

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import com.allternit.core.design.R as DesignR

/*
 * Runtime + Brain in every chat (Harness & Bots 2.0 fix): a plain chat's composer shows a Runtime
 * chip beside the brain chip, default Allternit. Picking another runtime starts a new chat on it
 * when the computer has that runtime; otherwise the choices stay open with why it can't run here.
 * The shell wires [ChatRuntime.check], [ChatRuntime.onStart] and [ChatRuntime.onPair] (the computer
 * list and the chat live there); the next session create carries [ChatRuntime.sessionBotBrain].
 */

/** Whether a runtime can run a chat right now. */
sealed interface RuntimeCheck {
    data object Ready : RuntimeCheck
    /** No computer is paired. */
    data object NoComputer : RuntimeCheck
    /** A computer exists but none answers. */
    data object Unreachable : RuntimeCheck
    /** The computer answers but doesn't have the runtime's CLI. */
    data class NotInstalled(val computer: String) : RuntimeCheck
    data class Failed(val message: String) : RuntimeCheck
}

/** The sentence a blocked pick shows (iOS wording). */
fun runtimeCheckMessage(r: RuntimeEntry, check: RuntimeCheck): String? = when (check) {
    RuntimeCheck.Ready -> null
    RuntimeCheck.NoComputer -> "${r.name} runs on a computer. Pair a computer to use it."
    RuntimeCheck.Unreachable -> "${r.name} runs on a computer. Your computer isn't answering. Open Allternit on it, then try again."
    is RuntimeCheck.NotInstalled -> "${r.name} isn't installed on ${check.computer}."
    is RuntimeCheck.Failed -> check.message
}

object ChatRuntime {
    /** The runtime new plain chats start on. */
    var runtimeId by mutableStateOf("allternit")

    /** Checks the computer for a runtime (shell: relay + `native-sessions/harnesses`). Null = no check, ready. */
    var check: (suspend (RuntimeEntry) -> RuntimeCheck)? = null

    /** Starts a new chat on the runtime (shell: new session + open the chat page). */
    var onStart: ((RuntimeEntry) -> Unit)? = null

    /** Opens the pair / computers screen. */
    var onPair: (() -> Unit)? = null

    /**
     * `metadata.botBrain` for the next session create (contract bot-record shape, normalized:
     * runtime + native_harness mode + harness), or null on the native runtime.
     */
    fun sessionBotBrain(): JsonObject? {
        val id = runtimeId
        if (id == "allternit") return null
        return resolveRuntime(buildJsonObject { put("runtime", id) }, RuntimeRegistry.runtimes)
    }

    /** Checks then starts; returns the blocking check, or null once the chat started. */
    suspend fun startOn(r: RuntimeEntry): RuntimeCheck? {
        if (r.id != "allternit") {
            val res = check?.invoke(r) ?: RuntimeCheck.Ready
            if (res != RuntimeCheck.Ready) return res
        }
        runtimeId = r.id
        onStart?.invoke(r)
        return null
    }
}

/**
 * The Runtime chip of a plain chat (mark + ▾, icon-sized like the bot chat's runtime chip),
 * opening the runtime choices. [brains] / [brainKey] are the shell's catalog and current pick, so
 * "Not with <Runtime>" and the auto-switch work; [onBrainSwitch] applies an automatic brain change.
 */
@Composable
fun ChatRuntimeChip(
    brains: List<BrainOption>,
    brainKey: String?,
    onBrainSwitch: (BrainOption) -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    val registry by RuntimeRegistry.state.collectAsState()
    var open by remember { mutableStateOf(false) }
    var blocked by remember { mutableStateOf<Pair<RuntimeEntry, RuntimeCheck>?>(null) }
    var checking by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) { RuntimeRegistry.load(context) }

    val runtimes = registry.runtimes.ifEmpty { listOf(BUILT_IN_ALLTERNIT) }
    val current = runtimes.runtime(ChatRuntime.runtimeId) ?: BUILT_IN_ALLTERNIT
    val botBrain = buildJsonObject {
        put("runtime", current.id)
        brainKey?.split("/", limit = 2)?.takeIf { it.size == 2 }?.let { (p, m) -> put("modelRef", buildJsonObject { put("providerID", p); put("modelID", m) }) }
    }
    val model = AgentLayersModel(
        botBrain = botBrain, agentView = null, runtimes = runtimes, brains = brains, bots = emptyList(),
        botId = null, botName = "This chat", locked = false, vendorName = null,
    )

    Row(
        modifier.heightIn(min = 48.dp).clip(RoundedCornerShape(10.dp))
            .clickable(role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); blocked = null; open = true }
            .semantics(mergeDescendants = true) { contentDescription = "Runtime, How it works: ${current.name}" }
            .testTag("composerRuntimeChip")
            .padding(horizontal = 6.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Mark(current.tool ?: current.id, 20.dp)
        Icon(painterResource(DesignR.drawable.ph_caret_down), null, tint = c.textSecondary.copy(alpha = 0.8f), modifier = Modifier.size(10.dp))
    }

    if (open) {
        val notice = when (val s = registry) {
            is RegistryState.Stale -> RegistryNotice("Couldn't load the latest runtimes. Showing the saved list. ${s.message}") { scope.launch { RuntimeRegistry.load(context, force = true) } }
            is RegistryState.Error -> RegistryNotice("Couldn't load the runtimes. ${s.message}") { scope.launch { RuntimeRegistry.load(context, force = true) } }
            else -> null
        }
        AgentLayerSheet(
            model, AgentLayer.Runtime, note = null, onDismiss = { open = false },
            onChange = { ch ->
                val id = (ch.botBrain["runtime"] as? JsonPrimitive)?.content ?: return@AgentLayerSheet
                val r = runtimes.runtime(id) ?: return@AgentLayerSheet
                if (r.id == current.id) { open = false; return@AgentLayerSheet }
                checking = r.id
                blocked = null
                scope.launch {
                    // A brain the new runtime can't run switches to the first that fits, as in bot chats.
                    val switched = (ch.botBrain["modelRef"] as? JsonObject)?.let { ref ->
                        val key = "${ref.s("providerID")}/${ref.s("modelID")}"
                        brains.firstOrNull { it.key == key && key != brainKey }
                    }
                    val res = ChatRuntime.startOn(r)
                    checking = null
                    if (res == null) { switched?.let(onBrainSwitch); open = false } else blocked = r to res
                }
            },
            registryNotice = notice, brainsLoading = false,
            extra = {
                checking?.let { id ->
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.semantics(mergeDescendants = true) {}.testTag("composerRuntimeChecking")) {
                        CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 1.5.dp, color = c.textSecondary)
                        Text("Checking your computer for ${runtimes.runtime(id)?.name ?: id}…", color = c.textSecondary, fontSize = 13.sp)
                    }
                }
                blocked?.let { (r, res) ->
                    val msg = runtimeCheckMessage(r, res) ?: return@let
                    val action = when (res) {
                        RuntimeCheck.NoComputer -> "Pair a computer"
                        RuntimeCheck.Unreachable, is RuntimeCheck.NotInstalled -> "Open Computers"
                        else -> null
                    }
                    NoticeLine(msg, action, ChatRuntime.onPair?.let { go -> { open = false; go() } }, warn = true)
                }
            },
        )
    }
}
