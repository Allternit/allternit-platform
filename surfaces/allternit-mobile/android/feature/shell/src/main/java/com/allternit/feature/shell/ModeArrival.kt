package com.allternit.feature.shell

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.dp
import com.allternit.core.design.PolishGlass
import com.allternit.core.design.PolishHaptic
import com.allternit.core.design.rememberReducedMotion
import com.allternit.feature.bots.BotAvatar
import com.allternit.feature.bots.BotModeStore
import com.allternit.feature.bots.BotRecord
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** Arrival numbers from the polish contract (web ARRIVAL / TEAM, iOS ModeArrivalView). */
private object ArrivalSpec {
    const val SHOW_MS = 600L
    const val SETTLE_MS = 1600L
    const val IDLE_MS = 2300L
    const val TEAM_RESET_MS = 1100L
    const val TEAM_BASE_DELAY_S = 0.08f
    const val TEAM_STEP_S = 0.07f
    /** team spring 260 / 24 as a damping ratio: 24 / (2 * sqrt(260)). */
    const val TEAM_DAMPING = 0.744f
    const val TEAM_STIFFNESS = 260f
    /** arrivalBig spring 200 / 22 as a damping ratio: 22 / (2 * sqrt(200)). */
    const val BIG_DAMPING = 0.778f
    const val BIG_STIFFNESS = 200f
    /** An armed team arrival that never found the roster ready expires (web TEAM_ARM_TTL_MS). */
    const val TEAM_ARM_TTL_MS = 4000L
    fun teamDelayMs(i: Int): Long = ((TEAM_BASE_DELAY_S + i * TEAM_STEP_S) * 1000).toLong()
}

private fun botColor(bot: BotRecord?, fallback: Color): Color {
    val hex = bot?.accentHex?.removePrefix("#") ?: return fallback
    return runCatching { Color(("FF$hex").takeLast(8).toLong(16)) }.getOrDefault(fallback)
}

/** The shell keeps one of these: entering Bots arms the team arrival, picking a bot runs the bot arrival. */
private class ArrivalState {
    var teamArmedAt by mutableLongStateOf(0L)
    var teamPlaying by mutableStateOf(false)
    var botId by mutableStateOf<String?>(null)
    /** wash, show, settle, idle */
    var botPhase by mutableStateOf("idle")
}

/**
 * Arrivals (web W13, iOS I13). Team arrival: entering Bots with no bot picked, the team flies up
 * from the orb with a 1.1 s colour wash and a light tick per landing. Bot arrival: a bot opened
 * from the shell (a link, a shelf pick, a new bot) gets a wash in its colour at 0 ms, rises out
 * of a blur at spring 200/22 (600 ms, soft tick), settles toward the header (1600 ms, light tick)
 * and is idle at 2300 ms. Both wait while a mode intro is open (ModeIntroGate). Reduced motion
 * draws nothing. The overlay never takes touches and is hidden from accessibility.
 */
@Composable
internal fun ModeArrivals(mode: ShellMode, onBotsHome: Boolean, openBotId: String?, botsStore: BotModeStore) {
    val reduced = rememberReducedMotion()
    val view = LocalView.current
    val state = remember { ArrivalState() }
    val hold = ModeIntroGate.open

    // Entering Bots arms the team arrival (also the first open in Bots).
    var prevMode by remember { mutableStateOf<ShellMode?>(null) }
    LaunchedEffect(mode) {
        if (mode == ShellMode.Bots && prevMode != ShellMode.Bots) state.teamArmedAt = System.currentTimeMillis()
        prevMode = mode
    }

    // Team arrival: plays once the roster is loaded, no intro is open and no bot is picked.
    val team = botsStore.bots.take(6)
    LaunchedEffect(state.teamArmedAt, hold, onBotsHome, openBotId, team.size, botsStore.agentsLoading) {
        val armed = state.teamArmedAt
        if (armed == 0L || hold || mode != ShellMode.Bots || !onBotsHome || openBotId != null) return@LaunchedEffect
        if (botsStore.agentsLoading || team.isEmpty()) {
            if (System.currentTimeMillis() - armed > ArrivalSpec.TEAM_ARM_TTL_MS) state.teamArmedAt = 0L
            return@LaunchedEffect
        }
        state.teamArmedAt = 0L
        if (reduced) return@LaunchedEffect
        state.teamPlaying = true
        team.indices.forEach { i ->
            launch {
                delay(ArrivalSpec.teamDelayMs(i) + 320)
                PolishHaptic.perform(view, PolishHaptic.Kind.Light)
            }
        }
        delay(maxOf(ArrivalSpec.TEAM_RESET_MS, ArrivalSpec.teamDelayMs(team.size - 1) + 700))
        state.teamPlaying = false
    }

    // Bot arrival: a different bot opened by the shell. The first value seen does not play.
    var prevBot by remember { mutableStateOf(openBotId) }
    LaunchedEffect(openBotId, hold) {
        val changed = prevBot != openBotId
        prevBot = openBotId
        if (openBotId == null || !changed || hold || reduced || mode != ShellMode.Bots) { state.botPhase = "idle"; state.botId = openBotId; return@LaunchedEffect }
        state.botId = openBotId
        state.botPhase = "wash"
        try {
            delay(ArrivalSpec.SHOW_MS); state.botPhase = "show"; PolishHaptic.perform(view, PolishHaptic.Kind.Soft)
            delay(ArrivalSpec.SETTLE_MS - ArrivalSpec.SHOW_MS); state.botPhase = "settle"; PolishHaptic.perform(view, PolishHaptic.Kind.Light)
            delay(ArrivalSpec.IDLE_MS - ArrivalSpec.SETTLE_MS)
        } finally {
            // Cut short by another pick or leaving: land, no half state.
            state.botPhase = "idle"
        }
    }

    if (mode != ShellMode.Bots || reduced) return
    if (state.teamPlaying) TeamArrivalLayer(team)
    val bot = botsStore.agents.firstOrNull { it.id == state.botId }
    if (bot != null && state.botPhase != "idle") BotArrivalLayer(bot, state.botPhase)
}

