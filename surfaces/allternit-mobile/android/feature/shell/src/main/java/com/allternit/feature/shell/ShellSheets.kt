package com.allternit.feature.shell

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme

/** A text button in a sheet's top bar (iOS toolbar "Done" / "Cancel" / "Start"). */
internal data class SheetAction(
    val title: String,
    val onClick: () -> Unit,
    val enabled: Boolean = true,
    val bold: Boolean = false,
    val testTag: String? = null,
)

/**
 * A full-height sheet with an iOS NavigationStack inline bar: grabber, centered [title], a
 * leading and a trailing text action. The body fills the rest. Same look as [PlusSheet].
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ShellSheet(
    title: String,
    onDismiss: () -> Unit,
    testTag: String,
    leading: SheetAction? = null,
    trailing: SheetAction? = null,
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
                        leading?.let { SheetTextButton(it) }
                        Spacer(Modifier.weight(1f))
                        trailing?.let { SheetTextButton(it) }
                    }
                }
            }
            content()
        }
    }
}

@Composable
private fun SheetTextButton(action: SheetAction) {
    val c = AllternitTheme.colors
    Box(
        modifier = Modifier
            .heightIn(min = 44.dp)
            .clip(RoundedCornerShape(12.dp))
            .then(if (action.testTag != null) Modifier.testTag(action.testTag) else Modifier)
            .clickable(enabled = action.enabled, role = Role.Button, onClick = action.onClick)
            .padding(horizontal = 12.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            action.title,
            color = if (action.enabled) c.textPrimary else c.textTertiary,
            fontSize = 16.sp,
            fontWeight = if (action.bold) FontWeight.SemiBold else FontWeight.Normal,
        )
    }
}
