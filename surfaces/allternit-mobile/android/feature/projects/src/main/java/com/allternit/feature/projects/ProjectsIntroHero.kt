package com.allternit.feature.projects

import androidx.compose.animation.Crossfade
import androidx.compose.animation.core.tween
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.BlockText
import com.allternit.core.design.R as DesignR
import com.allternit.core.design.rememberReducedMotion
import kotlinx.coroutines.delay

private val Coral = Color(0xFFD97757)
private val Amber = Color(0xFFB45309)
private val AmberRing = Color(0xFFFBBF24)
private val DoneGreen = Color(0xFF16A34A)

private data class HeroRow(val icon: Int, val title: String, val note: String, val ago: String, val pill: Pair<String, Color>? = null, val ring: Color? = null)

private val HeroRows = listOf(
    HeroRow(DesignR.drawable.ph_code, "Launch site", "Code · 2 sessions", "now", "2 working" to Coral, Coral.copy(alpha = 0.55f)),
    HeroRow(DesignR.drawable.ph_cpu, "Weekly report", "Bot · 3 bots · Al coordinates", "1h", "1 waiting" to Amber, AmberRing),
    HeroRow(DesignR.drawable.ph_chat_text, "Q4 plan", "Chat · 4 threads", "2d"),
)

/**
 * The moving copy of the Projects tab for the first-visit intro sheet: the real project rows
 * (mode icon, title, a coral "2 working" or amber "1 waiting" pill, the count line), then the
 * Board of the first project with a card moving from Working to Done. Reduced motion keeps
 * the list still.
 */
@Composable
fun ProjectsIntroHero(modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    val reduced = rememberReducedMotion()
    var phase by remember { mutableIntStateOf(0) } // 0 list, 1 board, 2 board with the card moved
    LaunchedEffect(reduced) {
        if (reduced) { phase = 0; return@LaunchedEffect }
        while (true) {
            phase = 0; delay(2600)
            phase = 1; delay(1100)
            phase = 2; delay(2600)
        }
    }
    val shape = RoundedCornerShape(24.dp)
    Box(
        modifier.fillMaxWidth().height(200.dp).clip(shape).background(c.fillNeutral)
            .semantics { contentDescription = "Preview of the Projects tab: a list of projects, then a board with a card moving to Done" },
    ) {
        Column(
            Modifier.fillMaxWidth().padding(12.dp).clip(RoundedCornerShape(18.dp)).background(c.bgPanel)
                .border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(18.dp)).padding(horizontal = 12.dp, vertical = 11.dp)
                .clearAndSetSemantics { },
        ) {
            Crossfade(targetState = if (phase == 0) 0 else 1, animationSpec = tween(250), label = "projectsHero") { page ->
                if (page == 0) HeroList() else HeroBoard(moved = phase == 2)
            }
        }
    }
}

@Composable
private fun HeroList() {
    val c = AllternitTheme.colors
    Column {
        Row(Modifier.fillMaxWidth().padding(bottom = 9.dp), verticalAlignment = Alignment.CenterVertically) {
            BlockText("PROJECTS", height = 8.dp)
            Box(Modifier.weight(1f))
            Text("3 open", color = c.textSecondary, fontSize = 10.sp)
        }
        HeroRows.forEach { r ->
            val rowShape = RoundedCornerShape(12.dp)
            Row(
                Modifier.fillMaxWidth().padding(bottom = 6.dp).clip(rowShape)
                    .border(BorderStroke(1.dp, r.ring ?: c.borderSubtle), rowShape)
                    .padding(horizontal = 9.dp, vertical = 7.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Icon(painterResource(r.icon), null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
                Column(Modifier.weight(1f)) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(r.title, color = c.textPrimary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
                        r.pill?.let { (label, color) ->
                            Text(
                                label, color = color, fontSize = 9.5.sp, fontWeight = FontWeight.SemiBold,
                                modifier = Modifier.clip(CircleShape).background(color.copy(alpha = 0.12f)).padding(horizontal = 6.dp, vertical = 1.dp),
                            )
                        }
                    }
                    Text(r.note, color = c.textTertiary, fontSize = 10.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(top = 1.dp))
                }
                Text(r.ago, color = c.textTertiary, fontSize = 10.sp)
            }
        }
    }
}

@Composable
private fun HeroBoard(moved: Boolean) {
    val c = AllternitTheme.colors
    Column {
        Text("‹ Projects  /  Launch site", color = c.textSecondary, fontSize = 10.5.sp)
        Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            listOf("Overview", "Delegate", "Board").forEach { t ->
                Text(t, color = if (t == "Board") c.textPrimary else c.textSecondary, fontSize = 11.sp, fontWeight = if (t == "Board") FontWeight.SemiBold else FontWeight.Normal)
            }
        }
        Box(Modifier.fillMaxWidth().padding(top = 4.dp).height(1.dp).background(c.borderSubtle))
        val queued = listOf("Write the FAQ")
        val working = if (moved) listOf("Hero copy") else listOf("Hero copy", "Fix checkout")
        val done = if (moved) listOf("Fix checkout", "Pick domain") else listOf("Pick domain")
        Row(Modifier.fillMaxWidth().padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            HeroColumn("Queued", queued, Modifier.weight(1f))
            HeroColumn("Working", working, Modifier.weight(1f))
            HeroColumn("Done", done, Modifier.weight(1f))
        }
    }
}

@Composable
private fun HeroColumn(name: String, cards: List<String>, modifier: Modifier) {
    val c = AllternitTheme.colors
    Column(modifier.heightIn(min = 100.dp).clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).padding(6.dp)) {
        Text(name, color = c.textSecondary, fontSize = 9.5.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(bottom = 5.dp))
        cards.forEach { n ->
            Row(
                Modifier.fillMaxWidth().padding(bottom = 4.dp).clip(RoundedCornerShape(7.dp)).background(c.bgPanel).padding(horizontal = 6.dp, vertical = 5.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(3.dp),
            ) {
                if (name == "Done") Icon(painterResource(DesignR.drawable.ph_check_bold), null, tint = DoneGreen, modifier = Modifier.size(9.dp))
                Text(n, color = c.textPrimary, fontSize = 9.5.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}
