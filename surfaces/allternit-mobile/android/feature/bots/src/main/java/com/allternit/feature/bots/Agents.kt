package com.allternit.feature.bots

import com.allternit.core.design.AllternitSerif
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
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
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
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.ProvideDockScroll
import com.allternit.feature.artifacts.LibraryField
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.artifacts.LibrarySearchField
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.LibrarySpinner
import com.allternit.feature.artifacts.PillRow
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import com.allternit.core.design.R as DesignR

// iOS Features/Agents: the agent hub, agent detail, Bot Home, marketplace, workspace files and
// the file editor, webhooks.

private enum class HubTab(val label: String) { Agents("Agents"), Sessions("Sessions"), Workspace("Workspace") }

/**
 * The agent | bot hub (iOS AgentHubView): Agents (Crew, Templates, Bots), Sessions (bot runs) and
 * Workspace (pick an agent to browse its files); Discover opens the marketplace, + new from template.
 */
@Composable
internal fun AgentHubScreen(host: BotsHost, store: BotModeStore, nav: BotNavigator) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var tab by remember { mutableStateOf(HubTab.Agents) }
    var search by remember { mutableStateOf("") }
    var templatesOpen by remember { mutableStateOf(false) }
    var pendingDelete by remember { mutableStateOf<BotRecord?>(null) }
    var notice by remember { mutableStateOf<String?>(null) }
    var sessions by remember { mutableStateOf<List<BotSessionRow>?>(null) }
    var sessionsError by remember { mutableStateOf<String?>(null) }
    var workspaceFor by remember { mutableStateOf<BotRecord?>(null) }
    var sessionsReload by remember { mutableStateOf(0) }
    LaunchedEffect(Unit) { store.refreshAgents() }
    LaunchedEffect(tab, sessionsReload) {
        if (tab == HubTab.Sessions && sessions == null) when (val r = BotsApi.listSessions()) {
            is BotsResult.Ok -> sessions = r.value.filter { s -> s.agentId != null && store.bot(s.agentId)?.isBot == true }
            is BotsResult.Failed -> sessionsError = r.message
        }
    }
    val q = search.trim().lowercase()
    val agents = store.agents.filter { q.isEmpty() || it.displayName.lowercase().contains(q) || it.name.lowercase().contains(q) }
    BotScreen("agent | bot hub", nav, testTag = "agentHub", trailing = {
        LibraryIconButton(DesignR.drawable.ph_globe, "Discover agents and bots", "hubDiscover") { nav.open(BotRoute.Marketplace) }
        LibraryIconButton(DesignR.drawable.ph_plus, "Create new agent", "hubCreate") { templatesOpen = true }
    }) {
        Column(Modifier.fillMaxSize()) {
            PillRow(HubTab.entries, tab, { it.label }, { "hubTab${it.name}" }, Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) { tab = it }
            Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
                when (tab) {
                    HubTab.Agents -> {
                        LibrarySearchField(search, "Search agents | bots", "hubSearch", Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) { search = it }
                        notice?.let { ErrorLine(it) }
                        if (!ListState(store.agentsLoading, store.agentsError, store.agents.isEmpty(), "Couldn't load agents", "Agents and bots run tasks on your behalf — create one from a template to get started.") { scope.launch { store.refreshAgents() } }) {
                            val crew = agents.filter { !it.isBot }
                            val bots = agents.filter { it.isBot }
                            if (crew.isEmpty() && bots.isEmpty()) Text("No agents or bots match your search.", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(20.dp))
                            if (crew.isNotEmpty()) { SectionHeader("Crew"); AgentGroup(crew, store, nav) { pendingDelete = it } }
                            SectionHeader("Templates")
                            RowGroup {
                                NavRow("New from template", com.allternit.core.design.R.drawable.ph_squares_four, "Research, review, write, and more", "hubNewFromTemplate", last = false) { templatesOpen = true }
                                NavRow("Discover agents | bots", DesignR.drawable.ph_globe, "Browse the marketplace", "hubMarketplace") { nav.open(BotRoute.Marketplace) }
                            }
                            if (bots.isNotEmpty()) { SectionHeader("Bots"); AgentGroup(bots, store, nav) { pendingDelete = it } }
                        }
                    }
                    HubTab.Sessions -> {
                        val list = sessions
                        if (!ListState(list == null && sessionsError == null, sessionsError, list.isNullOrEmpty(), "Couldn't load bot sessions", "No bot sessions yet. Start a bot to see its runs here.", com.allternit.core.design.EmptyArtKind.Envelope) {
                                sessions = null; sessionsError = null; sessionsReload++
                            } && list != null) {
                            SectionHeader("Bot sessions")
                            RowGroup {
                                list.forEachIndexed { i, s ->
                                    Column(Modifier.fillMaxWidth().clickable { s.agentId?.let { nav.open(BotRoute.Chat(it, s.id)) } }.testTag("hubSession${s.id}").padding(horizontal = 16.dp, vertical = 10.dp)) {
                                        Text(s.name, color = c.textPrimary, fontSize = 15.sp, maxLines = 1)
                                        Text("${s.agentName ?: store.bot(s.agentId ?: "")?.displayName ?: "Bot"} · ${s.messageCount} message${if (s.messageCount == 1) "" else "s"} · ${s.updatedAt.take(10)}", color = c.textSecondary, fontSize = 12.sp)
                                    }
                                    if (i < list.lastIndex) Hairline()
                                }
                            }
                        }
                    }
                    HubTab.Workspace -> {
                        if (store.agents.isEmpty()) Text("Create an agent or bot first, then select it here to browse its workspace files.", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(20.dp))
                        else {
                            SectionHeader("Select an agent or bot to inspect its workspace")
                            RowGroup {
                                store.agents.forEachIndexed { i, a -> NavRow(a.displayName, null, null, "hubWorkspace${a.id}", last = i == store.agents.lastIndex) { workspaceFor = a } }
                            }
                        }
                    }
                }
            }
        }
    }
    if (templatesOpen) AgentTemplateSheet(onDismiss = { templatesOpen = false }) { t ->
        templatesOpen = false
        scope.launch {
            when (val r = BotsApi.createFromTemplate(t.id)) {
                is BotsResult.Ok -> { store.refreshAgents(); notice = null }
                is BotsResult.Failed -> notice = r.message
            }
        }
    }
    pendingDelete?.let { a ->
        ConfirmSheet("Delete ${a.displayName}?", "This permanently deletes the agent. This can't be undone.", "Delete", "hubDeleteConfirm", onDismiss = { pendingDelete = null }) {
            pendingDelete = null
            scope.launch {
                when (val r = BotsApi.deleteAgent(a.id)) {
                    is BotsResult.Ok -> store.refreshAgents()
                    is BotsResult.Failed -> notice = r.message
                }
            }
        }
    }
    workspaceFor?.let { a -> BotWorkspaceFilesSheet(a, nav, onDismiss = { workspaceFor = null }) }
}

