package com.allternit.feature.shell

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.AnchoredDraggableDefaults
import androidx.compose.foundation.gestures.AnchoredDraggableState
import androidx.compose.foundation.gestures.DraggableAnchors
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.anchoredDraggable
import androidx.compose.foundation.gestures.animateTo
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.LocalTextStyle
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.allternit.core.api.gen.AgentSession
import com.allternit.core.auth.Account
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.Motion
import kotlinx.coroutines.launch

internal enum class DrawerValue { Closed, Open }

/** What fills the page under the drawer. Chat, sections and settings are stubs until later steps. */
private sealed interface ShellPage {
    data object Home : ShellPage
    data class Chat(val session: AgentSession?, val draft: ComposerDraft) : ShellPage
    data class Section(val row: RailRow) : ShellPage
    data object Settings : ShellPage
    data object AllChats : ShellPage
    data object Search : ShellPage
}

/**
 * The signed-in shell (iOS MainWorkspaceView, PWA PhoneShell): one page fills the screen, no
 * bottom bar. The drawer is min(80%, 360dp) wide (iOS sidebarWidth), springs in from the left (☰ tap or a pan from
 * the left edge), and the page pushes right with it, scaled to 0.94 with rounded corners. Holding
 * ☰ fans the modes out over everything; the fan and the drawer are never open together.
 *
 * [account] is null for the debug "Continue without signing in" path: no API calls then.
 */
