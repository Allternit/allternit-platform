package com.allternit.feature.shell

import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animateFloatAsState
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
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
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
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import kotlin.math.cos
import kotlin.math.roundToInt
import kotlin.math.sin

/**
 * The ☰ hold machine, cut down from iOS MenuHoldMachine/MenuHoldController (no bots shelf yet):
 * tap ☰ opens the drawer; hold [HOLD_MILLIS] fans the modes out of it; slide onto a card and
 * let go to switch; a hold released without moving keeps the fan open for a tap. [anchor] is
 * ☰'s bounds in root pixels (the shell root fills the window).
 */
@Stable
internal class MenuHoldState {
    var fanOpen by mutableStateOf(false)
        private set
    var hot by mutableStateOf<ShellMode?>(null)
    var anchor by mutableStateOf(Rect.Zero)

    fun openFan() {
        hot = null
        fanOpen = true
    }

    fun closeFan() {
        hot = null
        fanOpen = false
    }

    /** The fan card under [point] (root px), with a 6dp margin like iOS `insetBy(-6)`. */
    fun target(point: Offset, widthPx: Float, density: Density): ShellMode? {
        val margin = with(density) { 6.dp.toPx() }
        return ModeFanLayout.cardFrames(anchor, widthPx, density).withIndex()
            .firstOrNull { (_, r) -> r.inflate(margin).contains(point) }
            ?.let { ShellMode.fan[it.index] }
    }

    companion object {
        /** MenuHold.holdSeconds. */
        const val HOLD_MILLIS = 350L
    }
}

/** ModeFanLayout: a quarter circle of 60dp cards, radius 148dp, from below ☰ round to its right. */
internal object ModeFanLayout {
    val radius = 148.dp
    val card = 60.dp