@Composable
private fun AgentGroup(list: List<BotRecord>, store: BotModeStore, nav: BotNavigator, onDelete: (BotRecord) -> Unit) {
    val c = AllternitTheme.colors
    RowGroup {
        list.forEachIndexed { i, a ->
            Row(Modifier.fillMaxWidth().testTag("hubAgent${a.id}").clickable { nav.open(if (a.isBot) BotRoute.Home(a.id) else BotRoute.Settings(a.id)) }.padding(start = 16.dp, end = 4.dp, top = 8.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                BotAvatar(a, 34.dp)
                Column(Modifier.weight(1f)) {
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                        com.allternit.core.design.BotNameText(a.displayName, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1)
                        if (a.isPrimary) Text("PRIMARY", color = c.textTertiary, fontSize = 10.sp, fontWeight = FontWeight.SemiBold)
                    }
                    val parent = a.parentAgentId?.let { store.bot(it)?.displayName }
                    Text(parent?.let { "crew of $it" } ?: listOfNotNull(a.modelShort, a.status).joinToString(" · "), color = c.textSecondary, fontSize = 12.sp, maxLines = 1)
                }
                if (!a.isPrimary) LibraryIconButton(DesignR.drawable.ph_trash, "Delete ${a.displayName}", "hubDelete${a.id}", tint = c.textTertiary, iconSize = 18) { onDelete(a) }
            }
            if (i < list.lastIndex) Hairline()
        }
    }
}

/**
 * The agent editor (iOS AgentDetailView): Edit name, look and job; the system prompt, greeting and
 * suggested prompts; the model (Pick a Brain); workspace files; webhook triggers; behavior; Publish.
 */
@Composable
internal fun AgentDetailView(bot: BotRecord, store: BotModeStore, nav: BotNavigator) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var promptOpen by remember { mutableStateOf(false) }
    var greetingOpen by remember { mutableStateOf(false) }
    var brainOpen by remember { mutableStateOf(false) }
    var filesOpen by remember { mutableStateOf(false) }
    var webhooksOpen by remember { mutableStateOf(false) }
    var publishOpen by remember { mutableStateOf(false) }
    var editOpen by remember { mutableStateOf(false) }
    var behaviorOpen by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(vertical = 8.dp).testTag("agentDetail")) {
        Row(Modifier.padding(horizontal = 20.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            BotAvatar(bot, 52.dp)
            Column(Modifier.weight(1f)) {
                com.allternit.core.design.BotNameText(bot.displayName, color = c.textPrimary, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
                Text(bot.tagline, color = c.textSecondary, fontSize = 13.sp, maxLines = 2)
            }
        }
        if (bot.isBot) RowGroup { NavRow("Edit name, look and job", com.allternit.core.design.R.drawable.ph_pencil_simple, null, "agentEditLook") { editOpen = true } }
        SectionHeader("Identity")
        RowGroup {
            NavRow("System prompt", com.allternit.core.design.R.drawable.ph_file_text, if (bot.systemPrompt.isNullOrEmpty()) "Add" else "Edit", "agentSystemPrompt", last = false) { promptOpen = true }
            NavRow("Greeting", com.allternit.core.design.R.drawable.ph_hand, if (bot.greeting.isNullOrEmpty()) "Add" else "Edit", "agentGreeting") { greetingOpen = true }
        }
        Text("Both prompt sources are active: the platform layers the persona and instruction files with the system prompt — neither overrides the other.", color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp))
        SectionHeader("Model")
        RowGroup { NavRow("Brain", DesignR.drawable.ph_cpu, if (bot.model.isEmpty()) "Default" else "${bot.provider}/${bot.modelShort}", "agentModel") { brainOpen = true } }
        SectionHeader("Workspace files")
        RowGroup { NavRow("Workspace files", com.allternit.core.design.R.drawable.ph_folder, null, "agentWorkspaceFiles") { filesOpen = true } }
        SectionHeader("Behavior")
        RowGroup {
            NavRow("Behavior", DesignR.drawable.ph_sliders_horizontal, "View", "agentBehavior", last = false) { behaviorOpen = true }
            NavRow("Webhook triggers", DesignR.drawable.ph_webhooks_logo, null, "agentWebhooks") { webhooksOpen = true }
        }
        SectionHeader("Marketplace")
        RowGroup { NavRow("Publish to Marketplace", DesignR.drawable.ph_upload_simple, null, "agentPublish") { publishOpen = true } }
        Text("Shares a snapshot of this agent so others can install their own copy.", color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp))
    }
    if (promptOpen) TextEditSheet("System Prompt", bot.systemPrompt.orEmpty(), "agentPromptSheet", onDismiss = { promptOpen = false }) { text ->
        val r = BotsApi.updateAgent(bot.id, buildJsonObject { put("system_prompt", text) })
        if (r is BotsResult.Ok) { promptOpen = false; store.refreshAgents() }
        (r as? BotsResult.Failed)?.message
    }
    if (greetingOpen) GreetingEditorSheet(bot, onDismiss = { greetingOpen = false }) { greetingOpen = false; scope.launch { store.refreshAgents() } }
    if (brainOpen) BotBrainPickerSheet(bot, null, onDismiss = { brainOpen = false }) { brainOpen = false; scope.launch { store.refreshAgents() } }
    if (filesOpen) BotWorkspaceFilesSheet(bot, nav, onDismiss = { filesOpen = false })
    if (webhooksOpen) BotWebhooksSheet(onDismiss = { webhooksOpen = false })
    if (publishOpen) PublishAgentSheet(bot, onDismiss = { publishOpen = false })
    if (editOpen) EditBotSheet(bot.id, onDismiss = { editOpen = false }) { editOpen = false; scope.launch { store.refreshAgents() } }
    if (behaviorOpen) LibrarySheet(title = "Behavior", testTag = "agentBehaviorSheet", onDismiss = { behaviorOpen = false }, trailing = LibrarySheetAction("Done", bold = true) { behaviorOpen = false }) {
        RowGroup(Modifier.padding(vertical = 8.dp)) {
            NavRow("Type", null, bot.type, "behaviorType", last = false) {}
            NavRow("Trust tier", null, bot.trustTier, "behaviorTrust", last = false) {}
            NavRow("Max iterations", null, bot.raw["max_iterations"]?.let { (it as? JsonPrimitive)?.content } ?: "—", "behaviorIterations", last = false) {}
            NavRow("Temperature", null, bot.raw["temperature"]?.let { (it as? JsonPrimitive)?.content } ?: "—", "behaviorTemperature") {}
        }
        Text("These values are set when the agent is created. Edit them from the agent registry on the web.", color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.padding(20.dp))
    }
}

