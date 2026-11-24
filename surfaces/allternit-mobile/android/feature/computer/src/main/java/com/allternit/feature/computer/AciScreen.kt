package com.allternit.feature.computer

import com.allternit.core.design.AllternitSerif
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR
import com.allternit.feature.chat.ChatBackend

/** What the shell hands ACI mode (iOS ACITabView's bindings). */
data class AciHost(
    val signedIn: Boolean,
    val backend: ChatBackend,
    val menuButton: @Composable () -> Unit,
    val openSignIn: () -> Unit,
    val composerControls: @Composable (sessionOpen: Boolean, onNew: () -> Unit) -> Unit = { _, _ -> },
    /** Opens a linked phone's live view (iOS ACIComputersPane fullScreenCover PhoneLiveView); wave F4a. */
    val openPhoneLive: ((CloudPhone) -> Unit)? = null,
    /** The existing cloud restore points page (feature:settings), opened from the console Backups tab. */
    val restorePoints: (@Composable (onDone: () -> Unit) -> Unit)? = null,
)

private sealed interface AciDestination {
    data object Landing : AciDestination
    data class Browser(val url: String) : AciDestination
    data class Agent(val goal: String) : AciDestination
}

/**
 * ACI mode (iOS ACITabView, the web phone's AciScreen.tsx): a ☰-only header, then the view the
 * drawer's ACI rail picks. Agent is the "Computer the agent uses" picker over the browser landing,
 * the in-app browser or a live agent run; Computers lists paired computers, phones, backups and
 * workflows. The Pair suggestion sits on top until dismissed once.
 */
@Composable
fun AciRoot(host: AciHost) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    var destination by remember { mutableStateOf<AciDestination>(AciDestination.Landing) }
    var devices by remember { mutableStateOf<List<AciDevice>?>(null) }
    var pairSeen by remember { mutableStateOf(AciOnceNudge.seen(context, "pair-computer")) }
    var teachSeen by remember { mutableStateOf(AciOnceNudge.seen(context, "teach")) }
    val tab = AciNav.tab
    val landing = destination == AciDestination.Landing
    LaunchedEffect(host.signedIn) {
        devices = if (host.signedIn) (AciApi.devices() as? AciApi.Result.Ok)?.value.orEmpty() else emptyList()
    }
    val showPair = devices?.isEmpty() == true && !pairSeen && (tab == AciTab.Agent || tab == AciTab.Computers)

    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().testTag("aciScreen")) {
        // The browser and a run bring their own top bar.
        if (tab != AciTab.Agent || landing) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp)) { host.menuButton() }
        }
        val list = devices.orEmpty()
        if (tab == AciTab.Agent && landing && list.isNotEmpty()) {
            AciTargetPicker(host.backend, list, Modifier.padding(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 10.dp))
        }
        if (showPair && (tab != AciTab.Agent || landing)) {
            AciNudgeCard(
                hero = AciHero.Pair,
                title = "Pair a computer",
                message = "Connect Allternit Desktop with this account so the computer agent can work on your computer.",
                closeLabel = "Dismiss pairing suggestion",
                actionTitle = "Open Computers",
                onClose = {
                    AciOnceNudge.remember(context, "pair-computer")
                    pairSeen = true
                },
                onAction = {
                    AciOnceNudge.remember(context, "pair-computer")
                    pairSeen = true
                    AciNav.open(AciTab.Computers)
                },
            )
        }
        // iOS ACITabView showTeachNudge: on the Agent landing only, after the pair card, and never with it.
        if (tab == AciTab.Agent && landing && !showPair && !teachSeen && AciNav.teachOpen.not()) {
            AciNudgeCard(
                hero = AciHero.Setup,
                title = "Teach the agent a site",
                message = "It's in the browser bar's ⋮ menu, under Teach. Record a site once in Allternit Desktop and the agent saves the path as a skill it can replay. On your phone you can see what's been taught and import a recording.",
                closeLabel = "Dismiss Teach suggestion",
                actionTitle = "Open Teach",
                onClose = { AciOnceNudge.remember(context, "teach"); teachSeen = true },
                onAction = { AciOnceNudge.remember(context, "teach"); teachSeen = true; AciNav.openTeach() },
            )
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            when (tab) {
                AciTab.Agent -> when (val d = destination) {
                    AciDestination.Landing -> AciBrowserLanding(
                        controls = { host.composerControls(false) { destination = AciDestination.Landing } },
                        onOpen = { destination = AciDestination.Browser(it) },
                        onRunGoal = { destination = AciDestination.Agent(it) },
                    )
                    is AciDestination.Browser -> AciWebBrowser(
                        initialUrl = d.url,
                        controls = { host.composerControls(true) { destination = AciDestination.Landing } },
                        onExit = { destination = AciDestination.Landing },
                        onStartAgentRun = { destination = AciDestination.Agent(it) },
                    )
                    is AciDestination.Agent -> AciAgentRunView(
                        goal = d.goal,
                        controls = { host.composerControls(true) { destination = AciDestination.Landing } },
                        client = remember(host.backend) { AciAgentClient(host.backend) },
                        onExit = { destination = AciDestination.Landing },
                    )
                }
                AciTab.Computers -> AciComputersPane(host, devices, onRefresh = {
                    devices = (AciApi.devices() as? AciApi.Result.Ok)?.value ?: devices
                })
                AciTab.Apps -> AciMcpAppsView(host)
                AciTab.Extensions -> AciExtensionsView()
            }
        }
    }
    if (AciNav.teachOpen) {
        androidx.compose.ui.window.Dialog(onDismissRequest = { AciNav.teachOpen = false }, properties = androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
            AciTeachScreen(host, onRun = { prompt ->
                AciNav.teachOpen = false
                AciNav.open(AciTab.Agent)
                destination = AciDestination.Agent(prompt)
            }, onBack = { AciNav.teachOpen = false })
        }
    }
}

