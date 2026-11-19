package com.allternit.feature.shell.firstrun

import android.Manifest
import android.app.NotificationManager
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
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
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.ThinkingOrb
import com.allternit.core.design.ThinkingOrbSize
import com.allternit.core.design.ThinkingOrbState
import com.allternit.feature.chat.RelayDevice
import kotlin.math.cos
import kotlin.math.roundToInt
import kotlin.math.sin
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch

/** What this setup session did (iOS ConnectedInventory.SessionFacts). */
internal data class SessionFacts(val brain: BrainChoice?, val brainConnected: Boolean, val brainLabel: String?, val mainBotId: String?) {
    constructor(s: SetupState) : this(
        s.ai, s.outcomes[SetupField.Ai]?.status == OutcomeStatus.Connected, s.outcomes[SetupField.Ai]?.label, s.mainBotId,
    )
}

/*
 * The connected view's inventory (iOS ConnectedInventory, web `connectedInventory.ts`): each real
 * source loads on its own, so one failing source never blanks the others. A node is "conn" only
 * when its source says so, "new" when this session set it up, and "avail" otherwise.
 */
internal object ConnectedInventory {
    enum class SourceState { Ok, Error, Unavailable }
    data class Source<T>(val state: SourceState, val items: List<T>, val note: String? = null)

    data class Inventory(
        val providers: Source<SetupProviderAuth>,
        val cloudModels: Source<SetupModelRow>,
        val runtimes: List<RelayDevice>,
        val cloudComputers: Source<SetupCloudComputer>,
        val bots: Source<SetupNamed>,
        val channels: Source<SetupNamed>,
        val memory: Source<SetupNamed>,
        val projects: Source<SetupNamed>,
        val connectors: Source<SetupNamed>,
        val subscriptions: Source<Pair<String, Boolean>>,
    )

    const val RUNTIME_NOTE = "Needs a connected computer or Allternit Cloud computer"

    private fun note(e: Throwable): String {
        val msg = (e as? SetupError)?.let { "${it.status ?: ""} ${it.code ?: ""} ${it.message}" } ?: (e.message ?: "")
        if (Regex("runtime_unavailable|No paired Allternit runtime|503", RegexOption.IGNORE_CASE).containsMatchIn(msg)) return RUNTIME_NOTE
        if (Regex("401|403|unauthor|Sign in again", RegexOption.IGNORE_CASE).containsMatchIn(msg)) return "Sign in to see this"
        return "Couldn't load right now"
    }

    private suspend fun <R> settle(runtime: Boolean, reachable: Boolean, fn: suspend () -> List<R>): Source<R> {
        if (runtime && !reachable) return Source(SourceState.Unavailable, emptyList(), RUNTIME_NOTE)
        return try {
            Source(SourceState.Ok, fn())
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            val n = note(e)
            Source(if (n == RUNTIME_NOTE) SourceState.Unavailable else SourceState.Error, emptyList(), n)
        }
    }

    private val connectedChannelStates = setOf("CONNECTED", "DEGRADED")
    private val channelNames = mapOf(
        "slack" to "Slack", "teams" to "Microsoft Teams", "discord" to "Discord", "telegram" to "Telegram",
        "whatsapp" to "WhatsApp", "whatsapp-personal" to "WhatsApp", "email" to "Email", "sms" to "SMS",
    )

    /** Runtime-backed sources need a reachable computer (one online); cloud ones don't. */
    suspend fun load(runtimes: List<RelayDevice>): Inventory = coroutineScope {
        val reachable = runtimes.any { it.isOnline }
        val providers = async { settle(true, reachable) { SetupApi.providerAuthStatus() } }
        val models = async { settle(false, true) { SetupApi.cloudModelCatalog() } }
        val clouds = async { settle(false, true) { SetupApi.cloudComputers() } }
        val bots = async { settle(true, reachable) { SetupApi.roster() } }
        val channels = async {
            settle(true, reachable) {
                SetupApi.channelAccounts().filter { it["authType"].str() == "channel_oauth" }.mapNotNull { a ->
                    val id = a["id"].str() ?: return@mapNotNull null
                    val vendor = a["vendor"].str() ?: ""
                    SetupNamed(id, channelNames[vendor] ?: vendor.replaceFirstChar { it.uppercase() },
                        connected = a["state"].str() in connectedChannelStates, detail = a["displayName"].str())
                }
            }
        }
        val memory = async { settle(true, reachable) { SetupApi.memoryMounts() } }
        val projects = async { settle(true, reachable) { SetupApi.botProjects() } }
        val connectors = async { settle(true, reachable) { SetupApi.mcpConnectors() } }
        val subs = async { settle(true, reachable) { SetupApi.fabricAccounts() } }
        Inventory(providers.await(), models.await(), runtimes, clouds.await(), bots.await(), channels.await(),
            memory.await(), projects.await(), connectors.await(), subs.await())
    }