/** A long text editor sheet (iOS SystemPromptEditorSheet). [onSave] returns an error or null. */
@Composable
private fun TextEditSheet(title: String, initial: String, testTag: String, onDismiss: () -> Unit, onSave: suspend (String) -> String?) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var text by remember { mutableStateOf(initial) }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    LibrarySheet(
        title = title, testTag = testTag, onDismiss = onDismiss, leading = LibrarySheetAction("Cancel", onClick = onDismiss),
        trailing = LibrarySheetAction(if (saving) "Saving…" else "Save", enabled = !saving && text != initial, bold = true, testTag = "${testTag}Save") {
            saving = true
            scope.launch { error = onSave(text); saving = false }
        },
    ) {
        Box(Modifier.padding(16.dp).fillMaxWidth().heightIn(min = 320.dp, max = 560.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(12.dp)) {
            BasicTextField(text, { text = it }, textStyle = TextStyle(color = c.textPrimary, fontSize = 14.sp, fontFamily = FontFamily.Monospace, lineHeight = 20.sp), cursorBrush = SolidColor(c.textPrimary), modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).testTag("${testTag}Field"))
        }
        error?.let { ErrorLine(it) }
    }
}

/** iOS GreetingEditorSheet: the greeting and up to four suggested prompts (blank rows dropped). */
@Composable
private fun GreetingEditorSheet(bot: BotRecord, onDismiss: () -> Unit, onSaved: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var greeting by remember { mutableStateOf(bot.greeting.orEmpty()) }
    var prompts by remember { mutableStateOf((bot.suggestedPrompts + List(4) { "" }).take(4)) }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    LibrarySheet(
        title = "Greeting", testTag = "agentGreetingSheet", onDismiss = onDismiss, leading = LibrarySheetAction("Cancel", onClick = onDismiss),
        trailing = LibrarySheetAction(if (saving) "Saving…" else "Save", enabled = !saving, bold = true, testTag = "agentGreetingSave") {
            saving = true
            scope.launch {
                val cfg = bot.config.toMutableMap()
                if (greeting.isBlank()) cfg.remove("greeting") else cfg["greeting"] = JsonPrimitive(greeting)
                val kept = prompts.filter { it.isNotBlank() }
                if (kept.isEmpty()) cfg.remove("suggested_prompts") else cfg["suggested_prompts"] = JsonArray(kept.map { JsonPrimitive(it) })
                when (val r = BotsApi.updateAgent(bot.id, buildJsonObject { put("config", JsonObject(cfg)) })) {
                    is BotsResult.Ok -> onSaved()
                    is BotsResult.Failed -> error = r.message
                }
                saving = false
            }
        },
    ) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            LibraryField("Greeting", greeting, "Hi! What should we work on?", "agentGreetingField", singleLine = false, minHeight = 80) { greeting = it }
            Text("Shown at the top of a fresh chat when this agent is selected.", color = c.textTertiary, fontSize = 12.sp)
            prompts.forEachIndexed { i, p -> LibraryField(if (i == 0) "Suggested prompts" else "", p, "Suggested prompt", "agentPrompt$i") { v -> prompts = prompts.toMutableList().also { it[i] = v } } }
            Text("Quick-start chips shown next to the greeting. Leave blank rows empty — they're dropped on save.", color = c.textTertiary, fontSize = 12.sp)
            error?.let { ErrorLine(it, Modifier.padding(0.dp)) }
        }
    }
}

/**
 * Bot Home (iOS BotHomeView): Welcome (Chat, Run Task), Tasks (threads), Artifacts, Automation,
 * Runtime (connectors, secrets, webhooks, virtual computer) and Config.
 */
