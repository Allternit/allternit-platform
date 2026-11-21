package com.allternit.feature.bots.runtimes

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
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
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.artifacts.LibraryLayout
import com.allternit.feature.bots.BotAvatar
import com.allternit.feature.bots.BotsHost
import com.allternit.feature.bots.NewBotSheet
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR

/*
 * Settings > Connections > Runtimes (Harness & Bots 2.0 fix): every runtime from the registry in
 * the screen's List, Grid or Deck, each opening its detail: the runtime's look with a sample
 * session, its brains, the plans it allows, its live web app, and "New chat on" / "New bot on".
 */

private fun RuntimeEntry.asChoice(): LayerChoice = LayerChoice(
    id = id, name = name, detail = statusLabel ?: "${CATEGORY_LABEL[category] ?: category} · $vendor",
    group = CATEGORY_LABEL[category] ?: category, mark = tool ?: id,
    badges = buildList {
        if (liveUi != null) add("Live app" to BadgeTone.Live)
        add(runtimeBrainsLabel(this@asChoice) to BadgeTone.Neutral)
        if (statusTone == "warn" && statusLabel != null) add("Limited" to BadgeTone.Warn)
    },
)

/** The Runtimes group of Connections, drawn in the screen's [layout] and filtered by [query]. */
@Composable
fun RuntimesSection(layout: LibraryLayout, query: String, onOpen: (RuntimeEntry) -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val registry by RuntimeRegistry.state.collectAsState()
    LaunchedEffect(Unit) { RuntimeRegistry.load(context) }
    val runtimes = registry.runtimes.ifEmpty { listOf(BUILT_IN_ALLTERNIT) }
        .filter { matches(query, it.name, it.vendor, CATEGORY_LABEL[it.category]) }
    if (runtimes.isEmpty()) return
    Column(Modifier.fillMaxWidth().testTag("connectionsGroup_runtimes")) {
        Row(Modifier.fillMaxWidth().padding(start = 6.dp, end = 6.dp, top = 18.dp), verticalAlignment = Alignment.Bottom) {
            Text("Runtimes", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.weight(1f).semantics { heading() })
            Text("${runtimes.size}", color = c.textTertiary, fontSize = 12.sp)
        }
        Text(
            "How a chat or bot works. Allternit runs anywhere; the others run on a computer that has them.",
            color = c.textTertiary, fontSize = 12.sp, lineHeight = 16.sp, modifier = Modifier.padding(start = 6.dp, top = 2.dp, bottom = 8.dp),
        )
        when (val s = registry) {
            is RegistryState.Stale -> NoticeLine("Couldn't load the latest runtimes. Showing the saved list. ${s.message}", "Try again", { scope.launch { RuntimeRegistry.load(context, force = true) } }, warn = true)
            is RegistryState.Error -> NoticeLine("Couldn't load the runtimes. ${s.message}", "Try again", { scope.launch { RuntimeRegistry.load(context, force = true) } }, warn = true)
            else -> Unit
        }
        ChoiceLayout(runtimes.map { it.asChoice() }, layout, "Runtimes", botFace = { b, s -> BotAvatar(b, s) }) { ch ->
            runtimes.firstOrNull { it.id == ch.id }?.let(onOpen)
        }
    }
}

/** A short sample session for the look preview (no account data). */
private fun sampleItems(r: RuntimeEntry): List<LookItem> = listOf(
    LookItem.User("s1", "Fix the failing login test"),
    LookItem.Steps(
        "s2",
        listOf(
            LookStep("Read", "src/auth/login.ts", "84 lines", "success"),
            LookStep("Edit", "src/auth/login.ts", "+3 −1", "success"),
            LookStep("Run", "npm test -- login", "12 passed", "success"),
        ),
    ),
    LookItem.Agent("s3", "The token check compared seconds to milliseconds. Fixed it, and the login tests pass. (Sample session in ${r.name}.)", streaming = false),
)

