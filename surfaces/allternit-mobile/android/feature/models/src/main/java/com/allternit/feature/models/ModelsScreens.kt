package com.allternit.feature.models

import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import com.allternit.core.design.DockSpec
import com.allternit.core.design.DockingBlockTitle
import com.allternit.core.design.FilterPills
import com.allternit.core.design.GlassPill
import com.allternit.core.design.PolishGlass
import com.allternit.core.design.SectionLabel
import com.allternit.core.design.SlidingSegmented
import com.allternit.core.design.TabPage
import com.allternit.core.design.rememberDockOffset
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
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
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.api.ApiResult
import com.allternit.core.stream.ChatStreamEvent
import com.allternit.core.api.gen.AgentChatRequest
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.artifacts.LibrarySearchField
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.PillRow
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.RelayPick
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR
import com.allternit.core.design.EmptyArtKind
import com.allternit.core.design.InlineEmpty

/** What the shell gives Models: the menu button, sign-in, the chat backend (Benchmark) and "use in chat". */
class ModelsHost(
    val signedIn: Boolean,
    val backend: ChatBackend,
    val menuButton: @Composable () -> Unit,
    /** iOS openChat: select the model and open a new chat. */
    val onRun: (modelId: String) -> Unit,
)

private enum class Segment(val label: String) { Catalog("Catalog"), Device("On this phone") }
private enum class FwFilter(val label: String) { Fits("Fits this phone"), Everything("Everything"), PhoneCollection("Phone collection") }
private enum class Face(val raw: String, val label: String) { List("list", "List"), Grid("grid", "Grid"), Deck("deck", "Deck") }

private fun memoryBytes(context: android.content.Context): Long {
    val am = context.getSystemService(android.content.Context.ACTIVITY_SERVICE) as android.app.ActivityManager
    val info = android.app.ActivityManager.MemoryInfo()
    am.getMemoryInfo(info)
    return info.totalMem
}

private fun sizeText(bytes: Long?): String? = bytes?.takeIf { it > 0 }?.let {
    if (it >= 1L shl 30) String.format("%.1f GB", it / (1L shl 30).toDouble()) else String.format("%d MB", it / (1L shl 20))
}

private fun paramsText(p: Long?): String? = p?.takeIf { it > 0 }?.let {
    when {
        it >= 1_000_000_000L -> String.format("%.1fB", it / 1e9)
        it >= 1_000_000L -> String.format("%dM", it / 1_000_000L)
        else -> "$it"
    }
}

