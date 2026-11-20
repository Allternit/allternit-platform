package com.allternit.feature.shell.firstrun

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
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
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.Haptics
import com.allternit.core.design.ThinkingOrb
import com.allternit.core.design.ThinkingOrbSize
import com.allternit.feature.chat.RelayDevice
import com.allternit.feature.chat.RuntimeRelay
import kotlinx.coroutines.launch

/** The brain this setup picked: the connection id and the provider saved as the default (iOS SetupKitBrain). */
internal data class SetupKitBrain(val id: String, val provider: String, val modelId: String?, val label: String)

internal object SetupKit {
    val baseBrains = listOf("allternit", "claude", "copilot", "gemini", "grok", "kimi", "chatgpt")
    val moreBrains = listOf("api-key", "local")

    fun provider(forKey: String): String = when (forKey) {
        "openai-key" -> "openai"; "anthropic-key" -> "anthropic"; "google-key", "gemini" -> "google"; "kimi-key" -> "moonshot"
        else -> forKey.replace("-key", "")
    }

    fun whereClause(id: String): String = when (ConnectionsCatalog.entry(id)?.runsOn) {
        ConnRunsOn.Computer -> "runs on your Mac or PC"
        ConnRunsOn.Runner -> "runs on Allternit, no computer needed"
        else -> "works on any device"
    }

    fun outcome(brain: SetupKitBrain?): StepOutcome? {
        brain ?: return null
        val first = when {
            brain.id == "allternit" -> "Included in your account · nothing to set up"
            brain.id.endsWith("-key") || brain.id == "gemini" -> "Checked your key"
            else -> "Signed in with a code"
        }
        return StepOutcome(OutcomeStatus.Connected, brain.label, listOf(first, "Brain: ${brain.label} · ${whereClause(brain.id)}"))
    }
}

/**
 * FirstRunSetup (iOS FirstRunSetupView, allternit-ai `setup/FirstRunSetup.tsx` stacked layout):
 * the "Your workspace" graph on top, then Gizzi + A://, the 30-block progress, Gizzi's question,
 * the connect-kit rows, the result card for the picked brain, the stacked log of finished
 * steps, and the connected view. Every tag and result is driven by real state.
 *
 * [onFinish] gets the final state, the wizard data and a Settings section to open, if any.
 */