@Composable
internal fun BotHomeScreen(bot: BotRecord, host: BotsHost, store: BotModeStore, nav: BotNavigator) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var threads by remember { mutableStateOf<List<BotThread>?>(null) }
    var threadsError by remember { mutableStateOf<String?>(null) }
    var routines by remember { mutableStateOf<Int?>(null) }
    var files by remember { mutableStateOf<Int?>(null) }
    var webhooks by remember { mutableStateOf<Int?>(null) }
    var runOpen by remember { mutableStateOf(false) }
    suspend fun load() = coroutineScope {
        val t = async { BotsApi.listBotThreads(bot.id) }
        val r = async { BotsApi.listRoutines().orNull() }
        val f = async { BotsApi.listWorkspaceFiles(bot.id).orNull() }
        val w = async { BotsApi.listWebhooks().orNull() }
        when (val tr = t.await()) { is BotsResult.Ok -> { threads = tr.value; threadsError = null }; is BotsResult.Failed -> threadsError = tr.message }
        routines = r.await()?.count { it.agentId == bot.id }
        files = f.await()?.size
        webhooks = w.await()?.size
    }
    LaunchedEffect(bot.id) { load() }
    val accent = bot.accentHex?.let { hexColor(it) } ?: c.accentPrimary
    BotScreen(bot.name, nav, testTag = "botHome") {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            HomeCard("Welcome", accent) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    BotAvatar(bot, 48.dp, live = true)
                    Text(bot.greeting ?: bot.tagline, color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.weight(1f))
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Pill("Chat", "botHomeChat", primary = true, icon = com.allternit.core.design.R.drawable.ph_chat_circle) { nav.open(BotRoute.Chat(bot.id, null)) }
                    Pill("Run Task", "botHomeRunTask", icon = DesignR.drawable.ph_play_fill) { runOpen = true }
                }
            }
            HomeCard("Tasks", accent) {
                val list = threads
                when {
                    threadsError != null && list == null -> Row(verticalAlignment = Alignment.CenterVertically) {
                        Text("Couldn't load tasks", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.weight(1f))
                        Pill("Retry", "botHomeTasksRetry") { scope.launch { load() } }
                    }
                    list == null -> LibrarySpinner("Loading tasks")
                    list.isEmpty() -> {
                        Text("No tasks yet", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                        Text("Delegate a task to this bot and it will run in its workspace or sandbox.", color = c.textSecondary, fontSize = 13.sp)
                        Pill("Run First Task", "botHomeRunFirst", icon = DesignR.drawable.ph_play_fill) { runOpen = true }
                    }
                    else -> list.sortedWith(BotThread.order).take(5).forEach { t -> BotThreadRow(t, store, false) { nav.openThread(t) } }
                }
            }
            HomeCard("Artifacts", accent) {
                Text("Artifacts produced by this bot in chat will appear here.", color = c.textSecondary, fontSize = 13.sp)
                Pill("View library", "botHomeArtifacts") { nav.open(BotRoute.BotHome(bot.id, BotHomeTab.Artifacts)) }
            }
            HomeCard("Automation", accent) {
                Text(routines?.let { if (it == 0) "No cron jobs, routines, or loops for this bot yet." else "$it scheduled" } ?: "…", color = c.textSecondary, fontSize = 13.sp)
                Pill("View all", "botHomeAutomation") { nav.open(BotRoute.BotHome(bot.id, BotHomeTab.Routines)) }
            }
            HomeCard("Runtime", accent) {
                val connectors = (bot.raw["connector_bindings"] as? JsonArray ?: bot.config["connectorBindings"] as? JsonArray)?.size ?: 0
                val secrets = (bot.raw["secret_refs"] as? JsonArray ?: bot.config["secretRefs"] as? JsonArray)?.size ?: 0
                RuntimeLine("Connectors", if (connectors == 0) "None" else "$connectors")
                if (connectors == 0) Text("No connectors bound yet. Connect apps so this bot can act on your behalf.", color = c.textTertiary, fontSize = 12.sp)
                RuntimeLine("Secrets", if (secrets == 0) "No secrets declared." else "$secrets")
                RuntimeLine("Webhooks", webhooks?.toString() ?: "…")
                RuntimeLine("Virtual Computer", "View")
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Pill("Connections", "botHomeConnections") { host.openConnections() }
                    Pill("Computer", "botHomeComputer", icon = com.allternit.core.design.R.drawable.ph_desktop) { nav.open(BotRoute.Computer(bot.id)) }
                }
            }
            HomeCard("Config", accent) {
                RuntimeLine("Brain", if (bot.model.isEmpty()) "Default" else "${bot.provider}/${bot.model}")
                RuntimeLine("System prompt", if (bot.systemPrompt.isNullOrEmpty()) "Add" else "Edit")
                RuntimeLine("Greeting", if (bot.greeting.isNullOrEmpty()) "Add" else "Edit")
                RuntimeLine("Workspace files", files?.toString() ?: "…")
                Pill("Open settings", "botHomeSettings", icon = com.allternit.core.design.R.drawable.ph_gear) { nav.open(BotRoute.Settings(bot.id)) }
            }
        }
    }
    if (runOpen) RunTaskSheet(bot, onDismiss = { runOpen = false }) { sid -> runOpen = false; nav.open(BotRoute.Chat(bot.id, sid)); scope.launch { store.refreshThreads() } }
}

@Composable
private fun HomeCard(title: String, accent: androidx.compose.ui.graphics.Color, content: @Composable () -> Unit) {
    val c = AllternitTheme.colors
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(16.dp)).padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Dot(accent, 8.dp)
            Text(title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
        }
        content()
    }
}

@Composable
private fun RuntimeLine(label: String, value: String) {
    val c = AllternitTheme.colors
    Row(Modifier.fillMaxWidth()) {
        Text(label, color = c.textPrimary, fontSize = 14.sp, modifier = Modifier.weight(1f))
        Text(value, color = c.textSecondary, fontSize = 13.sp)
    }
}

