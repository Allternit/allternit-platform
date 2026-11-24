package com.allternit.feature.code

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.Haptics
import com.allternit.core.design.ProvideDockScroll
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.core.design.R as DesignR

/** The monospace face every Code screen uses for paths, diffs and the terminal. */
internal val CodeMono = FontFamily.Monospace

/** A Code sub-screen: the library header with a back arrow over the page colour. */
@Composable
internal fun CodeScaffold(
    title: String,
    subtitle: String? = null,
    onBack: () -> Unit,
    backTag: String,
    trailing: @Composable () -> Unit = {},
    /** The page scroll in dp (`rememberDockOffset(listState)`): the title docks into the bar. Null keeps the plain bar (T7/T8). */
    dockScroll: (() -> Float)? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val c = AllternitTheme.colors
    BackHandler(onBack = onBack)
    val body: @Composable () -> Unit = {
        Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding()) {
            LibraryHeader(title = title, subtitle = subtitle, onBack = onBack, backTag = backTag, trailing = trailing)
            content()
        }
    }
    if (dockScroll != null) ProvideDockScroll(dockScroll, body) else body()
}

/** iOS "Connect a computer or Allternit Cloud" (no computer: 428 / compute_required), never a raw error. */
@Composable
internal fun NoComputerState(onRetry: (() -> Unit)?, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    FriendlyState(
        style = FriendlyStyle.Error,
        title = "Connect a computer or Allternit Cloud",
        message = "Open Allternit Desktop on your computer, or wake your cloud computer, then retry.",
        modifier = modifier.testTag("codeNoComputer"),
        actionTitle = if (onRetry != null) "Retry" else null,
        onAction = onRetry,
        icon = { Icon(painterResource(DesignR.drawable.ph_desktop), null, tint = c.textSecondary, modifier = Modifier.size(28.dp)) },
    )
}

@Composable
internal fun ErrorState(title: String, message: String, onRetry: (() -> Unit)?, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    FriendlyState(
        style = FriendlyStyle.Offline,
        title = title,
        message = message,
        modifier = modifier,
        actionTitle = if (onRetry != null) "Retry" else null,
        onAction = onRetry,
        icon = { Icon(painterResource(DesignR.drawable.ph_warning), null, tint = c.textSecondary, modifier = Modifier.size(28.dp)) },
    )
}

@Composable
internal fun EmptyState(title: String, message: String, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    FriendlyState(
        style = FriendlyStyle.Empty,
        title = title,
        message = message,
        modifier = modifier,
        icon = { Icon(painterResource(DesignR.drawable.ph_terminal), null, tint = c.textSecondary, modifier = Modifier.size(28.dp)) },
    )
}

@Composable
internal fun Loading(modifier: Modifier = Modifier) {
    Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        CircularProgressIndicator(modifier = Modifier.size(28.dp), strokeWidth = 2.dp, color = AllternitTheme.colors.textSecondary)
    }
}

/**
 * Loads once per [key] (and again on retry / [reload]) and draws loading, no-computer, failed
 * and loaded states, the way every iOS Code screen does.
 */
@Composable
internal fun <T> CodeLoader(
    key: Any?,
    failedTitle: String,
    load: suspend () -> CodeResult<T>,
    content: @Composable (value: T, reload: () -> Unit) -> Unit,
) {
    var tick by remember { mutableIntStateOf(0) }
    var state by remember(key) { mutableStateOf<CodeResult<T>?>(null) }
    LaunchedEffect(key, tick) { state = load() }
    when (val s = state) {
        null -> Loading()
        CodeResult.NoComputer -> NoComputerState(onRetry = { state = null; tick++ })
        is CodeResult.Failed -> ErrorState(failedTitle, s.message, onRetry = { state = null; tick++ })
        is CodeResult.Ok -> content(s.value) { tick++ }
    }
}

/** A tappable row with a title, optional detail and a trailing slot. */
@Composable
internal fun CodeRow(
    title: String,
    detail: String? = null,
    testTag: String? = null,
    mono: Boolean = false,
    dim: Boolean = false,
    leading: (@Composable () -> Unit)? = null,
    trailing: (@Composable () -> Unit)? = null,
    onClick: () -> Unit,
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        Modifier.fillMaxWidth().heightIn(min = 52.dp)
            .then(if (testTag != null) Modifier.testTag(testTag) else Modifier)
            .clickable(role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (leading != null) leading()
        Column(Modifier.weight(1f)) {
            Text(
                title, color = if (dim) c.textTertiary else c.textPrimary, fontSize = if (mono) 14.sp else 16.sp,
                fontFamily = if (mono) CodeMono else null, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
            )
            if (detail != null) Text(detail, color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
        }
        if (trailing != null) trailing()
    }
}

/** A small rounded pill button (Approve, Reject, Save…). */
@Composable
internal fun CodePill(label: String, testTag: String? = null, filled: Boolean = false, tint: Color? = null, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val accent = tint ?: c.accentCode
    Box(
        Modifier.heightIn(min = 44.dp).clip(RoundedCornerShape(22.dp))
            .then(if (testTag != null) Modifier.testTag(testTag) else Modifier)
            .background(if (filled) accent.copy(alpha = if (enabled) 1f else 0.4f) else c.fillNeutral)
            .clickable(enabled = enabled, role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .padding(horizontal = 18.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = if (filled) c.onAccent else if (enabled) c.textPrimary else c.textTertiary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
    }
}

@Composable
internal fun SectionLabel(text: String) {
    Text(text.uppercase(), color = AllternitTheme.colors.textTertiary, fontSize = 11.sp, fontWeight = FontWeight.Medium, letterSpacing = 0.6.sp, modifier = Modifier.padding(start = 16.dp, top = 18.dp, bottom = 6.dp))
}

/** A vertically scrolling column for form-like screens. */
@Composable
internal fun ColumnScope.ScrollBody(content: @Composable ColumnScope.() -> Unit) {
    Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState())) { content() }
}
