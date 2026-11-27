package com.allternit.feature.shell

import android.view.View
import androidx.compose.ui.composed
import androidx.compose.runtime.withFrameNanos
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.spring
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.AwaitPointerEventScope
import androidx.compose.ui.input.pointer.PointerInputChange
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionOnScreen
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Popup
import androidx.compose.ui.window.PopupProperties
import com.allternit.core.design.AllternitColors
import com.allternit.core.design.AllternitTheme
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.min
import kotlin.math.roundToInt

private fun <T> lensSpec(reduced: Boolean, ratio: Float, stiffness: Float) =
    if (reduced) snap<T>() else spring<T>(ratio, stiffness)

/**
 * Liquid-lens scrub state (Option B, 2026-11-26): one glass lens that follows the finger across the pill.
 * Item bounds are measured on screen because the pill lives in a Popup window and the press may start on the
 * orb in the main window. All x values here are screen px except [lensX], which is relative to the pill content.
 */
private class LensScrub(
    private val scope: CoroutineScope,
    private val view: View,
    private val reduced: Boolean,
    val dp: Float,
) {
    private val n = ModeHubSpec.modes.size
    val lensX = Animatable(0f)
    val lensW = Animatable(0f)
    val swell = Animatable(1f)
    val stretch = Animatable(1f)
    var hovered by mutableIntStateOf(-1)
    var scrubbing by mutableStateOf(false)
    var placed by mutableStateOf(false)
    var currentIndex = 0

    private val left = FloatArray(n)
    private val right = FloatArray(n)
    private val known = BooleanArray(n)
    private var originX = 0f
    private var originKnown = false
    private var lastX = 0f

    private val followSpec get() = lensSpec<Float>(reduced, ModeHubSpec.FOLLOW_DAMPING_RATIO, ModeHubSpec.FOLLOW_STIFFNESS)
    private val swellSpec get() = lensSpec<Float>(reduced, ModeHubSpec.SWELL_DAMPING_RATIO, ModeHubSpec.SWELL_STIFFNESS)
    private val jellySpec get() = lensSpec<Float>(reduced, ModeHubSpec.JELLY_DAMPING_RATIO, ModeHubSpec.JELLY_STIFFNESS)
    private val snapSpec get() = lensSpec<Float>(reduced, ModeHubSpec.LENS_DAMPING_RATIO, ModeHubSpec.LENS_STIFFNESS)
    val stretchSpec get() = spring<Float>(ModeHubSpec.STRETCH_DAMPING_RATIO, ModeHubSpec.STRETCH_STIFFNESS)

    private fun measured() = originKnown && known.all { it }
    private fun width(i: Int) = right[i] - left[i]

    private fun indexAt(x: Float): Int {
        for (i in 0 until n - 1) if (x < (right[i] + left[i + 1]) / 2f) return i
        return n - 1
    }

    fun onOrigin(x: Float) { originX = x; originKnown = true; tryPlace() }
    fun onItem(i: Int, l: Float, r: Float) { left[i] = l; right[i] = r; known[i] = true; tryPlace() }

    private fun tryPlace() {
        if (placed || !measured()) return
        placed = true
        val i = if (scrubbing) indexAt(lastX) else currentIndex
        scope.launch {
            lensW.snapTo(width(i))
            lensX.snapTo(left[i] - originX)
        }
        if (scrubbing) { hovered = i; move(lastX) }
    }

    fun reset() {
        placed = false; originKnown = false; hovered = -1; scrubbing = false
        known.fill(false)
        scope.launch { swell.snapTo(1f); stretch.snapTo(1f) }
    }

    private fun follow(x: Float, i: Int): Float {
        val w = width(i)
        val over = ModeHubSpec.LENS_OVERSHOOT_DP * dp
        val minX = left[0] - originX - over
        val maxX = right[n - 1] - originX - w + over
        return (x - originX - w / 2f).coerceIn(minX, maxX)
    }

    fun begin(x: Float) {
        scrubbing = true
        HubHaptic.Light.play(view)
        if (!reduced) scope.launch { swell.animateTo(ModeHubSpec.LENS_PRESS_SCALE, swellSpec) }
        move(x)
    }

    fun move(x: Float) {
        lastX = x
        if (!placed) return
        val i = indexAt(x)
        if (hovered != i) {
            if (hovered != -1) HubHaptic.Selection.play(view)
            hovered = i
            val w = width(i)
            scope.launch { lensW.animateTo(w, snapSpec) }
        }
        val target = follow(x, i)
        scope.launch { lensX.animateTo(target, followSpec) }
    }

    /** Lift: snap the lens onto the item, jelly the swell back, pick after [ModeHubSpec.PICK_SETTLE_MILLIS]. */
    fun end(x: Float, onPicked: (Int) -> Unit) {
        lastX = x
        if (!placed) { cancel(); return }
        val i = indexAt(x)
        scrubbing = false
        hovered = i
        HubHaptic.Medium.play(view)
        val w = width(i)
        val target = left[i] - originX
        scope.launch { lensW.animateTo(w, snapSpec) }
        scope.launch { lensX.animateTo(target, snapSpec) }
        scope.launch { swell.animateTo(1f, jellySpec) }
        scope.launch {
            if (!reduced) delay(ModeHubSpec.PICK_SETTLE_MILLIS)
            onPicked(i)
        }
    }

    /** The touch was cancelled: settle back on the current mode without a pick. */
    fun cancel() {
        scrubbing = false
        hovered = -1
        scope.launch { swell.animateTo(1f, jellySpec) }
        if (placed) {
            val i = currentIndex
            val w = width(i)
            val target = left[i] - originX
            scope.launch { lensW.animateTo(w, snapSpec) }
            scope.launch { lensX.animateTo(target, snapSpec) }
        }
    }
}

