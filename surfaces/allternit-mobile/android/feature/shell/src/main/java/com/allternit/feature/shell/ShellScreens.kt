package com.allternit.feature.shell

import com.allternit.core.design.AProtocolMark
import com.allternit.core.design.AllternitSerif
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.text.TextAutoSize
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.DpOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.gen.AgentSession
import com.allternit.core.auth.Account
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.design.AllternitRadii
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.Haptics
import com.allternit.core.design.rememberDockOffset
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import com.allternit.core.design.DockingBlockTitle
import androidx.compose.ui.zIndex
import com.allternit.core.design.ModeAccents
import com.allternit.core.design.R as DesignR
import com.allternit.feature.onboarding.R as OnboardingR
import kotlinx.coroutines.launch

/**
 * The shell's sans face: Roboto, bundled by :feature:onboarding so the app reads the same on
 * every phone (the Moto's system face is a wide geometric one). iOS uses SF Pro at the same sizes.
 */
internal val ShellSans = FontFamily(
    Font(OnboardingR.font.roboto_regular, FontWeight.Normal),
    Font(OnboardingR.font.roboto_semibold, FontWeight.Medium),
    Font(OnboardingR.font.roboto_semibold, FontWeight.SemiBold),
    Font(OnboardingR.font.roboto_bold, FontWeight.Bold),
)

// MARK: - Header

/**
 * Phone top bar (iOS PhoneHeader): ☰ leads, a centered title, trailing actions. 52dp tall, no
 * separator. [wideSides] is iOS `wideSides` (96dp sides), used where ••• and compose sit together.
 */
