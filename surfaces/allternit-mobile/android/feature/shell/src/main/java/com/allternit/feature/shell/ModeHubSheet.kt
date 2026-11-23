package com.allternit.feature.shell

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.spring
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme

/** One preview line in a hub section (a chat, a project, a bot). [dot] is the status dot. */
internal data class HubItem(
    val key: String,
    val title: String,
    val subtitle: String? = null,
    val trailing: String? = null,
    val dot: Color? = null,
)

/**
 * The mode hub, phone variant of the mockup's `HubShelves` (base2.jsx): a glass sheet that rises from the
 * bottom edge. From top to bottom: "Mode > Section" with its More link, up to [ModeHubSpec.OPEN_ITEMS]
 * rows of the selected section, a row of section cards, and the four modes at the bottom.
 *
 * Touch rules: first tap on a card selects it, a second tap opens its screen. Tapping a mode that is not the
 * one on show browses it (selection haptic); tapping the one on show switches to it ([onPickMode]). Sections
 * with no Android screen are already filtered by [ModeHubSpec.sections].
 *
 * The host supplies the data: [itemsFor] returns the real items of a section (empty = "Nothing here yet").
 * The host decides what each callback does and closes the sheet (A2/A3); this file never navigates.
 * Reduced motion: no slide, no springs, states change at once. Back and a tap on the scrim call [onDismiss].
 */
@Composable
internal fun ModeHubSheet(
    visible: Boolean,
    currentMode: ShellMode,
    itemsFor: (ShellMode, HubSection) -> List<HubItem>,
    onPickMode: (ShellMode) -> Unit,
    onOpenSection: (ShellMode, HubSection) -> Unit,
    onOpenItem: (ShellMode, HubSection, HubItem) -> Unit,
    onDismiss: () -> Unit,
    modifier: Modifier = Modifier,
    botsWaiting: Int = 0,
) {
    val reduced = rememberHubReducedMotion()
    BackHandler(enabled = visible, onBack = onDismiss)
    val scrimAlpha by animateFloatAsState(
        targetValue = if (visible) 0.32f else 0f,
        animationSpec = if (reduced) snap() else spring(ModeHubSpec.SHEET_DAMPING_RATIO, ModeHubSpec.SHEET_STIFFNESS),
        label = "hubScrim",
    )
    Box(modifier = modifier.fillMaxSize()) {
        if (scrimAlpha > 0f) {
            Box(
                Modifier
                    .fillMaxSize()
                    .background(Color.Black.copy(alpha = scrimAlpha))
                    .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, onClick = onDismiss)
                    .semantics { contentDescription = "Closes the mode hub"; role = Role.Button }
                    .testTag("modeHubScrim"),
            )
        }
        AnimatedVisibility(
            visible = visible,
            modifier = Modifier.align(Alignment.BottomCenter),
            enter = if (reduced) fadeIn(snap()) else slideInVertically(spring(ModeHubSpec.SHEET_DAMPING_RATIO, ModeHubSpec.SHEET_STIFFNESS)) { it } + fadeIn(),
            exit = if (reduced) fadeOut(snap()) else slideOutVertically(spring(ModeHubSpec.SHEET_DAMPING_RATIO, ModeHubSpec.SHEET_STIFFNESS)) { it } + fadeOut(),
        ) {
            HubPanel(currentMode, itemsFor, onPickMode, onOpenSection, onOpenItem, botsWaiting, reduced)
        }
    }
}

