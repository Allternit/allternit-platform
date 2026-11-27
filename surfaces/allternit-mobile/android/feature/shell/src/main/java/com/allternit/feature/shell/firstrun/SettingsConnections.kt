package com.allternit.feature.shell.firstrun

import androidx.compose.foundation.background
import android.provider.Settings
import androidx.compose.animation.Crossfade
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.tween
import androidx.compose.foundation.gestures.snapping.rememberSnapFlingBehavior
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Icon
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import com.allternit.feature.artifacts.FilterLabel
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.artifacts.LibraryLayout
import com.allternit.feature.artifacts.LibraryLayoutButton
import com.allternit.feature.artifacts.LibrarySearchField
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.foundation.layout.heightIn
import androidx.compose.ui.semantics.Role
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiConfig
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.chat.RuntimeRelay
import com.allternit.feature.settings.SettingsPrefs
import com.allternit.feature.shell.MenuRow
import com.allternit.feature.shell.ShellApi
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR

/** iOS ConnGroup: the order, the title and the one-line blurb of each Settings > Connections group. */
internal val CONNECTION_GROUPS = listOf(
    Triple("brains", "Brains", "The AI that thinks. Use a plan you already pay for, or ours."),
    Triple("computers", "Computers", "Only needed when Allternit should click and type in apps for you."),
    Triple("phones", "Phones", "Talk to Allternit from your phone, and let it use your phone."),
    Triple("tools", "Coding tools", "Command-line agents. They run on a computer and use your plans."),
    Triple("voice", "Voice", "Who speaks when Allternit talks back."),
    Triple("apps", "Apps", "Mail, files and work apps Allternit can use for you."),
    Triple("channels", "Channels", "Where people can message your bots."),
)

private const val LAYOUT_KEY = "settings_connections_layout"
private val AMBER = Color(0xFFE8A33A)

/** The status filter in the menu (All, Connected, Needs you, Not connected). */
private enum class ConnStatus(val label: String) { All("All"), Connected("Connected"), Needs("Needs you"), Off("Not connected") }

/**
 * Settings > Connections (iOS ConnectionsSettingsView + ConnectionRowView + ConnectSheetView): the connect kit's
 * rows in seven groups, each with the same Connect sheet setup uses, and Disconnect for a row that is on.
 * It reuses [ConnectionsStore], [ConnectionRow] and [ConnectSheetHost] from first-run setup, never a second connect screen.
 */