/** iOS BotHomeView task composer: quick-start chips and the task prompt; Run starts a thread with it. */
@Composable
private fun RunTaskSheet(bot: BotRecord, onDismiss: () -> Unit, onStarted: (String) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var text by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    LibrarySheet(
        title = "Run Task", testTag = "botRunTaskSheet", onDismiss = onDismiss, leading = LibrarySheetAction("Cancel", onClick = onDismiss),
        trailing = LibrarySheetAction(if (busy) "Starting…" else "Run", enabled = text.isNotBlank() && !busy, bold = true, testTag = "botRunTaskRun") {
            busy = true
            scope.launch {
                val sid = BotsApi.createBotSession(bot, BotModeStore.projectTitle(text), mapOf("botThreadOf" to bot.id))
                val sent = (sid as? BotsResult.Ok)?.let { BotsApi.sendUserMessage(it.value, text.trim()) }
                if (sid is BotsResult.Ok && sent is BotsResult.Ok) onStarted(sid.value) else error = (sent as? BotsResult.Failed)?.message ?: (sid as? BotsResult.Failed)?.message
                busy = false
            }
        },
    ) {
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            val quick = bot.starterPrompts.ifEmpty { bot.suggestedPrompts }.take(4)
            if (quick.isNotEmpty()) {
                Text("Quick start", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.Medium)
                quick.forEach { q -> Text(q, color = c.textPrimary, fontSize = 13.5.sp, modifier = Modifier.clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).clickable { text = q }.padding(horizontal = 12.dp, vertical = 8.dp)) }
            }
            LibraryField("", text, "Describe the task you want this bot to run…", "botRunTaskPrompt", singleLine = false, minHeight = 140) { text = it }
            error?.let { ErrorLine(it, Modifier.padding(0.dp)) }
        }
    }
}

/** iOS MarketplaceView: published agents and bots with publisher, rating and installs. */
@Composable
internal fun MarketplaceScreen(nav: BotNavigator) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var listings by remember { mutableStateOf<List<MarketplaceListing>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var query by remember { mutableStateOf("") }
    suspend fun load() {
        loading = true
        when (val r = BotsApi.listListings(query)) { is BotsResult.Ok -> { listings = r.value; error = null }; is BotsResult.Failed -> error = r.message }
        loading = false
    }
    LaunchedEffect(query) { load() }
    val scroll = rememberScrollState()
    ProvideDockScroll(scroll) {
    BotScreen("Marketplace", nav, testTag = "marketplace") {
        Column(Modifier.fillMaxSize().verticalScroll(scroll).padding(bottom = 24.dp)) {
            LibrarySearchField(query, "Search the marketplace", "marketplaceSearch", Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) { query = it }
            if (!ListState(loading, error, listings.isEmpty(), "Couldn't load agents", "No published agents or bots yet. Check back later or publish your own from the workspace.") { scope.launch { load() } }) {
                listings.forEach { l ->
                    Column(
                        Modifier.padding(horizontal = 16.dp, vertical = 5.dp).fillMaxWidth().clip(RoundedCornerShape(20.dp)).background(c.fillNeutral)
                            .testTag("marketplaceListing${l.id}").clickable { nav.open(BotRoute.Listing(l.id)) }.padding(12.dp),
                        verticalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        Text(l.title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                        Text(l.description, color = c.textSecondary, fontSize = 13.sp, maxLines = 3)
                        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            l.publisherName?.let { Text(it, color = c.textTertiary, fontSize = 12.sp) }
                            Text("★ ${String.format(java.util.Locale.US, "%.1f", l.ratingAvg)} (${l.ratingCount})", color = c.textTertiary, fontSize = 12.sp)
                            Text("↓ ${l.installCount}", color = c.textTertiary, fontSize = 12.sp)
                        }
                    }
                }
            }
        }
    }
    }
}

/** iOS MarketplaceListingDetailView: details, Install, Rate this agent, reviews. */
@Composable
internal fun MarketplaceListingScreen(id: String, store: BotModeStore, nav: BotNavigator) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var detail by remember { mutableStateOf<Pair<MarketplaceListing, List<Triple<Int, String?, String?>>>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var installing by remember { mutableStateOf(false) }
    var installed by remember { mutableStateOf<String?>(null) }
    var installError by remember { mutableStateOf<String?>(null) }
    var rateOpen by remember { mutableStateOf(false) }
    suspend fun load() { when (val r = BotsApi.getListing(id)) { is BotsResult.Ok -> { detail = r.value; error = null }; is BotsResult.Failed -> error = r.message } }
    LaunchedEffect(id) { load() }
    BotScreen(detail?.first?.title ?: "Agent", nav, testTag = "marketplaceDetail") {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            val d = detail
            if (d == null) { ListState(error == null, error, true, "Couldn't load agent details", "") { scope.launch { load() } }; return@Column }
            val (l, reviews) = d
            Text(l.title, color = c.textPrimary, fontSize = 22.sp, fontFamily = AllternitSerif)
            l.publisherName?.let { Text("By $it", color = c.textSecondary, fontSize = 13.sp) }
            Text(l.description, color = c.textPrimary, fontSize = 15.sp)
            if (installed != null) Text("✓ Installed", color = AllternitStatic.StatusSuccess, fontSize = 14.sp, fontWeight = FontWeight.Medium)
            else Box(Modifier.observedPressScale(0.94f)) {
                Pill("Install", "marketplaceInstall", primary = true, busy = installing) {
                    installing = true
                    scope.launch {
                        when (val r = BotsApi.install(id)) { is BotsResult.Ok -> { installed = r.value; store.refreshAgents() }; is BotsResult.Failed -> installError = r.message }
                        installing = false
                    }
                }
            }
            installError?.let { ErrorLine(it, Modifier.padding(0.dp)) }
            Box(Modifier.observedPressScale(0.94f)) { Pill("Rate this agent", "marketplaceRate") { rateOpen = true } }
            Text("Reviews (${l.ratingCount})", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            reviews.forEach { (stars, text, who) ->
                Column {
                    Text("★".repeat(stars.coerceIn(0, 5)) + (who?.let { " · $it" } ?: ""), color = c.textSecondary, fontSize = 12.sp)
                    text?.let { Text(it, color = c.textPrimary, fontSize = 14.sp) }
                }
            }
        }
    }
    if (rateOpen) RateSheet(id, onDismiss = { rateOpen = false }) { rateOpen = false; scope.launch { load() } }
}

