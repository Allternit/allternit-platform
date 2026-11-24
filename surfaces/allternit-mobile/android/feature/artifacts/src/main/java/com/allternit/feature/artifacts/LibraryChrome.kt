package com.allternit.feature.artifacts

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.GlassPill
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR

/*
 * The library chrome both wave D modules share (iOS LibrarySectionChrome / ProjectsChrome and the
 * PhoneHeader the shell uses): a 52dp header with a leading back, a centered title and trailing
 * 44dp buttons; an inline-bar sheet; a pill segment row; a search field; a spinner.
 */

/** Phone top bar (iOS PhoneHeader), the same metrics as :feature:shell's. */
@Composable
fun LibraryHeader(
    title: String,
    subtitle: String? = null,
    onBack: (() -> Unit)?,
    backTag: String = "shellBack",
    leading: (@Composable () -> Unit)? = null,
    /** iOS glass chrome: the menu in a round glass button, the trailing actions in one glass capsule. */
    glass: Boolean = false,
    trailing: @Composable () -> Unit = {},
) {
    val c = AllternitTheme.colors
    Row(Modifier.fillMaxWidth().height(52.dp).padding(horizontal = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.width(96.dp), contentAlignment = Alignment.CenterStart) {
            when {
                leading != null && glass -> Box(Modifier.glassSurface(CircleShape), contentAlignment = Alignment.Center) { leading() }
                leading != null -> leading()
                onBack != null -> LibraryIconButton(DesignR.drawable.ph_caret_left, "Back", backTag, onClick = onBack)
            }
        }
        Column(Modifier.weight(1f), horizontalAlignment = Alignment.CenterHorizontally) {
            Text(
                title, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, letterSpacing = (-0.2).sp,
                maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.semantics { heading() },
            )
            if (!subtitle.isNullOrEmpty()) Text(subtitle, color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (glass) Box(Modifier.width(96.dp), contentAlignment = Alignment.CenterEnd) {
            Row(Modifier.glassSurface(CircleShape).padding(horizontal = 2.dp)) { trailing() }
        } else Row(Modifier.width(96.dp), horizontalArrangement = Arrangement.spacedBy(2.dp, Alignment.End)) { trailing() }
    }
}

/** The iOS glass surface the Projects and Artifacts bars share: a soft white fill with a hairline border and a light shadow. */
@Composable
fun Modifier.glassSurface(shape: androidx.compose.ui.graphics.Shape): Modifier {
    val c = AllternitTheme.colors
    return this.shadow(4.dp, shape, ambientColor = Color.Black.copy(alpha = 0.08f), spotColor = Color.Black.copy(alpha = 0.08f))
        .clip(shape).background(c.bgPanel.copy(alpha = 0.92f)).border(BorderStroke(0.5.dp, c.borderSubtle), shape)
}

/** The floating "New artifact" / "New project" glass pill with a "+". */
@Composable
fun LibraryNewButton(label: String, testTag: String, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    GlassPill(onClick = onClick, modifier = modifier.testTag(testTag), height = 50.dp, contentPadding = PaddingValues(horizontal = 22.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(painterResource(DesignR.drawable.ph_plus), null, tint = c.textPrimary, modifier = Modifier.size(16.dp))
            Text(label, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
        }
    }
}

/** A 44dp round icon button (iOS PhoneIconButton). */
@Composable
fun LibraryIconButton(icon: Int, label: String, testTag: String? = null, tint: Color? = null, iconSize: Int = 22, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Box(
        Modifier.size(44.dp).clip(CircleShape)
            .then(if (testTag != null) Modifier.testTag(testTag) else Modifier)
            .clickable(enabled = enabled, role = Role.Button, onClickLabel = label) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        Icon(painterResource(icon), null, tint = if (enabled) (tint ?: c.textPrimary) else c.textTertiary, modifier = Modifier.size(iconSize.dp))
    }
}

/** A text action in a sheet's top bar (iOS toolbar Cancel / Done / Save). */
data class LibrarySheetAction(val title: String, val enabled: Boolean = true, val bold: Boolean = false, val testTag: String? = null, val onClick: () -> Unit)

/** A sheet with an iOS NavigationStack inline bar (grabber, title, leading and trailing text actions). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LibrarySheet(
    title: String,
    testTag: String,
    onDismiss: () -> Unit,
    leading: LibrarySheetAction? = null,
    trailing: LibrarySheetAction? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val c = AllternitTheme.colors
    val state = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = state,
        containerColor = c.bgPrimary,
        shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
        dragHandle = null,
    ) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().testTag(testTag)) {
            Column(Modifier.fillMaxWidth().padding(start = 6.dp, end = 6.dp, top = 8.dp, bottom = 4.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                Box(Modifier.width(36.dp).height(5.dp).clip(CircleShape).background(c.borderStrong))
                Box(Modifier.fillMaxWidth().padding(top = 4.dp), contentAlignment = Alignment.Center) {
                    Text(title, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.semantics { heading() })
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        leading?.let { SheetText(it) }
                        Spacer(Modifier.weight(1f))
                        trailing?.let { SheetText(it) }
                    }
                }
            }
            content()
        }
    }
}

@Composable
private fun SheetText(a: LibrarySheetAction) {
    val c = AllternitTheme.colors
    Box(
        Modifier.heightIn(min = 44.dp).clip(RoundedCornerShape(12.dp))
            .then(if (a.testTag != null) Modifier.testTag(a.testTag) else Modifier)
            .clickable(enabled = a.enabled, role = Role.Button, onClick = a.onClick)
            .padding(horizontal = 12.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(a.title, color = if (a.enabled) c.textPrimary else c.textTertiary, fontSize = 16.sp, fontWeight = if (a.bold) FontWeight.SemiBold else FontWeight.Normal)
    }
}

/** A row of pills, one selected (iOS LibrarySectionChrome segment / filter chips). */
@Composable
fun <T> PillRow(options: List<T>, selected: T, label: (T) -> String, tag: (T) -> String, modifier: Modifier = Modifier, onPick: (T) -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(modifier.horizontalScrollable(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        options.forEach { o ->
            val on = o == selected
            Box(
                Modifier.height(34.dp).clip(CircleShape)
                    .background(if (on) c.textPrimary else c.fillNeutral)
                    .testTag(tag(o))
                    .clickable(role = Role.Tab) { Haptics.perform(view, Haptics.Kind.Tap); onPick(o) }
                    .semantics { contentDescription = label(o) + if (on) ", selected" else "" }
                    .padding(horizontal = 14.dp),
                contentAlignment = Alignment.Center,
            ) { Text(label(o), color = if (on) c.bgPrimary else c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium) }
        }
    }
}

@Composable
private fun Modifier.horizontalScrollable(): Modifier =
    this.horizontalScroll(androidx.compose.foundation.rememberScrollState())

/** The rounded search field at the top of a library (iOS .searchable look). */
@Composable
fun LibrarySearchField(value: String, placeholder: String, testTag: String, modifier: Modifier = Modifier, onChange: (String) -> Unit) {
    val c = AllternitTheme.colors
    Row(
        modifier.fillMaxWidth().height(40.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(painterResource(DesignR.drawable.ph_magnifying_glass), null, tint = c.textSecondary, modifier = Modifier.size(16.dp))
        Box(Modifier.weight(1f)) {
            if (value.isEmpty()) Text(placeholder, color = c.textTertiary, fontSize = 15.sp)
            BasicTextField(
                value = value, onValueChange = onChange, singleLine = true,
                textStyle = TextStyle(color = c.textPrimary, fontSize = 15.sp), cursorBrush = SolidColor(c.textPrimary),
                modifier = Modifier.fillMaxWidth().testTag(testTag),
            )
        }
        if (value.isNotEmpty()) LibraryIconButton(DesignR.drawable.ph_x, "Clear search", iconSize = 14) { onChange("") }
    }
}

/** A labelled text field in a sheet form (iOS Form TextField). */
@Composable
fun LibraryField(label: String, value: String, placeholder: String, testTag: String, singleLine: Boolean = true, minHeight: Int = 48, onChange: (String) -> Unit) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        if (label.isNotEmpty()) Text(label.uppercase(), color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.Medium, letterSpacing = 0.4.sp)
        Box(
            Modifier.fillMaxWidth().heightIn(min = minHeight.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral)
                .border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(12.dp)).padding(horizontal = 14.dp, vertical = 13.dp),
        ) {
            if (value.isEmpty()) Text(placeholder, color = c.textTertiary, fontSize = 16.sp)
            BasicTextField(
                value = value, onValueChange = onChange, singleLine = singleLine,
                textStyle = TextStyle(color = c.textPrimary, fontSize = 16.sp, lineHeight = 22.sp), cursorBrush = SolidColor(c.textPrimary),
                modifier = Modifier.fillMaxWidth().testTag(testTag),
            )
        }
    }
}

/** The centered spinner a library shows on its first load. */
@Composable
fun LibrarySpinner(label: String, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    Box(modifier.fillMaxWidth().padding(vertical = 48.dp), contentAlignment = Alignment.Center) {
        CircularProgressIndicator(Modifier.size(24.dp).semantics { contentDescription = label }, strokeWidth = 2.dp, color = c.textSecondary)
    }
}

/** A full-width 52dp row button on a detail screen (iOS List row with an icon). */
@Composable
fun LibraryRowButton(title: String, icon: Int, testTag: String, destructive: Boolean = false, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val tint = if (destructive) com.allternit.core.design.AllternitStatic.StatusError else c.textPrimary
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).testTag(testTag)
            .clickable(enabled = enabled, role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .heightIn(min = 52.dp).padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Icon(painterResource(icon), null, tint = if (enabled) tint else c.textTertiary, modifier = Modifier.size(20.dp))
        Text(title, color = if (enabled) tint else c.textTertiary, fontSize = 16.sp)
    }
}