@Composable
internal fun WorkspaceShell(account: Account?) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val view = LocalView.current
    val density = LocalDensity.current
    val scope = rememberCoroutineScope()

    var mode by remember { mutableStateOf(ShellModePrefs.load(context)) }
    var page by remember { mutableStateOf<ShellPage>(ShellPage.Home) }
    val store = remember { ShellSessionsStore() }
    val models = remember { ModelStore(context) }
    val hold = remember { MenuHoldState() }
    val drawer = remember { AnchoredDraggableState(DrawerValue.Closed) }

    fun reload() {
        if (account != null) scope.launch { store.load() }
    }
    LaunchedEffect(account?.userId) { if (account != null) store.load() }

    fun setDrawer(open: Boolean) {
        scope.launch { drawer.animateTo(if (open) DrawerValue.Open else DrawerValue.Closed, Motion.standardSpring()) }
    }

    // One light haptic per open (any way of opening), the fan closes, and Recents refresh
    // each time the drawer is revealed (iOS onChange(isSidebarOpen)).
    LaunchedEffect(drawer.settledValue) {
        if (drawer.settledValue == DrawerValue.Open) {
            Haptics.perform(view, Haptics.Kind.Tap)
            hold.closeFan()
            if (account != null) store.load()
        }
    }

    fun pickMode(m: ShellMode) {
        hold.closeFan()
        Haptics.perform(view, Haptics.Kind.Tap)
        if (m != mode) {
            mode = m
            ShellModePrefs.save(context, m)
        }
        page = ShellPage.Home
    }

    fun openPage(p: ShellPage) {
        page = p
        setDrawer(false)
    }

    val drawerOpen = drawer.settledValue == DrawerValue.Open || drawer.targetValue == DrawerValue.Open
    BackHandler(enabled = hold.fanOpen || drawerOpen || page != ShellPage.Home) {
        when {
            hold.fanOpen -> hold.closeFan()
            drawerOpen -> setDrawer(false)
            else -> page = ShellPage.Home
        }
    }

    // Every Text in the shell uses the bundled sans (iOS SF Pro sizes); the headline sets Serif itself.
    CompositionLocalProvider(LocalTextStyle provides LocalTextStyle.current.copy(fontFamily = ShellSans)) {
    BoxWithConstraints(Modifier.fillMaxSize().background(c.bgPrimary)) {
        val widthDp = drawerWidth(maxWidth)
        val widthPx = with(density) { widthDp.toPx() }
        val gapPx = with(density) { 24.dp.toPx() }
        LaunchedEffect(widthPx) {
            drawer.updateAnchors(
                DraggableAnchors {
                    DrawerValue.Closed at 0f
                    DrawerValue.Open at widthPx
                },
                drawer.targetValue,
            )
        }
        val fling = AnchoredDraggableDefaults.flingBehavior(drawer, { distance: Float -> distance * 0.25f }, Motion.standardSpring())
        val offset = drawer.offset.let { if (it.isNaN()) 0f else it }
        val p = if (widthPx > 0f) (offset / widthPx).coerceIn(0f, 1f) else 0f

        Box(
            Modifier
                .fillMaxSize()
                // Open: drag anywhere (page strip or drawer) to close it.
                .anchoredDraggable(
                    state = drawer,
                    orientation = Orientation.Horizontal,
                    enabled = drawer.settledValue == DrawerValue.Open,
                    flingBehavior = fling,
                ),
        ) {
            // The page: pushed right, scaled from its left edge, corners rounding as it moves.
            Box(
                Modifier
                    .fillMaxSize()
                    .graphicsLayer {
                        translationX = p * widthPx
                        val s = 1f - 0.06f * p
                        scaleX = s
                        scaleY = s
                        transformOrigin = TransformOrigin(0f, 0.5f)
                        shape = if (p > 0f) RoundedCornerShape((36 * p).dp) else RectangleShape
                        clip = p > 0f
                    }
                    .background(c.bgPrimary),
            ) {
                Box(Modifier.fillMaxSize().then(if (drawerOpen) Modifier.clearAndSetSemantics {} else Modifier)) {
                    when (val current = page) {
                        ShellPage.Home -> ShellHome(
                            mode = mode,
                            store = store,
                            models = models,
                            menuButton = {
                                PhoneMenuButton(
                                    mode = mode,
                                    hold = hold,
                                    holdEnabled = !drawerOpen,
                                    onOpen = { setDrawer(true) },
                                    onPick = ::pickMode,
                                )
                            },
                            onNewChat = { draft -> page = ShellPage.Chat(session = null, draft = draft) },
                            onNew = { page = ShellPage.Home },
                            onOpenSection = { row -> page = ShellPage.Section(row) },
                        )
                        is ShellPage.Chat -> ChatStub(
                            session = current.session,
                            draft = current.draft,
                            onBack = { page = ShellPage.Home },
                            onNewChat = { page = ShellPage.Home },
                        )
                        is ShellPage.Section -> SectionStub(current.row, onBack = { page = ShellPage.Home })
                        ShellPage.Settings -> SettingsScreen(account, onBack = { page = ShellPage.Home })
                        ShellPage.AllChats -> AllChatsScreen(mode, store, onBack = { page = ShellPage.Home }) {
                            page = ShellPage.Chat(session = it, draft = ComposerDraft())
                        }
                        ShellPage.Search -> ShellSearchScreen(store, onBack = { page = ShellPage.Home }) {
                            page = ShellPage.Chat(session = it, draft = ComposerDraft())
                        }
                    }
                }
                if (p > 0f) {
                    // The visible page strip: tap closes the drawer.
                    Box(
                        Modifier
                            .fillMaxSize()
                            .background(Color.Black.copy(alpha = 0.16f * p))
                            .testTag("drawerScrim")
                            .clickable(
                                interactionSource = remember { MutableInteractionSource() },
                                indication = null,
                                onClickLabel = "Close menu",
                            ) { setDrawer(false) }
                            .semantics { contentDescription = "Close menu" },
                    )
                }
            }

            // Left-edge pan opens the drawer. It starts under the header so ☰ keeps its taps,
            // and sits outside the moving page so drag deltas are not measured in a moving frame.
            if (drawer.settledValue == DrawerValue.Closed && !hold.fanOpen) {
                val top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 52.dp
                Box(
                    Modifier
                        .padding(top = top)
                        .width(24.dp)
                        .fillMaxHeight()
                        .testTag("drawerEdge")
                        .anchoredDraggable(
                            state = drawer,
                            orientation = Orientation.Horizontal,
                            enabled = true,
                            flingBehavior = fling,
                        ),
                )
            }

            // The drawer panel: rounded trailing corners, slides in from the edge.
            Box(
                Modifier
                    .width(widthDp)
                    .fillMaxHeight()
                    .graphicsLayer {
                        translationX = (p - 1f) * (widthPx + gapPx)
                        shadowElevation = if (p > 0f) 16.dp.toPx() else 0f
                        shape = RoundedCornerShape(topEnd = 26.dp, bottomEnd = 26.dp)
                        clip = true
                    }
                    .background(c.bgPrimary)
                    .testTag("shellDrawer")
                    .then(if (p == 0f) Modifier.clearAndSetSemantics {} else Modifier.semantics { contentDescription = "${mode.label} menu" }),
            ) {
                ShellDrawer(
                    mode = mode,
                    account = account,
                    store = store,
                    onModeDial = {
                        // D4: close the drawer, then fan the modes out of ☰.
                        scope.launch {
                            drawer.animateTo(DrawerValue.Closed, Motion.standardSpring())
                            hold.openFan()
                        }
                    },
                    onRow = { row -> openPage(ShellPage.Section(row)) },
                    onOpenSession = { openPage(ShellPage.Chat(session = it, draft = ComposerDraft())) },
                    // New chat: the home with an empty composer (iOS newItem on Home).
                    onNew = { openPage(ShellPage.Home) },
                    onSettings = { openPage(ShellPage.Settings) },
                    onRetry = ::reload,
                    onSearch = { openPage(ShellPage.Search) },
                    onViewAll = { openPage(ShellPage.AllChats) },
                )
            }
        }

        ModeFanOverlay(hold = hold, current = mode, onPick = ::pickMode)
    }
    }
}
