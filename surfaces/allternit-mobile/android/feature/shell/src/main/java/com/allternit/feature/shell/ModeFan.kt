package com.allternit.feature.shell

import android.provider.Settings
import android.view.HapticFeedbackConstants
import android.view.View
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.zIndex
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR
import com.allternit.feature.bots.BotModeStore
import com.allternit.feature.bots.BotRecord
import com.allternit.feature.bots.BotStatusAvatar
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.floor
import kotlin.math.roundToInt
import kotlin.math.sin

// ☰ menu hold (contract C8, decisions D4 and D11): a straight port of iOS MenuHoldMachine.swift
// (itself the web's menu-hold.ts): the same events, effects, constants and rules.
//
//  - Tap ☰ opens the drawer. Opening the drawer closes the fan / shelf.
//  - Hold ☰ for HOLD_MILLIS opens the fan (Home · Bots · Code · ACI). A hold does nothing
//    while the drawer is open.
//  - Slide over the cards: the card under the finger is the target. Release on a card switches
//    mode. Release on nothing keeps the fan open for a tap.
//  - Stay on the Bots card for BOTS_HOLD_MILLIS more: the bots shelf replaces the fan. Release on
//    a bot opens its main chat; the last item creates a bot.
//  - The drawer's mode dial (D4) sends OpenFan: the drawer closes and the fan opens. Never both.
//  - TalkBack: ☰ carries "Switch to <Mode>" and "Open your bots"; the Bots card "Open your bots".

internal object MenuHold {
    /** Hold on ☰ before the fan opens (iOS holdSeconds 0.35, web HOLD_MS 350). */
    const val HOLD_MILLIS = 350L
    /** Further hold on the Bots card before the shelf opens (iOS botsHoldSeconds 0.4). */
    const val BOTS_HOLD_MILLIS = 400L
    /** Finger travel before the hold fires that cancels it (iOS slop 12). */
    val slop = 12.dp
    /** The shelf's Create a bot item (web `'__create'`). */
    const val CREATE_BOT_ID = "__create"
    /** The modes on the fan, in order (desktop mode wheel order). */
    val fanModes: List<ShellMode> get() = listOf(ShellMode.Home, ShellMode.Bots, ShellMode.Code, ShellMode.Aci)
}

internal enum class MenuHoldOverlay { None, Fan, Shelf }

/** A fan card or a shelf bot. `Bot(CREATE_BOT_ID)` is Create a bot. */
internal sealed interface MenuHoldTarget {
    data class Mode(val mode: ShellMode) : MenuHoldTarget
    data class Bot(val id: String) : MenuHoldTarget
}

private val MenuHoldTarget?.isBotsCard: Boolean get() = this == MenuHoldTarget.Mode(ShellMode.Bots)

internal data class MenuHoldMachine(
    val overlay: MenuHoldOverlay = MenuHoldOverlay.None,
    /** A finger is down on ☰. */
    val pressing: Boolean = false,
    /** The current press became a hold (so its release is not a tap). */
    val held: Boolean = false,
    /** The card or bot under the finger. */
    val target: MenuHoldTarget? = null,
    /** Mirrors the shell's drawer (synced before every dispatch). */
    val drawerOpen: Boolean = false,
    /** A Bots-only host: holds open the shelf. */
    val lockedMode: Boolean = false,
    /** Opened by an action (drawer dial, TalkBack). */
    val openedByAction: Boolean = false,
)

internal sealed interface MenuHoldEvent {
    object Press : MenuHoldEvent
    object Release : MenuHoldEvent
    /** The press stopped being a tap or hold (the finger moved before the hold). */
    object Cancel : MenuHoldEvent
    object HoldTimer : MenuHoldEvent
    object BotsHoldTimer : MenuHoldEvent
    data class Move(val target: MenuHoldTarget?) : MenuHoldEvent
    /** A tap on a card or bot while the fan / shelf stays open. */
    data class Pick(val target: MenuHoldTarget) : MenuHoldEvent
    /** D4 drawer mode dial. */
    object OpenFan : MenuHoldEvent
    /** TalkBack "Open your bots". */
    object OpenShelf : MenuHoldEvent
    object DrawerOpened : MenuHoldEvent
    object DrawerClosed : MenuHoldEvent
    /** A tap outside the fan / shelf, or Back. */
    object OutsideTap : MenuHoldEvent
    data class SetLocked(val locked: Boolean) : MenuHoldEvent
}

internal enum class MenuHoldHaptic { Light, Medium, Selection }

internal sealed interface MenuHoldEffect {
    object StartHoldTimer : MenuHoldEffect
    object StartBotsHoldTimer : MenuHoldEffect
    object ClearBotsHoldTimer : MenuHoldEffect
    object ClearTimers : MenuHoldEffect
    object OpenDrawer : MenuHoldEffect
    object CloseDrawer : MenuHoldEffect
    data class SwitchMode(val mode: ShellMode) : MenuHoldEffect
    data class OpenBot(val id: String) : MenuHoldEffect
    data class Haptic(val kind: MenuHoldHaptic) : MenuHoldEffect
}

internal data class MenuHoldResult(val state: MenuHoldMachine, val effects: List<MenuHoldEffect>)

/** Keeps only targets the open overlay can hold: modes in the fan, bots on the shelf. */
private fun fits(overlay: MenuHoldOverlay, target: MenuHoldTarget?): MenuHoldTarget? = when {
    target == null -> null
    overlay == MenuHoldOverlay.Fan && target is MenuHoldTarget.Mode -> target
    overlay == MenuHoldOverlay.Shelf && target is MenuHoldTarget.Bot -> target
    else -> null
}