@Composable
private fun RateSheet(id: String, onDismiss: () -> Unit, onSaved: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var rating by remember { mutableStateOf(5) }
    var review by remember { mutableStateOf("") }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    LibrarySheet(title = "Rate this Agent", testTag = "marketplaceRateSheet", onDismiss = onDismiss, leading = LibrarySheetAction("Cancel", onClick = onDismiss),
        trailing = LibrarySheetAction(if (saving) "Saving…" else "Save", enabled = !saving, bold = true, testTag = "marketplaceRateSave") {
            saving = true
            scope.launch { when (val r = BotsApi.rate(id, rating, review)) { is BotsResult.Ok -> onSaved(); is BotsResult.Failed -> error = r.message }; saving = false }
        }) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("Rating", color = c.textSecondary, fontSize = 12.sp)
            PillRow((1..5).toList(), rating, { "$it ★" }, { "marketplaceStars$it" }) { rating = it }
            LibraryField("Review (optional)", review, "What worked, what didn't", "marketplaceReview", singleLine = false, minHeight = 100) { review = it }
            error?.let { ErrorLine(it, Modifier.padding(0.dp)) }
        }
    }
}

/** iOS PublishAgentSheet: title, category, tags, description, and what publishing shares. */
@Composable
private fun PublishAgentSheet(bot: BotRecord, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var title by remember { mutableStateOf(bot.displayName) }
    var category by remember { mutableStateOf("") }
    var tags by remember { mutableStateOf("") }
    var description by remember { mutableStateOf(bot.description.orEmpty()) }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    LibrarySheet(title = "Publish to Marketplace", testTag = "publishAgentSheet", onDismiss = onDismiss, leading = LibrarySheetAction("Cancel", onClick = onDismiss),
        trailing = LibrarySheetAction(if (saving) "Publishing…" else "Publish", enabled = !saving && title.isNotBlank() && description.isNotBlank(), bold = true, testTag = "publishAgentSave") {
            saving = true
            scope.launch {
                when (val r = BotsApi.publish(bot.id, title.trim(), description.trim(), category.trim(), tags.split(",").map { it.trim() }.filter { it.isNotEmpty() })) { is BotsResult.Ok -> onDismiss(); is BotsResult.Failed -> error = r.message }
                saving = false
            }
        }) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            LibraryField("Title", title, "Title", "publishTitle") { title = it }
            LibraryField("", category, "Category (optional)", "publishCategory") { category = it }
            LibraryField("", tags, "Tags, comma-separated (optional)", "publishTags") { tags = it }
            LibraryField("Description", description, "What it does", "publishDescription", singleLine = false, minHeight = 100) { description = it }
            Text("Publishing shares a snapshot of this agent's model, prompt, and configuration — installers get their own independent copy, not a live link back to you.", color = c.textTertiary, fontSize = 12.sp)
            error?.let { ErrorLine(it, Modifier.padding(0.dp)) }
        }
    }
}

/** iOS AgentWorkspaceLayout: the workspace's categories and their known docs. */
internal object AgentWorkspaceLayout {
    data class Category(val directory: String, val label: String, val guidance: String, val knownDocs: List<String>) {
        fun path(doc: String) = if (directory.isEmpty()) doc else "$directory/$doc"
    }
    val categories = listOf(
        Category("", "Core", "Read at chat time today (SOUL.md · STYLE.md)", listOf("SOUL.md", "TOOLS.md", "HEARTBEAT.md")),
        Category("identity", "Identity", "Who the agent is and how it presents itself", listOf("IDENTITY.md", "SOUL.md")),
        Category("cognitive", "Cognitive", "How the agent reasons and decides", listOf("COGNITIVE.md")),
        Category("brain", "Brain", "Working knowledge always at hand", listOf("BRAIN.md")),
        Category("memory", "Memory", "Durable facts kept across sessions", listOf("MEMORY.md")),
        Category("governance", "Governance", "Operating rules and guardrails", listOf("PLAYBOOK.md")),
        Category("business", "Business", "Offers, customers, and goals", listOf("BUSINESS.md")),
    )
    fun starter(path: String): String = when (val doc = path.substringAfterLast('/')) {
        "AGENTS.md" -> "# Agents\n\nWorkspace instructions for this agent — layered into every session alongside the system prompt.\n"
        "SOUL.md" -> "# Soul\n\nWho this agent is — temperament, voice, and values in a few honest paragraphs.\n"
        "IDENTITY.md" -> "# Identity\n\nName, role, and how this agent introduces itself.\n"
        "COGNITIVE.md" -> "# Cognitive\n\nHow this agent thinks: reasoning style, defaults, decision rules.\n"
        "BRAIN.md" -> "# Brain\n\nWorking knowledge this agent should always have loaded.\n"
        "MEMORY.md" -> "# Memory\n\nDurable facts worth keeping across sessions.\n"
        "PLAYBOOK.md" -> "# Playbook\n\nOperating rules: what to do, in what order, and what never to do.\n"
        "BUSINESS.md" -> "# Business\n\nCommercial context: offers, pricing, customers, goals.\n"
        "TOOLS.md" -> "# Tools\n\nTools this agent may use, and when.\n"
        "HEARTBEAT.md" -> "# Heartbeat\n\nRecurring checks this agent runs on a schedule.\n"
        else -> "# ${doc.removeSuffix(".md").replace('-', ' ').replace('_', ' ').split(' ').joinToString(" ") { w -> w.replaceFirstChar { it.uppercase() } }}\n\n"
    }
}