    enum class NodeStatus { New, Conn, Inc, Avail }
    enum class NodeGroup(val title: String) { Brains("Brains"), Computers("Computers"), Bots("Bots"), Channels("Channels"), Workspace("Workspace") }

    data class Node(
        val id: String, val group: NodeGroup, val title: String, val short: String, val desc: String,
        val status: NodeStatus, val statusText: String, val brain: Boolean,
    )

    private fun statusText(s: NodeStatus) = when (s) {
        NodeStatus.New -> "Connected · set up just now"
        NodeStatus.Conn -> "Connected"
        NodeStatus.Inc -> "Included in your workspace"
        NodeStatus.Avail -> "Available · add it any time"
    }

    private fun node(id: String, group: NodeGroup, title: String, short: String? = null, desc: String, status: NodeStatus,
                     brain: Boolean = false, text: String? = null) =
        Node(id, group, title, short ?: title, desc, status, text ?: statusText(status), brain)

    const val GROUP_CAP = 4

    private fun capped(nodes: List<Node>, group: NodeGroup, moreDesc: String, cap: Int = GROUP_CAP): List<Node> {
        if (nodes.size <= cap) return nodes
        val shown = nodes.take(cap - 1)
        val rest = nodes.size - shown.size
        return shown + node("${group.title}-more", group, "+$rest more", desc = moreDesc, status = NodeStatus.Conn, text = "$rest more connected")
    }

    private val providersFor = mapOf(
        BrainChoice.Claude to listOf("anthropic", "claude-cli"), BrainChoice.Chatgpt to listOf("openai", "codex-cli"),
        BrainChoice.Gemini to listOf("google", "antigravity"), BrainChoice.Local to listOf("ollama", "lmstudio", "sidecar"),
    )