/** The drawer's Models section (iOS ModelsTabView): Catalog / On this phone, search, filters, faces, detail. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ModelsRoot(host: ModelsHost) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    var segment by remember { mutableStateOf(Segment.Catalog) }
    var filter by remember { mutableStateOf(FwFilter.Fits) }
    var face by remember { mutableStateOf(Face.Grid) }
    var query by remember { mutableStateOf("") }
    var catalog by remember { mutableStateOf<List<FwRow>>(emptyList()) }
    var ranked by remember { mutableStateOf<List<String>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var down by remember { mutableStateOf<String?>(null) }
    var attempt by remember { mutableStateOf(0) }
    var selected by remember { mutableStateOf<FwRow?>(null) }
    var providers by remember { mutableStateOf(false) }
    var benchmark by remember { mutableStateOf(false) }
    var limit by remember { mutableStateOf(40) }
    val budget = remember { (memoryBytes(context) * 0.6).toLong() }

    LaunchedEffect(attempt) {
        loading = true
        when (val r = FreeweightsApi.catalog()) {
            is FwLoad.Live -> { catalog = r.value; down = null }
            is FwLoad.Down -> down = r.reason
        }
        loading = false
    }
    LaunchedEffect(query, filter) {
        if (filter != FwFilter.Everything) return@LaunchedEffect
        delay(300)
        (FreeweightsApi.search(query) as? FwLoad.Live)?.let { ranked = it.value }
    }
    BackHandler(enabled = benchmark) { benchmark = false }
    if (benchmark) { BenchmarkScreen(host, onBack = { benchmark = false }); return }

    val byId = remember(catalog) { catalog.associateBy { it.id } }
    val q = query.trim().lowercase()
    val shown: List<FwRow> = when (filter) {
        FwFilter.Everything -> ranked.mapNotNull { byId[it] ?: FwRow(it, null, null, null, null, null, emptyList(), null) }
        FwFilter.Fits -> catalog.filter { (it.totalSize ?: Long.MAX_VALUE) <= budget && it.kindIsModel() }
        FwFilter.PhoneCollection -> catalog.filter { it.section == "phone" }
    }.filter { q.isEmpty() || filter == FwFilter.Everything || it.id.lowercase().contains(q) || it.name.lowercase().contains(q) }
    val visible = if (filter == FwFilter.Everything) shown else shown.take(limit)

    val scroll = rememberScrollState()
    val dockOffset = rememberDockOffset(scroll)
    Box(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("modelsRoot")) {
        when (segment) {
            Segment.Device -> Column(Modifier.fillMaxSize()) {
                Spacer(Modifier.height(52.dp))
                SlidingSegmented(Segment.entries.map { it.label }, segment.ordinal, { segment = Segment.entries[it] }, Modifier.padding(horizontal = 14.dp, vertical = 4.dp).testTag("modelsSegments"), itemTag = { "modelsSegment${Segment.entries[it].name}" })
                OnThisPhoneScreen(host) { segment = Segment.Catalog }
            }
            Segment.Catalog -> PullToRefreshBox(isRefreshing = loading && catalog.isNotEmpty(), onRefresh = { attempt++ }, modifier = Modifier.fillMaxSize()) {
                Column(Modifier.fillMaxSize().verticalScroll(scroll).padding(bottom = 24.dp)) {
                    Spacer(Modifier.height(52.dp))
                    TabPage(
                        description = "Open-weights models with checked licenses. Download one to this phone, or send a pull to your computer.",
                        title = { Box(Modifier.padding(top = 12.dp).height(DockSpec.BH.dp).semantics { heading(); contentDescription = "Freeweights" }) },
                    ) {
                        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                            SlidingSegmented(Segment.entries.map { it.label }, segment.ordinal, { segment = Segment.entries[it] }, Modifier.testTag("modelsSegments"), itemTag = { "modelsSegment${Segment.entries[it].name}" })
                            LibrarySearchField(query, "Search Freeweights", "modelsSearch") { query = it; limit = 40 }
                            SlidingSegmented(Face.entries.map { it.label }, face.ordinal, { face = Face.entries[it] }, Modifier.testTag("modelsFace"), itemTag = { "modelsFace${Face.entries[it].raw}" })
                            FilterPills(FwFilter.entries.map { it.label }, filter.ordinal, { filter = FwFilter.entries[it]; limit = 40 }, Modifier.testTag("modelsFilter"), itemTag = { "modelsFilter${FwFilter.entries[it].name}" })
                            SectionLabel("Models")
                            when {
                                loading && catalog.isEmpty() -> Box(Modifier.fillMaxWidth().padding(top = 40.dp), Alignment.Center) { CircularProgressIndicator(color = c.textSecondary) }
                                down != null && catalog.isEmpty() -> FriendlyState(FriendlyStyle.Offline, "Freeweights is not answering", "The catalog couldn't be reached and there is no copy on this phone yet. Check your connection, then try again.", Modifier.padding(top = 20.dp), "Retry", { attempt++ }) { Icon(painterResource(DesignR.drawable.ph_warning), null, tint = AllternitStatic.StatusWarning) }
                                visible.isEmpty() -> FriendlyState(FriendlyStyle.Empty, if (query.isEmpty()) "Nothing here yet" else "No models match “$query”", "Try Everything, or a family name like qwen, gemma or smollm.", Modifier.padding(top = 20.dp), art = EmptyArtKind.Sheets) { }
                                else -> {
                                    Text("This phone can give a model up to ${sizeText(budget)} of memory.", color = c.textSecondary, fontSize = 12.sp)
                                    when (face) {
                                        Face.List -> Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(20.dp)).background(PolishGlass.track())) {
                                            visible.forEachIndexed { i, m ->
                                                ModelRow(m, budget) { selected = m }
                                                if (i < visible.lastIndex) Box(Modifier.padding(start = 70.dp, end = 14.dp).fillMaxWidth().height(1.dp).background(c.borderSubtle))
                                            }
                                        }
                                        Face.Grid -> visible.chunked(2).forEach { pair ->
                                            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                                                pair.forEach { m -> Box(Modifier.weight(1f)) { ModelTile(m, budget) { selected = m } } }
                                                if (pair.size == 1) Box(Modifier.weight(1f)) {}
                                            }
                                        }
                                        Face.Deck -> {
                                            val pager = rememberPagerState { visible.size }
                                            HorizontalPager(pager, Modifier.fillMaxWidth().heightIn(min = 300.dp)) { i -> ModelTile(visible[i], budget, big = true) { selected = visible[i] } }
                                        }
                                    }
                                    val more = if (filter == FwFilter.Everything) false else shown.size > visible.size
                                    if (more) Box(Modifier.fillMaxWidth().padding(top = 4.dp), Alignment.Center) {
                                        GlassPill(onClick = { limit += 40 }, modifier = Modifier.testTag("modelsMore"), height = 40.dp) {
                                            Text("Show more (${shown.size - visible.size} left)", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if (segment == Segment.Catalog) DockingBlockTitle("Freeweights", dockOffset, barCenter = 30.dp, modifier = Modifier.fillMaxSize(), top = 64.dp)
        LibraryHeader(title = if (segment == Segment.Device) "Models" else "", onBack = null, leading = host.menuButton, glass = true, trailing = {
            LibraryIconButton(DesignR.drawable.ph_sliders_horizontal, "Providers and default model", "modelsProviders") { providers = true }
            LibraryIconButton(DesignR.drawable.ph_arrows_down_up, "Refresh models", "modelsRefresh", enabled = !loading) { attempt++ }
        })
        FreeweightsIntroHost(host.signedIn)
    }
    selected?.let { m -> ModelDetailSheet(m, host, onDismiss = { selected = null }) }
    if (providers) ProvidersSheet(onDismiss = { providers = false }, onBenchmark = { providers = false; benchmark = true })
}

private fun FwRow.kindIsModel(): Boolean = true

@Composable
private fun ModelMark(m: FwRow, size: Int) {
    val c = AllternitTheme.colors
    Box(Modifier.size(size.dp).border(BorderStroke(1.5.dp, c.textPrimary), RoundedCornerShape(8.dp)), Alignment.Center) {
        Text(m.mono, color = c.textPrimary, fontSize = (size / 3).sp, fontWeight = FontWeight.Bold)
    }
}

@Composable
private fun FitLabel(m: FwRow, budget: Long) {
    val size = m.totalSize
    val (text, tint) = when {
        size == null || size <= 0 -> "Size unknown" to AllternitTheme.colors.textSecondary
        size <= budget -> "Fits this phone · ${sizeText(size)}" to AllternitStatic.StatusSuccess
        else -> "Too big for this phone · ${sizeText(size)}" to AllternitStatic.StatusWarning
    }
    Text(text, color = tint, fontSize = 12.sp, fontWeight = FontWeight.Medium)
}

@Composable
private fun ModelTile(m: FwRow, budget: Long, big: Boolean = false, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val shape = RoundedCornerShape(20.dp)
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    Column(
        Modifier.fillMaxWidth().clip(shape).background(PolishGlass.track()).background(if (pressed) c.textPrimary.copy(alpha = 0.06f) else Color.Transparent)
            .clickable(interactionSource = source, indication = null, role = Role.Button, onClickLabel = "Opens the model, its builds and Download", onClick = onClick).testTag("modelTile${m.id}").padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        ModelMark(m, if (big) 96 else 56)
        Text(m.name, color = c.textPrimary, fontSize = if (big) 20.sp else 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
        Text(listOfNotNull(paramsText(m.params)?.let { "$it params" }, m.publisher.ifEmpty { null }).joinToString(" · "), color = c.textSecondary, fontSize = 11.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        FitLabel(m, budget)
    }
}

/** A row in the grouped card: mark, name, publisher, round tags; the whole row highlights while pressed. */
@Composable
private fun ModelRow(m: FwRow, budget: Long, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val size = m.totalSize
    val fitText = when {
        size == null || size <= 0 -> "Size unknown"
        size <= budget -> "Fits this phone"
        else -> "Too big for this phone"
    }
    val fitTint = when {
        size == null || size <= 0 -> null
        size <= budget -> AllternitStatic.StatusSuccess
        else -> AllternitStatic.StatusWarning
    }
    Row(
        Modifier.fillMaxWidth().background(if (pressed) c.textPrimary.copy(alpha = 0.06f) else Color.Transparent)
            .clickable(interactionSource = source, indication = null, role = Role.Button, onClick = onClick).testTag("modelRow${m.id}")
            .padding(horizontal = 14.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        ModelMark(m, 44)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Text(m.name, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(listOfNotNull(paramsText(m.params)?.let { "$it params" }, m.publisher.ifEmpty { null }).joinToString(" · "), color = c.textSecondary, fontSize = 11.5.sp, maxLines = 1)
            Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                FwTag(fitText, tint = fitTint, dot = fitTint != null)
                m.license?.takeIf { it.isNotBlank() }?.let { FwTag(it) }
                sizeText(size)?.let { FwTag(it) }
            }
        }
        Icon(painterResource(DesignR.drawable.ph_caret_right), null, tint = c.textTertiary, modifier = Modifier.size(13.dp))
    }
}