/**
 * One touch from down to up, shared by the orb (main window) and the pill (Popup window).
 * Orb: a still tap runs [onTap]; a hold of [ModeHubSpec.HOLD_MILLIS] with travel under the slide threshold runs
 * [onHold]; a slide past [ModeHubSpec.SLIDE_START_DP] calls [onOpenSlide] and keeps the same pointer scrubbing.
 * Pill: scrubbing starts at once; a hold without travel still runs [onHold] (Shelves).
 */
private suspend fun AwaitPointerEventScope.scrubGesture(
    down: PointerInputChange,
    fromOrb: Boolean,
    scrub: LensScrub,
    scope: CoroutineScope,
    view: View,
    slopPx: Float,
    onHold: (() -> Unit)?,
    toScreenX: (Float) -> Float,
    onPressed: (Boolean) -> Unit,
    onTap: () -> Unit,
    onOpenSlide: () -> Unit,
    onPick: (Int) -> Unit,
) {
    val start = down.position
    var scrubbing = !fromOrb
    var held = false
    var moved = false
    var up = false
    var lastX = start.x
    var finished = false
    if (fromOrb) onPressed(true) else scrub.begin(toScreenX(start.x))
    val holdJob: Job? = if (onHold == null) null else scope.launch {
        delay(ModeHubSpec.HOLD_MILLIS)
        if (!moved && !held) {
            held = true
            if (fromOrb) onPressed(false) else scrub.cancel()
            HubHaptic.Medium.play(view)
            onHold()
        }
    }
    try {
        while (true) {
            val ev = awaitPointerEvent()
            val ch = ev.changes.firstOrNull { it.id == down.id } ?: break
            lastX = ch.position.x
            if (!ch.pressed) { up = true; ch.consume(); break }
            if (!scrubbing && ch.isConsumed) break
            val d = ch.position - start
            if (!moved && hypot(d.x, d.y) > slopPx) { moved = true; holdJob?.cancel() }
            if (!held) {
                if (fromOrb && !scrubbing && abs(d.x) > slopPx) {
                    scrubbing = true
                    onPressed(false)
                    onOpenSlide()
                    scrub.begin(toScreenX(ch.position.x))
                } else if (scrubbing) {
                    scrub.move(toScreenX(ch.position.x))
                }
                if (scrubbing) ch.consume()
            }
        }
        holdJob?.cancel()
        when {
            held -> onPressed(false)
            scrubbing -> {
                if (up) scrub.end(toScreenX(lastX), onPick) else scrub.cancel()
                onPressed(false)
            }
            else -> {
                onPressed(false)
                if (up && fromOrb) onTap()
            }
        }
        finished = true
    } finally {
        holdJob?.cancel()
        if (!finished) {
            onPressed(false)
            if (scrubbing && !held) scrub.cancel()
        }
    }
}

private fun Modifier.orbKeys(onHold: (() -> Unit)?, onTap: () -> Unit): Modifier = composed {
    val hold = rememberUpdatedState(onHold)
    val tap = rememberUpdatedState(onTap)
    onKeyEvent {
        when {
            it.type == KeyEventType.KeyUp && (it.key == Key.Enter || it.key == Key.Spacebar || it.key == Key.DirectionCenter) -> { tap.value(); true }
            it.type == KeyEventType.KeyUp && it.key == Key.Menu -> hold.value?.let { h -> h(); true } ?: false
            else -> false
        }
    }.focusable()
}

