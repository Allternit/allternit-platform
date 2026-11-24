package com.allternit.feature.automation

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInParent
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.BlockText
import com.allternit.core.design.PolishGlass
import com.allternit.core.design.rememberReducedMotion
import kotlinx.coroutines.delay
import com.allternit.core.design.R as DesignR

private val HeroPills = listOf("All", "Goals", "Routines", "Loops")
private val HeroPhaseMs = longArrayOf(1500, 1500, 1500, 2200)
private val Passed = Color(0xFF16A34A)

private class HeroTask(val kind: Int, val icon: Int, val title: String, val note: String, val trailing: String? = null)

/** Real-looking task rows, the same ones the list shows: a routine, a goal and a loop. */
private val HeroTasks = listOf(
    HeroTask(2, DesignR.drawable.ph_clock, "Morning inbox triage", "Routine · weekdays 9:00", "in 14h"),
    HeroTask(1, DesignR.drawable.ph_target, "Ship the FAQ", "Goal · 3 of 5 steps"),
    HeroTask(3, DesignR.drawable.ph_repeat, "Fix failing tests", "Loop · run 4 · until green"),
)

/**
 * The Automation Tasks intro hero (mockup HeroAutomation): the real task rows filter as the ink chip
 * moves All, Goals, Routines, Loops; on Loops the spinning loop turns to Passed. Under reduced motion
 * it holds on Loops, with the loop already Passed.
 */
@Composable
fun AutomationIntroHero(modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    val reduced = rememberReducedMotion()
    var phase by remember { mutableStateOf(if (reduced) 3 else 0) }
    LaunchedEffect(reduced) {
        if (reduced) { phase = 3; return@LaunchedEffect }
        while (true) {
            delay(HeroPhaseMs[phase])
            phase = (phase + 1) % HeroPills.size
        }
    }
    val density = LocalDensity.current
    val frames = remember { mutableStateMapOf<Int, Pair<Float, Float>>() }
    val target = frames[phase]
    val chipX by animateDpAsState(with(density) { (target?.first ?: 0f).toDp() }, spring(0.8f, 420f), label = "heroChipX")
    val chipW by animateDpAsState(with(density) { (target?.second ?: 0f).toDp() }, spring(0.8f, 420f), label = "heroChipW")

    Column(
        modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(PolishGlass.track()).padding(12.dp)
            .clearAndSetSemantics { },
    ) {
        BlockText("AUTOMATION TASKS", height = 7.dp)
        Box(Modifier.padding(top = 8.dp)) {
            if (target != null) Box(Modifier.offset(x = chipX).width(chipW).height(22.dp).clip(CircleShape).background(c.textPrimary))
            Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                HeroPills.forEachIndexed { i, label ->
                    val on = i == phase
                    Box(
                        Modifier.height(22.dp).onGloballyPositioned { frames[i] = it.positionInParent().x to it.size.width.toFloat() }
                            .clip(CircleShape).then(if (on) Modifier else Modifier.background(if (PolishGlass.isDark()) Color(0xFF3A3A3C) else Color.White))
                            .padding(horizontal = 9.dp),
                        contentAlignment = Alignment.Center,
                    ) { Text(label, color = if (on) c.bgPrimary else c.textPrimary, fontSize = 10.sp, fontWeight = FontWeight.Medium) }
                }
            }
        }
        Text("Your tasks", color = c.textSecondary, fontSize = 10.sp, fontWeight = FontWeight.Medium, modifier = Modifier.padding(top = 9.dp, bottom = 5.dp))
        HeroTasks.forEach { t ->
            AnimatedVisibility(
                visible = phase == 0 || phase == t.kind,
                enter = fadeIn(tween(220)) + expandVertically(),
                exit = fadeOut(tween(160)) + shrinkVertically(),
            ) { HeroRow(t, onLoops = phase == 3) }
        }
    }
}

@Composable
private fun HeroRow(t: HeroTask, onLoops: Boolean) {
    val c = AllternitTheme.colors
    val dark = PolishGlass.isDark()
    Row(
        Modifier.fillMaxWidth().padding(bottom = 5.dp).clip(RoundedCornerShape(11.dp)).background(if (dark) Color(0xFF3A3A3C) else Color.White).padding(horizontal = 8.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Box(Modifier.size(22.dp).clip(RoundedCornerShape(7.dp)).background(if (dark) Color(0xFF48484A) else Color.White), contentAlignment = Alignment.Center) {
            Icon(painterResource(t.icon), null, tint = c.textPrimary, modifier = Modifier.size(12.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(t.title, color = c.textPrimary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
            Text(t.note, color = c.textSecondary, fontSize = 9.5.sp, maxLines = 1)
            if (t.kind == 1) {
                val p by rememberInfiniteTransition(label = "heroBar").animateFloat(0.4f, 0.6f, infiniteRepeatable(tween(2400, easing = LinearEasing), RepeatMode.Reverse), label = "heroBarP")
                Box(Modifier.padding(top = 3.dp).fillMaxWidth(0.8f).height(3.dp).clip(CircleShape).background(if (dark) Color(0xFF48484A) else Color.White)) {
                    Box(Modifier.fillMaxWidth(p).height(3.dp).clip(CircleShape).background(c.textPrimary))
                }
            }
        }
        t.trailing?.let { Text(it, color = c.textSecondary, fontSize = 9.5.sp) }
        if (t.kind == 3) {
            if (onLoops) {
                var popped by remember { mutableStateOf(false) }
                LaunchedEffect(Unit) { delay(700); popped = true }
                val s by animateFloatAsState(if (popped) 1f else 0.6f, spring(0.6f, 500f), label = "heroPassed")
                Row(horizontalArrangement = Arrangement.spacedBy(3.dp), verticalAlignment = Alignment.CenterVertically) {
                    Icon(painterResource(DesignR.drawable.ph_check), null, tint = Passed, modifier = Modifier.size((10 * s).dp))
                    Text("Passed", color = Passed, fontSize = 9.5.sp, fontWeight = FontWeight.SemiBold)
                }
            } else {
                val a by rememberInfiniteTransition(label = "heroSpin").animateFloat(0f, 360f, infiniteRepeatable(tween(1200, easing = LinearEasing)), label = "heroSpinA")
                Icon(painterResource(DesignR.drawable.ph_arrows_clockwise), null, tint = c.textSecondary, modifier = Modifier.size(11.dp).rotate(a))
            }
        }
    }
}