/** The model page (iOS freeweightsDetailSheet): what it is, whether it fits, Use in chat, Send to your computer. */
@Composable
private fun ModelDetailSheet(m: FwRow, host: ModelsHost, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var pull by remember { mutableStateOf<PullStatus?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var sending by remember { mutableStateOf(false) }
    LaunchedEffect(pull?.id) {
        val id = pull?.id ?: return@LaunchedEffect
        while (pull?.status.let { it != "ready" && it != "completed" && it != "done" && it != "failed" && it != "error" }) {
            delay(2000)
            val s = ComputerPullApi.status(id)
            if (s == null) { error = "Lost touch with your computer. The pull keeps going there."; break }
            pull = s
        }
    }
    LibrarySheet(m.name, "modelDetailSheet", onDismiss, leading = LibrarySheetAction("Close", testTag = "modelDetailClose", onClick = onDismiss)) {
        Column(Modifier.padding(horizontal = 20.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
                ModelMark(m, 64)
                Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text(m.id, color = c.textSecondary, fontSize = 12.sp)
                    Text(listOfNotNull(paramsText(m.params)?.let { "$it params" }, sizeText(m.totalSize), m.license).joinToString(" · "), color = c.textPrimary, fontSize = 14.sp)
                }
            }
            if (m.formats.isNotEmpty()) Text("Formats: ${m.formats.joinToString(", ")}", color = c.textSecondary, fontSize = 13.sp)
            m.note?.let { Text(it, color = c.textSecondary, fontSize = 13.sp) }
            // Gap run GS-D: Download a phone build and run it here (iOS device builds).
            PhoneBuildsSection(m)
            if (!host.signedIn) Text("Sign in to send pulls to your computer.", color = c.textSecondary, fontSize = 13.sp)
            pull?.let { Text(ComputerPullApi.label(it.status), color = c.textPrimary, fontSize = 14.sp, modifier = Modifier.testTag("modelPullStatus")) }
            error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp) }
            Box(
                Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(RoundedCornerShape(12.dp)).background(if (host.signedIn && !sending && pull == null) c.accentPrimary else c.bgSecondary)
                    .clickable(enabled = host.signedIn && !sending && pull == null, role = Role.Button) {
                        sending = true; error = null
                        scope.launch { val (st, err) = ComputerPullApi.start(m.id); pull = st; error = err; sending = false }
                    }.testTag("modelSendPull"),
                Alignment.Center,
            ) { Text(if (sending) "Sending…" else "Send to my computer", color = c.onAccent, fontSize = 15.sp, fontWeight = FontWeight.SemiBold) }
            Text("Use in a chat", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium,
                modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(RoundedCornerShape(12.dp)).border(BorderStroke(1.dp, c.borderStrong), RoundedCornerShape(12.dp))
                    .clickable(role = Role.Button) { onDismiss(); host.onRun(m.id) }.testTag("modelUseInChat").padding(vertical = 14.dp), textAlign = androidx.compose.ui.text.style.TextAlign.Center)
            Text("For a legal notice such as a copyright claim, use the takedown address on freeweights.allternit.com.", color = c.textTertiary, fontSize = 12.sp)
        }
    }
}