/** The choice a release or tap on [target] makes. */
private fun choose(state: MenuHoldMachine, target: MenuHoldTarget): MenuHoldResult {
    val closed = state.copy(overlay = MenuHoldOverlay.None, target = null, held = false, pressing = false, openedByAction = false)
    val done = listOf(MenuHoldEffect.ClearTimers, MenuHoldEffect.Haptic(MenuHoldHaptic.Light))
    return when (target) {
        is MenuHoldTarget.Mode -> MenuHoldResult(closed, done + MenuHoldEffect.SwitchMode(target.mode))
        is MenuHoldTarget.Bot -> MenuHoldResult(closed, done + MenuHoldEffect.OpenBot(target.id))
    }
}

private fun same(state: MenuHoldMachine) = MenuHoldResult(state, emptyList())

internal fun reduceMenuHold(state: MenuHoldMachine, event: MenuHoldEvent): MenuHoldResult = when (event) {
    MenuHoldEvent.Press -> {
        val next = state.copy(pressing = true, held = false)
        // Hold does nothing while the drawer is open: no timer, the release closes the drawer.
        if (state.drawerOpen) same(next) else MenuHoldResult(next, listOf(MenuHoldEffect.ClearTimers, MenuHoldEffect.StartHoldTimer))
    }

    MenuHoldEvent.HoldTimer ->
        if (!state.pressing || state.held || state.drawerOpen) same(state)
        else MenuHoldResult(
            state.copy(held = true, overlay = if (state.lockedMode) MenuHoldOverlay.Shelf else MenuHoldOverlay.Fan, target = null, openedByAction = false),
            listOf(MenuHoldEffect.Haptic(MenuHoldHaptic.Medium)),
        )

    is MenuHoldEvent.Move -> {
        if (state.overlay == MenuHoldOverlay.None) same(state)
        else {
            val target = fits(state.overlay, event.target)
            if (target == state.target) same(state)
            else {
                val effects = mutableListOf<MenuHoldEffect>()
                if (target != null) effects += MenuHoldEffect.Haptic(MenuHoldHaptic.Selection)
                if (state.overlay == MenuHoldOverlay.Fan) {
                    if (state.target.isBotsCard) effects += MenuHoldEffect.ClearBotsHoldTimer
                    if (target.isBotsCard && state.pressing && state.held && !state.lockedMode) effects += MenuHoldEffect.StartBotsHoldTimer
                }
                MenuHoldResult(state.copy(target = target), effects)
            }
        }
    }

    MenuHoldEvent.BotsHoldTimer ->
        if (state.overlay != MenuHoldOverlay.Fan || !state.pressing || !state.target.isBotsCard) same(state)
        else MenuHoldResult(state.copy(overlay = MenuHoldOverlay.Shelf, target = null), listOf(MenuHoldEffect.Haptic(MenuHoldHaptic.Medium)))

    MenuHoldEvent.Release -> when {
        !state.pressing -> same(state)
        // A tap. With the drawer open it closes the drawer; otherwise it opens it (and closes the fan).
        !state.held && state.drawerOpen ->
            MenuHoldResult(state.copy(pressing = false), listOf(MenuHoldEffect.ClearTimers, MenuHoldEffect.CloseDrawer))
        !state.held ->
            MenuHoldResult(
                state.copy(pressing = false, overlay = MenuHoldOverlay.None, target = null, drawerOpen = true, openedByAction = false),
                listOf(MenuHoldEffect.ClearTimers, MenuHoldEffect.OpenDrawer),
            )
        else -> {
            val target = fits(state.overlay, state.target)
            // Let go on nothing: the fan / shelf stays open for a tap.
            if (target != null) choose(state, target)
            else MenuHoldResult(state.copy(pressing = false, held = false, target = null), listOf(MenuHoldEffect.ClearTimers))
        }
    }

    MenuHoldEvent.Cancel ->
        if (!state.pressing) same(state)
        else MenuHoldResult(
            if (state.held) state.copy(pressing = false, held = false, overlay = MenuHoldOverlay.None, target = null)
            else state.copy(pressing = false, held = false),
            listOf(MenuHoldEffect.ClearTimers),
        )

    is MenuHoldEvent.Pick -> fits(state.overlay, event.target)?.let { choose(state, it) } ?: same(state)

    MenuHoldEvent.OpenFan, MenuHoldEvent.OpenShelf -> {
        val effects = mutableListOf<MenuHoldEffect>(MenuHoldEffect.ClearTimers)
        if (state.drawerOpen) effects += MenuHoldEffect.CloseDrawer
        effects += MenuHoldEffect.Haptic(MenuHoldHaptic.Medium)
        MenuHoldResult(
            state.copy(
                overlay = if (event == MenuHoldEvent.OpenShelf || state.lockedMode) MenuHoldOverlay.Shelf else MenuHoldOverlay.Fan,
                target = null, pressing = false, held = false, drawerOpen = false, openedByAction = true,
            ),
            effects,
        )
    }

    MenuHoldEvent.DrawerOpened -> MenuHoldResult(
        state.copy(drawerOpen = true, overlay = MenuHoldOverlay.None, target = null, pressing = false, held = false, openedByAction = false),
        if (state.pressing) listOf(MenuHoldEffect.ClearTimers) else emptyList(),
    )

    MenuHoldEvent.DrawerClosed -> same(state.copy(drawerOpen = false))

    MenuHoldEvent.OutsideTap ->
        if (state.overlay == MenuHoldOverlay.None) same(state)
        else MenuHoldResult(
            state.copy(overlay = MenuHoldOverlay.None, target = null, pressing = false, held = false, openedByAction = false),
            listOf(MenuHoldEffect.ClearTimers),
        )

    is MenuHoldEvent.SetLocked -> same(
        if (event.locked && state.overlay == MenuHoldOverlay.Fan) state.copy(lockedMode = true, overlay = MenuHoldOverlay.None, target = null)
        else state.copy(lockedMode = event.locked),
    )
}