@Composable
internal fun ConnectionsSettingsScreen(
    onBack: () -> Unit,
    openPhoneControl: () -> Unit,
    openAndroidScreen: () -> Unit,
    /** Harness & Bots 2.0: the bots host for "New bot on <Runtime>" (brains, connections). */
    botsHost: com.allternit.feature.bots.BotsHost? = null,
    /** Opens a bot made from a runtime's detail page. */
    onBotCreated: (String) -> Unit = {},
) {
    val c = setupColors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val relay = remember { RuntimeRelay(ShellApi.okHttp, ApiConfig(), context) }
    val store = remember { ConnectionsStore() }
    val policyStore = remember { ToolPolicyStore() }
    val signedIn = com.allternit.core.auth.AllternitAuth.state.collectAsState().value is com.allternit.core.auth.AuthState.SignedIn
    var permissionsFor by remember { mutableStateOf<ConnEntry?>(null) }
    var sheet by remember { mutableStateOf<ConnSheet?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    var query by remember { mutableStateOf("") }
    var searchOpen by remember { mutableStateOf(false) }
    var menuOpen by remember { mutableStateOf(false) }
    var browse by remember { mutableStateOf(false) }
    var runtimeOpen by remember { mutableStateOf<com.allternit.feature.bots.runtimes.RuntimeEntry?>(null) }
    var status by remember { mutableStateOf(ConnStatus.All) }
    var confirmUnlink by remember { mutableStateOf(false) }
    val uri = LocalUriHandler.current
    var layout by remember { mutableStateOf(LibraryLayout.load(context, LAYOUT_KEY, LibraryLayout.List)) }
    LaunchedEffect(Unit) {
        launch { store.load(relay) }
        launch { store.loadCatalogCounts() }
        launch { store.loadPromotedApps() }
        launch { policyStore.load() }
    }

    fun reload() { scope.launch { store.load(relay) } }

    fun disconnect(id: String, name: String) {
        message = null
        scope.launch {
            runCatching { ConnectionsClient.disconnect(id, store.target(id)) }
                .onFailure { message = "Couldn't disconnect $name. ${ConnectionsClient.describe(it)}" }
            store.load(relay)
        }
    }

    fun connect(id: String) {
        message = null
        when (id) {
            "phone-control" -> openPhoneControl()
            "phone", "android-screen" -> openAndroidScreen()
            "gizzi", "server", "cloud-computer" -> sheet = ConnSheet.OnComputer(id)
            "openai-voice", "elevenlabs" -> message = "Voice keys are set under Voice and dictation."
            else -> sheet = connectRoute(id)
        }
    }

    fun fallback(id: String) {
        sheet = when {
            id == "this-mac" -> ConnSheet.OnComputer("this-mac")
            id == "github" -> ConnSheet.Login("copilot")
            id.endsWith("-key") || id == "api-key" -> ConnSheet.Keys(if (id == "api-key") null else id)
            else -> null
        }
    }

    /**
     * The Claude organization row's one action (iOS ConnectionRowView + ConnectionsSettingsView.act):
     * Link (off), Unlink after a confirm (on), Link again (needs), and for a reached spend limit
     * Fix, which opens Claude's Linked platforms page because linking again would not help.
     */
    fun claudeOrgAction(r: ConnectionWire): (@Composable () -> Unit)? = when {
        !store.isAvailable("claude-org") || r.state == ConnState.Connecting -> null
        r.state == ConnState.On -> ({
            SetupQuietButton("Unlink", Modifier.testTag("claude-org-unlink").semantics { contentDescription = "Unlink your Claude organization" }) { confirmUnlink = true }
        })
        r.state == ConnState.Needs && r.reasonCode == "spend_limit_reached" -> ({
            SetupQuietButton("Fix", Modifier.semantics { contentDescription = "Open Claude's Linked platforms page" }) {
                runCatching { uri.openUri(ConnectionsCatalog.CLAUDE_LINKED_PLATFORMS_URL) }
            }
        })
        r.state == ConnState.Needs -> ({ SetupQuietButton("Link again", Modifier.testTag("claude-org-relink")) { connect("claude-org") } })
        else -> ({
            SetupQuietButton("Link", Modifier.testTag("claude-org-link").semantics { contentDescription = "Link your Claude organization" }) { connect("claude-org") }
        })
    }

    fun visible(e: ConnEntry): Boolean {
        val q = query.trim().lowercase()
        return q.isEmpty() || e.name.lowercase().contains(q) || (e.blurb ?: "").lowercase().contains(q) ||
            CONNECTION_GROUPS.firstOrNull { it.first == e.group }?.second?.lowercase()?.contains(q) == true
    }

    val listed = ConnectionsCatalog.entries.filter { store.isListed(it.id) } + store.promoted
    val needs = listed.filter { store.state(it.id) == ConnState.Needs }
    val counts = mapOf(
        ConnStatus.All to listed.size,
        ConnStatus.Connected to listed.count { store.state(it.id) == ConnState.On },
        ConnStatus.Needs to needs.size,
        ConnStatus.Off to listed.count { store.state(it.id) == ConnState.Off },
    )

    fun shown(group: String): List<ConnEntry> {
        val base = ConnectionsCatalog.entries.filter { it.group == group && store.isListed(it.id) }
        val extra = store.promoted.filter { it.group == group && base.none { b -> b.id == it.id } }
        return (base + extra).filter { e ->
            visible(e) && when (status) {
                ConnStatus.All -> true
                ConnStatus.Connected -> store.state(e.id) == ConnState.On
                ConnStatus.Needs -> store.state(e.id) == ConnState.Needs
                ConnStatus.Off -> store.state(e.id) == ConnState.Off
            }
        }
    }

    fun isOn(e: ConnEntry) = store.state(e.id) == ConnState.On && e.way != ConnWay.Included

    /** The Brains sub-sections in List order; Grid and Deck show the same order flat. */
    fun brainSections(rows: List<ConnEntry>) = listOf(
        "Works on any device" to rows.filter { it.runsOn != ConnRunsOn.Computer && store.isAvailable(it.id) },
        "Needs your computer" to rows.filter { it.runsOn == ConnRunsOn.Computer },
        "Coming soon" to rows.filter { it.runsOn != ConnRunsOn.Computer && !store.isAvailable(it.id) },
    ).filter { it.second.isNotEmpty() }

    @Composable
    fun RowList(rows: List<ConnEntry>) {
        rows.forEachIndexed { i, e ->
            if (i > 0) HorizontalDivider(thickness = 0.5.dp, color = c.line)
            ConnectionRow(
                entry = e,
                store = store,
                tag = null,
                statusExtra = if (e.group == "apps" && signedIn && isOn(e)) policyStore.summary(e.id) else null,
                trailing = when {
                    e.id == "claude-org" -> claudeOrgAction(store.row(e.id))
                    e.group == "apps" && signedIn && isOn(e) -> ({
                        Column(horizontalAlignment = Alignment.End) {
                            Box(
                                Modifier.heightIn(min = 48.dp).clickable(role = Role.Button) { permissionsFor = e }.padding(horizontal = 8.dp)
                                    .testTag("toolPermissions-${e.id}").semantics { contentDescription = "Tool permissions for ${e.name}" },
                                contentAlignment = Alignment.Center,
                            ) { Text("Tool permissions", color = c.ink, fontSize = 13.sp, fontWeight = FontWeight.SemiBold) }
                            SetupQuietButton("Disconnect") { disconnect(e.id, e.name) }
                        }
                    })
                    isOn(e) -> ({ SetupQuietButton("Disconnect") { disconnect(e.id, e.name) } })
                    else -> null
                },
                onConnect = { connect(e.id) },
                onFallback = ::fallback,
            )
        }
    }

    @Composable
    fun CardFor(e: ConnEntry, modifier: Modifier) {
        val t: (@Composable (Modifier) -> Unit)? = when {
            e.group == "apps" && signedIn && isOn(e) -> { m ->
                Column(m, horizontalAlignment = Alignment.CenterHorizontally) {
                    Box(
                        Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(role = Role.Button) { permissionsFor = e }
                            .testTag("toolPermissions-${e.id}").semantics { contentDescription = "Tool permissions for ${e.name}" },
                        contentAlignment = Alignment.Center,
                    ) { Text(policyStore.summary(e.id)?.let { "Tool permissions · $it" } ?: "Tool permissions", color = c.ink, fontSize = 13.sp, fontWeight = FontWeight.SemiBold) }
                    SetupQuietButton("Disconnect", Modifier.fillMaxWidth()) { disconnect(e.id, e.name) }
                }
            }
            isOn(e) -> { m -> SetupQuietButton("Disconnect", m) { disconnect(e.id, e.name) } }
            else -> null
        }
        ConnectionCard(e, store, t, onConnect = { connect(e.id) }, onFallback = ::fallback, modifier = modifier)
    }

    // Layout changes crossfade; they snap with the system animator scale at 0 (same check as ThinkingOrb)
    // or the in-app Reduce motion switch (Settings > Reduce motion) on.
    val reduceMotion = remember(context) {
        runCatching { Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }.getOrDefault(false) ||
            runCatching { SettingsPrefs.reduceMotion(context) }.getOrDefault(false)
    }
    val connector = rememberMoreConnector(store, onSheet = { sheet = it }, onMessage = { message = it })
    val screenWidth = LocalConfiguration.current.screenWidthDp.dp

    @Composable
    fun CardLayout(rows: List<ConnEntry>, deck: Boolean) {
        val cards = rows.filter { it.id != "claude-org" }
        val wide = rows.filter { it.id == "claude-org" }
        Column(Modifier.fillMaxWidth().testTag(if (deck) "connectionsDeck" else "connectionsGrid"), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            if (deck) {
                val listState = rememberLazyListState()
                LazyRow(
                    state = listState,
                    flingBehavior = rememberSnapFlingBehavior(listState),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    items(cards, key = { it.id }) { e -> CardFor(e, Modifier.width(screenWidth * 0.78f).fillMaxHeight()) }
                }
            } else {
                cards.chunked(2).forEach { pair ->
                    Row(Modifier.fillMaxWidth().height(IntrinsicSize.Min), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        pair.forEach { e -> CardFor(e, Modifier.weight(1f).fillMaxHeight()) }
                        if (pair.size == 1) Spacer(Modifier.weight(1f))
                    }
                }
            }
            if (wide.isNotEmpty()) Column(Modifier.fillMaxWidth().background(c.card, RoundedCornerShape(12.dp))) { RowList(wide) }
        }
    }

    Box(Modifier.fillMaxSize()) {
    Column(Modifier.fillMaxSize().background(AllternitTheme.colors.bgPrimary).statusBarsPadding().navigationBarsPadding().testTag("settingsConnections")) {
        // One glass toolbar (approved List/Grid/Deck design): search and the menu in the trailing capsule.
        LibraryHeader(
            title = "Connections",
            onBack = null,
            glass = true,
            leading = { LibraryIconButton(DesignR.drawable.ph_caret_left, "Back", "settingsBack", onClick = onBack) },
            trailing = {
                LibraryIconButton(DesignR.drawable.ph_magnifying_glass, "Search", "connectionsSearch") {
                    searchOpen = !searchOpen
                    if (!searchOpen) query = ""
                }
                Box {
                    LibraryIconButton(DesignR.drawable.ph_sliders_horizontal, "Layout and filter", "connectionsMenu") { menuOpen = true }
                    if (needs.isNotEmpty()) Box(Modifier.align(Alignment.TopEnd).padding(top = 9.dp, end = 9.dp).size(8.dp).background(AMBER, CircleShape).testTag("connectionsNeedsDot"))
                }
            },
        )
        if (searchOpen) LibrarySearchField(query, "Search connections", "connectionsSearchField", Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) { query = it }
        if (status != ConnStatus.All) {
            Row(
                Modifier.padding(horizontal = 16.dp, vertical = 6.dp).clip(RoundedCornerShape(50)).background(c.card)
                    .clickable { status = ConnStatus.All }.padding(start = 12.dp, end = 8.dp, top = 6.dp, bottom = 6.dp).testTag("connectionsStatusChip"),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                Text(status.label, color = AllternitTheme.colors.textPrimary, fontSize = 13.sp)
                Icon(painterResource(DesignR.drawable.ph_x), "Clear filter", tint = c.sub, modifier = Modifier.size(12.dp))
            }
        }
        Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 32.dp)) {
            if (store.loadError != null) Text(store.loadError ?: "", color = c.errFg, fontSize = 13.sp, modifier = Modifier.padding(vertical = 6.dp).testTag("connectionsError"))
            message?.let { Text(it, color = c.sub, fontSize = 13.sp, modifier = Modifier.padding(vertical = 6.dp)) }
            // The ChatGPT subscription and Codex CLI sit in one card at the top of Brains (List layout, when both rows
            // are listed); Codex then leaves Coding tools (iOS groupsAccountPair).
            val groupsPair = layout == LibraryLayout.List && AccountPair.pairsUp((shown("brains") + shown("tools")).map { it.id })
            val pairEntries = if (groupsPair) listOf(AccountPair.CHATGPT_ID, AccountPair.CODEX_ID).mapNotNull { id -> (shown("brains") + shown("tools")).firstOrNull { it.id == id } } else emptyList()
            for ((group, title, blurb) in CONNECTION_GROUPS) {
                val rows = shown(group).filter { !(groupsPair && group == "tools" && it.id == AccountPair.CODEX_ID) }
                if (rows.isEmpty()) continue
                Row(Modifier.fillMaxWidth().padding(start = 6.dp, end = 6.dp, top = 18.dp), verticalAlignment = Alignment.Bottom) {
                    Text(title, color = AllternitTheme.colors.textSecondary, fontSize = 14.sp, modifier = Modifier.weight(1f))
                    Text("${rows.count { store.state(it.id) == ConnState.On }} connected", color = c.mut, fontSize = 12.sp)
                }
                Text(blurb, color = c.mut, fontSize = 12.sp, lineHeight = 16.sp, modifier = Modifier.padding(start = 6.dp, top = 2.dp, bottom = 8.dp))
                if (group != "brains" && rows.size > 1 && rows.all { !store.isAvailable(it.id) && store.row(it.id).fallbacks.firstOrNull() == "this-mac" }) {
                    Row(
                        Modifier.fillMaxWidth().background(c.card, RoundedCornerShape(16.dp)).padding(14.dp).testTag("connectionsGroupCollapsed_$group"),
                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        Column(Modifier.weight(1f)) {
                            Text(rows.joinToString(", ") { it.name }, color = AllternitTheme.colors.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 2)
                            Text("These run on a computer. Add one when you want them.", color = c.sub, fontSize = 12.5.sp)
                        }
                        SetupPrimaryButton("Add a computer") { sheet = ConnSheet.OnComputer("this-mac") }
                    }
                } else {
                    Crossfade(layout, animationSpec = if (reduceMotion) snap() else tween(180), label = "connectionsLayout") { lay ->
                        if (lay == LibraryLayout.List) {
                            Column(Modifier.fillMaxWidth().background(c.card, RoundedCornerShape(12.dp)).testTag("connectionsGroup_$group")) {
                                if (group == "brains") {
                                    val pairIds = pairEntries.map { it.id }.toSet()
                                    if (pairEntries.isNotEmpty()) AccountPairCard(pairEntries, store, onConnect = { connect(it) }, onDisconnect = { disconnect(it.id, it.name) }, onFallback = ::fallback)
                                    brainSections(rows.filter { it.id !in pairIds }).forEachIndexed { si, (name, list) ->
                                        if (si > 0 || pairEntries.isNotEmpty()) HorizontalDivider(thickness = 0.5.dp, color = c.line)
                                        Text(
                                            name.uppercase(), color = c.mut, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.6.sp,
                                            modifier = Modifier.fillMaxWidth().background(AllternitTheme.colors.textPrimary.copy(alpha = 0.03f)).padding(horizontal = 16.dp, vertical = 6.dp),
                                        )
                                        RowList(list)
                                    }
                                } else RowList(rows)
                            }
                        } else {
                            Column(Modifier.testTag("connectionsGroup_$group")) {
                                CardLayout(if (group == "brains") brainSections(rows).flatMap { it.second } else rows, deck = lay == LibraryLayout.Deck)
                            }
                        }
                    }
                }
                // The More tile and panel close each group (only without a search or status filter).
                if (query.isBlank() && status == ConnStatus.All) {
                    MoreSection(group, title, store, layout, connector, onFallback = ::fallback, onBrowse = { browse = true })
                }
            }
            // Harness & Bots 2.0: how a chat or bot works, in the same List · Grid · Deck (status filters are about connections).
            if (status == ConnStatus.All) com.allternit.feature.bots.runtimes.RuntimesSection(layout, query) { runtimeOpen = it }
        }
    }
    runtimeOpen?.let { r ->
        androidx.activity.compose.BackHandler { runtimeOpen = null }
        com.allternit.feature.bots.runtimes.RuntimeDetailScreen(r, botsHost, onBack = { runtimeOpen = null }, onBotCreated = { id -> runtimeOpen = null; onBotCreated(id) })
    }
    if (browse) ConnectionsBrowseScreen(store, connector, onFallback = ::fallback, onBack = { browse = false })
    }
    MoreConnectorDialog(connector)

    if (menuOpen) {
        LibrarySheet("Connections", "connectionsMenuSheet", onDismiss = { menuOpen = false }, trailing = LibrarySheetAction("Done", bold = true) { menuOpen = false }) {
            Column(Modifier.padding(horizontal = 12.dp).padding(bottom = 24.dp).verticalScroll(rememberScrollState())) {
                Box(Modifier.padding(horizontal = 8.dp)) { LibraryLayoutButton(layout) { layout = it; LibraryLayout.save(context, LAYOUT_KEY, it) } }
                FilterLabel("Show")
                for (s in ConnStatus.entries) {
                    MenuRow(null, "${s.label} ${counts[s] ?: 0}", checked = status == s, testTag = "connectionsStatus${s.name}") { status = s; menuOpen = false }
                }
                FilterLabel("More")
                MenuRow(DesignR.drawable.ph_magnifying_glass, "Find more connections", testTag = "connectionsFindMore") { menuOpen = false; browse = true }
            }
        }
    }

    permissionsFor?.let { e -> ToolPermissionsSheet(e.id, e.name, policyStore) { permissionsFor = null } }

    if (confirmUnlink) {
        AlertDialog(
            onDismissRequest = { confirmUnlink = false },
            title = { Text("Unlink your Claude organization?") },
            text = { Text("Chats on your Claude organization stop. Allternit stops billing it right away. You can link it again later.") },
            confirmButton = {
                TextButton(onClick = { confirmUnlink = false; disconnect("claude-org", "Claude organization") }, modifier = Modifier.testTag("claude-org-unlink-confirm")) {
                    Text("Unlink", color = c.errFg)
                }
            },
            dismissButton = { TextButton(onClick = { confirmUnlink = false }) { Text("Cancel") } },
        )
    }

    sheet?.let { s ->
        ConnectSheetHost(
            sheet = s,
            store = store,
            onConnected = { sheet = null; reload() },
            onFallback = ::fallback,
            onDismiss = { sheet = null; reload() },
        )
    }
}
