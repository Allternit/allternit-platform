package com.allternit.feature.shell

import android.content.Context
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.BlockText
import com.allternit.core.design.IntroKeys
import com.allternit.core.design.IntroSheet
import com.allternit.core.design.LocalIntroSeenStore
import com.allternit.core.design.PolishGlass
import com.allternit.core.design.rememberReducedMotion
import com.allternit.feature.bots.BotModeStore
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** True while a mode intro sheet is open. The arrivals (ModeArrival.kt) hold until it is false. */
internal object ModeIntroGate {
    var open by mutableStateOf(false)
}

private class ModeIntroCopy(val key: String, val eyebrow: String, val title: String, val line: String)

/** Verbatim from the polish contract (Home, Bots, Code, ACI). */
private fun copyFor(mode: ShellMode): ModeIntroCopy = when (mode) {
    ShellMode.Home -> ModeIntroCopy(IntroKeys.HOME, "HOME", "Start anything here.", "Chat, plan or hand off work. Change the brain whenever you want.")
    ShellMode.Bots -> ModeIntroCopy(IntroKeys.BOTS, "BOTS", "Your team keeps working when you leave.", "Tap a bot to talk to it, or describe a job and the team splits it up.")
    ShellMode.Code -> ModeIntroCopy(IntroKeys.CODE, "GIZZI CODE", "Describe it. Gizzi builds it.", "It runs on your computer or Allternit Cloud, and you steer it from your phone.")
    ShellMode.Aci -> ModeIntroCopy(IntroKeys.ACI, "ACI", "The agent uses the web for you.", "Give it a site and a goal. Watch it work, and take over any time.")
}

/** The Bots intro shares `botsIntroSeenAt` with "Meet your bots"; Meet also keeps this local copy. */
private fun meetSeenLocally(context: Context): Boolean =
    context.getSharedPreferences("allternit_shell", Context.MODE_PRIVATE).getString("allternit-bots-intro-seen", null) != null

/** Time to let the roster load before the Bots intro decides (web BOTS_SETTLE_MS). */
private const val BOTS_SETTLE_MS = 1500L

/**
 * First-visit intro sheets for the four modes (web W12, iOS I12). Watches the shell mode, so
 * the mode orb, the Shelves pick, a deep link and the first open all count. Home shows on the
 * first open; each other mode the first time it opens. Reads and writes through
 * [LocalIntroSeenStore] (null while signed out: no intro). A mode's intro is decided once per
 * session, so a closed one is never retried.
 */
@Composable
internal fun ModeIntroHost(mode: ShellMode, botsStore: BotModeStore) {
    val store = LocalIntroSeenStore.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val decided = remember(store) { mutableSetOf<ShellMode>() }
    var active by remember(store) { mutableStateOf<ShellMode?>(null) }
    var refreshed by remember(store) { mutableStateOf(false) }

    LaunchedEffect(store) {
        val s = store ?: return@LaunchedEffect
        runCatching { s.refresh() }
        refreshed = true
    }
    LaunchedEffect(active) { ModeIntroGate.open = active != null }
    androidx.compose.runtime.DisposableEffect(Unit) { onDispose { ModeIntroGate.open = false } }

    LaunchedEffect(store, refreshed, mode, botsStore.agentsLoading, botsStore.agents.size) {
        val s = store ?: return@LaunchedEffect
        if (!refreshed || active != null || mode in decided) return@LaunchedEffect
        val copy = copyFor(mode)
        if (s.isSeen(copy.key)) { decided += mode; return@LaunchedEffect }
        if (mode == ShellMode.Bots) {
            if (meetSeenLocally(context)) { decided += mode; return@LaunchedEffect }
            delay(BOTS_SETTLE_MS)
            if (botsStore.agentsLoading) return@LaunchedEffect // re-runs when the load ends
            // A main bot means "Meet your bots" owns the first visit and marks the shared key.
            decided += mode
            if (botsStore.agents.none { it.isMainBot }) active = mode
            return@LaunchedEffect
        }
        decided += mode
        active = mode
    }

    val shown = active ?: return
    val copy = copyFor(shown)
    IntroSheet(
        title = copy.title,
        line = copy.line,
        onContinue = {
            active = null
            scope.launch { store?.markSeen(copy.key) }
        },
        eyebrow = { BlockText(copy.eyebrow, height = 10.dp, color = PolishGlass.Coral) },
        hero = {
            when (shown) {
                ShellMode.Home -> HomeIntroHero()
                ShellMode.Bots -> BotsIntroHero()
                ShellMode.Code -> CodeIntroHero()
                ShellMode.Aci -> AciIntroHero()
            }
        },
    )
}

