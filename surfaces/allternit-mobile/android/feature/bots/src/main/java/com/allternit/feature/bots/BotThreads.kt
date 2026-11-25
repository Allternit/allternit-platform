package com.allternit.feature.bots

import com.allternit.core.design.AllternitSerif
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
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
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.artifacts.LibrarySearchField
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.PillRow
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR

@Composable
internal fun groupColor(g: BotThreadGroup) = when (g) {
    BotThreadGroup.Waiting -> AllternitStatic.StatusWarning
    BotThreadGroup.Working -> AllternitStatic.StatusSuccess
    BotThreadGroup.Queued -> AllternitStatic.StatusInfo
    BotThreadGroup.Idle, BotThreadGroup.Resolved -> AllternitTheme.colors.textTertiary
}

internal fun waitingLine(n: Int) = when (n) {
    0 -> "Nothing is waiting on you."
    1 -> "1 thread is waiting on you."
    else -> "$n threads are waiting on you."
}

/**
 * The threads overview (iOS BotThreadsOverview): the title and the waiting line, search and the
 * layout switch, then the open threads (not resolved, not the main chat) as cards. Polls every 15 s.
 */
@Composable
internal fun BotThreadsOverview(store: BotModeStore, botId: String?, title: String = "Threads", modifier: Modifier = Modifier, onOpen: (BotThread) -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var searching by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    var layoutOpen by remember { mutableStateOf(false) }
    var layout by remember { mutableStateOf(threadsLayout(context)) }
    LaunchedEffect(Unit) {
        while (true) {
            store.refreshThreads()
            delay(15_000)
        }
    }
    val all = botId?.let { id -> store.threads.filter { it.botId == id } } ?: store.threads
    val needle = query.trim().lowercase()
    val cards = all.filter { !it.incognito && it.group != "resolved" && !it.isMainChat }
        .filter { needle.isEmpty() || it.displayTitle.lowercase().contains(needle) }
        .sortedWith(BotThread.order)
    val waiting = botId?.let { store.waitingCount(it) } ?: store.waitingCount

    LazyColumn(modifier.fillMaxWidth().testTag("botThreadsOverview"), contentPadding = androidx.compose.foundation.layout.PaddingValues(bottom = 24.dp)) {
        item {
            Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 8.dp, top = 8.dp), verticalAlignment = Alignment.Bottom) {
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text(title, color = c.textPrimary, fontSize = 26.sp, fontFamily = AllternitSerif)
                    Text(waitingLine(waiting), color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.testTag("botThreadsWaiting"))
                }
                LibraryIconButton(DesignR.drawable.ph_magnifying_glass, "Search threads", "botThreadsSearch") { searching = !searching }
                LibraryIconButton(DesignR.drawable.ph_sliders_horizontal, "Layout", "botThreadsLayout") { layoutOpen = true }
            }
        }
        if (searching || query.isNotEmpty()) item { LibrarySearchField(query, "Search threads", "botThreadsSearchField", Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) { query = it } }
        if (cards.isEmpty()) item {
            val err = store.threadsError
            if (err != null && !store.isLoadingThreads) ListState(false, err, true, "Couldn't load threads", "") { scope.launch { store.refreshThreads() } }
            else if (store.isLoadingThreads) Text("Loading threads…", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 24.dp))
            else ListState(false, null, true, "", "No open threads. Start one from the composer.", art = com.allternit.core.design.EmptyArtKind.Envelope) {}
        } else if (layout == RosterLayout.List) items(cards, key = { it.id }) { t ->
            BotThreadCard(t, store, showsBot = botId == null, compact = true) { onOpen(t) }
        } else items(cards.chunked(2), key = { it.first().id }) { pair ->
            Row(Modifier.padding(horizontal = 16.dp, vertical = 5.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                pair.forEach { t -> Box(Modifier.weight(1f)) { BotThreadCard(t, store, showsBot = botId == null, compact = false) { onOpen(t) } } }
                if (pair.size == 1) Box(Modifier.weight(1f))
            }
        }
    }
    if (layoutOpen) LibrarySheet(title = "Layout", testTag = "botThreadsLayoutSheet", onDismiss = { layoutOpen = false }, trailing = LibrarySheetAction("Done", bold = true) { layoutOpen = false }) {
        PillRow(RosterLayout.entries, layout, { it.label }, { "botThreadsLayout${it.label}" }, Modifier.padding(16.dp)) {
            layout = it
            context.getSharedPreferences("allternit_bots", android.content.Context.MODE_PRIVATE).edit().putString("bot-threads", it.raw).apply()
        }
    }
}

private fun threadsLayout(context: android.content.Context): RosterLayout {
    val raw = context.getSharedPreferences("allternit_bots", android.content.Context.MODE_PRIVATE).getString("bot-threads", "list")
    return RosterLayout.entries.firstOrNull { it.raw == raw } ?: RosterLayout.List
}