/** The overlay the view may show: never the fan or shelf while the drawer is open. */
internal fun menuOverlayVisible(state: MenuHoldMachine): MenuHoldOverlay =
    if (state.drawerOpen) MenuHoldOverlay.None else state.overlay

/** `mode:<m>` / `bot:<id>` (web `targetKey`), for test ids. */
internal fun targetKey(target: MenuHoldTarget): String = when (target) {
    is MenuHoldTarget.Mode -> "mode:${target.mode.raw}"
    is MenuHoldTarget.Bot -> "bot:${target.id}"
}

/** Unread badge text: "9+" past nine (web `badgeText`). */
internal fun menuBadgeText(count: Int): String = if (count > 9) "9+" else "$count"

/** Dock magnify for a shelf bot [distance] columns from the one under the finger (iOS shelfMagnify). */
internal fun shelfMagnify(distance: Int?, reduced: Boolean): Pair<Float, Dp> = when {
    reduced || distance == null -> 1f to 0.dp
    distance == 0 -> 1.55f to (-14).dp
    distance == 1 -> 1.15f to (-4).dp
    else -> 1f to 0.dp
}

/** "Drop · 2 of 6" under the shelf. */
internal fun shelfPositionLabel(name: String, index: Int, count: Int) = "$name · ${index + 1} of $count"

/** One calm status line for a bot, no codes (web `botStatusLine`). */
internal fun botShelfStatusLine(waiting: Boolean, working: Boolean, activityLabel: String?, offline: Boolean, isMain: Boolean): String {
    val line = when {
        waiting -> "Waiting for you"
        working -> if (!activityLabel.isNullOrEmpty()) "Working · $activityLabel" else "Working"
        offline -> "Offline"
        else -> "Idle"
    }
    return if (isMain) "Main bot · $line" else line
}

/**
 * The shelf column under [point] (iOS shelfColumn): read by column because magnified avatars move
 * under the finger, with room around the glass so a slide that drifts off it still scrubs.
 */
internal fun shelfColumn(point: Offset, shelf: Rect, inset: Float, count: Int, density: Density): Int? {
    if (count <= 0) return null
    val zone = with(density) { Rect(shelf.left - 12.dp.toPx(), shelf.top - 80.dp.toPx(), shelf.right + 12.dp.toPx(), shelf.bottom + 160.dp.toPx()) }
    if (!zone.contains(point)) return null
    val inner = maxOf(1f, shelf.width - inset * 2)
    val column = floor((point.x - shelf.left - inset) / (inner / count)).toInt()
    return column.coerceIn(0, count - 1)
}

/**
 * The ☰ hold controller (iOS MenuHoldController): every ☰ / fan / shelf / drawer event goes
 * through [dispatch]; it runs the reducer's effects (timers, haptics, drawer, mode switch,
 * opening a bot). [anchor] is the pressed ☰'s bounds in root pixels, noted while the drawer is
 * closed (an open drawer moves and scales the page).
 */
@Stable
internal class MenuHoldState {
    var state by mutableStateOf(MenuHoldMachine())
        private set
    var anchor by mutableStateOf(Rect.Zero)
        private set
    /** The shelf's Create a bot item opens the new bot sheet. */
    var isCreateBotPresented by mutableStateOf(false)
    /** The overlay canvas width (root px), for clamping the arc. */
    var canvasWidthPx = 0f
    var density: Density = Density(1f)

    private var scope: CoroutineScope? = null
    private var view: View? = null
    private var isDrawerOpen: () -> Boolean = { false }
    private var isDrawerShown: () -> Boolean = { false }
    private var openDrawerAction: () -> Unit = {}
    private var closeDrawerAction: () -> Unit = {}
    private var switchModeAction: (ShellMode) -> Unit = {}
    private var openBotAction: (String) -> Unit = {}
    private var shelfBotIds: () -> List<String> = { emptyList() }
    private var pressOpen: (() -> Unit)? = null
    private var holdJob: Job? = null
    private var botsJob: Job? = null

    val visible: MenuHoldOverlay get() = menuOverlayVisible(state)
    /** The fan or the shelf shows (Back closes it; the edge pan waits). */
    val fanOpen: Boolean get() = visible != MenuHoldOverlay.None

    /** Installed by the shell (iOS MainWorkspaceView onAppear). */
    fun install(
        scope: CoroutineScope,
        view: View,
        isDrawerOpen: () -> Boolean,
        isDrawerShown: () -> Boolean,
        openDrawer: () -> Unit,
        closeDrawer: () -> Unit,
        switchMode: (ShellMode) -> Unit,
        openBot: (String) -> Unit,
        shelfBotIds: () -> List<String>,
    ) {
        this.scope = scope
        this.view = view
        this.isDrawerOpen = isDrawerOpen
        this.isDrawerShown = isDrawerShown
        this.openDrawerAction = openDrawer
        this.closeDrawerAction = closeDrawer
        this.switchModeAction = switchMode
        this.openBotAction = openBot
        this.shelfBotIds = shelfBotIds
    }

    fun dispatch(event: MenuHoldEvent) {
        val current = state.copy(drawerOpen = isDrawerOpen())
        val result = reduceMenuHold(current, event)
        if (result.state != state) state = result.state
        run(result.effects)
    }

    fun openFan() = dispatch(MenuHoldEvent.OpenFan)
    fun closeFan() = dispatch(MenuHoldEvent.OutsideTap)