private val HeroShape = RoundedCornerShape(24.dp)

@Composable
private fun heroBox(description: String, bg: Color, content: @Composable androidx.compose.foundation.layout.BoxScope.() -> Unit, modifier: Modifier = Modifier) {
    Box(
        modifier.fillMaxWidth().height(200.dp).clip(HeroShape).background(bg).semantics { contentDescription = description },
        content = content,
    )
}

/** The A:// mark built from blocks: ink blocks, coral core, dropping in row by row. */
@Composable
private fun HomeIntroHero() {
    val c = AllternitTheme.colors
    val reduced = rememberReducedMotion()
    val glyph = listOf("..#..", ".###.", "##.##", "#####", "#...#", "#...#")
    val cells = glyph.flatMapIndexed { r, row -> row.mapIndexedNotNull { col, ch -> if (ch == '#') r to col else null } }
    val progress = remember { Animatable(if (reduced) 1f else 0f) }
    LaunchedEffect(reduced) { if (!reduced) progress.animateTo(1f, tween(900)) }
    heroBox("Preview of the Home mode: the A:// mark built from blocks", c.fillNeutral, {
        Row(Modifier.align(Alignment.Center), verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Box(Modifier.size(width = 5 * 14.dp, height = 6 * 14.dp)) {
                cells.forEachIndexed { i, (r, col) ->
                    val t = ((progress.value * 1.6f) - (r * 0.1f + col * 0.04f)).coerceIn(0f, 1f)
                    Box(
                        Modifier.offset(col * 14.dp, r * 14.dp).size(12.dp)
                            .graphicsLayer { alpha = t; translationY = -(1f - t) * 24.dp.toPx() }
                            .background(if (r == 2 && col == 2 || r == 3 && col == 2) PolishGlass.Coral else c.textPrimary),
                    )
                }
            }
            Text("://", color = PolishGlass.Coral, fontSize = 34.sp, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.Bold, modifier = Modifier.graphicsLayer { alpha = progress.value })
        }
    })
}

/** Three bots linked by a dashed line with a coral pulse travelling between them. */
@Composable
private fun BotsIntroHero() {
    val c = AllternitTheme.colors
    val reduced = rememberReducedMotion()
    val pulse = remember { Animatable(0f) }
    LaunchedEffect(reduced) {
        if (reduced) return@LaunchedEffect
        pulse.animateTo(1f, infiniteRepeatable(tween(1200, easing = LinearEasing), RepeatMode.Restart))
    }
    val bots = listOf(Triple("Scout", Color(0xFF7C5CD6), 0.22f to 0.35f), Triple("Writer", Color(0xFF2F9E6B), 0.78f to 0.35f), Triple("Al", PolishGlass.Coral, 0.5f to 0.75f))
    heroBox("Preview of the Bots mode: three bots linked together", c.fillNeutral, {
        Canvas(Modifier.fillMaxSize()) {
            val dash = PathEffect.dashPathEffect(floatArrayOf(6.dp.toPx(), 9.dp.toPx()))
            fun p(i: Int) = Offset(size.width * bots[i].third.first, size.height * bots[i].third.second)
            listOf(0 to 2, 1 to 2, 0 to 1).forEach { (a, b) ->
                drawLine(c.textPrimary.copy(alpha = 0.35f), p(a), p(b), strokeWidth = 1.5.dp.toPx(), pathEffect = dash)
            }
            if (!reduced) {
                val from = p(2); val to = p(0)
                drawCircle(PolishGlass.Coral, 4.dp.toPx(), Offset(from.x + (to.x - from.x) * pulse.value, from.y + (to.y - from.y) * pulse.value), alpha = (1f - kotlin.math.abs(pulse.value - 0.5f) * 2f).coerceIn(0f, 1f))
            }
        }
        bots.forEachIndexed { i, (name, color, pos) ->
            val d = if (i == 2) 60.dp else 44.dp
            androidx.compose.foundation.layout.BoxWithConstraints(Modifier.fillMaxSize()) {
                Column(
                    Modifier.offset(maxWidth * pos.first - d / 2, maxHeight * pos.second - d / 2),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Box(Modifier.size(d).clip(CircleShape).background(color.copy(alpha = 0.22f)).border(BorderStroke(1.5.dp, color), CircleShape), contentAlignment = Alignment.Center) {
                        Text(name.take(1), color = color, fontWeight = FontWeight.Bold, fontSize = (d.value * 0.4f).sp)
                    }
                    Text(name, color = c.textSecondary, fontSize = 10.5.sp, modifier = Modifier.padding(top = 3.dp))
                }
            }
        }
    })
}