/**
 * "Computer the agent uses" (AciScreen.tsx `targetPicker`): runs go through the relay to the active
 * computer, so picking one pins the relay to it. A tap opens the list as a sheet.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AciTargetPicker(backend: ChatBackend, devices: List<AciDevice>, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    var open by remember { mutableStateOf(false) }
    var active by remember { mutableStateOf(backend.relay.selectedRuntimeId ?: backend.relay.activeRuntimeId) }
    val activeName = devices.firstOrNull { it.id == active }?.displayName ?: "Choose a computer"
    Row(
        modifier.fillMaxWidth().height(36.dp).clip(CircleShape).background(c.fillNeutral)
            .clickable(onClickLabel = "Computer the agent uses") { open = true }
            .padding(horizontal = 16.dp).testTag("aciTargetPicker"),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(activeName, color = c.textPrimary, fontSize = 16.sp, maxLines = 1, modifier = Modifier.weight(1f))
        Icon(painterResource(DesignR.drawable.ph_arrows_down_up), null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
    }
    if (open) {
        ModalBottomSheet(onDismissRequest = { open = false }, containerColor = c.bgPanel) {
            Text("Computer the agent uses", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp))
            devices.forEach { d ->
                Row(
                    Modifier.fillMaxWidth().clickable(enabled = d.isOnline) {
                        backend.relay.selectedRuntimeId = d.id
                        active = d.id
                        open = false
                    }.padding(horizontal = 20.dp, vertical = 14.dp).testTag("aciTarget_${d.id}"),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(if (d.isOnline) d.displayName else "${d.displayName} (offline)",
                        color = if (d.isOnline) c.textPrimary else c.textTertiary, fontSize = 16.sp, modifier = Modifier.weight(1f))
                    if (d.id == active) Icon(painterResource(DesignR.drawable.ph_check), null, tint = c.accentPrimary, modifier = Modifier.size(18.dp))
                }
            }
            Spacer(Modifier.height(24.dp))
        }
    }
}

/**
 * The browser landing (BrowserPane.tsx): dotted canvas, the mark, a random title and tagline, the
 * search/URL bar, the "Allternit Computer Agent" toggle and the shortcut tiles with Customize. With
 * the agent on, the bar takes a goal and starts a run.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun AciBrowserLanding(onOpen: (String) -> Unit, onRunGoal: (String) -> Unit, controls: @Composable () -> Unit = {}) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val shortcuts = remember { AciShortcutsStore(context) }
    val title = remember { TITLES.random() }
    val tagline = remember { TAGLINES.random() }
    var input by remember { mutableStateOf("") }
    var agentActive by remember { mutableStateOf(false) }
    var editMode by remember { mutableStateOf(false) }
    var adding by remember { mutableStateOf(false) }
    var newLabel by remember { mutableStateOf("") }
    var newUrl by remember { mutableStateOf("") }
    var newIcon by remember { mutableStateOf("") }
    fun submit() {
        val value = input.trim()
        if (value.isEmpty()) return
        if (agentActive) onRunGoal(value) else onOpen(AciWebInput.resolve(value))
        input = ""
    }
    Box(Modifier.fillMaxSize()) {
        val dot = c.textPrimary.copy(alpha = 0.18f)
        Canvas(Modifier.fillMaxSize()) {
            var y = 11.dp.toPx()
            val step = 22.dp.toPx()
            while (y < size.height) {
                var x = 11.dp.toPx()
                while (x < size.width) {
                    drawCircle(dot, radius = 1.dp.toPx(), center = Offset(x, y))
                    x += step
                }
                y += step
            }
        }
        Column(
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp, vertical = 48.dp).testTag("aciLanding"),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Column(Modifier.widthIn(max = 560.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                // iOS ACIBrowserLanding: the Matrix construct mark, 84pt.
                androidx.compose.foundation.Image(painterResource(DesignR.drawable.matrix_logo), contentDescription = "Allternit", modifier = Modifier.padding(bottom = 24.dp).size(84.dp))
                Text(title, color = c.textPrimary, fontSize = 32.sp, fontWeight = FontWeight.Medium, textAlign = TextAlign.Center, modifier = Modifier.padding(bottom = 8.dp))
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp), modifier = Modifier.padding(bottom = 32.dp)) {
                    Box(Modifier.width(32.dp).height(1.dp).background(c.borderStrong))
                    Text(tagline.uppercase(), color = c.textSecondary, fontSize = 14.sp, letterSpacing = 1.1.sp, maxLines = 1)
                    Box(Modifier.width(32.dp).height(1.dp).background(c.borderStrong))
                }
                controls()
                // The search/URL form: 56 dp neutral field, accent button.
                Row(
                    Modifier.fillMaxWidth().height(56.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral)
                        .border(1.dp, c.borderSubtle, RoundedCornerShape(12.dp)).padding(start = 24.dp, end = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(Modifier.weight(1f)) {
                        if (input.isEmpty()) Text(if (agentActive) "Describe a task for the Computer Agent…" else "Search the web or enter a URL…",
                            color = c.textTertiary, fontSize = 18.sp, maxLines = 1)
                        BasicTextField(
                            value = input, onValueChange = { input = it }, singleLine = true,
                            textStyle = TextStyle(color = c.textPrimary, fontSize = 18.sp),
                            cursorBrush = SolidColor(c.textPrimary),
                            keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = if (agentActive) KeyboardType.Text else KeyboardType.Uri, imeAction = ImeAction.Go),
                            keyboardActions = KeyboardActions(onGo = { submit() }),
                            modifier = Modifier.fillMaxWidth().testTag("aciSearchField"),
                        )
                    }
                    Box(
                        Modifier.size(36.dp).clip(RoundedCornerShape(8.dp)).background(c.accentPrimary)
                            .clickable(onClickLabel = if (agentActive) "Start agent run" else "Search") { submit() }.testTag("aciSearchGo"),
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(painterResource(if (agentActive) DesignR.drawable.ph_play_fill else DesignR.drawable.ph_magnifying_glass), null, tint = c.onAccent, modifier = Modifier.size(18.dp))
                    }
                }
                Spacer(Modifier.height(24.dp))
                Row(
                    Modifier.clip(CircleShape).background(if (agentActive) c.borderStrong else c.fillNeutral)
                        .border(1.dp, c.borderSubtle, CircleShape).clickable { agentActive = !agentActive }
                        .padding(horizontal = 14.dp, vertical = 6.dp).testTag("aciAgentToggle"),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Icon(painterResource(DesignR.drawable.ph_puzzle_piece), null, tint = if (agentActive) c.textPrimary else c.textSecondary, modifier = Modifier.size(12.dp))
                    Text("Allternit Computer Agent: ${if (agentActive) "Active" else "Off"}", color = if (agentActive) c.textPrimary else c.textSecondary, fontSize = 12.sp)
                }
                Spacer(Modifier.height(32.dp))
                Row(Modifier.fillMaxWidth().padding(bottom = 8.dp), horizontalArrangement = Arrangement.End) {
                    Text(if (editMode) "Done" else "Customize", color = if (editMode) c.textPrimary else c.textSecondary, fontSize = 12.sp,
                        textDecoration = TextDecoration.Underline, modifier = Modifier.clickable { editMode = !editMode }.padding(4.dp).testTag("aciCustomize"))
                }
                FlowRow(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp), verticalArrangement = Arrangement.spacedBy(12.dp), maxItemsInEachRow = 4) {
                    val tileMod = Modifier.weight(1f)
                    shortcuts.items.forEach { item ->
                        Box(tileMod) {
                            Column(
                                Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).border(1.dp, c.borderSubtle, RoundedCornerShape(12.dp))
                                    .clickable { if (!editMode) onOpen(item.url) }.padding(vertical = 16.dp, horizontal = 6.dp).testTag("aciShortcut_${item.id}"),
                                horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp),
                            ) {
                                Box(Modifier.size(48.dp).clip(CircleShape).background(c.borderSubtle), contentAlignment = Alignment.Center) {
                                    Text(item.icon ?: item.label.take(1), fontSize = 22.sp, color = c.textSecondary)
                                }
                                Text(item.label, color = c.textSecondary, fontSize = 12.sp, maxLines = 1)
                            }
                            if (editMode) {
                                Box(
                                    Modifier.align(Alignment.TopEnd).size(20.dp).clip(CircleShape).background(AllternitStatic.StatusError)
                                        .clickable(onClickLabel = "Remove ${item.label}") { shortcuts.remove(item.id) },
                                    contentAlignment = Alignment.Center,
                                ) { Icon(painterResource(DesignR.drawable.ph_x), null, tint = Color.White, modifier = Modifier.size(10.dp)) }
                            }
                        }
                    }
                    if (adding) {
                        Column(
                            tileMod.clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).border(1.dp, c.borderSubtle, RoundedCornerShape(12.dp)).padding(10.dp),
                            verticalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            SmallField("Label", newLabel) { newLabel = it }
                            SmallField("URL", newUrl, url = true) { newUrl = it }
                            SmallField("Emoji", newIcon) { newIcon = it }
                            Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                                Text("Save", color = c.onAccent, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center,
                                    modifier = Modifier.weight(1f).clip(RoundedCornerShape(6.dp)).background(c.accentPrimary).clickable {
                                        val label = newLabel.trim()
                                        var url = newUrl.trim()
                                        if (label.isNotEmpty() && url.isNotEmpty()) {
                                            if (!url.lowercase().startsWith("http://") && !url.lowercase().startsWith("https://")) url = "https://$url"
                                            shortcuts.add(label, url, newIcon)
                                            newLabel = ""; newUrl = ""; newIcon = ""; adding = false
                                        }
                                    }.padding(vertical = 5.dp).testTag("aciShortcutSave"))
                                Text("Cancel", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.clickable { adding = false }.padding(6.dp))
                            }
                        }
                    } else {
                        Column(
                            tileMod.dashedBorder(c.borderSubtle, 12.dp).clickable { adding = true }.padding(vertical = 16.dp).testTag("aciShortcutAdd"),
                            horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            Box(Modifier.height(48.dp), contentAlignment = Alignment.Center) {
                                Icon(painterResource(DesignR.drawable.ph_plus), null, tint = c.textTertiary, modifier = Modifier.size(18.dp))
                            }
                            Text("Add", color = c.textTertiary, fontSize = 12.sp)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun SmallField(placeholder: String, value: String, url: Boolean = false, onChange: (String) -> Unit) {
    val c = AllternitTheme.colors
    Box(Modifier.fillMaxWidth().height(26.dp).clip(RoundedCornerShape(6.dp)).background(c.bgPanel).border(1.dp, c.borderSubtle, RoundedCornerShape(6.dp)).padding(horizontal = 8.dp),
        contentAlignment = Alignment.CenterStart) {
        if (value.isEmpty()) Text(placeholder, color = c.textTertiary, fontSize = 12.sp)
        BasicTextField(value, onChange, singleLine = true, textStyle = TextStyle(color = c.textPrimary, fontSize = 12.sp), cursorBrush = SolidColor(c.textPrimary),
            keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = if (url) KeyboardType.Uri else KeyboardType.Text),
            modifier = Modifier.fillMaxWidth().testTag("aciShortcut$placeholder"))
    }
}

private val TITLES = listOf("Navigator's Deck", "Browse & Build", "The Web Layer", "Allternit Explorer", "Portal Online", "Web Surface")
private val TAGLINES = listOf("AI-Powered Browsing", "Navigate with Intelligence", "Your Web, Amplified", "Explore Fearlessly", "Browse. Automate. Build.")

// MARK: - Chrome shared by the ACI views (iOS ACIPhoneChrome)

@Composable
internal fun AciViewHeader(title: String, subtitle: String? = null, bottom: Dp = 24.dp) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().padding(top = 8.dp, bottom = bottom), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(title, color = c.textPrimary, fontSize = 30.sp, fontFamily = AllternitSerif, fontWeight = FontWeight.Medium, letterSpacing = (-0.3).sp)
        if (subtitle != null) Text(subtitle, color = c.textSecondary, fontSize = 14.sp)
    }
}

@Composable
internal fun AciWideButton(icon: Int, title: String, testTag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        Modifier.padding(bottom = 12.dp).fillMaxWidth().heightIn(min = 44.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral)
            .border(1.dp, c.borderSubtle, RoundedCornerShape(12.dp)).clickable { onClick() }.testTag(testTag),
        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally), verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(painterResource(icon), null, tint = c.textPrimary, modifier = Modifier.size(18.dp))
        Text(title, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
    }
}

@Composable
internal fun AciPillButton(title: String, testTag: String, label: String? = null, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        Modifier.heightIn(min = 44.dp).widthIn(min = 64.dp).clip(CircleShape).background(c.fillNeutral).border(1.dp, c.borderSubtle, CircleShape)
            .clickable(onClickLabel = label ?: title) { onClick() }.padding(horizontal = 18.dp).testTag(testTag),
        contentAlignment = Alignment.Center,
    ) { Text(title, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold) }
}

@Composable
internal fun AciPrimaryButton(title: String, testTag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(Modifier.fillMaxWidth().heightIn(min = 52.dp).clip(CircleShape).background(c.accentPrimary).clickable { onClick() }.testTag(testTag), contentAlignment = Alignment.Center) {
        Text(title, color = c.onAccent, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
    }
}

/** iOS ACIPromoHero.Kind: pair shows the Matrix mark, setup shows Gizzi. */
internal enum class AciHero { Pair, Setup }