    /** A ☰ reports where it is while nothing is open (D4 opens the fan from the newest one). */
    fun noteAnchor(rect: Rect) {
        if (rect.width <= 0f || isDrawerOpen() || isDrawerShown() || state.overlay != MenuHoldOverlay.None || state.pressing || rect == anchor) return
        anchor = rect
    }

    fun press(rect: Rect, onOpen: () -> Unit) {
        if (rect.width > 0f && !isDrawerOpen() && !isDrawerShown()) anchor = rect
        pressOpen = onOpen
        dispatch(MenuHoldEvent.Press)
    }

    /**
     * The finger moved ([point] in root px). Before the hold, travel past the slop cancels (a
     * scroll, not a hold); after it, the target follows the finger.
     */
    fun drag(point: Offset, translation: Offset) {
        if (!state.pressing) return
        if (!state.held) {
            if (translation.getDistance() > with(density) { MenuHold.slop.toPx() }) dispatch(MenuHoldEvent.Cancel)
            return
        }
        if (visible == MenuHoldOverlay.None) return
        dispatch(MenuHoldEvent.Move(target(point)))
    }

    fun endPress() {
        dispatch(MenuHoldEvent.Release)
        pressOpen = null
    }

    fun cancelPress() {
        dispatch(MenuHoldEvent.Cancel)
        pressOpen = null
    }

    /** The fan card or shelf bot under a root point. */
    fun target(point: Offset): MenuHoldTarget? = when (visible) {
        MenuHoldOverlay.Fan -> {
            val margin = with(density) { 6.dp.toPx() }
            ModeFanLayout.cardFrames(anchor, canvasWidthPx, density).withIndex()
                .firstOrNull { (_, r) -> r.inflate(margin).contains(point) }
                ?.let { MenuHoldTarget.Mode(MenuHold.fanModes[it.index]) }
        }
        MenuHoldOverlay.Shelf -> {
            val targets = shelfTargets()
            val layout = BotShelfLayout(anchor, canvasWidthPx, targets.size, density)
            shelfColumn(point, layout.frame, with(density) { BotShelfLayout.inset.toPx() }, targets.size, density)?.let { targets[it] }
        }
        MenuHoldOverlay.None -> null
    }

    /** The shelf's items in order: your bots, then Create a bot. */
    fun shelfTargets(): List<MenuHoldTarget> =
        shelfBotIds().map { MenuHoldTarget.Bot(it) } + MenuHoldTarget.Bot(MenuHold.CREATE_BOT_ID)

    /** The ☰ TalkBack action "Switch to <Mode>". */
    fun switchDirectly(mode: ShellMode) {
        haptic(MenuHoldHaptic.Light)
        dispatch(MenuHoldEvent.OutsideTap)
        switchModeAction(mode)
    }

    private fun haptic(kind: MenuHoldHaptic) {
        val v = view ?: return
        v.performHapticFeedback(
            when (kind) {
                MenuHoldHaptic.Light -> HapticFeedbackConstants.VIRTUAL_KEY
                MenuHoldHaptic.Medium -> HapticFeedbackConstants.LONG_PRESS
                MenuHoldHaptic.Selection -> HapticFeedbackConstants.CLOCK_TICK
            },
        )
    }

    private fun run(effects: List<MenuHoldEffect>) {
        for (effect in effects) {
            when (effect) {
                MenuHoldEffect.ClearTimers -> {
                    holdJob?.cancel(); holdJob = null
                    botsJob?.cancel(); botsJob = null
                }
                MenuHoldEffect.ClearBotsHoldTimer -> {
                    botsJob?.cancel(); botsJob = null
                }
                MenuHoldEffect.StartHoldTimer -> {
                    holdJob?.cancel()
                    holdJob = scope?.launch {
                        delay(MenuHold.HOLD_MILLIS)
                        holdJob = null
                        dispatch(MenuHoldEvent.HoldTimer)
                    }
                }
                MenuHoldEffect.StartBotsHoldTimer -> {
                    botsJob?.cancel()
                    botsJob = scope?.launch {
                        delay(MenuHold.BOTS_HOLD_MILLIS)
                        botsJob = null
                        dispatch(MenuHoldEvent.BotsHoldTimer)
                    }
                }
                is MenuHoldEffect.Haptic -> haptic(effect.kind)
                MenuHoldEffect.OpenDrawer -> (pressOpen ?: openDrawerAction)()
                MenuHoldEffect.CloseDrawer -> closeDrawerAction()
                is MenuHoldEffect.SwitchMode -> switchModeAction(effect.mode)
                is MenuHoldEffect.OpenBot ->
                    if (effect.id == MenuHold.CREATE_BOT_ID) isCreateBotPresented = true else openBotAction(effect.id)
            }
        }
    }
}

/** ModeFanLayout: a quarter arc of 60dp cards, radius 148dp, from straight below ☰ round to its right, nudged 16dp down. */
internal object ModeFanLayout {
    val radius = 148.dp
    val card = 60.dp

    fun cardFrames(anchor: Rect, widthPx: Float, density: Density): List<Rect> = with(density) {
        val r = radius.toPx()
        val c = card.toPx()
        val edge = 8.dp.toPx()
        val drop = 16.dp.toPx()
        val count = MenuHold.fanModes.size
        (0 until count).map { index ->
            val angle = (Math.PI / 2) * (1 - index.toDouble() / maxOf(1, count - 1))
            val x = (cos(angle) * r).roundToInt().toFloat()
            val y = (sin(angle) * r).roundToInt().toFloat() + drop
            val left = (anchor.center.x + x - c / 2).coerceIn(edge, maxOf(edge, widthPx - c - edge))
            val top = anchor.center.y + y - c / 2
            Rect(left, top, left + c, top + c)
        }
    }
}

