package com.allternit.feature.shell

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme

/** Drawer entry to Shelves, retaining the Bots unread badge. */
@Composable
internal fun ModeDialButton(mode: ShellMode, botsWaiting: Int, onOpenHub: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        modifier = Modifier
            .size(44.dp)
            .clip(CircleShape)
            .testTag("modeDial")
            // The host closes the drawer and opens Shelves.
            .clickable(role = Role.Button, onClickLabel = "Closes the menu and shows the modes", onClick = onOpenHub)
            .semantics {
                contentDescription = "${mode.label} mode. Switch mode${if (botsWaiting > 0) ", $botsWaiting waiting in Bots" else ""}"
            },
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.size(34.dp).clip(CircleShape).background(c.textPrimary), contentAlignment = Alignment.Center) {
            Icon(painter = painterResource(mode.icon), contentDescription = null, tint = c.bgPrimary, modifier = Modifier.size(17.dp))
        }
        if (botsWaiting > 0) ModeDialBadge(botsWaiting, Modifier.align(Alignment.TopEnd).offset(x = (-1).dp, y = 1.dp))
    }
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