@Composable
internal fun FirstRunSetupScreen(
    relay: RuntimeRelay,
    accountFirstName: String?,
    onFinish: suspend (SetupState, SetupWizardData, String?) -> Unit,
) {
    val c = setupColors
    val view = LocalView.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val handoff = LocalSetupHandoff.current
    var state by remember { mutableStateOf(SetupState()) }
    var data by remember { mutableStateOf(SetupWizardData()) }
    var kitBrain by remember { mutableStateOf<SetupKitBrain?>(null) }
    var catalog by remember { mutableStateOf<List<SetupModelRow>>(emptyList()) }
    var opening by remember { mutableStateOf(false) }
    // The account's main bot when Meet your bots opened (to tell a new one apart); unset = not read yet.
    var mainBotBefore by remember { mutableStateOf<Result<String?>?>(null) }
    val connections = remember { ConnectionsStore() }
    val devices: suspend () -> List<RelayDevice>? = { relay.listDevices() }
    val reload: suspend () -> Unit = { connections.load(relay) }

    LaunchedEffect(Unit) { catalog = SetupApi.cloudModelCatalog() }
    LaunchedEffect(Unit) { reload() }

    fun dispatch(action: SetupAction) {
        if (action.firesHaptic) Haptics.perform(view, Haptics.Kind.Tap)
        state = SetupReducer.reduce(state, action)
    }

    // Derived outcomes (iOS `effective`).
    val effective = state.copy(outcomes = state.outcomes.toMutableMap().apply {
        SetupKit.outcome(kitBrain)?.let { put(SetupField.Ai, it) }
        if (state.order.contains(SetupStep.Phone)) {
            // This app is signed in on this phone, so the phone is already connected.
            put(SetupField.Phone, StepOutcome(OutcomeStatus.Connected, "This phone",
                listOf("You're signed in on this phone", "Chats, bots and approvals work here")))
        }
    })

    fun pickKitBrain(brain: SetupKitBrain) {
        kitBrain = brain
        data = data.copy(defaultProvider = brain.provider, defaultModelId = brain.modelId)
        dispatch(SetupAction.ChooseAI(BrainChoice.forProvider(brain.provider) ?: BrainChoice.Other))
    }

    /** "Meet your bots" Done or Skip: seen on this phone and the account, then on to Connected. */
    fun finishMeetYourBots() {
        dispatch(SetupAction.Next)
        scope.launch {
            BotsIntro.markSeen(context)
            // The main bot counts as set up now only when it changed in that step.
            val main = runCatching { MeetAgentsApi.roster() }.getOrNull()?.firstOrNull { it.isMain }?.id ?: return@launch
            val before = mainBotBefore?.getOrNull()
            if (mainBotBefore?.isSuccess == true && before != main) dispatch(SetupAction.MainBot(main))
        }
    }

    fun finish(fix: String?) {
        scope.launch {
            try { onFinish(effective, data, fix) } finally { opening = false }
        }
    }

    AnimatedContent(
        targetState = state.step,
        contentKey = { if (it == SetupStep.Ai || it == SetupStep.Phone) SetupStep.Ai else it },
        transitionSpec = { fadeIn(tween(300)) togetherWith fadeOut(tween(300)) },
        label = "setup-step",
        modifier = Modifier.fillMaxSize().background(c.bg),
    ) { step ->
        when (step) {
            SetupStep.Connected -> ConnectedScreen(
                facts = SessionFacts(effective),
                devices = devices,
                onBack = { dispatch(SetupAction.Back) },
                onOpen = { fix, sources ->
                    if (!opening) {
                        opening = true
                        scope.launch {
                            // The name on WELCOME: Settings → Profile's preferred name, else the account's first name.
                            val name = handoffName(SetupApi.preferredName()) ?: handoffName(accountFirstName)
                            // iOS plays the branded hand-off and finishes at 3.3 s under it; without one, at once.
                            if (!handoff.start(sources, name) { finish(fix) }) finish(fix)
                        }
                    }
                },
            )
            SetupStep.MainBot -> {
                LaunchedEffect(Unit) {
                    if (mainBotBefore == null) mainBotBefore = runCatching { MeetAgentsApi.roster().firstOrNull { it.isMain }?.id }
                }
                MeetYourBotsScreen(onFinish = { finishMeetYourBots() }, doneLabel = "See it all")
            }
            else -> SetupRail(
                state = effective,
                kitBrain = kitBrain,
                connections = connections,
                allternitModel = catalog.firstOrNull()?.id?.removePrefix("allternit/"),
                onPick = ::pickKitBrain,
                onBack = { dispatch(SetupAction.Back) },
                onSkip = {
                    // Skipping Your AI saves no brain unless one really connected.
                    if (state.step == SetupStep.Ai && effective.outcomes[SetupField.Ai]?.status?.isFinished != true) {
                        data = data.copy(defaultProvider = null, defaultModelId = null)
                    }
                    dispatch(SetupAction.Skip)
                },
                onNext = { dispatch(SetupAction.Next) },
                reload = reload,
            )
        }
    }
}