@Composable
private fun HubPanel(
    currentMode: ShellMode,
    itemsFor: (ShellMode, HubSection) -> List<HubItem>,
    onPickMode: (ShellMode) -> Unit,
    onOpenSection: (ShellMode, HubSection) -> Unit,
    onOpenItem: (ShellMode, HubSection, HubItem) -> Unit,
    botsWaiting: Int,
    reduced: Boolean,
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    var browse by remember { mutableStateOf(currentMode) }
    var sectionId by remember { mutableStateOf(ModeHubSpec.sections(currentMode).first().id) }
    val sections = ModeHubSpec.sections(browse)
    val section = sections.firstOrNull { it.id == sectionId } ?: sections.first()
    val items = remember(browse, section.id) { itemsFor(browse, section) }
    val shape = RoundedCornerShape(26.dp)

    Column(
        modifier = Modifier
            .navigationBarsPadding()
            .padding(horizontal = 8.dp, vertical = 8.dp)
            .fillMaxWidth()
            .shadow(18.dp, shape, ambientColor = Color.Black.copy(alpha = 0.16f), spotColor = Color.Black.copy(alpha = 0.16f))
            .clip(shape)
            .background(c.bgPrimary.copy(alpha = 0.96f))
            .border(BorderStroke(1.dp, c.borderSubtle), shape)
            .padding(8.dp)
            .testTag("modeHubSheet"),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        // Mode > Section, with the section's More link.
        Row(Modifier.fillMaxWidth().padding(start = 6.dp, top = 2.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(
                "${browse.label} › ${section.label}",
                color = c.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            Text(
                "${section.more} ›",
                color = c.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.Medium, maxLines = 1,
                modifier = Modifier
                    .clip(CircleShape)
                    .clickable(role = Role.Button, onClickLabel = "Opens ${section.label}") {
                        HubHaptic.Medium.play(view)
                        onOpenSection(browse, section)
                    }
                    .padding(horizontal = 8.dp, vertical = 6.dp)
                    .testTag("modeHubMore"),
            )
        }
        // Rows: fixed height so the sheet never changes size under the finger.
        Column(Modifier.fillMaxWidth().height((ModeHubSpec.OPEN_ITEMS * ModeHubSpec.ROW_HEIGHT_DP).dp)) {
            if (items.isEmpty()) {
                Box(Modifier.fillMaxWidth().height(ModeHubSpec.ROW_HEIGHT_DP.dp).padding(horizontal = 9.dp), contentAlignment = Alignment.CenterStart) {
                    Text("Nothing here yet", color = c.textTertiary, fontSize = 12.5.sp)
                }
            } else {
                items.take(ModeHubSpec.OPEN_ITEMS).forEach { item ->
                    ItemRow(item) {
                        HubHaptic.Medium.play(view)
                        onOpenItem(browse, section, item)
                    }
                }
            }
        }
        // Cards: first tap selects, second opens.
        LazyRow(horizontalArrangement = Arrangement.spacedBy(6.dp), contentPadding = PaddingValues(end = 24.dp)) {
            items(sections, key = { it.id }) { s ->
                val on = s.id == section.id
                SectionCard(
                    section = s,
                    on = on,
                    preview = remember(browse, s.id) { itemsFor(browse, s) },
                    reduced = reduced,
                    onClick = {
                        if (on) {
                            HubHaptic.Medium.play(view)
                            onOpenSection(browse, s)
                        } else {
                            HubHaptic.Light.play(view)
                            sectionId = s.id
                        }
                    },
                )
            }
        }
        // Modes at the bottom.
        Row(
            Modifier.fillMaxWidth().clip(RoundedCornerShape(20.dp)).background(c.fillNeutral.copy(alpha = 0.5f)).padding(3.dp),
            horizontalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            ModeHubSpec.modes.forEach { m ->
                ModeButton(
                    mode = m,
                    on = m == browse,
                    badge = if (m == ShellMode.Bots) botsWaiting else 0,
                    reduced = reduced,
                    modifier = Modifier.weight(1f),
                ) {
                    if (m == browse) {
                        HubHaptic.Medium.play(view)
                        onPickMode(m)
                    } else {
                        HubHaptic.Selection.play(view)
                        browse = m
                        sectionId = ModeHubSpec.sections(m).first().id
                    }
                }
            }
        }
    }
}

@Composable
private fun ItemRow(item: HubItem, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        Modifier
            .fillMaxWidth()
            .height(ModeHubSpec.ROW_HEIGHT_DP.dp)
            .clip(RoundedCornerShape(10.dp))
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 9.dp)
            .testTag("modeHubItem${item.key}"),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (item.dot != null) Box(Modifier.size(6.dp).clip(CircleShape).background(item.dot))
        Column(Modifier.weight(1f)) {
            Text(item.title, color = c.textPrimary, fontSize = 12.5.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (item.subtitle != null) Text(item.subtitle, color = c.textTertiary, fontSize = 10.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (item.trailing != null) Text(item.trailing, color = c.textTertiary, fontSize = 10.5.sp, maxLines = 1)
    }
}

@Composable
private fun SectionCard(section: HubSection, on: Boolean, preview: List<HubItem>, reduced: Boolean, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val bg by animateColorAsState(
        if (on) c.fillNeutral else c.bgPrimary.copy(alpha = 0.6f),
        if (reduced) snap() else spring(ModeHubSpec.LENS_DAMPING_RATIO, ModeHubSpec.LENS_STIFFNESS),
        label = "cardLens",
    )
    val shape = RoundedCornerShape(14.dp)
    val first = preview.firstOrNull()
    val more = (preview.size - 1).coerceAtLeast(0)
    Column(
        Modifier
            .width(ModeHubSpec.CARD_WIDTH_DP.dp)
            .height(ModeHubSpec.CARD_HEIGHT_DP.dp)
            .clip(shape)
            .background(bg)
            .border(BorderStroke(1.dp, if (on) Color.Transparent else c.borderSubtle), shape)
            .clickable(role = Role.Button, onClickLabel = if (on) "Opens ${section.label}" else "Selects ${section.label}", onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 8.dp)
            .semantics { selected = on; contentDescription = section.label }
            .testTag("modeHubCard${section.id}"),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Icon(painterResource(section.icon), null, tint = if (on) c.textPrimary else c.textSecondary, modifier = Modifier.size(13.dp))
            Text(section.short, color = c.textPrimary, fontSize = 12.sp, fontWeight = if (on) FontWeight.Bold else FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        Text(first?.title ?: "Nothing yet", color = c.textSecondary, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(if (more > 0) "+$more more" else first?.subtitle ?: " ", color = c.textTertiary, fontSize = 10.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

@Composable
private fun ModeButton(mode: ShellMode, on: Boolean, badge: Int, reduced: Boolean, modifier: Modifier, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val lens by animateColorAsState(
        if (on) c.bgPrimary else Color.Transparent,
        if (reduced) snap() else spring(ModeHubSpec.LENS_DAMPING_RATIO, ModeHubSpec.LENS_STIFFNESS),
        label = "modeLens",
    )
    Box(
        modifier
            .clip(RoundedCornerShape(17.dp))
            .background(lens)
            .clickable(role = Role.Tab, onClickLabel = if (on) "Switches to ${mode.label}" else "Shows ${mode.label}", onClick = onClick)
            .padding(vertical = 9.dp, horizontal = 4.dp)
            .semantics { selected = on; contentDescription = mode.label }
            .testTag("modeHubMode${mode.name}"),
        contentAlignment = Alignment.Center,
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Icon(painterResource(mode.icon), null, tint = if (on) mode.accent else c.textSecondary, modifier = Modifier.size(15.dp))
            Text(mode.label, color = if (on) c.textPrimary else c.textSecondary, fontSize = 12.sp, fontWeight = if (on) FontWeight.Bold else FontWeight.Medium, maxLines = 1)
            if (badge > 0) ModeDialBadge(badge)
        }
    }
}