/** One thread card (iOS OverviewCards item): status, title, the bot, the status line, the time and progress. */
@Composable
private fun BotThreadCard(t: BotThread, store: BotModeStore, showsBot: Boolean, compact: Boolean, onOpen: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val g = BotThreadGroup.of(t.group)
    val status = when (g) {
        BotThreadGroup.Waiting, BotThreadGroup.Working -> g.title
        BotThreadGroup.Queued -> "Queued"
        else -> null
    }
    Column(
        Modifier.padding(horizontal = if (compact) 16.dp else 0.dp, vertical = if (compact) 5.dp else 0.dp).fillMaxWidth()
            .clip(RoundedCornerShape(14.dp)).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(14.dp))
            .testTag("botThread${t.id}")
            .clickable(role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onOpen() }
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            val bot = store.bot(t.botId)
            if (showsBot && bot != null) BotStatusAvatar(bot, 22.dp, working = g == BotThreadGroup.Working, waiting = g == BotThreadGroup.Waiting) else Dot(groupColor(g))
            if (status != null) Text(status, color = groupColor(g), fontSize = 12.sp, fontWeight = FontWeight.Medium)
            if (showsBot && bot != null) com.allternit.core.design.BotNameText(bot.displayName, color = c.textTertiary, fontSize = 12.sp, maxLines = 1)
            androidx.compose.foundation.layout.Spacer(Modifier.weight(1f))
            Text(botTimeRelative(t.lastActivityAt), color = c.textTertiary, fontSize = 11.5.sp)
        }
        Text(t.displayTitle, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 2, overflow = TextOverflow.Ellipsis)
        t.statusLine?.takeIf { it.isNotEmpty() }?.let { Text(it, color = c.textSecondary, fontSize = 13.sp, maxLines = 2, overflow = TextOverflow.Ellipsis) }
        t.progressLabel?.let { ProgressRing(t, it) }
    }
}

@Composable
private fun ProgressRing(t: BotThread, label: String) {
    val c = AllternitTheme.colors
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        val track = c.fillNeutral
        val ink = c.textPrimary
        val frac = t.progress[0].toFloat() / maxOf(t.progress[1], 1)
        Canvas(Modifier.size(12.dp).rotate(-90f)) {
            drawArc(track, 0f, 360f, false, style = Stroke(2.dp.toPx()), size = Size(size.width, size.height))
            drawArc(ink, 0f, 360f * frac, false, style = Stroke(2.dp.toPx()))
        }
        Text(label, color = c.textSecondary, fontSize = 11.5.sp)
    }
}

/** A grouped list row (iOS BotThreadRow): face or dot, TASK tag, title, status line, progress, time. */
@Composable
internal fun BotThreadRow(t: BotThread, store: BotModeStore, showsBot: Boolean, onOpen: () -> Unit) {
    val c = AllternitTheme.colors
    val g = BotThreadGroup.of(t.group)
    Row(
        Modifier.fillMaxWidth().clickable(role = Role.Button, onClick = onOpen).testTag("botThreadRow${t.id}").heightIn(min = 48.dp).padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        val bot = store.bot(t.botId)
        if (showsBot && bot != null) BotAvatar(bot, 22.dp) else Box(Modifier.size(22.dp), contentAlignment = Alignment.Center) { Dot(groupColor(g)) }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                if (t.kind == "task") Text("TASK", color = c.textTertiary, fontSize = 10.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.6.sp)
                Text(t.displayTitle, color = c.textPrimary, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            t.statusLine?.takeIf { it.isNotEmpty() }?.let { Text(it, color = c.textSecondary, fontSize = 12.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis) }
        }
        t.progressLabel?.let { ProgressRing(t, it) }
        Text(botTimeRelative(t.lastActivityAt), color = c.textTertiary, fontSize = 11.5.sp)
    }
}

/**
 * Grouped threads with folding sections (iOS BotThreadsPanel): Waiting on you, Working, Queued,
 * Idle, Resolved (folded by default, with Reopen when the caller can).
 */