/** iOS ModelManagementView entry: the runtime's models and the Benchmark row. */
@Composable
private fun ProvidersSheet(onDismiss: () -> Unit, onBenchmark: () -> Unit) {
    val c = AllternitTheme.colors
    var models by remember { mutableStateOf<List<RuntimeModelRow>?>(null) }
    LaunchedEffect(Unit) { models = runtimeModels() }
    LibrarySheet("Providers and default model", "modelsProvidersSheet", onDismiss, leading = LibrarySheetAction("Done", testTag = "modelsProvidersDone", onClick = onDismiss)) {
        Column(Modifier.padding(horizontal = 20.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text("Benchmark", color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.Medium, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(role = Role.Button, onClick = onBenchmark).testTag("modelsBenchmarkRow").padding(vertical = 12.dp))
            Text("Models your account can use", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 8.dp))
            val list = models
            when {
                list == null -> Text("Loading…", color = c.textSecondary, fontSize = 14.sp)
                list.isEmpty() -> InlineEmpty("No models yet. Pick one from the composer once a computer is connected.", art = EmptyArtKind.ConnectScene)
                else -> list.take(60).forEach { Text(it.name, color = c.textPrimary, fontSize = 14.sp, modifier = Modifier.padding(vertical = 6.dp)) }
            }
        }
    }
}