@Composable
private fun TeamArrivalLayer(team: List<BotRecord>) {
    BoxWithConstraints(Modifier.fillMaxSize().clearAndSetSemantics { }.testTag("teamArrival")) {
        val size = 48.dp
        val gap = 22.dp
        val rowTop = 116.dp
        val fromY = maxHeight - rowTop - 120.dp
        val fade = remember { Animatable(1f) }
        LaunchedEffect(Unit) {
            delay(ArrivalSpec.TEAM_RESET_MS - 250)
            fade.animateTo(0f, tween(250))
        }
        Box(Modifier.fillMaxSize().graphicsLayer { alpha = fade.value }) {
            team.forEachIndexed { i, bot ->
                val color = botColor(bot, PolishGlass.Coral)
                val wash = remember { Animatable(0f) }
                val fly = remember { Animatable(0f) }
                LaunchedEffect(Unit) {
                    delay(ArrivalSpec.teamDelayMs(i))
                    launch { wash.animateTo(1f, tween(450)); wash.animateTo(0f, tween(650)) }
                    fly.animateTo(1f, spring(ArrivalSpec.TEAM_DAMPING, ArrivalSpec.TEAM_STIFFNESS))
                }
                val x = 20.dp + (size + gap) * i
                val centerX = maxWidth / 2 - size / 2
                Box(
                    Modifier.offset(x - 40.dp, rowTop - 60.dp).size(size + 80.dp, 170.dp)
                        .graphicsLayer { alpha = wash.value }
                        .background(Brush.radialGradient(listOf(color.copy(alpha = 0.32f), Color.Transparent))),
                )
                Box(
                    Modifier
                        .offset(centerX + (x - centerX) * fly.value, rowTop + fromY * (1f - fly.value))
                        .graphicsLayer {
                            val s = 0.3f + 0.7f * fly.value
                            scaleX = s; scaleY = s; alpha = fly.value.coerceIn(0f, 1f)
                        },
                ) { BotAvatar(bot, size) }
            }
        }
    }
}

@Composable
private fun BotArrivalLayer(bot: BotRecord, phase: String) {
    val color = botColor(bot, Color(0xFF8A8A8E))
    val wash = remember { Animatable(0f) }
    val big = remember { Animatable(0f) }
    LaunchedEffect(Unit) { wash.animateTo(1f, tween(600)) }
    LaunchedEffect(phase) {
        if (phase == "show") big.animateTo(1f, spring(ArrivalSpec.BIG_DAMPING, ArrivalSpec.BIG_STIFFNESS))
        if (phase == "settle") { launch { wash.animateTo(0f, tween(400)) }; big.animateTo(2f, tween(450)) }
    }
    BoxWithConstraints(Modifier.fillMaxSize().clearAndSetSemantics { }.testTag("botArrival")) {
        Box(
            Modifier.fillMaxWidth().height(maxHeight * 0.55f)
                .graphicsLayer { alpha = wash.value; scaleY = 0.3f + 0.7f * wash.value; transformOrigin = androidx.compose.ui.graphics.TransformOrigin(0.5f, 0f) }
                .background(Brush.verticalGradient(listOf(color.copy(alpha = 0.5f), color.copy(alpha = 0.16f), Color.Transparent))),
        )
        val v = big.value
        Box(
            Modifier.align(Alignment.TopCenter).statusBarsPadding().offset(y = 120.dp)
                .graphicsLayer {
                    // 0 to 1: rises from 90 dp below at 55% scale and fades in; 1 to 2: shrinks and lifts into the header.
                    val rise = v.coerceIn(0f, 1f)
                    val land = (v - 1f).coerceIn(0f, 1f)
                    alpha = rise * (1f - land)
                    val s = (0.55f + 0.45f * rise) * (1f - 0.8f * land)
                    scaleX = s; scaleY = s
                    translationY = (1f - rise) * 90.dp.toPx() - land * 130.dp.toPx()
                },
        ) { BotAvatar(bot, 124.dp) }
    }
}
