package com.allternit.feature.shell

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.spring
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.composed
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Tap runs [onTap]; holding for [ModeHubSpec.HOLD_MILLIS] runs [onHold] once (medium haptic) and the
 * release afterwards does nothing. The press scales to 0.94 (contract), except in reduced motion.
 */
internal fun Modifier.holdOrTap(
    @Suppress("UNUSED_PARAMETER") reduced: Boolean,
    onPressedChange: (Boolean) -> Unit,
    onHold: () -> Unit,
    onTap: () -> Unit,
): Modifier = composed {
    val view = LocalView.current
    val hold = rememberUpdatedState(onHold)
    val tap = rememberUpdatedState(onTap)
    val pressed = rememberUpdatedState(onPressedChange)
    pointerInput(Unit) {
        awaitEachGesture {
            awaitFirstDown()
            pressed.value(true)
            var cancelled = false
            val up = withTimeoutOrNull(ModeHubSpec.HOLD_MILLIS) {
                waitForUpOrCancellation().also { if (it == null) cancelled = true }
            }
            if (up == null && !cancelled) {
                // The timeout hit with the finger still down: the hold completes.
                HubHaptic.Medium.play(view)
                pressed.value(false)
                hold.value()
                waitForUpOrCancellation()
            } else {
                pressed.value(false)
                if (up != null) tap.value()
            }
        }
    }
}

/**
 * The mode orb (CONTRACT.md "New orb" sibling; mock `Combo`, phone variant): a 36dp glass circle showing the
 * current mode's icon. Tap opens a pill of the 4 modes ([expanded]); tapping a mode picks it ([onPick]) and
 * the host collapses the pill. Hold 450 ms on the orb or any pill item opens the hub sheet ([onOpenHub]).
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
    onOpenHub: () -> Unit,
    modifier: Modifier = Modifier,
    botsWaiting: Int = 0,
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val reduced = rememberHubReducedMotion()
    var pressed by remember { mutableStateOf(false) }
    val scale by animateFloatAsState(
        targetValue = if (pressed && !reduced) ModeHubSpec.PRESS_SCALE else 1f,
        animationSpec = if (reduced) snap() else spring(ModeHubSpec.ORB_DAMPING_RATIO, ModeHubSpec.ORB_STIFFNESS),
        label = "modeOrbPress",
    )
    val shape = CircleShape
    val glass = Modifier
        .shadow(6.dp, shape, ambientColor = Color.Black.copy(alpha = 0.10f), spotColor = Color.Black.copy(alpha = 0.10f))
        .clip(shape)
        .background(c.bgPrimary.copy(alpha = 0.94f))
        .border(BorderStroke(1.dp, c.borderSubtle), shape)

    Box(modifier = modifier.graphicsLayer { scaleX = scale; scaleY = scale }) {
        if (!expanded) {
            Box(
                modifier = Modifier
                    .size(36.dp)
                    .then(glass)
                    .testTag("modeOrb")
                    .holdOrTap(
                        reduced = reduced,
                        onPressedChange = { pressed = it },
                        onHold = { onOpenHub() },
                        onTap = { HubHaptic.Light.play(view); onExpandedChange(true) },
                    )
                    .semantics {
                        role = Role.Button
                        contentDescription = "Mode: ${mode.label}. Change mode"
                        onClick(label = "Shows the modes") { onExpandedChange(true); true }
                        customActions = listOf(CustomAccessibilityAction("Show all modes") { onOpenHub(); true })
                    },
                contentAlignment = Alignment.Center,
            ) {
                Icon(painterResource(mode.icon), contentDescription = null, tint = mode.accent, modifier = Modifier.size(16.dp))
            }
            if (mode != ShellMode.Bots && botsWaiting > 0) {
                ModeDialBadge(botsWaiting, Modifier.align(Alignment.TopEnd).padding(end = 0.dp))
            }
        } else {
            Row(
                modifier = Modifier
                    .then(glass)
                    .padding(4.dp)
                    .testTag("modeOrbPill"),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = androidx.compose.foundation.layout.Arrangement.spacedBy(2.dp),
            ) {
                ModeHubSpec.modes.forEach { m ->
                    PillMode(
                        mode = m,
                        current = m == mode,
                        reduced = reduced,
                        badge = if (m == ShellMode.Bots) botsWaiting else 0,
                        onPressedChange = { pressed = it },
                        onHold = onOpenHub,
                        onTap = { HubHaptic.Light.play(view); onPick(m) },
                    )
                }
            }
        }
    }
}

@Composable
private fun PillMode(
    mode: ShellMode,
    current: Boolean,
    reduced: Boolean,
    badge: Int,
    onPressedChange: (Boolean) -> Unit,
    onHold: () -> Unit,
    onTap: () -> Unit,
) {
    val c = AllternitTheme.colors
    val lens by animateColorAsState(
        targetValue = if (current) c.fillNeutral else Color.Transparent,
        animationSpec = if (reduced) snap() else spring(ModeHubSpec.LENS_DAMPING_RATIO, ModeHubSpec.LENS_STIFFNESS),
        label = "pillLens",
    )
    Box(
        modifier = Modifier
            .height(30.dp)
            .defaultMinSize(minWidth = 44.dp)
            .clip(CircleShape)
            .background(lens)
            .testTag("modeOrbPill${mode.name}")
            .holdOrTap(reduced, onPressedChange, onHold, onTap)
            .semantics {
                role = Role.Button
                selected = current
                contentDescription = mode.label
                onClick(label = "Switches to ${mode.label}") { onTap(); true }
            }
            .padding(horizontal = 11.dp),
        contentAlignment = Alignment.Center,
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = androidx.compose.foundation.layout.Arrangement.spacedBy(6.dp)) {
            Icon(painterResource(mode.icon), null, tint = if (current) mode.accent else c.textSecondary, modifier = Modifier.size(15.dp))
            Text(
                mode.label,
                color = if (current) c.textPrimary else c.textSecondary,
                fontSize = 12.5.sp,
                fontWeight = if (current) FontWeight.SemiBold else FontWeight.Medium,
                maxLines = 1,
            )
        }
        if (badge > 0) ModeDialBadge(badge, Modifier.align(Alignment.TopEnd))
    }
}
