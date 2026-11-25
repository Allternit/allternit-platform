package com.allternit.feature.automation

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.GlassPill
import com.allternit.core.design.Haptics
import com.allternit.core.design.PolishGlass
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.core.design.R as DesignR

/** Shared chrome for the Automation screens (the iOS FriendlyStateView, rows and pills these views use). */

@Composable
internal fun AutoScaffold(title: String, onBack: () -> Unit, backTag: String, trailing: @Composable () -> Unit = {}, content: @Composable ColumnScope.() -> Unit) {
    BackHandler(onBack = onBack)
    Column(Modifier.fillMaxSize().background(AllternitTheme.colors.bgPrimary).statusBarsPadding()) {
        LibraryHeader(title = title, onBack = onBack, backTag = backTag, trailing = trailing)
        content()
    }
}

/** No computer on the account: a white card with a glass Connect button, never a red error (iOS #2827). */
@Composable
internal fun NoComputerState(modifier: Modifier = Modifier, onRetry: (() -> Unit)?) {
    val c = AllternitTheme.colors
    val shape = RoundedCornerShape(18.dp)
    Row(
        modifier.fillMaxWidth().testTag("automationNoComputer")
            .clip(shape).background(if (PolishGlass.isDark()) c.bgPanel else Color.White)
            .border(1.dp, PolishGlass.border(), shape)
            .padding(start = 14.dp, top = 10.dp, bottom = 10.dp, end = 10.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(painterResource(DesignR.drawable.ph_desktop), null, tint = c.textSecondary, modifier = Modifier.size(20.dp))
        Column(Modifier.weight(1f)) {
            Text("Automation tasks run on your computer", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium)
            Text("Open Allternit Desktop, or wake your cloud computer.", color = c.textSecondary, fontSize = 12.sp)
        }
        if (onRetry != null) {
            GlassPill(onClick = onRetry, height = 36.dp, modifier = Modifier.testTag("automationConnect")) {
                Text("Connect", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium)
            }
        }
    }
}

@Composable
internal fun ErrorState(title: String, message: String, onRetry: (() -> Unit)?, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    FriendlyState(
        style = FriendlyStyle.Offline, title = title, message = message, modifier = modifier,
        actionTitle = if (onRetry != null) "Try again" else null, onAction = onRetry,
        icon = { Icon(painterResource(DesignR.drawable.ph_warning), null, tint = c.textSecondary, modifier = Modifier.size(28.dp)) },
    )
}

@Composable
internal fun EmptyState(title: String, message: String, modifier: Modifier = Modifier, actionTitle: String? = null, onAction: (() -> Unit)? = null, art: com.allternit.core.design.EmptyArtKind? = null) {
    val c = AllternitTheme.colors
    FriendlyState(
        style = FriendlyStyle.Empty, title = title, message = message, modifier = modifier, actionTitle = actionTitle, onAction = onAction, art = art,
        icon = { Icon(painterResource(DesignR.drawable.ph_calendar_blank), null, tint = c.textSecondary, modifier = Modifier.size(28.dp)) },
    )
}

@Composable
internal fun Loading(modifier: Modifier = Modifier) {
    Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        CircularProgressIndicator(modifier = Modifier.size(28.dp), strokeWidth = 2.dp, color = AllternitTheme.colors.textSecondary)
    }
}

/** A small rounded button (Run now, Delete, Create). */
@Composable
internal fun ActionPill(label: String, testTag: String, modifier: Modifier = Modifier, filled: Boolean = false, destructive: Boolean = false, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val accent = if (destructive) AllternitStatic.StatusError else c.accentCode
    Box(
        modifier.heightIn(min = 44.dp).clip(RoundedCornerShape(22.dp)).testTag(testTag)
            .background(if (filled) accent.copy(alpha = if (enabled) 1f else 0.4f) else c.fillNeutral)
            .clickable(enabled = enabled, role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .padding(horizontal = 18.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = if (filled) c.onAccent else if (destructive) accent else if (enabled) c.textPrimary else c.textTertiary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
    }
}

@Composable
internal fun SectionLabel(text: String) {
    Text(text.uppercase(), color = AllternitTheme.colors.textTertiary, fontSize = 11.sp, fontWeight = FontWeight.Medium, letterSpacing = 0.6.sp, modifier = Modifier.padding(start = 16.dp, top = 18.dp, bottom = 6.dp))
}

@Composable
internal fun InfoRow(label: String, value: String, valueColor: Color? = null) {
    val c = AllternitTheme.colors
    Row(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 5.dp), horizontalArrangement = Arrangement.SpaceBetween) {
        Text(label, color = c.textSecondary, fontSize = 13.sp)
        Text(value, color = valueColor ?: c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(start = 16.dp))
    }
}

/** The state colour of a run or a task (iOS RoutinesListView.statusColor). */
@Composable
internal fun stateColor(state: String): Color {
    val c = AllternitTheme.colors
    return when (state) {
        "running", "working", "active" -> AllternitStatic.StatusInfo
        "done", "completed", "success", "succeeded", "ok" -> AllternitStatic.StatusSuccess
        "failed", "error", "cancelled" -> AllternitStatic.StatusError
        "paused", "skipped" -> c.textTertiary
        else -> c.textSecondary
    }
}

/** The last runs as a strip of dots, oldest to newest (iOS RoutineRunRow's 14-run strip). */
@Composable
internal fun RunStrip(runs: List<RunTick>, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    Row(modifier.semantics { contentDescription = "Last ${runs.size} runs" }, horizontalArrangement = Arrangement.spacedBy(3.dp), verticalAlignment = Alignment.CenterVertically) {
        runs.take(14).reversed().forEach { r ->
            Box(Modifier.size(8.dp).clip(CircleShape).background(if (r.isQuiet) c.borderStrong else stateColor(r.status)))
        }
    }
}

@Composable
internal fun AutoSwitch(checked: Boolean, label: String, testTag: String, enabled: Boolean = true, onChange: () -> Unit) {
    Switch(
        checked = checked, onCheckedChange = { onChange() }, enabled = enabled,
        modifier = Modifier.testTag(testTag).semantics { contentDescription = label },
    )
}

@Composable
internal fun TypeTag(type: TaskType) {
    val c = AllternitTheme.colors
    val tint = when (type) {
        TaskType.Goal -> c.accentCowork
        TaskType.Routine -> c.accentCode
        TaskType.Loop -> c.accentBrowser
        TaskType.Heartbeat -> c.accentChat
    }
    Box(Modifier.clip(RoundedCornerShape(6.dp)).background(tint.copy(alpha = 0.16f)).padding(horizontal = 8.dp, vertical = 2.dp)) {
        Text(type.label, color = tint, fontSize = 11.sp, fontWeight = FontWeight.Medium)
    }
}