/** iOS BotWorkspaceFilesSheet: files by category, Create for missing known docs, + New Context File. */
@Composable
internal fun BotWorkspaceFilesSheet(bot: BotRecord, nav: BotNavigator, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var files by remember { mutableStateOf<List<WorkspaceFileInfo>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var creating by remember { mutableStateOf(false) }
    var newOpen by remember { mutableStateOf(false) }
    suspend fun load() {
        loading = true
        when (val r = BotsApi.listWorkspaceFiles(bot.id)) { is BotsResult.Ok -> { files = r.value; error = null }; is BotsResult.Failed -> error = r.message }
        loading = false
    }
    LaunchedEffect(bot.id) { load() }
    fun edit(f: WorkspaceFileInfo) { onDismiss(); nav.open(BotRoute.FileEditor(bot.id, f)) }
    LibrarySheet(title = "Workspace Files", testTag = "botWorkspaceFiles", onDismiss = onDismiss, leading = LibrarySheetAction("Done", onClick = onDismiss),
        trailing = LibrarySheetAction("+", enabled = !(loading && files.isEmpty()), bold = true, testTag = "botWorkspaceNew") { newOpen = true }) {
        Column(Modifier.fillMaxWidth().heightIn(max = 640.dp).verticalScroll(rememberScrollState()).padding(bottom = 20.dp)) {
            if (!ListState(loading, error, files.isEmpty() && error != null, "Couldn't load workspace files", "") { scope.launch { load() } }) {
                AgentWorkspaceLayout.categories.forEach { cat ->
                    val prefix = if (cat.directory.isEmpty()) "" else cat.directory + "/"
                    val catFiles = files.filter { if (cat.directory.isEmpty()) '/' !in it.path else it.path.startsWith(prefix) }
                    val missing = cat.knownDocs.filter { d -> catFiles.none { it.path == prefix + d } }
                    SectionHeader(cat.label)
                    Text(cat.guidance, color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.padding(start = 20.dp, bottom = 6.dp))
                    RowGroup {
                        catFiles.forEach { f -> NavRow(f.fileName, com.allternit.core.design.R.drawable.ph_file_text, f.sizeLabel, "botWorkspaceFile${f.path}", last = false) { edit(f) } }
                        missing.forEachIndexed { i, d ->
                            NavRow(d, DesignR.drawable.ph_plus, if (creating) "…" else "Create", "botWorkspaceCreate${cat.path(d)}", last = i == missing.lastIndex) {
                                if (creating) return@NavRow
                                creating = true
                                scope.launch {
                                    val path = cat.path(d)
                                    val content = AgentWorkspaceLayout.starter(path)
                                    when (val r = BotsApi.writeWorkspaceFile(bot.id, path, content, null)) {
                                        is BotsApi.FileSave.Saved -> { load(); edit(WorkspaceFileInfo(path, content.length, null)) }
                                        is BotsApi.FileSave.Failed -> error = "Couldn't create $d: ${r.message}"
                                        is BotsApi.FileSave.Conflict -> load()
                                    }
                                    creating = false
                                }
                            }
                        }
                        if (catFiles.isEmpty() && missing.isEmpty()) Text("Empty", color = c.textTertiary, fontSize = 13.sp, modifier = Modifier.padding(16.dp))
                    }
                }
            }
        }
    }
    if (newOpen) NewWorkspaceFileSheet(bot.id, files.map { it.path }.toSet(), onDismiss = { newOpen = false }) { f -> newOpen = false; edit(f) }
}

/** iOS BotNewWorkspaceFileSheet: category, a name saved as a lowercase .md inside it. */
@Composable
private fun NewWorkspaceFileSheet(agentId: String, existing: Set<String>, onDismiss: () -> Unit, onCreated: (WorkspaceFileInfo) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var cat by remember { mutableStateOf(AgentWorkspaceLayout.categories[0]) }
    var name by remember { mutableStateOf("") }
    var creating by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val fileName = name.trim().replace(" ", "-").lowercase().let { if (it.isEmpty()) "" else if (it.endsWith(".md")) it else "$it.md" }
    val full = if (fileName.isEmpty()) "" else cat.path(fileName)
    val dup = full.isNotEmpty() && full in existing
    LibrarySheet(title = "New Context File", testTag = "botNewWorkspaceFile", onDismiss = onDismiss, leading = LibrarySheetAction("Cancel", onClick = onDismiss),
        trailing = LibrarySheetAction(if (creating) "Creating…" else "Create", enabled = fileName.isNotEmpty() && !dup && !creating, bold = true, testTag = "botNewWorkspaceCreate") {
            creating = true
            scope.launch {
                val content = AgentWorkspaceLayout.starter(full)
                when (val r = BotsApi.writeWorkspaceFile(agentId, full, content, null)) {
                    is BotsApi.FileSave.Saved -> onCreated(WorkspaceFileInfo(full, content.length, null))
                    is BotsApi.FileSave.Failed -> error = "Couldn't create the file: ${r.message}"
                    is BotsApi.FileSave.Conflict -> error = "A file at this path already exists — open it from the list instead."
                }
                creating = false
            }
        }) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            PillRow(AgentWorkspaceLayout.categories, cat, { it.label }, { "botNewFileCat${it.label}" }) { cat = it }
            Text(cat.guidance, color = c.textTertiary, fontSize = 12.sp)
            LibraryField("Document", name, "e.g. Launch Notes", "botNewFileName") { name = it }
            Text(when {
                dup -> "A file at this path already exists — open it from the list instead."
                full.isNotEmpty() -> full
                else -> "Saved as a lowercase .md name inside the category."
            }, color = if (dup) AllternitStatic.StatusError else c.textTertiary, fontSize = 12.sp)
            error?.let { ErrorLine(it, Modifier.padding(0.dp)) }
        }
    }
}