@Composable
internal fun PhoneHeader(
    title: String,
    subtitle: String? = null,
    wideSides: Boolean = false,
    leading: @Composable () -> Unit,
    trailing: @Composable () -> Unit = {},
    /** Drawn in place of the title text (Code's gizzi code lockup); [title] stays the heading. */
    titleContent: (@Composable () -> Unit)? = null,
) {
    val c = AllternitTheme.colors
    val side = if (wideSides) 96.dp else 52.dp
    Row(
        modifier = Modifier.fillMaxWidth().height(52.dp).padding(horizontal = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(side), contentAlignment = Alignment.CenterStart) { leading() }
        Column(Modifier.weight(1f), horizontalAlignment = Alignment.CenterHorizontally) {
            // A blank title draws nothing (iOS PhoneHeader `if let title, !title.isEmpty`).
            if (titleContent != null) Box(Modifier.semantics { heading(); contentDescription = title }) { titleContent() } else if (title.isNotBlank()) Text(
                text = title,
                color = c.textPrimary,
                fontSize = 16.sp,
                fontWeight = FontWeight.SemiBold,
                letterSpacing = (-0.2).sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.semantics { heading() },
            )
            if (!subtitle.isNullOrEmpty()) {
                Text(subtitle, color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        Row(Modifier.width(side), horizontalArrangement = Arrangement.spacedBy(2.dp, Alignment.End)) { trailing() }
    }
}

/** A 44dp header button (iOS PhoneIconButton). */
@Composable
internal fun PhoneIconButton(icon: Int, label: String, testTag: String? = null, iconSize: Int = 22, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Box(
        modifier = Modifier
            .size(44.dp)
            .clip(CircleShape)
            .then(if (testTag != null) Modifier.testTag(testTag) else Modifier)
            .clickable(role = Role.Button, onClickLabel = label) {
                Haptics.perform(view, Haptics.Kind.Tap)
                onClick()
            }
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        Icon(painter = painterResource(icon), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(iconSize.dp))
    }
}

// MARK: - Home

/**
 * The signed-in home (iOS ChatView in launch style): the header (☰ with the mode badge,
 * "Allternit", ••• and New chat), the pixel A:// mark over the serif headline, and the composer.
 * Recents live in the drawer, as on iOS. Bots and ACI have their own roots (waves E, F3); Code adds its home.
 */
@Composable
internal fun ShellHome(
    mode: ShellMode,
    store: ShellSessionsStore,
    models: ModelStore,
    menuButton: @Composable () -> Unit,
    onNewChat: (ComposerDraft) -> Unit,
    onOpenSection: (RailRow) -> Unit,
    /** Wave F1: Code mode's home (folder, tools, recent threads) replaces the placeholder and the composer starts a thread. */
    codeHome: (@Composable (ScrollState) -> Unit)? = null,
    /** Code mode's header button: the Code sessions view (iOS CodeSessionsButton → CodeOverview). */
    onCodeSessions: (() -> Unit)? = null,
    /** Code mode's composer deck row (iOS CodeDeckRow): environment, folder, branch, worktree, permission, usage, view menu. */
    codeActions: CodeComposerActions? = null,
    /** The home composer's voice button (iOS ComposerView onVoiceMode on the home chat): a new chat in voice mode. */
    onVoiceMode: (() -> Unit)? = null,
) {
    var menuOpen by remember { mutableStateOf(false) }
    var phoneArmed by remember { mutableStateOf(false) }
    // Home launch: the A://TERNIT block title docks into the top bar when the launch content scrolls (T8).
    val homeScroll = rememberScrollState()
    val homeDock = rememberDockOffset(homeScroll)
    // Code launch: GIZZI CODE docks into the bar the same way (T13); CodeHome scrolls on this state.
    val codeScroll = rememberScrollState()
    val codeDock = rememberDockOffset(codeScroll)
    val codeDocks = mode == ShellMode.Code && codeHome != null && onCodeSessions != null
    // T13: the bar keeps the selected computer's name (iOS ShellHeaderTitle); "Code" when none is selected.
    var computerName by remember { mutableStateOf<String?>(null) }
    val codeHost = codeActions?.host
    LaunchedEffect(codeDocks, codeHost) {
        if (codeDocks && codeHost != null) {
            val relay = codeHost.backend.relay
            computerName = relay.listDevices()?.firstOrNull { it.id == relay.activeRuntimeId }?.name?.takeIf { it.isNotBlank() }
        }
    }
    Column(Modifier.fillMaxSize().statusBarsPadding()) {
        Box(Modifier.fillMaxWidth().zIndex(1f)) {
        if (mode == ShellMode.Home) {
            HomeDockTitle("TERNIT", homeDock, markPrefix = true, markLabel = "Allternit")
        } else if (codeDocks) {
            HomeDockTitle("GIZZI CODE", codeDock)
        }
        PhoneHeader(
            // Home has no header title (Eoj 2026-11-20): the A:// mark on the page is the brand.
            title = if (mode == ShellMode.Home) "" else mode.label,
            wideSides = mode == ShellMode.Home,
            leading = menuButton,
            trailing = {
                if (mode == ShellMode.Home) {
                    Box {
                        com.allternit.core.design.GlassPill(
                            onClick = { menuOpen = true },
                            modifier = Modifier.testTag("chatOptions").semantics { contentDescription = "Chat options" },
                            contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 12.dp),
                        ) {
                            Icon(painterResource(com.allternit.core.design.R.drawable.ph_dots_three), contentDescription = null, tint = AllternitTheme.colors.textPrimary, modifier = Modifier.size(20.dp))
                        }
                        ChatOptionsMenu(
                            expanded = menuOpen,
                            phoneArmed = phoneArmed,
                            onDismiss = { menuOpen = false },
                            onRunOnPhone = {
                                phoneArmed = true
                                menuOpen = false
                            },
                        )
                    }
                }
                if (mode == ShellMode.Code && onCodeSessions != null) {
                    // iOS CodeModeView header: the Code sessions view (CodeSessionsButton), no new-chat pencil.
                    com.allternit.core.design.GlassPill(
                        onClick = onCodeSessions,
                        modifier = Modifier.testTag("codeSessionsButton").semantics { contentDescription = "Code sessions" },
                        contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 12.dp),
                    ) {
                        Icon(painterResource(com.allternit.core.design.R.drawable.ph_list_checks), contentDescription = null, tint = AllternitTheme.colors.textPrimary, modifier = Modifier.size(20.dp))
                    }
                }
            },
            // iOS Code launchpad title: the gizzi code lockup.
            // T13: the bar keeps its name until the first letter of GIZZI CODE flies, fades over that flight,
            // and returns on scroll up; docked, the block title itself holds the bar centre.
            titleContent = if (codeDocks) ({
                val nLetters = remember { com.allternit.core.design.titleCells("GIZZI CODE").n }
                Text(
                    text = computerName ?: mode.label,
                    color = AllternitTheme.colors.textPrimary,
                    overflow = TextOverflow.Ellipsis,
                    fontSize = 16.sp,
                    fontWeight = FontWeight.SemiBold,
                    letterSpacing = (-0.2).sp,
                    maxLines = 1,
                    modifier = Modifier.graphicsLayer {
                        alpha = 1f - com.allternit.core.design.dockLetterProgress(
                            (codeDock() / com.allternit.core.design.DockSpec.DOCK).coerceIn(0f, 1f), nLetters, 0,
                        )
                    },
                )
            }) else null,
        )
        }
        Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
            if (mode == ShellMode.Home) {
                // Scrolls only when the content overflows (small phone, big font); a tall phone keeps the big title.
                Column(Modifier.fillMaxSize().verticalScroll(homeScroll)) {
                    // Room for the big title, which sits in window coordinates under the bar. Carries the old mark's tag.
                    Spacer(Modifier.height(HOME_DOCK_TITLE_ROOM).testTag("homeMark"))
                    LaunchGreeting(showMark = false)
                }
            }
            else if (codeHome != null && mode == ShellMode.Code) {
                Column(Modifier.fillMaxSize()) {
                    // iOS CodeModeView: What's new sits right under the sessions button.
                    Row(Modifier.fillMaxWidth().padding(horizontal = 10.dp).padding(top = 2.dp), horizontalArrangement = Arrangement.End) {
                        com.allternit.feature.code.CodeWhatsNewButton()
                    }
                    Box(Modifier.weight(1f).fillMaxWidth()) { codeHome(codeScroll) }
                }
            } else ModeArrives(mode)
        }
        if (mode == ShellMode.Code && codeHome != null && codeActions != null) {
            CodeLaunchComposer(models = models, noComputer = store.noComputer, actions = codeActions, onSend = onNewChat, onOpenSection = onOpenSection)
        } else if (mode == ShellMode.Home || (mode == ShellMode.Code && codeHome != null)) {
            Composer(
                models = models,
                noComputer = store.noComputer,
                phoneArmed = phoneArmed,
                onClearPhone = { phoneArmed = false },
                onSend = onNewChat,
                onOpenSection = onOpenSection,
                onVoiceMode = if (mode == ShellMode.Home) onVoiceMode else null,
            )
        } else {
            Spacer(Modifier.navigationBarsPadding())
        }
    }
}