@Composable
private fun SetupRail(
    state: SetupState,
    kitBrain: SetupKitBrain?,
    connections: ConnectionsStore,
    allternitModel: String?,
    onPick: (SetupKitBrain) -> Unit,
    onBack: () -> Unit,
    onSkip: () -> Unit,
    onNext: () -> Unit,
    reload: suspend () -> Unit,
) {
    val c = setupColors
    val field = state.field
    val busy = field?.let { state.outcomes[it]?.status == OutcomeStatus.Working } ?: false
    val orb = orbState(state.step, busy)
    val word = orbWord(state.step, busy)
    val scroll = rememberScrollState()
    LaunchedEffect(state.step) { scroll.scrollTo(0) }

    BoxWithConstraints(Modifier.fillMaxSize().background(c.bg).windowInsetsPadding(WindowInsets.safeDrawing)) {
        val graphHeight = (maxHeight.value * 0.30f).coerceIn(170f, 260f).dp
        Column(Modifier.fillMaxSize().padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 14.dp)) {
            Box(
                Modifier.fillMaxWidth().height(graphHeight).clip(RoundedCornerShape(16.dp)).background(c.bg)
                    .border(1.dp, c.line, RoundedCornerShape(16.dp)),
            ) {
                SetupBlockField()
                SetupGraph(state, orb, word, Modifier.fillMaxSize())
            }
            // Gizzi + A://
            Row(Modifier.padding(top = 12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                SetupGizzi(30.dp)
                ThinkingOrb(state = orb, size = ThinkingOrbSize.Inline, label = "A:// is $word")
                com.allternit.core.design.BotNameText("Gizzi + A://", fontSize = 13.5.sp, fontWeight = FontWeight.Bold, color = c.ink, maxLines = 1)
                Row(
                    Modifier.semantics(mergeDescendants = true) { contentDescription = "A:// is $word" },
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Box(Modifier.size(6.dp).background(c.coral, RoundedCornerShape(2.dp)))
                    Text(word, style = mono(10.5.sp), color = c.mut, maxLines = 1)
                }
            }
            SetupProgressBlocks(state.step, state.order, Modifier.padding(top = 8.dp))

            Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(scroll).padding(top = 4.dp, bottom = 8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                val done = completedCards(state)
                if (done.isNotEmpty()) {
                    Column(Modifier.semantics { contentDescription = "Done so far" }, verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        done.forEach { SetupDoneRow(it) }
                    }
                }
                when (state.step) {
                    SetupStep.Ai -> {
                        Question(SetupCopy.question(SetupField.Ai))
                        SetupKitAIStep(connections, kitBrain, allternitModel, onPick, reload)
                        if (kitBrain != null) resultCard(state, SetupField.Ai)?.let { SetupResultCardView(it, busy, Modifier.padding(top = 4.dp)) }
                    }
                    SetupStep.Phone -> {
                        Question(SetupCopy.question(SetupField.Phone))
                        resultCard(state, SetupField.Phone)?.let { SetupResultCardView(it, false) }
                        SetupKitPhoneStep(connections, reload)
                    }
                    else -> {}
                }
            }

            // Nav
            Row(Modifier.padding(top = 10.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                SetupGhostButton("Back", enabled = state.step != state.order.first(), onClick = onBack)
                SetupGhostButton("Skip", onClick = onSkip)
                SetupPrimaryButton(if (state.nextIsConnected) "See it all" else "Next", Modifier.weight(1f), enabled = state.canAdvance, onClick = onNext)
            }
        }
    }
}

@Composable
private fun Question(text: String) {
    Text(text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = setupColors.ink,
        modifier = Modifier.padding(top = 10.dp, bottom = 8.dp).popIn(0))
}

/** A finished step in the log above the question (iOS SetupDoneRow). */
@Composable
private fun SetupDoneRow(card: ResultCard) {
    val c = setupColors
    Row(
        Modifier.fillMaxWidth().heightIn(min = 36.dp).background(c.fill, RoundedCornerShape(10.dp)).padding(horizontal = 10.dp, vertical = 6.dp)
            .semantics(mergeDescendants = true) {},
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(if (card.done) "✓" else "–", fontSize = 12.sp, fontWeight = FontWeight.Bold, color = if (card.done) c.ok else c.mut)
        Text(card.title.uppercase(), style = mono(10.sp, FontWeight.SemiBold), color = c.mut)
        Text(card.summary, fontSize = 12.5.sp, fontWeight = FontWeight.SemiBold, color = c.ink, maxLines = 1, modifier = Modifier.weight(1f))
        ToneChip(card.chip, card.tone)
    }
}

@Composable
private fun ToneChip(text: String, tone: ChipTone) {
    val c = setupColors
    val (bg, fg) = when (tone) {
        ChipTone.Ok -> c.okBg to c.okFg
        ChipTone.Warn -> c.warnBg to c.warnFg
        ChipTone.Error -> c.errBg to c.errFg
        ChipTone.Soft -> c.fill2 to c.sub
    }
    Text(text, fontSize = 11.sp, fontWeight = FontWeight.Bold, color = fg,
        modifier = Modifier.background(bg, CircleShape).padding(horizontal = 9.dp, vertical = 3.dp))
}

/** The result card for the current choice (iOS SetupResultCardView). */
@Composable
private fun SetupResultCardView(card: ResultCard, working: Boolean, modifier: Modifier = Modifier) {
    val c = setupColors
    Column(
        modifier.fillMaxWidth().background(c.card, RoundedCornerShape(12.dp)).border(1.dp, c.line, RoundedCornerShape(12.dp))
            .padding(12.dp).semantics(mergeDescendants = true) {}.popIn(0),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(card.title.uppercase(), style = mono(10.5.sp, FontWeight.SemiBold), color = c.mut, modifier = Modifier.weight(1f))
            if (working) CircularProgressIndicator(Modifier.size(12.dp).padding(end = 6.dp), strokeWidth = 1.5.dp, color = c.coral)
            ToneChip(card.chip, card.tone)
        }
        card.lines.forEachIndexed { i, line ->
            val last = i == card.lines.lastIndex
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(if (last && card.done) "✓" else "›", style = mono(12.sp, FontWeight.Bold), color = if (last && card.done) c.ok else c.mut)
                Text(line, fontSize = 13.sp, color = if (i == 0) c.ink else c.sub)
            }
        }
    }
}

