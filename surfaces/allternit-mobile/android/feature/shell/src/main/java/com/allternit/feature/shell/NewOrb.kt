package com.allternit.feature.shell

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.window.Popup
import androidx.compose.ui.window.PopupProperties
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR

/** Shell supplies navigation; standalone composer previews have no shell controls. */
internal val LocalComposerOrbs = staticCompositionLocalOf<@Composable (Boolean, Boolean, (() -> Unit)?) -> Unit> { { _, _, _ -> } }

@Composable
internal fun ComposerOrbs(
    mode: ShellMode,
    sessionOpen: Boolean,
    botsWaiting: Int,
    onPickMode: (ShellMode) -> Unit,
    onOpenHub: () -> Unit,
    onNew: () -> Unit,
    onQuickTask: () -> Unit,
) {
    var modesExpanded by remember(mode) { mutableStateOf(false) }
    var newExpanded by remember(mode) { mutableStateOf(false) }
    BackHandler(enabled = modesExpanded || newExpanded) { modesExpanded = false; newExpanded = false }
    // Beat: tap replaces orb with its pill in place; Back/pick collapses immediately.
    // No travel or spring on text/layout; system reduced motion has the same instant state change.
    Row(Modifier.fillMaxWidth().padding(bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        ModeOrb(mode, modesExpanded, { modesExpanded = it; newExpanded = false },
            onPick = { modesExpanded = false; onPickMode(it) },
            onOpenHub = { modesExpanded = false; newExpanded = false; onOpenHub() }, botsWaiting = botsWaiting)
        Spacer(Modifier.weight(1f))
        if (sessionOpen && !modesExpanded) {
            NewOrb(mode, newExpanded, { newExpanded = it }, onNew, onQuickTask)
        }
    }
}

/** Phone New orb: Quick task at the left, mode-specific New item at the right. */
@Composable
internal fun NewOrb(mode: ShellMode, expanded: Boolean, onExpandedChange: (Boolean) -> Unit, onNew: () -> Unit, onQuickTask: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Box(Modifier.size(44.dp)) {
        if (expanded) {
            Popup(alignment = Alignment.TopEnd, onDismissRequest = { onExpandedChange(false) }, properties = PopupProperties(focusable = true)) {
                Row(
                    Modifier.clip(CircleShape).background(c.bgPrimary)
                        .border(1.dp, c.borderSubtle, CircleShape).testTag("newOrbPill"),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(2.dp),
                ) {
                    Text(ModeHubSpec.QUICK_TASK_LABEL, color = c.textPrimary, fontSize = 12.sp,
                        modifier = Modifier.heightIn(min = 44.dp).clip(CircleShape).clickable(role = Role.Button) {
                            onExpandedChange(false)
                            HubHaptic.Medium.play(view)
                            onQuickTask()
                        }.padding(horizontal = 12.dp, vertical = 14.dp).testTag("newOrbQuickTask"))
                    Text(ModeHubSpec.newItem(mode), color = c.textPrimary, fontSize = 12.sp,
                        modifier = Modifier.heightIn(min = 44.dp).clip(CircleShape).clickable(role = Role.Button) {
                            onExpandedChange(false)
                            HubHaptic.Medium.play(view)
                            onNew()
                        }.padding(horizontal = 12.dp, vertical = 14.dp).testTag("newOrbNew"))
                }
            }
        } else {
            Box(Modifier.size(44.dp).testTag("newOrb").clickable(role = Role.Button, onClickLabel = ModeHubSpec.newItem(mode)) {
                HubHaptic.Light.play(view); onExpandedChange(true)
            }, contentAlignment = Alignment.Center) {
                Icon(painterResource(DesignR.drawable.ph_note_pencil), ModeHubSpec.newItem(mode), tint = c.textPrimary, modifier = Modifier.size(18.dp))
            }
        }
    }
}
