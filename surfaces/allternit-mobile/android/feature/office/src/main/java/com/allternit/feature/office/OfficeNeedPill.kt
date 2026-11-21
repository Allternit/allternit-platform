package com.allternit.feature.office

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics

/** "1 needs you" / "N need you" (the pill's copy, iOS OfficeNeedPill). */
internal fun needYouLine(count: Int) = if (count == 1) "1 needs you" else "$count need you"

/** A `#rrggbb` bot colour, or a neutral grey. */
internal fun officeColor(hex: String?): Color {
    val h = hex?.removePrefix("#")?.takeIf { it.length == 6 } ?: return Color(0xFF9CA3AF)
    return h.toLongOrNull(16)?.let { Color(0xFF000000 or it) } ?: Color(0xFF9CA3AF)
}

/**
 * The "N need you" pill over the floor's top-left corner (step 26, iOS OfficeNeedPill): stacked dots in the waiting
 * bots' colours (at most three) and the count. Tapping it opens the first waiting bot's sheet with its approval card.
 */
@Composable
fun OfficeNeedPill(count: Int, colors: List<Color>, onClick: () -> Unit, modifier: Modifier = Modifier) {
    if (count <= 0) return
    val c = AllternitTheme.colors
    val view = LocalView.current
    val line = needYouLine(count)
    val open = { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
    Row(
        modifier.height(36.dp).clip(RoundedCornerShape(18.dp)).background(c.bgPrimary)
            .border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(18.dp))
            .clickable(onClickLabel = "Open", role = Role.Button) { open() }
            .semantics { contentDescription = "$line. Opens the first one." }
            .padding(start = 10.dp, end = 14.dp).testTag("officeNeedPill"),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        val dots = colors.take(3).ifEmpty { listOf(Color(0xFF9CA3AF)) }
        // Overlapping dots, each ringed in the pill's surface so the stack reads.
        Box(Modifier.width((14 + (dots.size - 1) * 9).dp).height(14.dp)) {
            dots.forEachIndexed { i, color ->
                Box(
                    Modifier.offset(x = (i * 9).dp).size(14.dp).clip(CircleShape).background(c.bgPrimary).padding(1.5.dp)
                        .clip(CircleShape).background(color),
                )
            }
        }
        Text(line, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, modifier = Modifier.clearAndSetSemantics {})
    }
}