/** Step 1: Your AI, as connect-kit rows in three small groups (iOS SetupKitAIStep). */
@Composable
private fun SetupKitAIStep(
    store: ConnectionsStore,
    brain: SetupKitBrain?,
    allternitModel: String?,
    onPick: (SetupKitBrain) -> Unit,
    reload: suspend () -> Unit,
) {
    val c = setupColors
    val scope = rememberCoroutineScope()
    var showMore by remember { mutableStateOf(false) }
    var sheet by remember { mutableStateOf<ConnSheet?>(null) }
    val brainIsMore = brain?.let { it.id in SetupKit.moreBrains || it.id.endsWith("-key") } == true
    val ids = if (showMore || brainIsMore) SetupKit.baseBrains + SetupKit.moreBrains else SetupKit.baseBrains
    val entries = ids.mapNotNull(ConnectionsCatalog::entry)
    val anyDevice = entries.filter { it.runsOn != ConnRunsOn.Computer && store.isAvailable(it.id) }
    val computer = entries.filter { it.runsOn == ConnRunsOn.Computer }
    val soon = entries.filter { it.runsOn != ConnRunsOn.Computer && !store.isAvailable(it.id) }

    fun connected(id: String) {
        val row = store.row(id)
        when {
            id == "allternit" -> onPick(SetupKitBrain(id, "allternit", allternitModel, "Allternit Cloud"))
            // The computer's Codex sign-in; ChatGPT run for the person only is chat-only.
            id == "chatgpt" && row.runtimeId != null -> onPick(SetupKitBrain(id, "codex-cli", null, row.found ?: "ChatGPT"))
            id.endsWith("-key") -> onPick(SetupKitBrain(id, SetupKit.provider(id), null, "${ConnectionsCatalog.keyProvider(id)?.name ?: id} key"))
            id == "gemini" -> onPick(SetupKitBrain("google-key", "google", null, "Gemini · AI Studio key"))
        }
    }

    fun use(id: String) {
        if (id == "api-key") {
            ConnectionsCatalog.keyProviders.firstOrNull { store.state(it.id) == ConnState.On }?.let { connected(it.id) }
            return
        }
        connected(id)
    }

    fun fallback(id: String) {
        sheet = when {
            id == "this-mac" -> ConnSheet.OnComputer("this-mac")
            id == "github" -> ConnSheet.Login("copilot")
            id.endsWith("-key") || id == "api-key" -> { showMore = true; ConnSheet.Keys(if (id == "api-key") null else id) }
            else -> null
        }
    }

    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        listOf("Works on any device" to anyDevice, "Needs your computer" to computer, "Coming soon" to soon)
            .filter { it.second.isNotEmpty() }
            .forEach { (title, rows) ->
                Text(title.uppercase(), style = mono(10.5.sp, FontWeight.SemiBold), color = c.mut, letterSpacing = 0.8.sp,
                    modifier = Modifier.padding(start = 2.dp, top = 4.dp))
                Column(
                    Modifier.fillMaxWidth().background(c.card, RoundedCornerShape(12.dp)).border(1.dp, c.line, RoundedCornerShape(12.dp))
                        .semantics { contentDescription = title },
                ) {
                    rows.forEachIndexed { i, e ->
                        if (i > 0) HorizontalDivider(thickness = 0.5.dp, color = c.line)
                        val isBrain = brain?.let { b -> b.id == e.id || (e.id == "api-key" && b.id.endsWith("-key")) || (e.id == "gemini" && b.id == "google-key") } == true
                        val r = store.row(e.id)
                        val personOnly = r.personOnly || e.id in ConnectionsCatalog.runnerIds || (e.id == "chatgpt" && r.runtimeId == null)
                        ConnectionRow(
                            entry = if (e.id == "allternit") e.copy(name = "Allternit") else e,
                            store = store,
                            tag = if (isBrain) "Your brain" else null,
                            trailing = when {
                                isBrain -> ({ Text("✓", fontSize = 18.sp, fontWeight = FontWeight.Bold, color = c.ok, modifier = Modifier.semantics { contentDescription = "Your brain" }) })
                                r.state == ConnState.On && !personOnly -> ({ SetupQuietButton("Use this") { use(e.id) } })
                                r.state == ConnState.On -> ({ Text("Chat only", fontSize = 13.sp, color = c.sub) })
                                else -> null
                            },
                            onConnect = {
                                if (e.id == "allternit") connected("allternit") else sheet = connectRoute(e.id)
                            },
                            onFallback = ::fallback,
                        )
                    }
                }
            }
        if (!showMore && !brainIsMore) {
            Text("More ways: an API key, a local brain", fontSize = 13.sp, fontWeight = FontWeight.Medium, color = c.sub,
                modifier = Modifier.padding(top = 2.dp).clickable { showMore = true }.padding(vertical = 8.dp))
        }
    }

    sheet?.let { s ->
        ConnectSheetHost(
            sheet = s,
            store = store,
            onConnected = { id -> sheet = null; connected(id); scope.launch { reload() } },
            onFallback = ::fallback,
            onDismiss = { sheet = null; scope.launch { reload() } },
        )
    }
}