private data class BenchResult(val modelLabel: String, val ttft: Double, val gen: Double, val tokens: Int) {
    val tps: Double get() = if (gen > 0) tokens / gen else 0.0
}

/** iOS BenchmarkView: this phone's specs, a model picker, "Run Benchmark" (one fixed prompt, timed) and the history. */
@Composable
private fun BenchmarkScreen(host: ModelsHost, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    var models by remember { mutableStateOf<List<RuntimeModelRow>>(emptyList()) }
    var picked by remember { mutableStateOf<String?>(null) }
    var running by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var results by remember { mutableStateOf<List<BenchResult>>(emptyList()) }
    LaunchedEffect(Unit) { models = runtimeModels() }

    fun run() {
        running = true; error = null
        scope.launch {
            val label = models.firstOrNull { it.id == picked }?.name ?: "Backend default"
            try {
                val runtimeId = (host.backend.relay.resolve() as? RelayPick.Computer)?.runtimeId
                if (runtimeId == null) { error = "Connect a computer first: the benchmark runs on it."; running = false; return@launch }
                val session = (host.backend.createSession("Benchmark", "chat", null, null, null, null, "regular") as? ApiResult.Ok)?.value
                if (session == null) { error = "Couldn't start the benchmark. Try again."; running = false; return@launch }
                val start = System.nanoTime()
                var first = 0L
                var text = ""
                host.backend.streamTurn(runtimeId, AgentChatRequest(chatId = session.id, message = "Write a two-paragraph explanation of how photosynthesis works.", runtimeModelId = picked)).collect { e ->
                    if (e is ChatStreamEvent.TextDelta) { if (first == 0L) first = System.nanoTime(); text += e.text }
                }
                if (first == 0L || text.isEmpty()) error = "The model didn't return any text to measure."
                else results = listOf(BenchResult(label, (first - start) / 1e9, (System.nanoTime() - first) / 1e9, maxOf(1, text.length / 4))) + results
                host.backend.deleteSession(session.id)
            } catch (e: Exception) {
                error = "The benchmark couldn't finish. Check your connection and try again."
            }
            running = false
        }
    }

    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("benchmarkRoot")) {
        LibraryHeader(title = "Benchmark", onBack = onBack, backTag = "benchmarkBack")
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Text("This device", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
            listOf("Device" to "${Build.MANUFACTURER} ${Build.MODEL}", "OS" to "Android ${Build.VERSION.RELEASE}", "CPU cores" to "${Runtime.getRuntime().availableProcessors()}", "Memory" to (sizeText(memoryBytes(context)) ?: "")).forEach { (k, v) ->
                Row { Text(k, color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f)); Text(v, color = c.textSecondary, fontSize = 14.sp) }
            }
            Text("Speed test", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 8.dp))
            PillRow(listOf<String?>(null) + models.take(8).map { it.id }, picked, { id -> models.firstOrNull { it.id == id }?.name ?: "Backend default" }, { "benchModel${it ?: "default"}" }) { picked = it }
            Box(
                Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(RoundedCornerShape(12.dp)).background(if (running) c.bgSecondary else c.accentPrimary)
                    .clickable(enabled = !running, role = Role.Button) { run() }.testTag("benchmarkRun"), Alignment.Center,
            ) { Text(if (running) "Running…" else "Run Benchmark", color = c.onAccent, fontSize = 15.sp, fontWeight = FontWeight.SemiBold) }
            error?.let { Text(it, color = AllternitStatic.StatusWarning, fontSize = 12.sp) }
            Text("Sends one fixed prompt and times the response over the network. An approximation, not a real inference-engine benchmark.", color = c.textTertiary, fontSize = 12.sp)
            if (results.isNotEmpty()) {
                Text("History", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 8.dp))
                results.forEach { r ->
                    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPanel).padding(14.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(r.modelLabel, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                        Text(String.format("First token %.2fs · %.1f tokens/s (about %d tokens)", r.ttft, r.tps, r.tokens), color = c.textSecondary, fontSize = 13.sp)
                    }
                }
            }
        }
    }
}