    fun cardFrames(anchor: Rect, widthPx: Float, density: Density): List<Rect> = with(density) {
        val r = radius.toPx()
        val c = card.toPx()
        val edge = 8.dp.toPx()
        val drop = 16.dp.toPx()
        val count = ShellMode.fan.size
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
 * The ☰ button (iOS PhoneMenuButton, contract C8). Tap → [onOpen]. Hold → the mode fan.
 * Test id `phoneMenuButton`. TalkBack: activate opens the menu; one action per mode.
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
    val view = LocalView.current
    val density = LocalDensity.current
    // The gesture outlives recompositions (keyed on holdEnabled), so it reads the latest callbacks.
    val open by rememberUpdatedState(onOpen)
    val pick by rememberUpdatedState(onPick)
    Box(
        modifier = modifier
            .size(44.dp)
            .testTag("phoneMenuButton")
            .onGloballyPositioned { hold.anchor = it.boundsInRoot() }
            .semantics {
                role = Role.Button
                contentDescription = "Open menu"
                stateDescription = "${mode.label} mode"
                onClick(label = "Open menu") { onOpen(); true }
                customActions = ShellMode.fan.map { m ->
                    CustomAccessibilityAction("Switch to ${m.label}") { onPick(m); true }
                }
            }
            .pointerInput(holdEnabled) {
                val slop = 12.dp.toPx()
                awaitEachGesture {
                    val down = awaitFirstDown()
                    down.consume()
                    // true = released (tap), false = moved past slop (not ours), null = held.
                    val early: Boolean? = if (!holdEnabled) {
                        true.also { waitForUp(down.id) }
                    } else {
                        withTimeoutOrNull(MenuHoldState.HOLD_MILLIS) {
                            var result = false
                            while (true) {
                                val change = awaitPointerEvent().changes.firstOrNull { it.id == down.id }
                                if (change == null) break
                                if (!change.pressed) {
                                    change.consume()
                                    result = true
                                    break
                                }
                                if ((change.position - down.position).getDistance() > slop) break
                            }
                            result
                        }
                    }
                    when (early) {
                        true -> open()
                        false -> Unit
                        null -> {
                            Haptics.perform(view, Haptics.Kind.Tap)
                            hold.openFan()
                            val origin = hold.anchor.topLeft
                            var moved = false
                            while (true) {
                                val change = awaitPointerEvent().changes.firstOrNull { it.id == down.id } ?: break
                                change.consume()
                                if ((change.position - down.position).getDistance() > slop) moved = true
                                val target = hold.target(origin + change.position, view.width.toFloat(), density)
                                if (target != hold.hot && target != null) Haptics.perform(view, Haptics.Kind.Tap)
                                hold.hot = target
                                if (!change.pressed) {
                                    when {
                                        target != null -> pick(target)
                                        moved -> hold.closeFan()
                                        // A still hold leaves the fan open for a tap.
                                        else -> hold.hot = null
                                    }
                                    break
                                }
                            }
                        }
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

private suspend fun androidx.compose.ui.input.pointer.AwaitPointerEventScope.waitForUp(
    id: androidx.compose.ui.input.pointer.PointerId,
) {
    while (true) {
        val change = awaitPointerEvent().changes.firstOrNull { it.id == id } ?: return
        if (!change.pressed) {
            change.consume()
            return
        }
    }
}

/**
 * The fan over everything (iOS MenuOverlayHost + ModeFanView): a light scrim and one 60dp card
 * per mode, folding out of ☰ with a small stagger. Ids `modeFanCard<raw>`.
 */
@Composable
internal fun ModeFanOverlay(
    hold: MenuHoldState,
    current: ShellMode,
    onPick: (ShellMode) -> Unit,
) {
    val density = LocalDensity.current
    val open = hold.fanOpen
    val scrim by animateFloatAsState(if (open) 1f else 0f, tween(160), label = "fanScrim")
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val widthPx = with(density) { maxWidth.toPx() }
        if (open || scrim > 0.01f) {
            Box(
                Modifier
                    .fillMaxSize()
                    .graphicsLayer { alpha = scrim }
                    .background(Color.Black.copy(alpha = 0.08f))
                    .clickable(
                        interactionSource = remember { MutableInteractionSource() },
                        indication = null,
                        enabled = open,
                        onClickLabel = "Close modes",
                    ) { hold.closeFan() },
            )
        }
        val frames = ModeFanLayout.cardFrames(hold.anchor, widthPx, density)
        Box(Modifier.fillMaxSize().then(if (open) Modifier.semantics { contentDescription = "Switch mode" } else Modifier)) {
            ShellMode.fan.forEachIndexed { index, mode ->
                val t by animateFloatAsState(
                    targetValue = if (open) 1f else 0f,
                    animationSpec = if (open) {
                        tween(durationMillis = 240, delayMillis = index * 28, easing = FastOutSlowInEasing)
                    } else {
                        tween(durationMillis = 140, easing = FastOutSlowInEasing)
                    },
                    label = "fanCard$index",
                )
                if (t > 0.01f) {
                    val frame = frames[index]
                    val dx = hold.anchor.center.x - frame.center.x
                    val dy = hold.anchor.center.y - frame.center.y
                    FanCard(
                        mode = mode,
                        current = mode == current,
                        hot = hold.hot == mode,
                        enabled = open,
                        onClick = { onPick(mode) },
                        modifier = Modifier
                            .offset { IntOffset(frame.left.roundToInt(), frame.top.roundToInt()) }
                            .graphicsLayer {
                                // FanFold: half size, transparent, at ☰'s centre when folded.
                                val fold = 1f - t
                                translationX = dx * fold
                                translationY = dy * fold
                                alpha = t
                                val s = 0.5f + 0.5f * t
                                scaleX = s
                                scaleY = s
                            },
                    )
                }
            }
        }
    }
}

@Composable
private fun FanCard(
    mode: ShellMode,
    current: Boolean,
    hot: Boolean,
    enabled: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = AllternitTheme.colors
    val scale by animateFloatAsState(if (hot) 1.12f else 1f, tween(140), label = "fanHot")
    val shape = RoundedCornerShape(18.dp)
    Column(
        modifier = modifier
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
            }
            .size(ModeFanLayout.card)
            .shadow(12.dp, shape, ambientColor = Color.Black.copy(alpha = 0.12f), spotColor = Color.Black.copy(alpha = 0.12f))
            .clip(shape)
            .background(c.bgPrimary.copy(alpha = 0.95f))
            .border(BorderStroke(1.dp, c.textPrimary.copy(alpha = 0.11f)), shape)
            .testTag("modeFanCard${mode.raw}")
            .semantics {
                selected = current
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
}