/**
 * One connect-kit row (iOS ConnectionRowView, compact): logo tile, name with "New", the status
 * line, and one action. A row that can't be used yet shows its reason and its fallback button.
 */
@Composable
internal fun ConnectionRow(
    entry: ConnEntry,
    store: ConnectionsStore,
    tag: String?,
    trailing: (@Composable () -> Unit)?,
    onConnect: () -> Unit,
    onFallback: (String) -> Unit,
) {
    val c = setupColors
    val r = store.row(entry.id)
    val available = store.isAvailable(entry.id)
    val status = if (!available) store.unavailableReason(entry.id) else store.statusLine(entry)
    Row(
        Modifier.fillMaxWidth().heightIn(min = 60.dp).padding(horizontal = 12.dp, vertical = 10.dp).testTag("conn-row-${entry.id}"),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        val logoKey = ConnectionsCatalog.logoKey(entry.id)
        if (logoKey != null) {
            com.allternit.core.design.AppLogo(entry.name, id = logoKey, size = 34.dp)
        } else Box(
            Modifier.size(34.dp).background(c.card, RoundedCornerShape(9.dp)).border(1.dp, c.line, RoundedCornerShape(9.dp)),
            contentAlignment = Alignment.Center,
        ) { Text(entry.mono, style = mono(13.sp, FontWeight.Bold), color = c.ink) }
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(entry.name, fontSize = 15.sp, fontWeight = FontWeight.Medium, color = c.ink, maxLines = 1)
                if (tag != null) Text(tag, fontSize = 11.sp, color = c.okFg, modifier = Modifier.background(c.okBg, CircleShape).padding(horizontal = 7.dp, vertical = 2.dp))
                else if (entry.isNew && r.state != ConnState.On) Text("New", fontSize = 11.sp, color = c.sub, modifier = Modifier.background(c.fill2, CircleShape).padding(horizontal = 7.dp, vertical = 2.dp))
            }
            Text(status, fontSize = 12.5.sp, color = if (r.state == ConnState.Needs) c.errFg else c.sub)
        }
        when {
            trailing != null -> trailing()
            r.state == ConnState.Connecting -> CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.sub)
            !available -> r.fallbacks.firstOrNull()?.let { f -> SetupQuietButton(ConnectionsCatalog.fallbackLabel(f)) { onFallback(f) } }
            entry.way == ConnWay.Included -> Text("Included", fontSize = 14.sp, color = c.sub, modifier = Modifier.clickable(onClick = onConnect).padding(8.dp))
            r.state == ConnState.Needs -> SetupQuietButton("Reconnect", onClick = onConnect)
            else -> SetupQuietButton("Connect", onClick = onConnect)
        }
    }
}

