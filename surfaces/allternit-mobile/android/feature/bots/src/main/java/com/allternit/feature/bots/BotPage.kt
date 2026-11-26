package com.allternit.feature.bots

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
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
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.LibrarySearchField
import com.allternit.feature.artifacts.PillRow
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.ArtifactRecord
import kotlinx.coroutines.async
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR

/** iOS BotHomeTab (bot-home-tabs.ts). */
enum class BotHomeTab(val label: String) { Threads("Chat"), Artifacts("Artifacts"), Routines("Routines"), Memory("Memory"), Config("Settings") }

/**
 * The bot page (iOS BotPageView): the banner (live face, name, role and model, Edit bot,
 * Workspace), the summary tiles, then Model, Threads, Tasks, Routines, Artifacts, Memory, Computer,
 * Channels.
 */
@Composable
internal fun BotPageView(bot: BotRecord, store: BotModeStore, nav: BotNavigator, onOpenThreads: (() -> Unit)? = null, onOpenTasks: (() -> Unit)? = null, backend: com.allternit.feature.chat.ChatBackend? = null, onCall: (() -> Unit)? = null) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var routinesOn by remember { mutableStateOf<Int?>(null) }
    var channelNames by remember { mutableStateOf<List<String>?>(null) }
    var summary by remember { mutableStateOf<BotSummary?>(null) }
    var computerRunning by remember { mutableStateOf(false) }
    var editOpen by remember { mutableStateOf(false) }
    LaunchedEffect(bot.id) {
        val r = async { BotsApi.listRoutines().orNull() }
        val ch = async { BotsApi.listChannels(bot.id).orNull() }
        val s = async { BotsApi.summary(bot.id).orNull() }
        val comp = async { BotsApi.listComputers(bot.id).orNull() }
        routinesOn = r.await()?.count { it.agentId == bot.id && it.isEnabled }
        channelNames = ch.await()?.filter { it.enabled }?.map { it.label }
        summary = s.await()
        computerRunning = comp.await()?.any { it.isRunning } == true
    }
    val accent = bot.accentHex?.let { hexColor(it) }
    val working = store.workingCount(bot.id)
    val waiting = store.waitingCount(bot.id)
    val myThreads = store.threads(bot.id).sortedByDescending { it.lastActivityAt }
    // Current task: the thread that is working or waiting on you, the same line the roster shows.
    val nowTask = myThreads.firstOrNull { it.group == "working" || it.group == "waiting" }?.let { it.statusLine?.takeIf { l -> l.isNotEmpty() } ?: it.displayTitle.takeIf { t -> t.isNotEmpty() } }
    val lastActive = botTimeRelative(myThreads.firstOrNull()?.lastActivityAt ?: bot.lastRunAt)
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 28.dp).testTag("botPage"), verticalArrangement = Arrangement.spacedBy(16.dp)) {
        Column(
            Modifier.padding(horizontal = 16.dp).fillMaxWidth().clip(RoundedCornerShape(22.dp))
                .background(Brush.verticalGradient(if (accent != null) listOf(accent.copy(alpha = 0.32f), accent.copy(alpha = 0.06f)) else listOf(c.textPrimary.copy(alpha = 0.18f), c.textPrimary.copy(alpha = 0.03f))))
                .padding(vertical = 20.dp, horizontal = 16.dp),
            horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            BotAvatar(bot, 92.dp, live = true)
            com.allternit.core.design.BotNameText(bot.displayName, color = c.textPrimary, fontSize = 22.sp, fontWeight = FontWeight.SemiBold)
            val role = listOfNotNull(bot.tagline, bot.modelShort).filter { it.isNotEmpty() }.joinToString(" · ")
            if (role.isNotEmpty()) Text(role, color = c.textSecondary, fontSize = 13.sp, textAlign = TextAlign.Center)
            if (nowTask != null) Text("Now: $nowTask", color = c.textPrimary, fontSize = 13.sp, maxLines = 2, textAlign = TextAlign.Center, modifier = Modifier.testTag("botPageNow"))
            if (lastActive.isNotEmpty()) Text("Last active $lastActive", color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.testTag("botPageLastActive"))
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 6.dp)) {
                if (onCall != null) Pill("Call", "botPageCall", icon = com.allternit.core.design.R.drawable.ph_phone) { onCall() }
                Pill("Edit bot", "botPageEdit", icon = com.allternit.core.design.R.drawable.ph_pencil_simple) { editOpen = true }
                Pill("Workspace", "botPageWorkspace", icon = com.allternit.core.design.R.drawable.ph_file_text) { nav.open(BotRoute.Settings(bot.id)) }
            }
        }
        BotSummaryTiles(summary)
        if (myThreads.isNotEmpty()) {
            Column(Modifier.testTag("botPageRecent")) {
                SectionHeader("Recent activity")
                RowGroup {
                    myThreads.take(3).forEachIndexed { i, t ->
                        NavRow(t.displayTitle.ifEmpty { t.statusLine.orEmpty() }, com.allternit.core.design.R.drawable.ph_chat_circle, botTimeShort(t.lastActivityAt), "botPageRecent${t.id}", last = i == minOf(3, myThreads.size) - 1) { nav.openThread(t) }
                    }
                }
            }
        }
        RowGroup {
            NavRow("Model", DesignR.drawable.ph_cpu, summary?.model?.split("/")?.lastOrNull()?.takeIf { it.isNotBlank() } ?: bot.modelShort, "botPageModel", last = false) { nav.open(BotRoute.Settings(bot.id)) }
            NavRow("Threads", com.allternit.core.design.R.drawable.ph_chat_circle, "${store.openThreadCount(bot.id)} open", "botPageThreads", last = false) { onOpenThreads?.invoke() ?: nav.open(BotRoute.BotHome(bot.id, BotHomeTab.Threads)) }
            NavRow("Tasks", com.allternit.core.design.R.drawable.ph_list, if (waiting > 0) "$working working · $waiting need you" else "$working working", "botPageTasks", last = false) { onOpenTasks?.invoke() ?: nav.open(BotRoute.Tasks(bot.id)) }
            NavRow("Routines", com.allternit.core.design.R.drawable.ph_clock, routinesOn?.let { "$it on" }, "botPageRoutines", last = false) { nav.open(BotRoute.BotHome(bot.id, BotHomeTab.Routines)) }
            NavRow("Artifacts", com.allternit.core.design.R.drawable.ph_squares_four, null, "botPageArtifacts", last = false) { nav.open(BotRoute.BotHome(bot.id, BotHomeTab.Artifacts)) }
            NavRow("Memory", com.allternit.core.design.R.drawable.ph_brain, null, "botPageMemory", last = false) { nav.open(BotRoute.BotHome(bot.id, BotHomeTab.Memory)) }
            NavRow("Computer", com.allternit.core.design.R.drawable.ph_desktop, if (computerRunning) "Running" else "Idle", "botPageComputer", last = false) { nav.open(BotRoute.Computer(bot.id)) }
            NavRow("Channels", com.allternit.core.design.R.drawable.ph_link, channelNames?.let { if (it.isEmpty()) "None" else it.joinToString(", ") }, "botPageChannels") { nav.open(BotRoute.Channels(bot.id)) }
        }
    }
    if (editOpen) EditBotSheet(bot.id, onDismiss = { editOpen = false }, backend = backend) { editOpen = false; scope.launch { store.refreshAll() } }
}

