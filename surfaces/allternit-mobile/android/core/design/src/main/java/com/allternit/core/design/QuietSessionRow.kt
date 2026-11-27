package com.allternit.core.design

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * The one quiet list row (Claude polish a5, port of iOS QuietSessionRow): a leading glyph or avatar,
 * a one-line title, an optional status mark, and the last-used time at the right of the title line
 * ("2m", "3h", "Nov 17"). Touch has no hover, so the time is always visible. Used for recents, bot
 * thread and group rows, so every list reads the same.
 *
 * [status] is passed in by the caller from real data only (a running dot, an offline glyph, a tag).
 * Remote-control state is not read by the Android client yet, so rows show their type glyph until it is.
 * Swipe and hold actions stay with the caller: pass them through [modifier] or wrap the row.
 */
@Composable
fun QuietSessionRow(
    title: String,
    modifier: Modifier = Modifier,
    time: String = "",
    emphasized: Boolean = false,
    testTag: String? = null,
    onClick: () -> Unit,
    leading: @Composable () -> Unit,
    status: @Composable RowScope.() -> Unit = {},
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val scale by animateFloatAsState(if (pressed) 0.98f else 1f, spring(dampingRatio = 0.7f, stiffness = 600f), label = "quietRowPress")
    Row(
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 8.dp)
            .clip(RoundedCornerShape(AllternitRadii.MD))
            .let { if (testTag != null) it.testTag(testTag) else it }
            .graphicsLayer { scaleX = scale; scaleY = scale }
            .background(if (pressed) c.fillNeutral else Color.Transparent)
            .clickable(interactionSource = source, indication = null, role = Role.Button) {
                Haptics.perform(view, Haptics.Kind.Tap)
                onClick()
            }
            .heightIn(min = 44.dp)
            .padding(horizontal = 12.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        leading()
        Text(
            text = title,
            color = c.textPrimary,
            fontSize = 16.5.sp,
            fontWeight = if (emphasized) FontWeight.SemiBold else FontWeight.Normal,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        status()
        if (time.isNotEmpty()) Text(time, color = c.textSecondary, fontSize = 12.sp, maxLines = 1)
    }
}

/** The type glyph at the row's left (20dp column, 16dp icon), the same slot group avatars fill. */
@Composable
fun QuietSessionGlyph(icon: Int) {
    Box(Modifier.width(20.dp), contentAlignment = Alignment.Center) {
        Icon(painterResource(icon), contentDescription = null, tint = AllternitTheme.colors.textSecondary, modifier = Modifier.size(16.dp))
    }
}
