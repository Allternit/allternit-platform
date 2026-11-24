package com.allternit.feature.shell

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic

/**
 * The drawer's mode switcher (iOS HistorySidebarView header orb): the tap-only mode orb, whose pill of the
 * four modes opens over the drawer's top line; a pick switches mode and the rail swaps in place. Shelves
 * opens only from a hold on the composer (or ACI) orb. Keeps the Bots unread badge.
 */
@Composable
internal fun DrawerModeOrb(mode: ShellMode, botsWaiting: Int, onPickMode: (ShellMode) -> Unit) {
    var expanded by remember(mode) { mutableStateOf(false) }
    ModeOrb(
        mode = mode,
        expanded = expanded,
        onExpandedChange = { expanded = it },
        onPick = { expanded = false; onPickMode(it) },
        onOpenHub = null,
        botsWaiting = botsWaiting,
    )
}

/** The red unread count on the dial, the Bots card and the shelf (`phone-mode-badge`): 16dp, "9+" past nine. */
@Composable
internal fun ModeDialBadge(count: Int, modifier: Modifier = Modifier) {
    Box(
        modifier = modifier
            .defaultMinSize(minWidth = 16.dp, minHeight = 16.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(AllternitStatic.StatusError)
            .padding(horizontal = 4.dp)
            .clearAndSetSemantics {},
        contentAlignment = Alignment.Center,
    ) {
        Text(if (count > 9) "9+" else "$count", color = Color.White, fontSize = 10.sp, fontWeight = FontWeight.Bold, maxLines = 1)
    }
}
