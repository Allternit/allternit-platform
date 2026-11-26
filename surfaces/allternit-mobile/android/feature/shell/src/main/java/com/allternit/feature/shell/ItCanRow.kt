package com.allternit.feature.shell

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.gen.AgentModeCanDefault
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR

/** One "It can" toggle (registry `can`): the key rides on the session, the label is the plain-words chip. */
internal enum class ItCanKey(val key: String, val label: String, val icon: Int) {
    Connectors("connectors", "Use my tools", DesignR.drawable.ph_plugs_connected),
    Storage("storage", "Keep its own data", DesignR.drawable.ph_database),
    Ai("ai", "Ask AI while open", DesignR.drawable.ph_sparkle),
}

/** The toggles this kind can honour, each with its registry default; a kind that cannot honour one hides it. */
internal fun DeckMode.itCanDefaults(): Map<String, Boolean> = buildMap {
    fun add(k: ItCanKey, d: AgentModeCanDefault) { if (d != AgentModeCanDefault.NONE) put(k.key, d == AgentModeCanDefault.ON) }
    add(ItCanKey.Connectors, entry.connectors)
    add(ItCanKey.Storage, entry.storage)
    add(ItCanKey.Ai, entry.ai)
}

/** The thin "It can" row above the tray (R2-deck-picked): only the toggles [state] holds, tap to flip. */
@Composable
internal fun ItCanRow(state: Map<String, Boolean>, onToggle: (String) -> Unit, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    Row(
        modifier = modifier.horizontalScroll(rememberScrollState()).testTag("itCanRow"),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text("It can", color = c.textSecondary, fontSize = 12.sp, maxLines = 1)
        ItCanKey.entries.filter { state.containsKey(it.key) }.forEach { k ->
            val on = state[k.key] == true
            Row(
                modifier = Modifier
                    .heightIn(min = 44.dp)
                    .clip(RoundedCornerShape(50))
                    .clickable(role = Role.Switch, onClickLabel = k.label) { onToggle(k.key) }
                    .semantics { stateDescription = if (on) "On" else "Off" }
                    .testTag("itCan${k.key}")
                    .padding(vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Row(
                    modifier = Modifier
                        .clip(RoundedCornerShape(50))
                        .background(if (on) c.bgPanel else c.bgPrimary)
                        .border(1.dp, if (on) c.borderStrong else c.borderSubtle, RoundedCornerShape(50))
                        .padding(horizontal = 10.dp, vertical = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Icon(painterResource(if (on) DesignR.drawable.ph_check else k.icon), contentDescription = null, tint = if (on) c.textPrimary else c.textSecondary, modifier = Modifier.size(14.dp))
                    Text(k.label, color = if (on) c.textPrimary else c.textSecondary, fontSize = 12.sp, maxLines = 1)
                }
            }
        }
    }
}