/**
 * The workspace file editor (iOS WorkspaceFileEditorView): the file's text with its size, Save with
 * the hash it loaded at; a newer version from another device opens Reload / Keep mine.
 */
@Composable
internal fun WorkspaceFileEditorScreen(bot: BotRecord, file: WorkspaceFileInfo, backend: com.allternit.feature.chat.ChatBackend, nav: BotNavigator) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var content by remember { mutableStateOf("") }
    var loaded by remember { mutableStateOf("") }
    var hash by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(true) }
    var loadError by remember { mutableStateOf<String?>(null) }
    var saving by remember { mutableStateOf(false) }
    var actionError by remember { mutableStateOf<String?>(null) }
    var saved by remember { mutableStateOf(false) }
    var conflict by remember { mutableStateOf<Pair<String?, String>?>(null) }
    suspend fun load() {
        loading = true
        loadError = null
        when (val r = BotsApi.readWorkspaceFile(bot.id, file.path)) {
            is BotsResult.Ok -> { content = r.value.second; loaded = content; hash = r.value.third }
            is BotsResult.Failed -> if (r.status == 404) { content = ""; loaded = ""; hash = null } else loadError = r.message
        }
        loading = false
    }
    fun save(force: Boolean) {
        saving = true
        actionError = null
        saved = false
        scope.launch {
            when (val r = BotsApi.writeWorkspaceFile(bot.id, file.path, content, if (force) null else hash)) {
                is BotsApi.FileSave.Saved -> { loaded = content; hash = r.hash; saved = true }
                is BotsApi.FileSave.Conflict -> conflict = r.hash to r.content
                is BotsApi.FileSave.Failed -> actionError = r.message
            }
            saving = false
        }
    }
    LaunchedEffect(file.path) { load() }
    BotScreen(file.fileName, nav, subtitle = "${content.length} chars", testTag = "workspaceFileEditor", trailing = {
        Pill(if (saving) "Saving…" else if (saved && content == loaded) "Saved" else "Save", "workspaceFileSave", primary = content != loaded, enabled = content != loaded && !saving) { save(false) }
    }) {
        Column(Modifier.fillMaxSize()) {
            if (file.path.endsWith("STYLE.md")) Text("Managed by response-style settings — changes will be overwritten", color = AllternitStatic.StatusWarning, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp))
            actionError?.let { ErrorLine(it) }
            when {
                loading -> LibrarySpinner("Loading the file")
                loadError != null -> ListState(false, loadError, true, "Couldn't load the file", "") { scope.launch { load() } }
                else -> {
                    Box(Modifier.weight(1f).fillMaxWidth().padding(16.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(12.dp)) {
                        BasicTextField(content, { content = it; saved = false }, textStyle = TextStyle(color = c.textPrimary, fontSize = 14.sp, fontFamily = FontFamily.Monospace, lineHeight = 20.sp), cursorBrush = SolidColor(c.textPrimary), modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).testTag("workspaceFileText"))
                    }
                    EditWithChatPanel(bot, file, backend, content) { proposal -> content = proposal; saved = false }
                }
            }
        }
    }
    conflict?.let { (h, newer) ->
        LibrarySheet(title = "Changed elsewhere", testTag = "workspaceFileConflict", onDismiss = { conflict = null }, leading = LibrarySheetAction("Cancel") { conflict = null }) {
            Text("This file was changed on another device or by the bot while you were editing. Reload to see the new version, or keep yours and save over it.", color = c.textSecondary, fontSize = 15.sp, modifier = Modifier.padding(20.dp))
            Row(Modifier.padding(16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Pill("Reload", "workspaceFileReload") { content = newer; loaded = newer; hash = h; conflict = null }
                Pill("Keep mine", "workspaceFileKeepMine", primary = true) { conflict = null; save(true) }
            }
        }
    }
}

/** iOS BotWebhooksSheet: the organization's webhook subscriptions. */
@Composable
internal fun BotWebhooksSheet(onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var subs by remember { mutableStateOf<List<WebhookSubscription>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    suspend fun load() { loading = true; when (val r = BotsApi.listWebhooks()) { is BotsResult.Ok -> { subs = r.value; error = null }; is BotsResult.Failed -> error = r.message }; loading = false }
    LaunchedEffect(Unit) { load() }
    LibrarySheet(title = "Org Webhooks", testTag = "botWebhooksSheet", onDismiss = onDismiss, leading = LibrarySheetAction("Done", onClick = onDismiss)) {
        Column(Modifier.fillMaxWidth().heightIn(max = 600.dp).verticalScroll(rememberScrollState()).padding(vertical = 8.dp)) {
            if (!ListState(loading, error, subs.isEmpty(), "Couldn't load webhooks", "No organization subscriptions yet. Webhooks are organization-wide. They let outside services push events into Allternit.", com.allternit.core.design.EmptyArtKind.Bell) { scope.launch { load() } }) {
                RowGroup {
                    subs.forEachIndexed { i, s ->
                        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(s.url, color = c.textPrimary, fontSize = 14.sp, maxLines = 1, modifier = Modifier.weight(1f))
                                Dot(if (s.active) AllternitStatic.StatusSuccess else c.textTertiary)
                            }
                            Text(s.events.joinToString(" · "), color = c.textSecondary, fontSize = 12.sp)
                        }
                        if (i < subs.lastIndex) Hairline()
                    }
                }
            }
        }
    }
}