    fun buildGraph(inv: Inventory, s: SessionFacts): List<Node> {
        val out = mutableListOf<Node>()
        val authed = inv.providers.items.filter { it.authenticated }.map { it.id }.toSet()
        val rows = listOf(
            Triple(BrainChoice.Allternit, "Hosted models", if (inv.cloudModels.items.isEmpty()) "Allternit Cloud models, billed to your Allternit plan."
            else "Allternit Cloud models (${inv.cloudModels.items.size} in the catalog right now), billed to your Allternit plan."),
            Triple(BrainChoice.Claude, "Your account", "Your own Anthropic account — an API key or your Claude subscription."),
            Triple(BrainChoice.Chatgpt, "Your account", "Your own OpenAI account — an API key or your ChatGPT sign-in."),
            Triple(BrainChoice.Gemini, "Your account", "Your own Google account — an API key or a Google sign-in."),
            Triple(BrainChoice.Local, "On your computer", "A model that runs on your computer. No network needed once downloaded."),
        )
        for ((id, _, desc) in rows) {
            val isSession = s.brainConnected && s.brain == id
            val before = id != BrainChoice.Allternit && providersFor[id].orEmpty().any { it in authed }
            val title = if (id == BrainChoice.Allternit) "Allternit Cloud" else id.label
            out += node("brain-${id.raw}", NodeGroup.Brains, title, short = if (id == BrainChoice.Local) "Local" else title, desc = desc,
                status = if (isSession) NodeStatus.New else if (before) NodeStatus.Conn else NodeStatus.Avail, brain = true,
                text = if (isSession) "Default brain · set up just now" else null)
        }
        for (sb in BrainChoice.subscriptions) {
            val logins = inv.subscriptions.items.filter { it.first == sb.subscriptionProvider }
            val isSession = s.brainConnected && s.brain == sb
            if (logins.isEmpty() && !isSession) continue
            val ready = logins.any { it.second }
            out += node("brain-${sb.raw}", NodeGroup.Brains, "${sb.shortName} sub", desc = "Your ${sb.shortName} subscription, signed in on your Sessions computer.",
                status = if (isSession) NodeStatus.New else if (ready) NodeStatus.Conn else NodeStatus.Avail, brain = true,
                text = if (isSession) "Default brain · set up just now" else if (ready) "Connected" else "Signed out · sign in again in Settings → Connections")
        }
        if (s.brainConnected && s.brain == BrainChoice.Other && s.brainLabel != null) {
            out += node("brain-other", NodeGroup.Brains, s.brainLabel, desc = "Connected during setup.", status = NodeStatus.New, brain = true,
                text = "Default brain · set up just now")
        }
        // Computers: real runtimes + cloud computers, then the open placeholders.
        val comps = mutableListOf<Node>()
        for (r in inv.runtimes) {
            comps += node("rt-${r.id}", NodeGroup.Computers, r.name, desc = "A computer paired to your account.",
                status = if (r.status == "offline") NodeStatus.Avail else NodeStatus.Conn,
                text = if (r.status == "offline") "Paired · offline right now" else if (r.status == "busy") "Connected · busy" else null)
        }
        for (cc in inv.cloudComputers.items) {
            comps += node("cc-${cc.id}", NodeGroup.Computers, "Allternit Cloud computer", short = "Cloud",
                desc = "An always-on computer in Allternit Cloud.${cc.specs?.let { " $it." } ?: ""}",
                status = if (cc.isRunning) NodeStatus.Conn else NodeStatus.Avail, text = if (cc.isRunning) "Connected" else "Status: ${cc.status}")
        }
        if (inv.cloudComputers.items.isEmpty()) {
            comps += node("comp-cloud", NodeGroup.Computers, "Allternit Cloud computer", short = "Cloud",
                desc = "An always-on computer in Allternit Cloud. Comes with a paid plan.", status = NodeStatus.Avail)
        }
        if (comps.size < GROUP_CAP) {
            comps += node("comp-server", NodeGroup.Computers, "Your server", short = "Server",
                desc = "A machine you own, paired over SSH or a backend URL.", status = NodeStatus.Avail)
        }
        out += capped(comps, NodeGroup.Computers, "More computers are paired to this account. See Settings → Computers.", GROUP_CAP - 1)
        out += node("phone", NodeGroup.Computers, "This phone", short = "Phone",
            desc = "You are setting up from this phone. Chats, bots and approvals work here.", status = NodeStatus.Conn)
        // Bots: the main bot first (always present), then the rest.
        val main = inv.bots.items.firstOrNull { it.isMain }
        if (main != null) {
            val isNew = s.mainBotId == main.id
            out += node("bot-main", NodeGroup.Bots, main.name, desc = "Your main bot — the one pinned at the top of Bots that answers first.",
                status = if (isNew) NodeStatus.New else NodeStatus.Conn, text = if (isNew) "Main bot · set up just now" else "Main bot")
        } else {
            val ok = inv.bots.state == SourceState.Ok
            out += node("bot-main", NodeGroup.Bots, "Main bot", desc = if (ok) "No main bot yet. Pick or set one up in Bots." else inv.bots.note ?: "Pick or set one up in Bots.",
                status = NodeStatus.Avail, text = if (ok) null else inv.bots.note)
        }
        out += capped(inv.bots.items.filter { !it.isMain }.map { node("bot-${it.id}", NodeGroup.Bots, it.name, desc = "A bot on your account.", status = NodeStatus.Conn) },
            NodeGroup.Bots, "More bots on your account. See Bots.", GROUP_CAP - 1)
        // Channels: real accounts, else the channels you can connect.
        val channels = inv.channels.items.map { ch ->
            node("ch-${ch.id}", NodeGroup.Channels, ch.name, desc = ch.detail?.let { "${ch.name} · $it" } ?: "Talk to your bots from ${ch.name}.",
                status = if (ch.connected) NodeStatus.Conn else NodeStatus.Avail, text = if (ch.connected) null else "Needs attention in Settings → Connections")
        }.toMutableList()
        val names = channels.map { it.title }.toSet()
        for (name in listOf("Slack", "Email", "SMS", "Telegram")) {
            if (channels.size >= GROUP_CAP || name in names) continue
            channels += node("ch-${name.lowercase()}", NodeGroup.Channels, name,
                desc = if (inv.channels.state == SourceState.Ok) "Talk to your bots from $name." else inv.channels.note ?: "Talk to your bots from $name.",
                status = NodeStatus.Avail)
        }
        out += capped(channels, NodeGroup.Channels, "More channels are connected. See Settings → Connections.")
        // Workspace
        val memCreated = inv.memory.items.any { it.connected }
        out += node("memory", NodeGroup.Workspace, "Memory Drive", desc = "What Gizzi remembers about your work. You can see and edit it.",
            status = if (memCreated) NodeStatus.Conn else if (inv.memory.state == SourceState.Ok) NodeStatus.Inc else NodeStatus.Avail,
            text = if (memCreated) "Connected" else if (inv.memory.state == SourceState.Ok) "Included · created the first time you open it" else inv.memory.note)
        val projectCount = inv.projects.items.size
        out += node("projects", NodeGroup.Workspace, "Projects", desc = "Chats, tasks, code and canvases grouped by project.",
            status = if (projectCount > 0) NodeStatus.Conn else NodeStatus.Inc,
            text = if (projectCount > 0) "$projectCount project${if (projectCount == 1) "" else "s"}" else if (inv.projects.state == SourceState.Ok) "Included · none yet" else "Included")
        val conns = inv.connectors.items.count { it.connected }
        out += node("connectors", NodeGroup.Workspace, "Connectors", desc = "MCP apps and connectors that let agents use your tools.",
            status = if (conns > 0) NodeStatus.Conn else NodeStatus.Avail,
            text = if (conns > 0) "$conns connected" else if (inv.connectors.state == SourceState.Ok) null else inv.connectors.note)
        return out
    }
}