/**
 * Where the shelf sits (iOS BotShelfLayout): 56dp under ☰, 12dp from the left edge, one slot per
 * item (your bots, then Create a bot). Slots shrink so every bot fits on one row.
 */
internal class BotShelfLayout(anchor: Rect, widthPx: Float, count: Int, density: Density) {
    val frame: Rect
    val slot: Dp
    val avatar: Dp

    init {
        val n = maxOf(1, count)
        val roomDp = with(density) { maxOf(0f, widthPx - (edge * 2 + inset * 2).toPx()).toDp() }
        slot = minOf(52.dp, roomDp / n)
        avatar = maxOf(20.dp, minOf(38.dp, slot - 10.dp))
        frame = with(density) {
            val left = edge.toPx()
            val top = anchor.bottom + 56.dp.toPx()
            Rect(left, top, left + (slot * n + inset * 2).toPx(), top + height.toPx())
        }
    }

    companion object {
        val inset = 12.dp
        val height = 64.dp
        val edge = 12.dp
    }
}

/** System reduce motion (animator duration scale 0), as StageBackdrop reads it. */
@Composable
private fun rememberReducedMotion(): Boolean {
    val context = LocalContext.current
    return remember(context) {
        runCatching { Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }.getOrDefault(false)
    }
}

/** iOS Motion.fan: spring(response 0.26, damping 0.8). */
private fun <T> fanSpring() = spring<T>(dampingRatio = 0.8f, stiffness = 584f)
private const val FAN_STAGGER_MILLIS = 25L

/** The iOS glass surface on fan cards, the shelf and its label. */
private fun Modifier.menuGlass(shape: androidx.compose.ui.graphics.Shape, fill: Color, line: Color, shadow: Dp): Modifier =
    this.shadow(shadow, shape, ambientColor = Color.Black.copy(alpha = 0.12f), spotColor = Color.Black.copy(alpha = 0.12f))
        .clip(shape)
        .background(fill.copy(alpha = 0.95f))
        .border(BorderStroke(1.dp, line.copy(alpha = 0.11f)), shape)

/**
 * The ☰ button (iOS PhoneMenuButton, contract C8). Tap → [onOpen]. Hold → the mode fan; slide
 * to a card and let go to switch; stay on Bots for the bots shelf. A small badge shows the current
 * mode. Test id `phoneMenuButton`. TalkBack: activate opens the menu; one action per mode, and
 * "Open your bots".
 */
@Composable
internal fun PhoneMenuButton(
    mode: ShellMode,
    hold: MenuHoldState,
    holdEnabled: Boolean,
    onOpen: () -> Unit,
    onPick: (ShellMode) -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = AllternitTheme.colors
    // The gesture outlives recompositions (keyed on holdEnabled), so it reads the latest values.
    val open by rememberUpdatedState(onOpen)
    val bounds = remember { mutableStateOf(Rect.Zero) }
    Box(
        modifier = modifier
            .size(44.dp)
            .testTag("phoneMenuButton")
            .onGloballyPositioned {
                val rect = it.boundsInRoot()
                bounds.value = rect
                if (holdEnabled) hold.noteAnchor(rect)
            }
            .semantics {
                role = Role.Button
                contentDescription = "Open menu"
                stateDescription = "${mode.label} mode"
                onClick(label = "Open menu") { onOpen(); true }
                customActions = if (holdEnabled) {
                    MenuHold.fanModes.map { m ->
                        CustomAccessibilityAction("Switch to ${m.label}") { hold.switchDirectly(m); true }
                    } + CustomAccessibilityAction("Open your bots") { hold.dispatch(MenuHoldEvent.OpenShelf); true }
                } else emptyList()
            }
            .pointerInput(holdEnabled) {
                val slop = MenuHold.slop.toPx()
                awaitEachGesture {
                    val down = awaitFirstDown()
                    down.consume()
                    if (!holdEnabled) {
                        // Tap only (the overview cover): a release within the slop opens the menu.
                        var travelled = false
                        while (true) {
                            val change = awaitPointerEvent().changes.firstOrNull { it.id == down.id } ?: break
                            if ((change.position - down.position).getDistance() > slop) travelled = true
                            if (!change.pressed) {
                                change.consume()
                                if (!travelled) open()
                                break
                            }
                        }
                        return@awaitEachGesture
                    }
                    hold.press(bounds.value, open)
                    var finished = false
                    try {
                        while (true) {
                            val change = awaitPointerEvent().changes.firstOrNull { it.id == down.id }
                            if (change == null) break
                            change.consume()
                            if (!change.pressed) {
                                finished = true
                                hold.endPress()
                                break
                            }
                            hold.drag(bounds.value.topLeft + change.position, change.position - down.position)
                        }
                    } catch (e: CancellationException) {
                        // The system took the touch: treat it as a cancel.
                        if (!finished) hold.cancelPress()
                        finished = true
                        throw e
                    } finally {
                        if (!finished) hold.cancelPress()
                    }
                }
            },
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            painter = painterResource(R.drawable.shell_list),
            contentDescription = null,
            tint = c.textPrimary,
            modifier = Modifier.size(22.dp),
        )
        // MenuModeBadge (iOS): the current mode's icon knocked out of a 15dp ink dot, ☰'s corner.
        Box(
            modifier = Modifier
                .align(Alignment.BottomEnd)
                .offset(x = (-4).dp, y = (-4).dp)
                .size(15.dp)
                .clip(CircleShape)
                .background(c.textPrimary)
                .testTag("phoneMenuModeBadge${mode.raw}"),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                painter = painterResource(mode.icon),
                contentDescription = null,
                tint = c.bgPrimary,
                modifier = Modifier.size(9.dp),
            )
        }
    }
}

