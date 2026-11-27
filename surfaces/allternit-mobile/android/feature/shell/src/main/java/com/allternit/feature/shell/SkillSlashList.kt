package com.allternit.feature.shell

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.border
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ServerSkill
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR

/**
 * The `/` list above the composer: enabled server skills whose id starts with what is typed after the slash.
 * Sparkle, `/<id>` in bold monospace, the description, and where it comes from on the right. Tap fills `/<id> `.
 * Hidden when nothing matches (no skills, signed out, flag off).
 */
@Composable
internal fun SkillSlashList(skills: List<ServerSkill>, text: String, onPick: (ServerSkill) -> Unit) {
    if (!text.startsWith("/") || text.contains(' ') || text.contains('\n')) return
    val prefix = text.drop(1)
    val rows = skills.filter { it.enabled && it.id.startsWith(prefix, ignoreCase = true) }
    if (rows.isEmpty()) return
    val c = AllternitTheme.colors
    val shape = RoundedCornerShape(14.dp)
    Column(
        Modifier.fillMaxWidth().padding(bottom = 8.dp).clip(shape).background(c.bgPanel)
            .border(BorderStroke(1.dp, c.borderSubtle), shape)
            .heightIn(max = 240.dp).verticalScroll(rememberScrollState()).testTag("skillSlashList"),
    ) {
        rows.forEach { s ->
            Row(
                Modifier.fillMaxWidth().clickable(role = Role.Button, onClickLabel = "Use skill ${s.id}") { onPick(s) }
                    .padding(horizontal = 14.dp, vertical = 10.dp).testTag("skillRow_${s.id}"),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Icon(painterResource(DesignR.drawable.ph_sparkle), null, tint = c.textSecondary, modifier = Modifier.size(16.dp))
                Column(Modifier.weight(1f)) {
                    Text("/${s.id}", color = c.textPrimary, fontSize = 14.sp, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.Bold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    if (s.description.isNotBlank()) Text(s.description, color = c.textSecondary, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                Text(s.from, color = c.textTertiary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(start = 4.dp))
            }
        }
    }
}