/**
 * The mode orb (CONTRACT.md "New orb" sibling; mock `Combo`, phone variant): a 36dp glass circle showing the
 * current mode's icon. Tap opens a pill of the 4 modes ([expanded]); tapping a mode picks it ([onPick]) and
 * the host collapses the pill. Hold 450 ms on the orb or the open pill opens the hub sheet ([onOpenHub]);
 * a null [onOpenHub] is tap-only (the drawer's switcher: Shelves opens only from the composer or ACI orb).
 *
 * Liquid lens (Option B, 2026-11-26, Eoj OK): press the orb and slide to open the pill and scrub in one move;
 * a glass lens follows the finger, swells, stretches and magnifies the item under it; lifting picks, and
 * [onPick] runs [ModeHubSpec.PICK_SETTLE_MILLIS] later so the settle is seen. The gesture lives on the outer
 * Box, which outlasts the orb node and the Popup.
 *
 * The host owns [expanded] so a tap outside (or the hub opening) can collapse it. Hidden in the HUD
 * composer and in the Bots-only app: the host does not compose it there.
 *
 * Test ids `modeOrb`, `modeOrbPill`, `modeOrbPill<Mode>`. TalkBack: activate expands; custom action "Show all modes".
 */
@Composable
internal fun ModeOrb(
    mode: ShellMode,
    expanded: Boolean,
    onExpandedChange: (Boolean) -> Unit,
    onPick: (ShellMode) -> Unit,
    onOpenHub: (() -> Unit)?,
    modifier: Modifier = Modifier,
    botsWaiting: Int = 0,
    /** The bot Bots mode is focused on (page or chat); its face replaces the mode icon. A bot with no avatar shows Gizzi (web W14, iOS I14). Null shows the mode icon. */
    focusedBot: com.allternit.feature.bots.BotRecord? = null,
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val reduced = rememberHubReducedMotion()
    val density = LocalDensity.current.density
    val scope = rememberCoroutineScope()
    var pressed by remember { mutableStateOf(false) }
    val scale by animateFloatAsState(
        targetValue = if (pressed && !reduced) ModeHubSpec.PRESS_SCALE else 1f,
        animationSpec = if (reduced) snap() else spring(ModeHubSpec.ORB_DAMPING_RATIO, ModeHubSpec.ORB_STIFFNESS),
        label = "modeOrbPress",
    )
    val scrub = remember(scope, view, reduced, density) { LensScrub(scope, view, reduced, density) }
    scrub.currentIndex = ModeHubSpec.modes.indexOf(mode).coerceAtLeast(0)
    val shape = CircleShape
    val glass = Modifier
        .shadow(6.dp, shape, ambientColor = Color.Black.copy(alpha = 0.10f), spotColor = Color.Black.copy(alpha = 0.10f))
        .clip(shape)
        .background(c.bgPrimary.copy(alpha = 0.94f))
        .border(BorderStroke(1.dp, c.borderSubtle), shape)

    val expandedNow = rememberUpdatedState(expanded)
    val holdNow = rememberUpdatedState(onOpenHub)
    val expandedChange = rememberUpdatedState(onExpandedChange)
    val pickNow = rememberUpdatedState(onPick)
    var boxCoords by remember { mutableStateOf<LayoutCoordinates?>(null) }
    val openHub = onOpenHub

    Box(
        modifier = modifier
            .onGloballyPositioned { boxCoords = it }
            // Before graphicsLayer, so local x is unscaled. Survives the orb node leaving and the Popup opening.
            .pointerInput(Unit) {
                val slop = ModeHubSpec.SLIDE_START_DP.dp.toPx()
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    // The 44dp placeholder stays under the Popup while open: ignore presses there.
                    if (expandedNow.value) return@awaitEachGesture
                    scrubGesture(
                        down = down, fromOrb = true, scrub = scrub, scope = scope, view = view, slopPx = slop,
                        onHold = holdNow.value,
                        toScreenX = { x -> (boxCoords?.takeIf { it.isAttached }?.positionOnScreen()?.x ?: 0f) + x },
                        onPressed = { pressed = it },
                        onTap = { HubHaptic.Light.play(view); expandedChange.value(true) },
                        onOpenSlide = { expandedChange.value(true) },
                        onPick = { i -> pickNow.value(ModeHubSpec.modes[i]) },
                    )
                }
            }
            .graphicsLayer { scaleX = scale; scaleY = scale },
    ) {
        if (!expanded) {
            Box(
                modifier = Modifier
                    .size(44.dp)
                    .then(glass)
                    .testTag("modeOrb")
                    .orbKeys(openHub) { HubHaptic.Light.play(view); onExpandedChange(true) }
                    .semantics {
                        role = Role.Button
                        contentDescription = if (focusedBot != null) "Mode: ${mode.label}, ${focusedBot.displayName}. Change mode" else "Mode: ${mode.label}. Change mode"
                        onClick(label = "Shows the modes") { onExpandedChange(true); true }
                        if (onOpenHub != null) customActions = listOf(CustomAccessibilityAction("Show all modes") { onOpenHub(); true })
                    },
                contentAlignment = Alignment.Center,
            ) {
                if (focusedBot != null) {
                    com.allternit.feature.bots.BotAvatar(focusedBot, size = 30.dp)
                } else {
                    Icon(painterResource(mode.icon), contentDescription = null, tint = mode.accent, modifier = Modifier.size(16.dp))
                }
            }
            if (mode != ShellMode.Bots && botsWaiting > 0) {
                ModeDialBadge(botsWaiting, Modifier.align(Alignment.TopEnd).padding(end = 0.dp))
            }
        } else {
            Box(Modifier.size(44.dp))
            Popup(alignment = Alignment.TopStart, onDismissRequest = { onExpandedChange(false) }, properties = PopupProperties(focusable = true)) {
                OrbPill(
                    mode = mode,
                    reduced = reduced,
                    botsWaiting = botsWaiting,
                    scrub = scrub,
                    glass = glass,
                    onOpenHub = onOpenHub,
                    onPick = onPick,
                )
            }
        }
    }
}