private val HOME_DOCK_TITLE_ROOM = 96.dp

/** The A://TERNIT block title, drawn in window coordinates like DockTitleLayer (which has no markPrefix). */
@Composable
private fun HomeDockTitle(text: String, scrollOffset: () -> Float, markPrefix: Boolean = false, markLabel: String? = null) {
    var originX by remember { mutableStateOf(0) }
    var originY by remember { mutableStateOf(0) }
    val cfg = androidx.compose.ui.platform.LocalConfiguration.current
    val density = androidx.compose.ui.platform.LocalDensity.current
    val w = (cfg.screenWidthDp * density.density).toInt()
    val h = (cfg.screenHeightDp * density.density).toInt()
    Box(
        Modifier
            .onPlacedAt { x, y -> originX = x; originY = y }
            .layout { m, _ ->
                val placeable = m.measure(androidx.compose.ui.unit.Constraints.fixed(w, h))
                layout(0, 0) { placeable.place(androidx.compose.ui.unit.IntOffset(-originX, -originY)) }
            },
    ) {
        DockingBlockTitle(text, scrollOffset, markPrefix = markPrefix, markLabel = markLabel)
    }
}

private fun Modifier.onPlacedAt(f: (Int, Int) -> Unit): Modifier =
    this.onGloballyPositioned { c ->
        val p = c.positionInWindow()
        f(p.x.toInt(), p.y.toInt())
    }

/**
 * iOS ChatLaunchGreeting on the main chat: the A:// mark (not Gizzi), 22dp, over a 32sp serif
 * headline. Pinned to the reference build's line.
 */
@Composable
internal fun LaunchGreeting(showMark: Boolean = true) {
    val c = AllternitTheme.colors
    // Panel A: fades up 8dp on open. Reduced motion (animations off) shows it at rest.
    val context = androidx.compose.ui.platform.LocalContext.current
    val reduced = remember {
        android.provider.Settings.Global.getFloat(context.contentResolver, android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
    }
    val rise = remember { androidx.compose.animation.core.Animatable(if (reduced) 1f else 0f) }
    androidx.compose.runtime.LaunchedEffect(Unit) { rise.animateTo(1f, com.allternit.core.design.Motion.standardSpring()) }
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 24.dp)
            .padding(bottom = 24.dp)
            .graphicsLayer {
                alpha = rise.value.coerceIn(0f, 1f)
                translationY = (1f - rise.value) * 8.dp.toPx()
            },
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        if (showMark) AProtocolMark(height = 22.dp, modifier = Modifier.testTag("homeMark"))
        Text(
            text = "What are we working on?",
            color = c.textPrimary,
            lineHeight = 39.sp,
            fontFamily = AllternitSerif,
            letterSpacing = (-1.04).sp, // panel A: tracking -0.4 tighter than the old -0.64
            textAlign = TextAlign.Center,
            // One line like iOS: 32sp, stepping down on narrower phones instead of wrapping.
            maxLines = 1,
            softWrap = false,
            autoSize = TextAutoSize.StepBased(minFontSize = 23.sp, maxFontSize = 33.sp, stepSize = 1.sp),
            modifier = Modifier.fillMaxWidth().testTag("homeGreeting"),
        )
    }
}