private data class Placed(val node: ConnectedInventory.Node, val at: Offset)

private const val PW = 370f
private const val PH = 330f
private val HUB = Offset(185f, 150f)

private fun layout(nodes: List<ConnectedInventory.Node>): List<Placed> {
    val brains = nodes.filter { it.group == ConnectedInventory.NodeGroup.Brains }
    val placed = brains.mapIndexed { i, n ->
        val a = Math.toRadians(-90.0 + 360.0 / brains.size * i)
        Placed(n, Offset((HUB.x + 108 * cos(a)).roundToInt().toFloat(), (HUB.y + 92 * sin(a)).roundToInt().toFloat()))
    }.toMutableList()
    val comps = nodes.filter { it.group == ConnectedInventory.NodeGroup.Computers }
    comps.forEachIndexed { i, n ->
        val x = if (comps.size <= 1) HUB.x else (48 + i * (PW - 96) / (comps.size - 1)).roundToInt().toFloat()
        placed += Placed(n, Offset(x, HUB.y + 156))
    }
    return placed
}

/**
 * The end of setup (iOS SetupConnectedView, web `setup/ConnectedView.tsx` phone layout): a graph
 * of everything ACTUALLY connected, with the A:// core at the centre, the rest of the workspace as
 * chips, and a detail card for the tapped node. It opens on the live setup checklist; "Continue"
 * reveals the graph. "Open Allternit" finishes setup. iOS plays its branded hand-off first when it
 * can and finishes at once when it can't; Android always finishes at once.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun ConnectedScreen(
    facts: SessionFacts,
    devices: suspend () -> List<RelayDevice>?,
    onBack: () -> Unit,
    /** Finishes setup; `fix` is a fix to open, `sources` the connected nodes in window pixels (the hand-off). */
    onOpen: (fix: String?, sources: List<Offset>) -> Unit,
) {
    val c = setupColors
    var inventory by remember { mutableStateOf<ConnectedInventory.Inventory?>(null) }
    var focus by remember { mutableStateOf<String?>(null) }
    var checklistOpen by remember { mutableStateOf(true) }
    var opening by remember { mutableStateOf(false) }
    var graphBounds by remember { mutableStateOf<androidx.compose.ui.geometry.Rect?>(null) }
    val density = androidx.compose.ui.platform.LocalDensity.current.density

    LaunchedEffect(Unit) { inventory = ConnectedInventory.load(devices().orEmpty()) }

    val all = inventory?.let { ConnectedInventory.buildGraph(it, facts) }.orEmpty()
    val placed = layout(all)
    val focusId = focus ?: all.firstOrNull { it.status == ConnectedInventory.NodeStatus.New }?.id ?: all.firstOrNull()?.id
    val focused = all.firstOrNull { it.id == focusId }

    fun open(fix: String?) {
        if (opening) return
        opening = true
        // Connected nodes collapse into the mark's blocks from where they sit on screen; the rest fly in from afar.
        val b = graphBounds
        val sources = if (b == null || b.width <= 0f) emptyList() else {
            val scale = minOf(b.width / (PW * density), b.height / (PH * density), 1.15f)
            val ox = b.center.x - PW * density * scale / 2
            val oy = b.center.y - PH * density * scale / 2
            val live = placed.filter { it.node.status != ConnectedInventory.NodeStatus.Avail }
            live.ifEmpty { placed }.take(23).map { Offset(ox + it.at.x * density * scale, oy + it.at.y * density * scale) }
        }
        onOpen(fix, sources)
    }

    Box(Modifier.fillMaxSize().background(c.bg)) {
        Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing)) {
            // Header
            Row(
                Modifier.fillMaxWidth().height(52.dp).background(c.cardGlass).padding(horizontal = 16.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                ThinkingOrb(state = ThinkingOrbState.Connecting, size = ThinkingOrbSize.Inline, paused = true, label = "A://")
                Text("Everything connected", fontSize = 14.sp, fontWeight = FontWeight.Bold, color = c.ink)
                Spacer(Modifier.weight(1f))
                Text("SETUP DONE", style = mono(10.5.sp, FontWeight.SemiBold), color = c.mut, letterSpacing = 0.8.sp)
            }
            Box(Modifier.fillMaxWidth().height(1.dp).background(c.line))
            // Graph
            Box(Modifier.weight(1f).fillMaxWidth().heightIn(min = 180.dp).clipToBounds(), contentAlignment = Alignment.Center) {
                SetupBlockField()
                if (inventory == null) {
                    Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
                        ThinkingOrb(state = ThinkingOrbState.Searching, size = ThinkingOrbSize.Avatar, label = "A:// is searching")
                        Text("Gathering what's connected…", fontSize = 13.sp, color = c.sub)
                    }
                } else {
                    val t = loopClock(22f, "connected")
                    SetupFitBox(PW, PH, Modifier.fillMaxSize().onGloballyPositioned { graphBounds = it.boundsInWindow() }) {
                        Box(Modifier.fillMaxSize()) {
                            Canvas(Modifier.fillMaxSize()) {
                                fun p(o: Offset) = Offset(o.x.dp.toPx(), o.y.dp.toPx())
                                placed.forEach { pl ->
                                    val on = pl.node.status != ConnectedInventory.NodeStatus.Avail
                                    drawLine(if (on) c.ink.copy(alpha = 0.9f) else c.edge, p(HUB), p(pl.at),
                                        strokeWidth = (if (on) 2 else 1).dp.toPx(),
                                        pathEffect = PathEffect.dashPathEffect(if (on) floatArrayOf(10.dp.toPx(), 4.dp.toPx()) else floatArrayOf(4.dp.toPx(), 5.dp.toPx())))
                                }
                                placed.filter { it.node.status == ConnectedInventory.NodeStatus.New || it.node.status == ConnectedInventory.NodeStatus.Conn }
                                    .take(8).forEach { setupPackets(p(HUB), p(it.at), c.coral, t, 2.2f, 8.dp) }
                                coreRing(p(HUB), 64.dp, c.coral, t, minScale = 0.8f, maxScale = 1.6f, width = 1.5.dp)
                            }
                            Box(Modifier.centerAt(HUB.x, HUB.y)) {
                                ThinkingOrb(state = ThinkingOrbState.Working, size = ThinkingOrbSize.Avatar, label = "Allternit core")
                            }
                            placed.forEachIndexed { i, pl ->
                                Box(Modifier.centerAt(pl.at.x, pl.at.y)) {
                                    GraphNode(pl.node, focused = pl.node.id == focusId, Modifier.popIn(200 + i * 40)) { focus = pl.node.id }
                                }
                            }
                        }
                    }
                }
            }
            // Groups
            if (inventory != null) {
                val items = listOf(ConnectedInventory.NodeGroup.Bots, ConnectedInventory.NodeGroup.Channels, ConnectedInventory.NodeGroup.Workspace)
                    .flatMap { g -> all.filter { it.group == g } }
                FlowRow(Modifier.fillMaxWidth().padding(horizontal = 14.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    items.forEach { n ->
                        val avail = n.status == ConnectedInventory.NodeStatus.Avail
                        Row(
                            Modifier.height(28.dp).background(c.card, RoundedCornerShape(8.dp))
                                .then(if (avail) Modifier.nodeBorder(false, 8.dp) else Modifier.border(1.dp, c.line, RoundedCornerShape(8.dp)))
                                .clickable { focus = n.id }.padding(horizontal = 9.dp)
                                .semantics(mergeDescendants = true) { contentDescription = "${n.title}, ${n.statusText}" },
                            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp),
                        ) {
                            Text(n.short, fontSize = 11.5.sp, color = c.sub, maxLines = 1)
                            if (!avail) Text("✓", fontSize = 11.5.sp, fontWeight = FontWeight.Bold, color = c.ink)
                        }
                    }
                }
            }
            // Detail + buttons
            Column(Modifier.padding(start = 14.dp, end = 14.dp, top = 10.dp, bottom = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                focused?.let { n ->
                    val avail = n.status == ConnectedInventory.NodeStatus.Avail
                    Column(
                        Modifier.fillMaxWidth().background(c.card, RoundedCornerShape(14.dp)).border(1.dp, c.line, RoundedCornerShape(14.dp))
                            .padding(horizontal = 16.dp, vertical = 14.dp).semantics(mergeDescendants = true) {},
                        verticalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        Text(n.group.title.uppercase(), fontSize = 10.5.sp, letterSpacing = 0.8.sp, color = c.mut)
                        Text(n.title, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, color = c.ink)
                        Text(n.desc, fontSize = 12.5.sp, color = c.sub, maxLines = 4)
                        Text(n.statusText, fontSize = 11.sp, fontWeight = FontWeight.Bold, color = if (avail) c.sub else c.okFg,
                            modifier = Modifier.padding(top = 6.dp).background(if (avail) c.fill else c.okBg, CircleShape).padding(horizontal = 9.dp, vertical = 3.dp))
                    }
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    SetupGhostButton("Back", enabled = !opening, onClick = onBack)
                    Spacer(Modifier.weight(1f))
                    SetupPrimaryButton(if (opening) "Opening…" else "Open Allternit  →", Modifier.testTag("setup-open-allternit"), enabled = !opening) { open(null) }
                }
            }
        }
        if (checklistOpen && !opening) {
            SetupChecklistCard(onFix = { section -> checklistOpen = false; open(section) }, onContinue = { checklistOpen = false })
        }
    }
}