/** The promo hero: neutral fill, a 22 dp grid, two tilted cards and the mark. */
@Composable
internal fun AciPromoHero(kind: AciHero = AciHero.Pair, height: Dp = 132.dp, imageHeight: Dp = 64.dp, topRadius: Dp = 16.dp) {
    val c = AllternitTheme.colors
    val line = c.borderStrong
    val card = c.bgPrimary
    Box(Modifier.fillMaxWidth().height(height).clip(RoundedCornerShape(topStart = topRadius, topEnd = topRadius)).background(c.fillNeutral), contentAlignment = Alignment.Center) {
        Canvas(Modifier.fillMaxSize()) {
            val step = 22.dp.toPx()
            var x = 0f
            while (x <= size.width) { drawRect(line, Offset(x, 0f), androidx.compose.ui.geometry.Size(1f, size.height)); x += step }
            var y = 0f
            while (y <= size.height) { drawRect(line, Offset(0f, y), androidx.compose.ui.geometry.Size(size.width, 1f)); y += step }
            val cw = 84.dp.toPx()
            val ch = 56.dp.toPx()
            listOf(-7f to Offset(18.dp.toPx(), size.height - 10.dp.toPx() - ch / 2), 6f to Offset(size.width - 18.dp.toPx() - cw, size.height - 10.dp.toPx() - ch / 2)).forEach { (deg, o) ->
                rotateCard(deg, o, cw, ch, card, line)
            }
        }
        androidx.compose.foundation.Image(
            painterResource(if (kind == AciHero.Pair) DesignR.drawable.matrix_logo else DesignR.drawable.gizzi_mascot),
            contentDescription = null,
            modifier = Modifier.height(imageHeight),
        )
    }
}