/**
 * The scrim, the fan and the bots shelf over the whole shell, above the drawer and the page
 * (iOS MenuOverlayHost). Nothing hit-tests while it is closed.
 */
@Composable
internal fun MenuOverlayHost(
    hold: MenuHoldState,
    current: ShellMode,
    botsStore: BotModeStore,
    signedIn: Boolean,
) {
    val density = LocalDensity.current
    val reduced = rememberReducedMotion()
    val visible = hold.visible
    val scrim by animateFloatAsState(if (visible != MenuHoldOverlay.None) 1f else 0f, tween(160), label = "menuScrim")
    // The shelf reads your bots: load them as soon as a hold opens the fan.
    LaunchedEffect(visible != MenuHoldOverlay.None, signedIn) {
        if (visible != MenuHoldOverlay.None && signedIn && botsStore.agents.isEmpty()) runCatching { botsStore.refreshAll() }
    }
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val widthPx = with(density) { maxWidth.toPx() }
        SideEffect {
            hold.canvasWidthPx = widthPx
            hold.density = density
        }
        if (visible != MenuHoldOverlay.None || scrim > 0.01f) {
            Box(
                Modifier
                    .fillMaxSize()
                    .graphicsLayer { alpha = scrim }
                    .background(Color.Black.copy(alpha = 0.08f))
                    .clickable(
                        interactionSource = remember { MutableInteractionSource() },
                        indication = null,
                        enabled = visible != MenuHoldOverlay.None,
                        onClickLabel = "Close",
                    ) { hold.dispatch(MenuHoldEvent.OutsideTap) }
                    .clearAndSetSemantics {},
            )
        }
        ModeFanCards(hold, open = visible == MenuHoldOverlay.Fan, current = current, widthPx = widthPx, waiting = botsStore.waitingCount, reduced = reduced)
        BotShelf(hold, open = visible == MenuHoldOverlay.Shelf, botsStore = botsStore, widthPx = widthPx, reduced = reduced)
    }
}

/** Home · Bots · Code · ACI as glass cards on an arc out of ☰ (iOS ModeFanView). Ids `modeFanCard<raw>`. */
@Composable
private fun ModeFanCards(hold: MenuHoldState, open: Boolean, current: ShellMode, widthPx: Float, waiting: Int, reduced: Boolean) {
    val density = LocalDensity.current
    val frames = ModeFanLayout.cardFrames(hold.anchor, widthPx, density)
    Box(Modifier.fillMaxSize().then(if (open) Modifier.semantics { contentDescription = "Switch mode" } else Modifier)) {
        MenuHold.fanModes.forEachIndexed { index, mode ->
            val t = remember { Animatable(0f) }
            LaunchedEffect(open, reduced) {
                if (reduced) {
                    t.animateTo(if (open) 1f else 0f, tween(200))
                } else {
                    // Cards spring out of ☰ with a 25 ms stagger; release folds them back (iOS Motion.fan).
                    delay(index * FAN_STAGGER_MILLIS)
                    t.animateTo(if (open) 1f else 0f, fanSpring())
                }
            }
            val progress = t.value
            if (progress > 0.01f || open) {
                val frame = frames[index]
                val dx = hold.anchor.center.x - frame.center.x
                val dy = hold.anchor.center.y - frame.center.y
                FanCard(
                    mode = mode,
                    current = mode == current,
                    hot = hold.state.target == MenuHoldTarget.Mode(mode),
                    unread = if (mode == ShellMode.Bots) waiting else 0,
                    enabled = open,
                    reduced = reduced,
                    onClick = { hold.dispatch(MenuHoldEvent.Pick(MenuHoldTarget.Mode(mode))) },
                    onOpenShelf = { hold.dispatch(MenuHoldEvent.OpenShelf) },
                    modifier = Modifier
                        .offset { IntOffset(frame.left.roundToInt(), frame.top.roundToInt()) }
                        .graphicsLayer {
                            if (reduced) {
                                alpha = progress.coerceIn(0f, 1f)
                            } else {
                                // FanFold: half size, transparent, at ☰'s centre when folded.
                                val fold = 1f - progress
                                translationX = dx * fold
                                translationY = dy * fold
                                alpha = progress.coerceIn(0f, 1f)
                                val s = 0.5f + 0.5f * progress
                                scaleX = s
                                scaleY = s
                            }
                        },
                )
            }
        }
    }
}

@Composable
private fun FanCard(
    mode: ShellMode,
    current: Boolean,
    hot: Boolean,
    unread: Int,
    enabled: Boolean,
    reduced: Boolean,
    onClick: () -> Unit,
    onOpenShelf: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = AllternitTheme.colors
    val scale by animateFloatAsState(if (hot && !reduced) 1.12f else 1f, if (reduced) tween(0) else fanSpring(), label = "fanHot")
    val shape = RoundedCornerShape(18.dp)
    Box(
        modifier = modifier
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
            }
            .size(ModeFanLayout.card),
    ) {
        Column(
            modifier = Modifier
                .size(ModeFanLayout.card)
                .menuGlass(shape, c.bgPrimary, c.textPrimary, 12.dp)
                .then(if (reduced && hot) Modifier.border(BorderStroke(2.dp, c.accentPrimary), shape) else Modifier)
                .testTag("modeFanCard${mode.raw}")
                .semantics {
                    selected = current
                    contentDescription = if (mode == ShellMode.Bots && unread > 0) "Bots, $unread waiting" else mode.label
                    // TalkBack can't hold: the Bots card carries "Open your bots".
                    if (mode == ShellMode.Bots) customActions = listOf(CustomAccessibilityAction("Open your bots") { onOpenShelf(); true })
                }
                .clickable(enabled = enabled, role = Role.Button, onClickLabel = "Switch to ${mode.label}", onClick = onClick),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(3.dp, Alignment.CenterVertically),
        ) {
            Box(
                modifier = Modifier
                    .size(30.dp)
                    .clip(CircleShape)
                    .background(if (current) c.textPrimary else c.fillNeutral),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    painter = painterResource(mode.icon),
                    contentDescription = null,
                    tint = if (current) c.bgPrimary else c.textPrimary,
                    modifier = Modifier.size(16.dp),
                )
            }
            Text(mode.label, color = c.textPrimary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
        }
        if (unread > 0) ModeDialBadge(unread, Modifier.align(Alignment.TopEnd).offset(x = 4.dp, y = (-4).dp))
    }
}