/** One runtime's detail page. [onNewChat] opens the started chat; [onBotCreated] opens the new bot. */
@Composable
fun RuntimeDetailScreen(
    runtime: RuntimeEntry,
    host: BotsHost?,
    onBack: () -> Unit,
    onBotCreated: (String) -> Unit,
) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var newBotOpen by remember { mutableStateOf(false) }
    var blocked by remember { mutableStateOf<RuntimeCheck?>(null) }
    var starting by remember { mutableStateOf(false) }
    val plans = plansOf(runtime).map { PLAN_NAMES[it] ?: it }

    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().navigationBarsPadding().testTag("runtimeDetail_${runtime.id}")) {
        LibraryHeader(
            title = runtime.name, onBack = null, glass = true,
            leading = { LibraryIconButton(DesignR.drawable.ph_caret_left, "Back", "runtimeDetailBack", onClick = onBack) },
        )
        Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 32.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp), modifier = Modifier.semantics(mergeDescendants = true) {}) {
                Mark(runtime.tool ?: runtime.id, 40.dp)
                Column {
                    Text(runtime.name, color = c.textPrimary, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
                    Text("${CATEGORY_LABEL[runtime.category] ?: runtime.category} · ${runtime.vendor}", color = c.textSecondary, fontSize = 13.sp)
                    runtime.statusLabel?.let { Text(it, color = c.textTertiary, fontSize = 12.sp) }
                }
            }
            // The look, with a sample session (the native runtime explains it has none).
            Box(
                Modifier.fillMaxWidth().height(320.dp).clip(RoundedCornerShape(14.dp)).border(1.dp, c.borderSubtle, RoundedCornerShape(14.dp))
                    .semantics { contentDescription = "${runtime.name} look, sample session" }.testTag("runtimeDetailLook"),
            ) {
                RuntimeLookView(runtime, look = null, items = sampleItems(runtime), isStreaming = false, onSend = {}, onStop = {})
            }
            InfoRow("Brains", runtime.brains?.let { list -> list.map(::providerName).distinct().joinToString(", ") } ?: "Any brain", "runtimeDetailBrains")
            InfoRow("Plans allowed", if (plans.isEmpty()) "None. Use an API key, a local model or credits." else plans.joinToString(", "), "runtimeDetailPlans")
            InfoRow(
                "Live web app",
                runtime.liveUi?.let { "${it.label}. Opens on your computer while a chat runs." } ?: "None. Its work shows in the chat.",
                "runtimeDetailLive",
            )
            blocked?.let { res ->
                val msg = runtimeCheckMessage(runtime, res)
                if (msg != null) NoticeLine(
                    msg, if (res == RuntimeCheck.NoComputer) "Pair a computer" else if (res is RuntimeCheck.Failed) null else "Open Computers",
                    ChatRuntime.onPair, warn = true,
                )
            }
            ActionButton(if (starting) "Checking your computer…" else "New chat on ${runtime.name}", primary = true, testTag = "runtimeDetailNewChat", enabled = !starting) {
                starting = true
                blocked = null
                scope.launch { blocked = ChatRuntime.startOn(runtime); starting = false }
            }
            ActionButton("New bot on ${runtime.name}", primary = false, testTag = "runtimeDetailNewBot") { newBotOpen = true }
        }
    }
    if (newBotOpen) NewBotSheet(
        store = null, host = host, onBrowseTemplates = null, onDismiss = { newBotOpen = false },
        onCreated = { ids -> newBotOpen = false; ids.firstOrNull()?.let(onBotCreated) },
        initialRuntime = runtime.id,
    )
}

@Composable
private fun InfoRow(label: String, value: String, testTag: String) {
    val c = AllternitTheme.colors
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(horizontal = 14.dp, vertical = 10.dp)
            .semantics(mergeDescendants = true) {}.testTag(testTag),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Text(label, color = c.textTertiary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
        Text(value, color = c.textPrimary, fontSize = 14.sp)
    }
}

@Composable
private fun ActionButton(text: String, primary: Boolean, testTag: String, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Box(
        Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(RoundedCornerShape(24.dp))
            .background(if (primary) c.textPrimary else c.fillNeutral)
            .clickable(enabled = enabled, role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .testTag(testTag).padding(horizontal = 16.dp, vertical = 12.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(text, color = if (primary) c.bgPrimary else c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
    }
}