/** Dark terminal card: a task types in, then the steps tick off. */
@Composable
private fun CodeIntroHero() {
    val reduced = rememberReducedMotion()
    val task = "add guest checkout"
    val steps = listOf("✓ read 4 files", "✓ wrote checkout.tsx", "✓ 12 tests pass")
    var typed by remember { mutableIntStateOf(if (reduced) task.length else 0) }
    var ticked by remember { mutableIntStateOf(if (reduced) steps.size else 0) }
    LaunchedEffect(reduced) {
        if (reduced) return@LaunchedEffect
        while (typed < task.length) { delay(45); typed++ }
        while (ticked < steps.size) { delay(420); ticked++ }
    }
    val mono = FontFamily.Monospace
    heroBox("Preview of the Code mode: a task is typed and its steps tick off", Color(0xFF18181A), {
        Column(Modifier.padding(horizontal = 18.dp, vertical = 16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.padding(bottom = 16.dp)) {
                BlockText("GIZZI CODE", height = 12.dp, color = Color(0xFFEDEDED))
                Box(Modifier.weight(1f))
                Text("MacBook Pro", color = Color(0xFF8A8A8E), fontSize = 10.sp)
            }
            Text("❯ ${task.take(typed)}", color = Color(0xFFEDEDED), fontFamily = mono, fontSize = 12.5.sp)
            steps.take(ticked).forEach { Text(it, color = Color(0xFF7FD48A), fontFamily = mono, fontSize = 12.5.sp, modifier = Modifier.padding(top = 7.dp)) }
        }
        val frac = (typed.toFloat() / task.length) * 0.4f + (ticked.toFloat() / steps.size) * 0.6f
        Box(Modifier.align(Alignment.BottomStart).fillMaxWidth(frac).height(3.dp).background(PolishGlass.Coral))
    })
}

/** A page the agent browses: a cursor glides to "Add to cart" and clicks it. */
@Composable
private fun AciIntroHero() {
    val c = AllternitTheme.colors
    val reduced = rememberReducedMotion()
    var clicked by remember { mutableStateOf(reduced) }
    val cursor = remember { Animatable(if (reduced) 1f else 0f) }
    LaunchedEffect(reduced) {
        if (reduced) return@LaunchedEffect
        delay(700)
        cursor.animateTo(1f, tween(900))
        clicked = true
    }
    heroBox("Preview of the ACI mode: an agent browses a shop page and adds an item to the cart", c.fillNeutral, {
        Column(
            Modifier.fillMaxSize().padding(14.dp).clip(RoundedCornerShape(14.dp)).background(Color.White)
                .border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(14.dp)),
        ) {
            Text("shop.example.com", color = c.textSecondary, fontSize = 11.sp, modifier = Modifier.padding(horizontal = 10.dp, vertical = 7.dp))
            Row(Modifier.padding(12.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Box(Modifier.size(70.dp).clip(RoundedCornerShape(10.dp)).background(Color(0xFFEFE7FB)))
                Column {
                    Box(Modifier.padding(top = 2.dp).size(width = 120.dp, height = 8.dp).clip(RoundedCornerShape(4.dp)).background(c.textPrimary.copy(alpha = 0.12f)))
                    Box(Modifier.padding(top = 7.dp).size(width = 70.dp, height = 8.dp).clip(RoundedCornerShape(4.dp)).background(c.textPrimary.copy(alpha = 0.08f)))
                    Text(
                        if (clicked) "✓ Added" else "Add to cart", color = Color.White, fontSize = 11.5.sp, fontWeight = FontWeight.SemiBold,
                        modifier = Modifier.padding(top = 12.dp).clip(CircleShape).background(if (clicked) Color(0xFF16A34A) else Color(0xFF1F1E1D)).padding(horizontal = 12.dp, vertical = 6.dp),
                    )
                }
            }
        }
        // The cursor: a coral arrow tip gliding from the lower right to the button.
        Box(
            Modifier.offset { IntOffset((260.dp.toPx() + (138.dp.toPx() - 260.dp.toPx()) * cursor.value).toInt(), (170.dp.toPx() + (96.dp.toPx() - 170.dp.toPx()) * cursor.value).toInt()) }
                .size(14.dp).clip(RoundedCornerShape(topStart = 2.dp, topEnd = 14.dp, bottomEnd = 14.dp, bottomStart = 14.dp)).background(PolishGlass.Coral)
                .clearAndSetSemantics { },
        )
    })
}