/** The chat ⋯ menu on a new chat (iOS ChatMenuPopover with chatMenuItems): Run on my phone. */
@Composable
private fun ChatOptionsMenu(expanded: Boolean, phoneArmed: Boolean, onDismiss: () -> Unit, onRunOnPhone: () -> Unit) {
    val c = AllternitTheme.colors
    DropdownMenu(
        expanded = expanded,
        onDismissRequest = onDismiss,
        offset = DpOffset(0.dp, 4.dp),
        shape = RoundedCornerShape(16.dp),
        containerColor = c.bgPrimary,
        modifier = Modifier.width(300.dp),
    ) {
        MenuRow(
            icon = com.allternit.core.design.R.drawable.ph_device_mobile,
            title = "Run on my phone",
            checked = phoneArmed,
            testTag = "chatMenurunOnPhone",
            onClick = onRunOnPhone,
        )
    }
}

/** One 44dp menu row: 24dp glyph, 15sp label, an optional check (iOS ChatMenuPopover row). */
@Composable
internal fun MenuRow(icon: Int?, title: String, checked: Boolean = false, testTag: String? = null, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 8.dp)
            .clip(RoundedCornerShape(10.dp))
            .then(if (testTag != null) Modifier.testTag(testTag) else Modifier)
            .clickable(role = Role.Button, onClick = onClick)
            .heightIn(min = 44.dp)
            .padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (icon != null) {
            Icon(painterResource(icon), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(20.dp))
        }
        Text(title, color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
        if (checked) Icon(painterResource(DesignR.drawable.ph_check), contentDescription = "On", tint = c.textPrimary, modifier = Modifier.size(16.dp))
    }
}

/** A mode whose screen is not built yet: say so plainly. */
@Composable
private fun ModeArrives(mode: ShellMode) {
    val c = AllternitTheme.colors
    Column(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp).testTag("modeArrives${mode.raw}"),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(
            modifier = Modifier
                .size(64.dp)
                .clip(RoundedCornerShape(AllternitRadii.XL))
                .background(mode.accent.copy(alpha = ModeAccents.SOFT_ALPHA)),
            contentAlignment = Alignment.Center,
        ) {
            Icon(painter = painterResource(mode.icon), contentDescription = null, tint = mode.accent, modifier = Modifier.size(28.dp))
        }
        Text(
            text = "${mode.label} arrives in the next update",
            color = c.textPrimary,
            fontSize = 20.sp,
            fontFamily = AllternitSerif,
            textAlign = TextAlign.Center,
        )
        Text(
            text = when (mode) {
                ShellMode.Bots -> "Your bots, their threads and their phones come to this app next."
                ShellMode.Code -> "Code threads, terminals and reviews come to this app next."
                ShellMode.Aci -> "Open the mode again from the menu."
                ShellMode.Home -> ""
            },
            color = c.textSecondary,
            fontSize = 14.sp,
            lineHeight = 20.sp,
            textAlign = TextAlign.Center,
            modifier = Modifier.widthIn(max = 340.dp),
        )
    }
}

// MARK: - Sub-pages

@Composable
private fun BackButton(onBack: () -> Unit) {
    PhoneIconButton(DesignR.drawable.ph_caret_left, "Back", testTag = "shellBack", onClick = onBack)
}

/** A drawer section whose screen is not built yet (Projects, Artifacts, …). */
@Composable
internal fun SectionStub(row: RailRow, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("sectionStub${row.id}")) {
        PhoneHeader(title = row.label, leading = { BackButton(onBack) })
        FriendlyState(
            style = FriendlyStyle.Empty,
            title = "${row.label} arrives in the next update",
            message = "Everything you already have there stays where it is, and shows up here when this screen comes to the app.",
            modifier = Modifier.padding(horizontal = 20.dp),
        ) {
            Icon(painter = painterResource(row.icon), contentDescription = null, tint = c.textSecondary)
        }
    }
}

/* Settings moved to :feature:settings (wave G1); the shell hosts SettingsRoot. */

@Composable
internal fun PillButton(title: String, testTag: String? = null, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        modifier = Modifier
            .clip(RoundedCornerShape(22.dp))
            .border(BorderStroke(1.dp, c.borderStrong), RoundedCornerShape(22.dp))
            .then(if (testTag != null) Modifier.testTag(testTag) else Modifier)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .heightIn(min = 44.dp)
            .padding(horizontal = 18.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(title, color = if (enabled) c.textPrimary else c.textTertiary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
    }
}