/** iOS BotSummaryTiles: sessions, messages, tokens, spend. */
@Composable
private fun BotSummaryTiles(s: BotSummary?) {
    val c = AllternitTheme.colors
    val tiles = listOf(
        "Sessions" to (s?.sessions?.toString() ?: "–"),
        "Messages" to (s?.messages?.toString() ?: "–"),
        "Tokens" to (s?.tokens?.let { if (it >= 1000) "${it / 1000}k" else "$it" } ?: "–"),
        "Spend" to (s?.costUsd?.let { String.format(java.util.Locale.US, "$%.2f", it) } ?: "–"),
    )
    Row(Modifier.padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        tiles.forEach { (label, value) ->
            Column(Modifier.weight(1f).clip(RoundedCornerShape(12.dp)).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(12.dp)).padding(10.dp)) {
                Text(value, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                Text(label, color = c.textTertiary, fontSize = 11.sp)
            }
        }
    }
}

/** The bot screen on its tabs (iOS BotHomeTabsView): Chat, Artifacts, Routines, Memory, Settings. */
@Composable
internal fun BotHomeTabsScreen(bot: BotRecord, initial: BotHomeTab, host: BotsHost, store: BotModeStore, nav: BotNavigator) {
    val c = AllternitTheme.colors
    var tab by remember { mutableStateOf(initial) }
    BotDockFrame { dockContent -> Column(Modifier.fillMaxSize().statusBarsPadding().testTag("botHomeTabs")) {
        LibraryHeader(title = bot.displayName, onBack = nav::back, backTag = "botsBack")
        Spacer(Modifier.height(48.dp))
        PillRow(BotHomeTab.entries, tab, { it.label }, { "botHomeTab${it.name}" }, Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) { tab = it }
        Box(Modifier.weight(1f).fillMaxWidth().then(dockContent)) {
            when (tab) {
                BotHomeTab.Threads -> Column {
                    Row(
                        Modifier.padding(16.dp).fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(14.dp))
                            .testTag("botHomeMainChat").clickable { nav.open(BotRoute.Chat(bot.id, null)) }.padding(12.dp),
                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        BotAvatar(bot, 36.dp)
                        Column(Modifier.weight(1f)) {
                            Text("Main chat", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                            Text(bot.tagline, color = c.textSecondary, fontSize = 12.5.sp, maxLines = 1)
                        }
                        Icon(painterResource(com.allternit.core.design.R.drawable.ph_caret_right), null, tint = c.textTertiary, modifier = Modifier.size(14.dp))
                    }
                    BotThreadsOverview(store, bot.id, showTitle = false) { nav.openThread(it) }
                }
                BotHomeTab.Artifacts -> BotArtifactsList(bot, host)
                BotHomeTab.Routines -> BotRoutinesView(bot)
                BotHomeTab.Memory -> BotMemoryView(bot, nav, host.backend)
                BotHomeTab.Config -> AgentDetailView(bot, store, nav)
            }
        }
    } }
}