/** Step 2: Your phone. The phone is already here; a computer is offered for later (iOS SetupKitPhoneStep). */
@Composable
private fun SetupKitPhoneStep(store: ConnectionsStore, reload: suspend () -> Unit) {
    val c = setupColors
    val scope = rememberCoroutineScope()
    var showPair by remember { mutableStateOf(false) }
    val on = store.state("this-mac") == ConnState.On
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("LATER, IF YOU NEED IT", style = mono(10.5.sp, FontWeight.SemiBold), color = c.mut, letterSpacing = 0.8.sp,
            modifier = Modifier.padding(start = 2.dp, top = 10.dp))
        Column(
            Modifier.fillMaxWidth().background(c.card, RoundedCornerShape(12.dp)).border(1.dp, c.line, RoundedCornerShape(12.dp))
                .testTag("setup-add-computer"),
        ) {
            val entry = ConnectionsCatalog.entry("this-mac")!!.let { if (on) it.copy(name = "Your computer") else it.copy(name = "Add a computer") }
            ConnectionRow(entry, store, null, if (on) ({ Spacer(Modifier.width(0.dp)) }) else null, onConnect = { showPair = true }, onFallback = { showPair = true })
            Text("Only when a task needs one: clicking apps, using a browser. Chat works without it.", fontSize = 12.5.sp, color = c.sub,
                modifier = Modifier.padding(start = 12.dp, end = 12.dp, bottom = 10.dp))
        }
    }
    if (showPair) {
        ConnectSheetHost(ConnSheet.OnComputer("this-mac"), store, onConnected = { showPair = false }, onFallback = {},
            onDismiss = { showPair = false; scope.launch { reload() } })
    }
}
