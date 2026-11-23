package com.allternit.core.design

import android.provider.Settings
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
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
import androidx.compose.ui.draw.scale
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** Numbers from the mobile polish contract (intro sheet). */
object IntroSheetSpec {
    const val INSET_DP = 6
    const val RADIUS_DP = 38
    const val ENTER_FROM_DP = 560
    const val UNLOCK_MS = 1200L
    const val COLOR_FADE_MS = 350
    const val SPRING_STIFFNESS = 320f
    /** damping 32 as a ratio: 32 / (2 * sqrt(320)). */
    const val SPRING_DAMPING_RATIO = 0.894f
    val Scrim = Color(0x38141414)
    val Locked = Color(0xFFCFCFCF)
}

/** True when the system animation scale is off (Remove animations). */
@Composable
fun rememberReducedMotion(): Boolean {
    val resolver = LocalContext.current.contentResolver
    return remember {
        runCatching { Settings.Global.getFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }.getOrDefault(false)
    }
}

/**
 * First-visit sheet. Slides up on a spring (a fade under reduced motion) over a scrim.
 * [eyebrow] is a slot so the coral block-type label can be supplied by the caller
 * (BlockText lives in another lane); [hero] is the moving copy of the real tab.
 * Continue unlocks after 1.2 s, plays a medium haptic, then the sheet leaves and
 * [onContinue] runs (the caller marks the intro seen there).
 */
@Composable
fun IntroSheet(
    title: String,
    line: String,
    onContinue: () -> Unit,
    modifier: Modifier = Modifier,
    eyebrow: @Composable () -> Unit = {},
    hero: @Composable () -> Unit = {},
    continueLabel: String = "Continue",
) {
    val colors = AllternitTheme.colors
    val view = LocalView.current
    val reduced = rememberReducedMotion()
    val scope = rememberCoroutineScope()
    val progress = remember { Animatable(0f) } // 0 = off screen, 1 = settled
    var unlocked by remember { mutableStateOf(false) }
    var leaving by remember { mutableStateOf(false) }

    LaunchedEffect(Unit) {
        if (reduced) progress.animateTo(1f, tween(200))
        else progress.animateTo(1f, spring(IntroSheetSpec.SPRING_DAMPING_RATIO, IntroSheetSpec.SPRING_STIFFNESS))
    }
    LaunchedEffect(Unit) { delay(IntroSheetSpec.UNLOCK_MS); unlocked = true }

    fun leave() {
        if (leaving) return
        leaving = true
        PolishHaptic.perform(view, PolishHaptic.Kind.Medium)
        scope.launch {
            if (reduced) progress.animateTo(0f, tween(150))
            else progress.animateTo(0f, spring(IntroSheetSpec.SPRING_DAMPING_RATIO, IntroSheetSpec.SPRING_STIFFNESS))
            onContinue()
        }
    }

    Dialog(
        onDismissRequest = { /* Continue is the only way out, like the other surfaces */ },
        properties = DialogProperties(usePlatformDefaultWidth = false, dismissOnClickOutside = false, dismissOnBackPress = false),
    ) {
        Box(
            modifier
                .fillMaxSize()
                .background(IntroSheetSpec.Scrim.copy(alpha = IntroSheetSpec.Scrim.alpha * progress.value.coerceIn(0f, 1f))),
            contentAlignment = Alignment.BottomCenter,
        ) {
            Column(
                Modifier
                    .navigationBarsPadding()
                    .padding(IntroSheetSpec.INSET_DP.dp)
                    .fillMaxWidth()
                    .graphicsLayer {
                        if (reduced) alpha = progress.value.coerceIn(0f, 1f)
                        else translationY = (1f - progress.value) * IntroSheetSpec.ENTER_FROM_DP.dp.toPx()
                    }
                    .clip(RoundedCornerShape(IntroSheetSpec.RADIUS_DP.dp))
                    .background(colors.bgPrimary)
                    .padding(horizontal = 20.dp, vertical = 22.dp),
            ) {
                hero()
                Column(Modifier.padding(horizontal = 8.dp, vertical = 0.dp).padding(top = 16.dp)) {
                    eyebrow()
                    Text(
                        title,
                        color = colors.textPrimary, fontSize = 23.sp, lineHeight = 27.sp, fontWeight = FontWeight.Bold,
                        modifier = Modifier.padding(top = 6.dp, bottom = 8.dp).semantics { heading() },
                    )
                    Text(line, color = colors.textSecondary, fontSize = 14.sp, lineHeight = 20.sp)
                }
                Spacer20()
                ContinueButton(label = continueLabel, enabled = unlocked && !leaving, onClick = ::leave)
            }
        }
    }
}

@Composable
private fun Spacer20() = Box(Modifier.height(20.dp))

@Composable
private fun ContinueButton(label: String, enabled: Boolean, onClick: () -> Unit) {
    val colors = AllternitTheme.colors
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val bg by animateColorAsState(
        if (enabled) colors.textPrimary else IntroSheetSpec.Locked,
        tween(IntroSheetSpec.COLOR_FADE_MS), label = "continueBg",
    )
    Box(
        Modifier
            .fillMaxWidth()
            .height(52.dp)
            .scale(if (pressed && enabled) 0.97f else 1f)
            .clip(RoundedCornerShape(26.dp))
            .background(bg)
            .clickable(interactionSource = source, indication = null, enabled = enabled, role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = colors.bgPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
    }
}