@Composable
internal fun BotThreadsPanel(store: BotModeStore, threads: List<BotThread>, botId: String?, greeting: String?, onOpen: (BotThread) -> Unit, onReopen: ((BotThread) -> Unit)? = null) {
    val c = AllternitTheme.colors
    var collapsed by remember { mutableStateOf(setOf("resolved")) }
    val rows = threads.filter { !it.incognito }.sortedWith(BotThread.order)
    Column(Modifier.fillMaxWidth()) {
        if (greeting != null) Column(Modifier.padding(horizontal = 20.dp, vertical = 8.dp)) {
            Text(greeting, color = c.textPrimary, fontSize = 22.sp, fontFamily = AllternitSerif)
            Text(waitingLine(rows.count { it.group == "waiting" }), color = c.textSecondary, fontSize = 13.sp)
        }
        if (rows.isEmpty()) Column(Modifier.fillMaxWidth().padding(20.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            com.allternit.core.design.EmptyArt(com.allternit.core.design.EmptyArtKind.Envelope)
            Text("No threads yet. Start one from the composer.", color = c.textSecondary, fontSize = 14.sp)
        }
        val grouped = rows.groupBy { it.group }
        BotThreadGroup.entries.forEach { g ->
            val items = grouped[g.raw].orEmpty()
            if (items.isEmpty()) return@forEach
            val folded = g.raw in collapsed
            Row(
                Modifier.fillMaxWidth().clickable { collapsed = if (folded) collapsed - g.raw else collapsed + g.raw }
                    .semantics { stateDescription = if (folded) "Collapsed" else "Expanded" }.testTag("botThreadsGroup${g.raw}")
                    .padding(horizontal = 16.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                Icon(painterResource(com.allternit.core.design.R.drawable.ph_caret_down), null, tint = c.textTertiary, modifier = Modifier.size(12.dp).rotate(if (folded) -90f else 0f))
                Text(g.title, color = groupColor(g), fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                Text("${items.size}", color = c.textTertiary, fontSize = 12.sp)
            }
            if (!folded) items.forEach { t ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.weight(1f)) { BotThreadRow(t, store, botId == null) { onOpen(t) } }
                    if (g == BotThreadGroup.Resolved && onReopen != null) Pill("Reopen", "botThreadReopen${t.id}") { onReopen(t) }
                }
            }
        }
    }
}

/** The bot's plans as task groups (iOS BotThreadTasksView): each thread with steps, foldable, the open one first. */
@Composable
internal fun BotThreadTasks(store: BotModeStore, bot: BotRecord, currentSessionId: String?, onOpen: (BotThread) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var folded by remember { mutableStateOf(emptySet<String>()) }
    LaunchedEffect(Unit) { store.refreshThreads() }
    val rows = store.threads.filter { it.botId == bot.id && !it.incognito && it.group != "resolved" && it.todo.isNotEmpty() }
        .sortedWith(compareByDescending<BotThread> { it.currentSessionId != null && it.currentSessionId == currentSessionId }.then(BotThread.order))
    LazyColumn(Modifier.fillMaxSize().testTag("botTasks"), contentPadding = androidx.compose.foundation.layout.PaddingValues(vertical = 8.dp)) {
        if (rows.isEmpty()) item {
            ListState(store.isLoadingThreads, store.threadsError, true, "Couldn't load tasks", "No tasks yet. When ${bot.displayName} plans work in a thread, its steps show here.", com.allternit.core.design.EmptyArtKind.Bell) { scope.launch { store.refreshThreads() } }
        }
        items(rows, key = { it.id }) { t ->
            val done = t.todo.count { it.second == "done" }
            val isFolded = t.id in folded
            val g = BotThreadGroup.of(t.group)
            Column(Modifier.padding(horizontal = 16.dp, vertical = 5.dp).fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(14.dp))) {
                Row(Modifier.fillMaxWidth().padding(start = 4.dp, end = 12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    LibraryIconButton(com.allternit.core.design.R.drawable.ph_caret_down, if (isFolded) "Show steps" else "Hide steps", "botTaskFold${t.id}", iconSize = 12) {
                        folded = if (isFolded) folded - t.id else folded + t.id
                    }
                    Row(Modifier.weight(1f).clickable { onOpen(t) }.padding(vertical = 12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Dot(groupColor(g), 7.dp)
                        Text(t.displayTitle, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                        Text("$done/${t.todo.size}", color = c.textSecondary, fontSize = 12.sp)
                        Icon(painterResource(com.allternit.core.design.R.drawable.ph_caret_right), null, tint = c.textTertiary, modifier = Modifier.size(12.dp))
                    }
                }
                if (!isFolded) t.todo.forEach { (text, state) ->
                    Row(Modifier.fillMaxWidth().clickable { onOpen(t) }.padding(start = 44.dp, end = 12.dp, top = 4.dp, bottom = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        val (icon, tint) = when (state) {
                            "done" -> DesignR.drawable.ph_check_circle_fill to AllternitStatic.StatusSuccess
                            "active" -> com.allternit.core.design.R.drawable.ph_clock to AllternitStatic.StatusInfo
                            else -> com.allternit.core.design.R.drawable.ph_x to c.textTertiary
                        }
                        if (state == "pending" || (state != "done" && state != "active")) Box(Modifier.size(14.dp).border(1.5.dp, tint, androidx.compose.foundation.shape.CircleShape))
                        else Icon(painterResource(icon), null, tint = tint, modifier = Modifier.size(14.dp))
                        Text(text, color = if (state == "done") c.textTertiary else c.textPrimary, fontSize = 13.5.sp)
                    }
                }
            }
        }
    }
}