/** Artifacts the bot made in its chats (iOS BotArtifactsList): the library filtered to the bot's sessions. */
@Composable
internal fun BotArtifactsList(bot: BotRecord, host: BotsHost) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var artifacts by remember { mutableStateOf<List<ArtifactRecord>>(emptyList()) }
    suspend fun load() {
        loading = true
        when (val s = BotsApi.listSessions()) {
            is BotsResult.Ok -> {
                val ids = s.value.filter { it.agentId == bot.id }.map { it.id }.toSet()
                when (val a = LibraryNet.get("artifacts")) {
                    is com.allternit.core.api.ApiResult.Ok -> {
                        artifacts = a.value.listUnder("artifacts", "items").filter { (it.str("sessionId") ?: it.str("session_id")) in ids }.mapNotNull { o ->
                            ArtifactRecord(o.str("id") ?: return@mapNotNull null, o.str("type") ?: o.str("kind") ?: "document", o.str("title") ?: "Artifact", o.str("url"), o.str("content"))
                        }
                        error = null
                    }
                    else -> error = "Couldn't load artifacts."
                }
            }
            is BotsResult.Failed -> error = s.message
        }
        loading = false
    }
    LaunchedEffect(bot.id) { load() }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).testTag("botArtifacts")) {
        if (!ListState(loading, error, artifacts.isEmpty(), "Couldn't load artifacts", "Artifacts ${bot.displayName} makes in its chats show here.") { scope.launch { load() } }) {
            RowGroup(Modifier.padding(vertical = 8.dp)) {
                artifacts.forEachIndexed { i, a ->
                    NavRow(a.title, com.allternit.core.design.R.drawable.ph_file_text, a.kind.uppercase(), "botArtifact${a.id}", last = i == artifacts.lastIndex) { host.openArtifact(a) }
                }
            }
        }
    }
}

