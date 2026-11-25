package com.allternit.core.design

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.disabled
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch

/**
 * Hold to confirm: a 1 second press-and-hold capsule with a fill (iOS ACIHoldButton, web ConfirmMorph hold mode).
 * Releasing early cancels. A screen reader gets a plain activate action, since holding is not possible there.
 * Delete, remove and end actions use this, never a dialog.
 */
@Composable
fun HoldButton(
    title: String,
    testTag: String,
    modifier: Modifier = Modifier,
    danger: Boolean = false,
    enabled: Boolean = true,
    onHeld: () -> Unit,
) {
    val c = AllternitTheme.colors
    val progress = remember { Animatable(0f) }
    val scope = rememberCoroutineScope()
    val tint = if (danger) AllternitStatic.StatusError else c.textPrimary
    Box(
        modifier.fillMaxWidth().heightIn(min = 52.dp).clip(CircleShape).background(c.fillNeutral).border(1.dp, if (danger) tint else c.borderSubtle, CircleShape)
            .alpha(if (enabled) 1f else 0.45f)
            .semantics(mergeDescendants = true) {
                role = Role.Button
                if (enabled) onClick(label = title.replaceFirst("Hold to ", "").replaceFirstChar { it.uppercase() }) { onHeld(); true } else disabled()
            }
            .pointerInput(enabled) {
                if (!enabled) return@pointerInput
                detectTapGestures(onPress = {
                    val hold = scope.launch {
                        progress.animateTo(1f, tween(1000, easing = LinearEasing))
                        onHeld()
                        progress.snapTo(0f)
                    }
                    tryAwaitRelease()
                    hold.cancel()
                    scope.launch { progress.animateTo(0f, tween(150)) }
                })
            }.testTag(testTag),
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.align(Alignment.CenterStart).fillMaxHeight().fillMaxWidth(progress.value).background(tint.copy(alpha = 0.18f)))
        Text(title, color = tint, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
    }
}
