package com.allternit.feature.automation

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
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
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import kotlinx.coroutines.launch

/**
 * The Tasks board (iOS TasksBoardScreen, "section: tasks"): the Factory's cards by column, with filter chips
 * and a "Scheduled" link back to the schedule. Read-only, like iOS.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun TasksBoardScreen(host: AutomationHost, onScheduled: () -> Unit) {
    val c = AllternitTheme.colors
    val api = remember(host.backend) { AutomationApi(host.backend) }
    val scope = rememberCoroutineScope()
    var state by remember { mutableStateOf<AResult<List<BoardCard>>?>(null) }
    var filter by remember { mutableStateOf<BoardColumn?>(null) }
    var refreshing by remember { mutableStateOf(false) }
    var tick by remember { mutableIntStateOf(0) }
    LaunchedEffect(tick) { state = api.board() }

    PullToRefreshBox(isRefreshing = refreshing, onRefresh = { scope.launch { refreshing = true; state = api.board(); refreshing = false } }, modifier = Modifier.fillMaxSize().testTag("tasksBoard")) {
        when (val s = state) {
            null -> Loading()
            AResult.NoComputer -> NoComputerState { state = null; tick++ }
            is AResult.Failed -> ErrorState("Couldn't load your tasks", s.message, { state = null; tick++ })
            is AResult.Ok -> {
                val cards = s.value
                Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 12.dp)) {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        Row(Modifier.weight(1f).horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                            Chip("All", cards.size, filter == null, "tasksBoardAll") { filter = null }
                            BoardColumn.entries.forEach { col -> Chip(col.label, cards.count { it.column == col }, filter == col, "tasksBoard_${col.name}") { filter = col } }
                        }
                        Text("Scheduled ↗", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.heightIn(min = 44.dp).testTag("tasksBoardScheduled").clickable(role = Role.Button, onClick = onScheduled).padding(horizontal = 8.dp, vertical = 12.dp))
                    }
                    if (cards.isEmpty()) com.allternit.core.design.InlineEmpty("No tasks yet. Tasks you or your bots add in a chat show up here.", Modifier.padding(top = 14.dp).testTag("tasksBoardEmpty"), art = com.allternit.core.design.EmptyArtKind.Envelope)
                    BoardColumn.entries.forEach { col ->
                        val inCol = cards.filter { it.column == col }
                        if ((filter == null || filter == col) && inCol.isNotEmpty()) {
                            Column(Modifier.padding(top = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                Text("${col.label} · ${inCol.size}".uppercase(), color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.4.sp)
                                inCol.forEach { card ->
                                    Column(Modifier.fillMaxWidth().heightIn(min = 44.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(horizontal = 14.dp, vertical = 12.dp).testTag("tasksBoardCard_${card.nodeId}"), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                        Text(card.title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                                        Text(card.detailLine, color = c.textSecondary, fontSize = 12.sp)
                                    }
                                }
                            }
                        }
                    }
                    val f = filter
                    if (f != null && cards.isNotEmpty() && cards.none { it.column == f }) Text("No tasks in this state.", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(top = 14.dp))
                }
            }
        }
    }
}

@Composable
private fun Chip(title: String, count: Int, selected: Boolean, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        Modifier.heightIn(min = 44.dp).clip(RoundedCornerShape(8.dp)).testTag(tag).semantics { this.selected = selected }
            .background(if (selected) c.fillNeutral else c.bgPrimary).clickable(role = Role.Tab, onClick = onClick).padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Text(title, color = if (selected) c.textPrimary else c.textSecondary, fontSize = 12.sp)
        Text("$count", color = c.textTertiary, fontSize = 12.sp)
    }
}