/** iOS BotRoutinesView: the bot's scheduled routines with the time editor, then its other routines. */
@Composable
internal fun BotRoutinesView(bot: BotRecord) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var scheduled by remember { mutableStateOf<List<AutomationRoutine>>(emptyList()) }
    var others by remember { mutableStateOf<List<AutomationRoutine>>(emptyList()) }
    var editing by remember { mutableStateOf<AutomationRoutine?>(null) }
    var saving by remember { mutableStateOf(false) }
    var saveError by remember { mutableStateOf<String?>(null) }
    fun title(name: String): String = if (name.startsWith("[bot:")) name.substringAfter("]").trim() else name
    suspend fun load() {
        loading = true
        when (val r = BotsApi.listRoutines()) {
            is BotsResult.Ok -> {
                val mine = r.value.filter { it.agentId == bot.id }
                scheduled = mine.filter { it.scheduleType == "cron" }
                others = mine.filter { it.scheduleType != "cron" }
                error = null
            }
            is BotsResult.Failed -> error = r.message
        }
        loading = false
    }
    LaunchedEffect(bot.id) { load() }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp).testTag("botRoutines")) {
        if (scheduled.isNotEmpty()) {
            SectionHeader("On a schedule")
            RowGroup {
                scheduled.forEachIndexed { i, r ->
                    val label = listOfNotNull(RoutineRule.fromCron(r.scheduleExpression)?.label ?: r.scheduleExpression, if (r.isEnabled) null else "Paused").joinToString(" · ")
                    Column(Modifier.fillMaxWidth().clickable { saveError = null; editing = r }.testTag("botRoutine${r.id}").padding(horizontal = 16.dp, vertical = 10.dp)) {
                        Text(title(r.name), color = c.textPrimary, fontSize = 15.sp, maxLines = 2)
                        Text(label, color = c.textSecondary, fontSize = 12.5.sp)
                    }
                    if (i < scheduled.lastIndex) Hairline()
                }
            }
        }
        if (!ListState(loading, error, scheduled.isEmpty() && others.isEmpty(), "Couldn't load routines", "${bot.displayName} has no routines yet. Add one from Automation.") { scope.launch { load() } } && others.isNotEmpty()) {
            SectionHeader("Other routines")
            RowGroup {
                others.forEachIndexed { i, r ->
                    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp)) {
                        Text(title(r.name), color = c.textPrimary, fontSize = 15.sp, maxLines = 1)
                        Text(listOfNotNull(r.scheduleType, if (r.isEnabled) null else "Paused").joinToString(" · "), color = c.textSecondary, fontSize = 12.5.sp)
                    }
                    if (i < others.lastIndex) Hairline()
                }
            }
        }
    }
    editing?.let { r ->
        RoutineScheduleEditor(
            title = title(r.name),
            initial = RoutineRule.fromCron(r.scheduleExpression) ?: RoutineRule(RoutineRule.Repeat.Weekdays, 9 * 60, days = RoutineRule.WEEKDAYS),
            saving = saving, error = saveError,
            onSave = { rule ->
                saving = true
                scope.launch {
                    when (val res = BotsApi.updateSchedule(r.id, rule.cron)) {
                        is BotsResult.Ok -> { editing = null; load() }
                        is BotsResult.Failed -> saveError = "Couldn’t save the schedule: ${res.message}"
                    }
                    saving = false
                }
            },
            onCancel = { editing = null },
        )
    }
}