@Composable
private fun OrbPill(
    mode: ShellMode,
    reduced: Boolean,
    botsWaiting: Int,
    scrub: LensScrub,
    glass: Modifier,
    onOpenHub: (() -> Unit)?,
    onPick: (ShellMode) -> Unit,
) {
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    val c = AllternitTheme.colors
    val dark = c == AllternitColors.Dark
    val holdNow = rememberUpdatedState(onOpenHub)
    val pickNow = rememberUpdatedState(onPick)
    var origin by remember { mutableStateOf<LayoutCoordinates?>(null) }

    DisposableEffect(scrub) { onDispose { scrub.reset() } }

    // Settle the lens on the current mode when the pill opens without a finger on it.
    LaunchedEffect(scrub.placed, mode) {
        if (scrub.placed && !scrub.scrubbing) scrub.cancel()
    }

    // Stretch from the lens speed, smoothed on the stretch spring (web: useVelocity -> useSpring).
    LaunchedEffect(scrub, reduced) {
        if (reduced) return@LaunchedEffect
        var lastX = scrub.lensX.value
        var lastT = androidx.compose.runtime.withFrameNanos { it }
        while (true) {
            val t = androidx.compose.runtime.withFrameNanos { it }
            val x = scrub.lensX.value
            val dt = (t - lastT) / 1_000_000_000f
            if (dt > 0f) {
                val v = abs(x - lastX) / scrub.dp / dt
                val target = 1f + min(v / ModeHubSpec.LENS_STRETCH_VELOCITY, ModeHubSpec.LENS_STRETCH_MAX)
                if (abs(target - scrub.stretch.targetValue) > 0.002f) launch { scrub.stretch.animateTo(target, scrub.stretchSpec) }
            }
            lastX = x; lastT = t
        }
    }

    Box(
        modifier = Modifier
            .then(glass)
            .padding(4.dp)
            .testTag("modeOrbPill")
            .onGloballyPositioned { coords ->
                origin = coords
                scrub.onOrigin(coords.positionOnScreen().x)
            }
            .pointerInput(Unit) {
                val slop = ModeHubSpec.SLIDE_START_DP.dp.toPx()
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    down.consume()
                    scrubGesture(
                        down = down, fromOrb = false, scrub = scrub, scope = scope, view = view, slopPx = slop,
                        onHold = holdNow.value,
                        toScreenX = { x -> (origin?.takeIf { it.isAttached }?.positionOnScreen()?.x ?: 0f) + x },
                        onPressed = {},
                        onTap = {},
                        onOpenSlide = {},
                        onPick = { i -> pickNow.value(ModeHubSpec.modes[i]) },
                    )
                }
            },
    ) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = androidx.compose.foundation.layout.Arrangement.spacedBy(2.dp),
        ) {
            ModeHubSpec.modes.forEachIndexed { i, m ->
                val lit = if (scrub.hovered >= 0) scrub.hovered == i else m == mode
                PillMode(
                    mode = m,
                    current = m == mode,
                    active = lit,
                    magnify = scrub.scrubbing && scrub.hovered == i,
                    reduced = reduced,
                    badge = if (m == ShellMode.Bots) botsWaiting else 0,
                    onBounds = { l, r -> scrub.onItem(i, l, r) },
                    onHold = onOpenHub,
                    onTap = { HubHaptic.Light.play(view); onPick(m) },
                )
            }
        }
        // One lens above the items: moved with graphicsLayer, never by relayout of the pill.
        Box(
            modifier = Modifier
                .align(Alignment.TopStart)
                .layout { measurable, _ ->
                    val w = scrub.lensW.value.roundToInt().coerceAtLeast(0)
                    val h = 44.dp.roundToPx()
                    val p = measurable.measure(Constraints.fixed(w, h))
                    layout(0, 0) { p.place(0, 0) }
                }
                .graphicsLayer {
                    alpha = if (scrub.placed) 1f else 0f
                    translationX = scrub.lensX.value
                    val st = scrub.stretch.value
                    val sw = scrub.swell.value
                    scaleX = sw * st
                    scaleY = sw * (1f - (st - 1f) * ModeHubSpec.LENS_STRETCH_SQUASH)
                }
                .shadow(
                    6.dp, CircleShape,
                    ambientColor = Color.Black.copy(alpha = if (dark) 0.35f else 0.16f),
                    spotColor = Color.Black.copy(alpha = if (dark) 0.35f else 0.16f),
                )
                .clip(CircleShape)
                .background(Color.White.copy(alpha = if (dark) 0.10f else 0.22f))
                .drawBehind {
                    drawRect(
                        Brush.radialGradient(
                            0f to Color.White.copy(alpha = 0.55f),
                            0.45f to Color.Transparent,
                            center = Offset(size.width * 0.28f, size.height * 0.12f),
                            radius = size.width * 0.6f,
                        ),
                    )
                }
                .border(BorderStroke(1.dp, Color.White.copy(alpha = if (dark) 0.14f else 0.55f)), CircleShape),
        )
    }
}