@Composable
private fun GraphNode(n: ConnectedInventory.Node, focused: Boolean, modifier: Modifier, onClick: () -> Unit) {
    val c = setupColors
    val lit = n.status != ConnectedInventory.NodeStatus.Avail
    val shape = RoundedCornerShape(10.dp)
    Row(
        modifier
            .then(if (n.brain) Modifier.widthIn(min = 84.dp) else Modifier)
            .height(if (n.brain) 32.dp else 30.dp)
            .background(c.card, shape)
            .then(
                when {
                    !lit -> Modifier.nodeBorder(false, 10.dp)
                    else -> Modifier.border(1.dp, if (n.status == ConnectedInventory.NodeStatus.Inc) c.line else c.ink, shape)
                },
            )
            .then(if (focused || n.status == ConnectedInventory.NodeStatus.New) Modifier.border(3.dp, c.coral.copy(alpha = if (focused) 0.35f else 0.28f), RoundedCornerShape(12.dp)) else Modifier)
            .clickable(onClick = onClick)
            .padding(horizontal = if (n.brain) 9.dp else 10.dp)
            .semantics(mergeDescendants = true) { contentDescription = "${n.title}, ${n.statusText}" },
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        val dot = when (n.status) {
            ConnectedInventory.NodeStatus.Avail -> c.lineDash
            ConnectedInventory.NodeStatus.Inc -> c.ink
            else -> c.ok
        }
        Box(Modifier.size(7.dp).background(dot, CircleShape))
        Text(n.short, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, color = if (lit) c.ink else c.sub, maxLines = 1)
    }
}