/** iOS BotMemoryView: the impact map, Teach <bot>, and what it remembers (search, swipe-free Forget). */
@Composable
internal fun BotMemoryView(bot: BotRecord, nav: BotNavigator, backend: com.allternit.feature.chat.ChatBackend? = null) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var memories by remember { mutableStateOf<List<BotMemoryItem>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var draft by remember { mutableStateOf("") }
    var search by remember { mutableStateOf("") }
    suspend fun load() {
        loading = true
        when (val r = BotsApi.listMemories(bot.id)) {
            is BotsResult.Ok -> { memories = r.value; error = null }
            is BotsResult.Failed -> error = r.message
        }
        loading = false
    }
    LaunchedEffect(bot.id) { load() }
    val q = search.trim()
    val filtered = if (q.isEmpty()) memories else memories.filter { it.content.contains(q, ignoreCase = true) }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp).testTag("botMemory")) {
        LibrarySearchField(search, "Search memory", "botMemorySearch", Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) { search = it }
        ImpactMapView(bot.id, backend = backend, openAction = { item -> impactOpen(bot, nav, item) }, onChanged = { scope.launch { load() } })
        SectionHeader("Teach ${bot.displayName}")
        RowGroup {
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.weight(1f).heightIn(min = 36.dp), contentAlignment = Alignment.CenterStart) {
                    if (draft.isEmpty()) Text("Something to remember", color = c.textTertiary, fontSize = 15.sp)
                    BasicTextField(draft, { draft = it }, textStyle = TextStyle(color = c.textPrimary, fontSize = 15.sp), cursorBrush = SolidColor(c.textPrimary), modifier = Modifier.fillMaxWidth().testTag("botMemoryDraft"))
                }
                Pill("Add", "botMemoryAdd", enabled = draft.isNotBlank()) {
                    val text = draft.trim()
                    draft = ""
                    scope.launch {
                        when (val r = BotsApi.addMemory(bot.id, text)) {
                            is BotsResult.Ok -> load()
                            is BotsResult.Failed -> { error = r.message; draft = text }
                        }
                    }
                }
            }
        }
        Gap(8.dp)
        if (!ListState(loading, error, filtered.isEmpty() && memories.isEmpty(), "Couldn't load memory", "Nothing remembered yet.", com.allternit.core.design.EmptyArtKind.Sheets) { scope.launch { load() } }) {
            if (filtered.isEmpty()) Text("No matches.", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(20.dp))
            else RowGroup {
                filtered.forEachIndexed { i, m ->
                    Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 4.dp, top = 8.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                            Text(m.content, color = c.textPrimary, fontSize = 14.sp)
                            Text(listOfNotNull(m.type, m.source, botTimeRelative(m.createdAt).ifEmpty { null }).joinToString(" · "), color = c.textTertiary, fontSize = 11.5.sp)
                        }
                        Pill("Forget", "botMemoryForget${m.id}") {
                            memories = memories.filter { it.id != m.id }
                            scope.launch { if (BotsApi.forgetMemory(m.id) is BotsResult.Failed) load() }
                        }
                    }
                    if (i < filtered.lastIndex) Hairline()
                }
            }
        }
    }
}

/** iOS BotChannelsView: each channel with its toggle, the bot's email identity, Connect a channel. */
@Composable
internal fun BotChannelsView(bot: BotRecord) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var channels by remember { mutableStateOf<List<BotChannel>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    suspend fun load() {
        loading = true
        when (val r = BotsApi.listChannels(bot.id)) {
            is BotsResult.Ok -> { channels = r.value; error = null }
            is BotsResult.Failed -> error = r.message
        }
        loading = false
    }
    LaunchedEffect(bot.id) { load() }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(vertical = 8.dp).testTag("botChannels")) {
        if (!ListState(loading, error, channels.isEmpty(), "Couldn't load channels", "No messaging connections yet.", com.allternit.core.design.EmptyArtKind.Envelope) { scope.launch { load() } }) {
            RowGroup {
                channels.forEachIndexed { i, ch ->
                    ToggleRow(
                        ch.label, listOfNotNull(ch.handle?.let { "@$it" }, ch.state, if (ch.isDefault) "default" else null).joinToString(" · "), ch.enabled, "botChannel${ch.accountId}",
                    ) { on ->
                        scope.launch {
                            (BotsApi.setChannel(bot.id, ch.accountId, on) as? BotsResult.Failed)?.let { error = it.message }
                            load()
                        }
                    }
                    if (i < channels.lastIndex) Hairline()
                }
            }
        }
        bot.email?.let { email ->
            SectionHeader("Identity")
            RowGroup { NavRow(email, com.allternit.core.design.R.drawable.ph_link, null, "botChannelEmail") {} }
        }
        Gap(12.dp)
        // Giving a bot a phone number (iOS GiveBotPhoneSheet) is wave F's Phone work; Connections covers the rest.
        Text("Connect a channel from the bot's phone settings on the web or iPhone app.", color = c.textTertiary, fontSize = 12.5.sp, modifier = Modifier.padding(horizontal = 20.dp))
    }
}

@Suppress("unused")
private val transparent = Color.Transparent