@Composable
private fun PillMode(
    mode: ShellMode,
    current: Boolean,
    active: Boolean,
    magnify: Boolean,
    reduced: Boolean,
    badge: Int,
    onBounds: (Float, Float) -> Unit,
    onHold: (() -> Unit)?,
    onTap: () -> Unit,
) {
    val c = AllternitTheme.colors
    val mag by animateFloatAsState(
        targetValue = if (magnify && !reduced) ModeHubSpec.LENS_MAGNIFY_SCALE else 1f,
        animationSpec = if (reduced) snap() else spring(ModeHubSpec.LENS_DAMPING_RATIO, ModeHubSpec.LENS_STIFFNESS),
        label = "pillMagnify",
    )
    Box(
        modifier = Modifier
            .height(44.dp)
            .defaultMinSize(minWidth = 44.dp)
            .testTag("modeOrbPill${mode.name}")
            .onGloballyPositioned { onBounds(it.positionOnScreen().x, it.positionOnScreen().x + it.size.width) }
            .orbKeys(onHold, onTap)
            .semantics {
                role = Role.Button
                selected = current
                contentDescription = mode.label
                onClick(label = "Switches to ${mode.label}") { onTap(); true }
            }
            .graphicsLayer { scaleX = mag; scaleY = mag }
            .padding(horizontal = 11.dp),
        contentAlignment = Alignment.Center,
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = androidx.compose.foundation.layout.Arrangement.spacedBy(6.dp)) {
            Icon(painterResource(mode.icon), null, tint = if (active) mode.accent else c.textSecondary, modifier = Modifier.size(15.dp))
            Text(
                mode.label,
                color = if (active) c.textPrimary else c.textSecondary,
                fontSize = 12.5.sp,
                fontWeight = if (current) FontWeight.SemiBold else FontWeight.Medium,
                maxLines = 1,
            )
        }
        if (badge > 0) ModeDialBadge(badge, Modifier.align(Alignment.TopEnd))
    }
}