/**
 * The bots shelf (iOS BotShelfView, web PhoneBotShelf): hold ☰, slide to Bots and keep holding
 * 0.4 s. Your bots sit on one horizontal glass shelf with a dock magnify: the bot under the finger
 * grows to 1.55, its neighbours to 1.15. Under the shelf: "Name · i of n" and the bot's status.
 * Release on a bot (or tap one) opens its main chat; the last item creates a bot. Reduced motion:
 * no magnify and no slide; a ring marks the bot under the finger and the shelf fades.
 */
@Composable
private fun BotShelf(hold: MenuHoldState, open: Boolean, botsStore: BotModeStore, widthPx: Float, reduced: Boolean) {
    val density = LocalDensity.current
    val t = remember { Animatable(0f) }
    LaunchedEffect(open, reduced) {
        t.animateTo(if (open) 1f else 0f, if (reduced) tween(200) else spring(dampingRatio = 0.82f, stiffness = 439f))
    }
    val progress = t.value
    if (progress <= 0.01f && !open) return
    val bots = botsStore.bots
    val layout = BotShelfLayout(hold.anchor, widthPx, bots.size + 1, density)
    val hotIndex: Int? = when (val target = hold.state.target) {
        is MenuHoldTarget.Bot -> if (target.id == MenuHold.CREATE_BOT_ID) bots.size else bots.indexOfFirst { it.id == target.id }.takeIf { it >= 0 }
        else -> null
    }
    val c = AllternitTheme.colors
    val shelfShape = RoundedCornerShape(BotShelfLayout.height / 2)
    Column(
        Modifier
            .offset { IntOffset(layout.frame.left.roundToInt(), layout.frame.top.roundToInt()) }
            .graphicsLayer {
                alpha = progress.coerceIn(0f, 1f)
                if (!reduced) {
                    // A flat bar that slides out of ☰ sideways (scale from 0.2, leading anchor).
                    val s = 0.2f + 0.8f * progress
                    scaleX = s
                    scaleY = s
                    transformOrigin = TransformOrigin(0f, 0.5f)
                }
            },
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(
            Modifier
                .width(with(density) { layout.frame.width.toDp() })
                .height(BotShelfLayout.height)
                .menuGlass(shelfShape, c.bgPrimary, c.textPrimary, 14.dp)
                .padding(horizontal = BotShelfLayout.inset)
                .semantics { contentDescription = "Your bots" },
            verticalAlignment = Alignment.CenterVertically,
        ) {
            bots.forEachIndexed { index, bot ->
                ShelfBot(hold, botsStore, bot, index, bots.size, hotIndex?.let { abs(it - index) }, layout, reduced, open)
            }
            ShelfCreate(hold, hotIndex?.let { abs(it - bots.size) }, layout, reduced, open)
        }
        ShelfLabel(botsStore, bots, hotIndex)
    }
}

@Composable
private fun ShelfBot(
    hold: MenuHoldState,
    store: BotModeStore,
    bot: BotRecord,
    index: Int,
    count: Int,
    distance: Int?,
    layout: BotShelfLayout,
    reduced: Boolean,
    enabled: Boolean,
) {
    val waiting = store.waitingCount(bot.id) > 0
    val working = !waiting && store.workingCount(bot.id) > 0
    val unread = store.waitingCount(bot.id)
    val extra = listOf(if (waiting) "waiting for you" else if (working) "working" else "", if (unread > 0) "$unread unread" else "")
        .filter { it.isNotEmpty() }
        .joinToString(", ")
    Box(
        Modifier
            .zIndex(if (distance == 0) 2f else if (distance == 1) 1f else 0f)
            .width(layout.slot)
            .height(BotShelfLayout.height)
            .testTag("botShelf-${targetKey(MenuHoldTarget.Bot(bot.id))}")
            .semantics {
                contentDescription = "Open ${bot.displayName}'s main chat${if (extra.isEmpty()) "" else ", $extra"}"
                stateDescription = shelfPositionLabel(bot.displayName, index, count)
            }
            .clickable(enabled = enabled, role = Role.Button) { hold.dispatch(MenuHoldEvent.Pick(MenuHoldTarget.Bot(bot.id))) },
        contentAlignment = Alignment.Center,
    ) {
        ShelfMagnify(distance, reduced) {
            Box {
                BotStatusAvatar(bot, layout.avatar, working = working, waiting = waiting)
                if (unread > 0) ModeDialBadge(unread, Modifier.align(Alignment.TopEnd).offset(x = 7.dp, y = (-5).dp))
            }
        }
    }
}