private fun androidx.compose.ui.graphics.drawscope.DrawScope.rotateCard(deg: Float, o: Offset, w: Float, h: Float, fill: Color, stroke: Color) {
    val pivot = Offset(o.x + w / 2, o.y + h / 2)
    rotate(deg, pivot) {
        drawRoundRect(fill, o, androidx.compose.ui.geometry.Size(w, h), CornerRadius(12.dp.toPx()))
        drawRoundRect(stroke, o, androidx.compose.ui.geometry.Size(w, h), CornerRadius(12.dp.toPx()), style = Stroke(1.dp.toPx()))
    }
}

/** A suggestion card (iOS ACINudgeCard): hero with × over it, title, line, one primary action. */
@Composable
internal fun AciNudgeCard(hero: AciHero, title: String, message: String, closeLabel: String, actionTitle: String, onClose: () -> Unit, onAction: () -> Unit) {
    val c = AllternitTheme.colors
    Column(
        Modifier.padding(12.dp).fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(c.bgPanel).padding(bottom = 20.dp).testTag("aciNudge"),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Box {
            AciPromoHero(hero)
            Box(
                Modifier.padding(start = 16.dp, top = 16.dp).size(40.dp).clip(CircleShape).background(c.bgPrimary).clickable(onClickLabel = closeLabel) { onClose() }.testTag("aciNudgeClose"),
                contentAlignment = Alignment.Center,
            ) { Icon(painterResource(DesignR.drawable.ph_x), closeLabel, tint = c.textSecondary, modifier = Modifier.size(17.dp)) }
        }
        Column(Modifier.padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Text(title, color = c.textPrimary, fontSize = 26.sp, fontWeight = FontWeight.SemiBold)
            Text(message, color = c.textSecondary, fontSize = 17.sp, lineHeight = 23.sp)
            AciPrimaryButton(actionTitle, "aciNudgeAction", onAction)
        }
    }
}

/** A dashed-outline card (iOS ACIDashedCard). */
@Composable
internal fun AciDashedCard(radius: Dp = 16.dp, content: @Composable ColumnScope.() -> Unit) {
    Column(Modifier.fillMaxWidth().dashedBorder(AllternitTheme.colors.borderStrong, radius), horizontalAlignment = Alignment.CenterHorizontally, content = content)
}

internal fun Modifier.dashedBorder(color: Color, radius: Dp): Modifier = this.then(
    Modifier.drawWithContentDashed(color, radius),
)

private fun Modifier.drawWithContentDashed(color: Color, radius: Dp): Modifier = this.drawBehind {
    drawRoundRect(color, cornerRadius = CornerRadius(radius.toPx()), style = Stroke(width = 1.dp.toPx(), pathEffect = PathEffect.dashPathEffect(floatArrayOf(4.dp.toPx(), 3.dp.toPx()))))
}

@Suppress("unused")
private val unusedBorder = BorderStroke(0.dp, Color.Transparent)