/**
 * The live-checked setup checklist (iOS SetupChecklistCard; the same rows as Settings › Setup).
 * A row is checked only when its live check passed; a row that needs you has one button that
 * opens its fix once setup is finished. "Check again" re-runs every check.
 */
@Composable
private fun SetupChecklistCard(onFix: (String) -> Unit, onContinue: () -> Unit) {
    val c = setupColors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var rows by remember { mutableStateOf<List<SetupCheckRow>>(emptyList()) }
    var run by remember { mutableIntStateOf(0) }

    /** Notification permission on this phone (only the device can answer). */
    fun notificationsRow(): SetupCheckRow {
        val enabled = (context.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager)?.areNotificationsEnabled() == true
        val asked = Build.VERSION.SDK_INT < 33 ||
            context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
        val (status, detail) = when {
            enabled && asked -> SetupCheckStatus.Ready to "Allternit can notify you on this phone."
            !enabled && Build.VERSION.SDK_INT < 33 -> SetupCheckStatus.NeedsSetup to "Notifications are blocked for Allternit. Allow them in Android Settings."
            else -> SetupCheckStatus.NotChecked to "Allternit has not asked to notify you on this phone yet."
        }
        return SetupCheckRow("notifications", "Notifications", false, status, detail, "Open notifications", "notifications")
    }

    suspend fun recheck() {
        run += 1
        val mine = run
        loading = true
        error = null
        val device = notificationsRow()
        try {
            val server = SetupApi.setupChecks()
            if (mine != run) return
            rows = SetupApi.sortChecks(server + device)
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            if (mine != run) return
            // On a failed request the device row still shows, with the error beside it.
            rows = listOf(device)
            error = e.message?.takeIf { it.isNotEmpty() } ?: "The checks could not be run."
        } finally {
            if (mine == run) loading = false
        }
    }

    LaunchedEffect(Unit) { recheck() }

    Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.38f)).clickable(enabled = false) {}, contentAlignment = Alignment.Center) {
        Column(
            Modifier.padding(16.dp).widthIn(max = 520.dp).fillMaxWidth().verticalScroll(rememberScrollState())
                .background(c.card, RoundedCornerShape(16.dp)).border(1.dp, c.line, RoundedCornerShape(16.dp)).padding(14.dp)
                .testTag("setup-checklist"),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            val ready = rows.count { it.status == SetupCheckStatus.Ready }
            val blocking = rows.count { it.required && it.status != SetupCheckStatus.Ready }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Box(Modifier.size(26.dp), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(
                        progress = { if (rows.isEmpty()) 0f else ready.toFloat() / rows.size },
                        modifier = Modifier.size(26.dp), color = c.ok, trackColor = c.fill2, strokeWidth = 3.dp,
                    )
                }
                Column {
                    Text(if (rows.isNotEmpty() && blocking == 0) "Everything you need is ready" else "Check your setup",
                        fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = c.ink)
                    Text("$ready of ${rows.size} ready", fontSize = 12.sp, color = c.sub)
                }
            }
            if (loading && rows.isEmpty()) Text("Running checks…", fontSize = 13.sp, color = c.sub, modifier = Modifier.padding(vertical = 8.dp))
            error?.let { Text("Allternit could not run the checks: $it", fontSize = 13.sp, color = c.errFg) }
            rows.forEach { row ->
                Row(Modifier.semantics(mergeDescendants = true) {}, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    val ok = row.status == SetupCheckStatus.Ready
                    Box(
                        Modifier.size(18.dp).then(if (ok) Modifier.background(c.ok, CircleShape) else Modifier.border(1.5.dp, c.lineDash, CircleShape)),
                        contentAlignment = Alignment.Center,
                    ) { if (ok) Text("✓", fontSize = 11.sp, fontWeight = FontWeight.Bold, color = Color.White) }
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                        Text(row.title + if (row.required) "" else " (optional)", fontSize = 13.5.sp, fontWeight = FontWeight.SemiBold, color = c.ink)
                        Text("${row.status.label}. ${row.detail}", fontSize = 12.5.sp, color = c.sub)
                        if (!ok && row.fixLabel != null && row.fixSection != null) {
                            SetupQuietButton(row.fixLabel, Modifier.padding(top = 4.dp)) { onFix(row.fixSection) }
                        }
                    }
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Spacer(Modifier.weight(1f))
                SetupGhostButton(if (loading) "Checking…" else "Check again", enabled = !loading) { scope.launch { recheck() } }
                SetupPrimaryButton("Continue", Modifier.testTag("setup-checklist-continue"), onClick = onContinue)
            }
        }
    }
}