@Composable
private fun ShelfCreate(hold: MenuHoldState, distance: Int?, layout: BotShelfLayout, reduced: Boolean, enabled: Boolean) {
    val c = AllternitTheme.colors
    Box(
        Modifier
            .zIndex(if (distance == 0) 2f else 0f)
            .width(layout.slot)
            .height(BotShelfLayout.height)
            .testTag("botShelfCreate")
            .semantics { contentDescription = "Create a bot" }
            .clickable(enabled = enabled, role = Role.Button) { hold.dispatch(MenuHoldEvent.Pick(MenuHoldTarget.Bot(MenuHold.CREATE_BOT_ID))) },
        contentAlignment = Alignment.Center,
    ) {
        ShelfMagnify(distance, reduced) {
            val dashColor = c.borderStrong
            Box(
                Modifier
                    .size(layout.avatar)
                    .drawDashedCircle(dashColor),
                contentAlignment = Alignment.Center,
            ) {
                Icon(painterResource(DesignR.drawable.ph_plus), null, tint = c.textSecondary, modifier = Modifier.size(15.dp))
            }
        }
    }
}

/** The Create a bot ring: a 1dp dashed circle (iOS StrokeStyle dash [3, 3]). */
private fun Modifier.drawDashedCircle(color: Color): Modifier = this.drawBehind {
    val stroke = 1.dp.toPx()
    drawCircle(
        color = color,
        radius = size.minDimension / 2 - stroke / 2,
        style = Stroke(width = stroke, pathEffect = PathEffect.dashPathEffect(floatArrayOf(3.dp.toPx(), 3.dp.toPx()))),
    )
}

/** Dock magnify from the bottom edge, or a ring under reduced motion (iOS ShelfMagnify). */
@Composable
private fun ShelfMagnify(distance: Int?, reduced: Boolean, content: @Composable () -> Unit) {
    val c = AllternitTheme.colors
    val (scaleTarget, liftTarget) = shelfMagnify(distance, reduced)
    val scale by animateFloatAsState(scaleTarget, if (reduced) tween(0) else spring(dampingRatio = 0.72f, stiffness = 815f), label = "shelfScale")
    val lift by animateFloatAsState(liftTarget.value, if (reduced) tween(0) else spring(dampingRatio = 0.72f, stiffness = 815f), label = "shelfLift")
    Box(
        Modifier
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
                transformOrigin = TransformOrigin(0.5f, 1f)
                translationY = lift.dp.toPx()
            }
            .then(if (reduced && distance == 0) Modifier.border(BorderStroke(2.dp, c.accentPrimary), CircleShape).padding(4.dp) else Modifier),
        contentAlignment = Alignment.Center,
    ) { content() }
}

@Composable
private fun ShelfLabel(store: BotModeStore, bots: List<BotRecord>, hotIndex: Int?) {
    val c = AllternitTheme.colors
    val shape = RoundedCornerShape(14.dp)
    Column(
        Modifier
            .padding(start = BotShelfLayout.inset - 4.dp)
            .menuGlass(shape, c.bgPrimary, c.textPrimary, 6.dp)
            .padding(horizontal = 12.dp, vertical = 6.dp)
            // Each bot's own label already says its name, place and status.
            .clearAndSetSemantics {},
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        when {
            hotIndex != null && hotIndex < bots.size -> {
                val bot = bots[hotIndex]
                val waiting = store.waitingCount(bot.id) > 0
                val working = store.workingCount(bot.id) > 0
                Text(shelfPositionLabel(bot.displayName, hotIndex, bots.size), color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
                Text(
                    botShelfStatusLine(waiting, working, null, offline = bot.status == "offline", isMain = bot.isMainBot),
                    color = c.textSecondary, fontSize = 12.sp, maxLines = 1,
                )
            }
            hotIndex != null || bots.isEmpty() ->
                Text("Create a bot", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
            else -> Text("Your bots · ${bots.size}", color = c.textSecondary, fontSize = 12.sp, maxLines = 1)
        }
    }
}

/**
 * The drawer's mode dial (iOS ModeDialButton, web PhoneModeWheel): the active mode as a round
 * filled icon with Bots' unread count. Tap closes the drawer, then opens the fan out of ☰ (D4),
 * so hold is never the only way to switch mode. Test id `modeDial`.
 */
@Composable
internal fun ModeDialButton(mode: ShellMode, botsWaiting: Int, onOpenFan: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        modifier = Modifier
            .size(44.dp)
            .clip(CircleShape)
            .testTag("modeDial")
            // The machine plays the medium haptic and closes the drawer.
            .clickable(role = Role.Button, onClickLabel = "Closes the menu and shows the modes", onClick = onOpenFan)
            .semantics {
                contentDescription = "${mode.label} mode. Switch mode${if (botsWaiting > 0) ", $botsWaiting waiting in Bots" else ""}"
            },
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.size(34.dp).clip(CircleShape).background(c.textPrimary), contentAlignment = Alignment.Center) {
            Icon(painter = painterResource(mode.icon), contentDescription = null, tint = c.bgPrimary, modifier = Modifier.size(17.dp))
        }
        if (botsWaiting > 0) ModeDialBadge(botsWaiting, Modifier.align(Alignment.TopEnd).offset(x = (-1).dp, y = 1.dp))
    }
}

/** The red unread count on the dial, the Bots card and the shelf (`phone-mode-badge`): 16dp, "9+" past nine. */
@Composable
internal fun ModeDialBadge(count: Int, modifier: Modifier = Modifier) {
    Box(
        modifier = modifier
            .defaultMinSize(minWidth = 16.dp, minHeight = 16.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(AllternitStatic.StatusError)
            .padding(horizontal = 4.dp)
            .clearAndSetSemantics {},
        contentAlignment = Alignment.Center,
    ) {
        Text(menuBadgeText(count), color = Color.White, fontSize = 10.sp, fontWeight = FontWeight.Bold, maxLines = 1)
    }
}
