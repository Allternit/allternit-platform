package com.allternit.feature.bots.runtimes

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.Crossfade
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.feature.artifacts.LibraryLayout
import com.allternit.feature.artifacts.LibraryLayoutSwitch
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.bots.BotAvatar
import com.allternit.core.design.ProviderLogoMark
import com.allternit.core.design.R as DesignR

/*
 * The layered picker (approved mockup a5, web AgentCard, iOS BotBrainPickerSheet), phone shape:
 * header (bot, kind, View segmented Allternit / Runtime; no Split on phone), search, the List · Grid ·
 * Deck switch reused from Connections (#2755), then Runtime / Brain / Bot rows on a rail that runs
 * down to the Allternit control layer line, the auto-switch note and the plain sentence.
 * Grid is two columns. A Connected bot locks Runtime and Brain to "Run by <Vendor>".
 */

private const val LAYOUT_KEY = "agent-card-layout"

/** Registry problems the picker shows above the runtime choices (web RuntimeIssueNotice). */
data class RegistryNotice(val message: String, val onRetry: () -> Unit)

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun AgentLayersPicker(
    model: AgentLayersModel,
    query: String,
    onQuery: (String) -> Unit,
    onChange: (LayersChange) -> Unit,
    note: String?,
    modifier: Modifier = Modifier,
    runtimesLoading: Boolean = false,
    registryNotice: RegistryNotice? = null,
    brainsLoading: Boolean = false,
    brainsError: String? = null,
    onOpenConnections: (() -> Unit)? = null,
    /** The bot's face for the header and the Bot row; null draws the initial. */
    botFace: (@Composable (Dp) -> Unit)? = null,
    /** Show the View segmented control (bot page / form). */
    showView: Boolean = true,
) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val reduceMotion = rememberReduceMotion()
    var layout by remember { mutableStateOf(LibraryLayout.load(context, LAYOUT_KEY, LibraryLayout.Grid)) }
    var open by rememberSaveable { mutableStateOf<AgentLayer?>(AgentLayer.Runtime) }

    Column(modifier.fillMaxWidth().testTag("agentLayers"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        // Header: face, name, kind; View segmented.
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Box(Modifier.size(40.dp), contentAlignment = Alignment.Center) { botFace?.invoke(40.dp) ?: InitialTile(model.botName, 40.dp) }
            Column(Modifier.weight(1f)) {
                Text(model.botName, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(if (model.locked) "Connected bot · ${model.vendor}" else "Allternit bot", color = c.textSecondary, fontSize = 12.5.sp, maxLines = 1)
            }
            if (showView) ViewSegmented(model.view, runtimeLabel = if (model.locked) "${model.botName} view" else "Runtime view") { onChange(model.pickView(it)) }
        }
        // Search + layout.
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            SearchField(query, onQuery, Modifier.weight(1f))
        }
        LibraryLayoutSwitch(layout) { layout = it; LibraryLayout.save(context, LAYOUT_KEY, it) }

        // The rail: Runtime, Brain, Bot, then the control layer line.
        Column(Modifier.fillMaxWidth()) {
            AgentLayer.entries.forEach { layer ->
                val lockedRow = model.locked && layer != AgentLayer.Bot
                RailRow(last = false) {
                    LayerHeader(
                        layer = layer, value = model.chipText(layer), locked = lockedRow, expanded = open == layer && !lockedRow,
                        mark = when (layer) { AgentLayer.Runtime -> model.runtime?.let { it.tool ?: it.id } ?: model.runtimeId; AgentLayer.Brain -> model.brainProvider ?: "allternit"; AgentLayer.Bot -> model.botName },
                        face = if (layer == AgentLayer.Bot) botFace else null,
                    ) { open = if (open == layer) null else layer }
                    AnimatedVisibility(
                        visible = open == layer && !lockedRow,
                        enter = if (reduceMotion) fadeIn(snap()) else expandVertically(tween(200)) + fadeIn(tween(160)),
                        exit = if (reduceMotion) fadeOut(snap()) else shrinkVertically(tween(180)) + fadeOut(tween(120)),
                    ) {
                        Column(Modifier.padding(top = 8.dp, bottom = 6.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            when (layer) {
                                AgentLayer.Runtime -> registryNotice?.let { NoticeLine(it.message, "Try again", it.onRetry, warn = true) }
                                AgentLayer.Brain -> when {
                                    brainsError != null -> NoticeLine(brainsError, null, null, warn = true)
                                    !brainsLoading && model.brains.isEmpty() -> NoticeLine(
                                        "No brains yet. Connect a plan, an API key or a local model.",
                                        if (onOpenConnections != null) "Open Connections" else null, onOpenConnections, warn = false,
                                    )
                                }
                                AgentLayer.Bot -> Unit
                            }
                            val loading = (layer == AgentLayer.Runtime && runtimesLoading && model.runtimeChoices.isEmpty()) ||
                                (layer == AgentLayer.Brain && brainsLoading && model.brains.isEmpty())
                            Crossfade(layout, animationSpec = if (reduceMotion) snap() else tween(180), label = "agentLayersLayout") { lay ->
                                if (loading) SkeletonTiles(lay, layer.label)
                                else ChoiceLayout(model.choices(layer), lay, layer.label, botFace = { b, s -> BotAvatar(b, s) }) { ch ->
                                    when (layer) {
                                        AgentLayer.Runtime -> model.runtimes.firstOrNull { it.id == ch.id }?.let { onChange(model.pickRuntime(it)) }
                                        AgentLayer.Brain -> model.brains.firstOrNull { it.key == ch.id }?.let { onChange(model.pickBrain(it)) }
                                        AgentLayer.Bot -> onChange(model.pickBot(ch.id))
                                    }
                                }
                            }
                            if (layer == AgentLayer.Brain && model.brains.size > 48 && query.isBlank()) {
                                Text("Showing 48 of ${model.brains.size}. Search to find the rest.", color = c.textTertiary, fontSize = 12.sp)
                            }
                        }
                    }
                }
            }
            RailRow(last = true) {
                Text(
                    "Allternit control layer · policy, tool approvals, memory, provenance, billing",
                    color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.padding(vertical = 6.dp),
                )
            }
        }
        note?.let { SwitchNote(it) }
        Text(model.sentence, color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.testTag("agentLayersSentence"))
    }
}

/** One layer's choices in a sheet (the composer chips open these). */
@Composable
fun AgentLayerSheet(
    model: AgentLayersModel,
    layer: AgentLayer,
    note: String?,
    onDismiss: () -> Unit,
    onChange: (LayersChange) -> Unit,
    registryNotice: RegistryNotice? = null,
    brainsLoading: Boolean = false,
    onOpenConnections: (() -> Unit)? = null,
    /** A plain chat's runtime availability line (pair a computer / not installed), drawn above the sentence. */
    extra: (@Composable () -> Unit)? = null,
) {
    val c = AllternitTheme.colors
    LibrarySheet(
        title = "${layer.label} · ${layer.hint}", testTag = "agentLayerSheet${layer.name}", onDismiss = onDismiss,
        trailing = LibrarySheetAction("Done", bold = true, testTag = "agentLayerDone", onClick = onDismiss),
    ) {
        Column(
            Modifier.fillMaxWidth().heightIn(max = 620.dp).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (layer == AgentLayer.Runtime) registryNotice?.let { NoticeLine(it.message, "Try again", it.onRetry, warn = true) }
            if (layer == AgentLayer.Brain && !brainsLoading && model.brains.isEmpty()) {
                NoticeLine("No brains yet. Connect a plan, an API key or a local model.", if (onOpenConnections != null) "Open Connections" else null, onOpenConnections, warn = false)
            }
            if (layer == AgentLayer.Brain && brainsLoading && model.brains.isEmpty()) SkeletonTiles(LibraryLayout.List, layer.label)
            else ChoiceLayout(model.choices(layer), LibraryLayout.List, layer.label, botFace = { b, s -> BotAvatar(b, s) }) { ch ->
                when (layer) {
                    AgentLayer.Runtime -> model.runtimes.firstOrNull { it.id == ch.id }?.let { onChange(model.pickRuntime(it)) }
                    AgentLayer.Brain -> model.brains.firstOrNull { it.key == ch.id }?.let { onChange(model.pickBrain(it)) }
                    AgentLayer.Bot -> onChange(model.pickBot(ch.id))
                }
            }
            note?.let { SwitchNote(it) }
            extra?.invoke()
            Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Box(Modifier.padding(top = 6.dp).size(6.dp).clip(CircleShape).background(Color(0xFF1F6F45)))
                Text(model.sentence, color = c.textSecondary, fontSize = 13.sp)
            }
        }
    }
}

// ─── Parts ─────────────────────────────────────────────────────────────────────────────────

/** A row on the rail: a node and the line down to the next row (the last row ends the line). */
@Composable
private fun RailRow(last: Boolean, content: @Composable () -> Unit) {
    val c = AllternitTheme.colors
    val line = c.borderSubtle
    Row(
        Modifier.fillMaxWidth().drawBehind {
            // The rail runs from this node down to the next one; the control layer line ends it.
            val x = 11.dp.toPx()
            val top = if (last) 0f else 20.dp.toPx()
            val bottom = if (last) 10.dp.toPx() else size.height
            drawLine(line, Offset(x, top), Offset(x, bottom), strokeWidth = 1.5.dp.toPx())
        },
    ) {
        Box(Modifier.width(22.dp)) {
            Box(
                Modifier.align(Alignment.TopCenter).padding(top = if (last) 10.dp else 18.dp).size(if (last) 9.dp else 11.dp).clip(CircleShape)
                    .background(if (last) c.textPrimary else c.bgPrimary).border(1.5.dp, c.textPrimary, CircleShape),
            )
        }
        Column(Modifier.weight(1f).padding(start = 6.dp)) { content() }
    }
}

@Composable
private fun LayerHeader(layer: AgentLayer, value: String, locked: Boolean, expanded: Boolean, mark: String, face: (@Composable (Dp) -> Unit)?, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(RoundedCornerShape(12.dp))
            .then(if (locked) Modifier else Modifier.clickable(role = Role.Button, onClickLabel = if (expanded) "Hide ${layer.label.lowercase()} choices" else "Show ${layer.label.lowercase()} choices") { Haptics.perform(view, Haptics.Kind.Tap); onClick() })
            .semantics(mergeDescendants = true) {
                heading()
                contentDescription = "${layer.label}, ${layer.hint}: $value${if (locked) ", locked" else ""}"
                if (!locked) stateDescription = if (expanded) "Expanded" else "Collapsed"
            }
            .testTag("agentLayer${layer.name}")
            .padding(horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(layer.label, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
        Row(
            Modifier.clip(CircleShape).background(c.fillNeutral).padding(horizontal = 10.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            if (locked) Icon(painterResource(DesignR.drawable.ph_lock), null, tint = c.textSecondary, modifier = Modifier.size(13.dp))
            else if (face != null) Box(Modifier.size(18.dp)) { face(18.dp) } else Mark(mark, 18.dp)
            Text(value, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 150.dp))
            if (!locked) Icon(painterResource(DesignR.drawable.ph_caret_down), null, tint = c.textSecondary, modifier = Modifier.size(11.dp).rotate(if (expanded) 180f else 0f))
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun ChoiceLayout(choices: List<LayerChoice>, layout: LibraryLayout, label: String, botFace: @Composable (com.allternit.feature.bots.BotRecord, Dp) -> Unit, onPick: (LayerChoice) -> Unit) {
    val c = AllternitTheme.colors
    if (choices.isEmpty()) {
        com.allternit.core.design.InlineEmpty("Nothing matches.", art = com.allternit.core.design.EmptyArtKind.Search)
        return
    }
    when (layout) {
        LibraryLayout.List -> Column(
            Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).border(1.dp, c.borderSubtle, RoundedCornerShape(14.dp)).semantics { contentDescription = label },
        ) {
            var lastGroup: String? = null
            choices.forEachIndexed { i, ch ->
                if (ch.group != null && ch.group != lastGroup) {
                    Text(ch.group, color = c.textTertiary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(start = 12.dp, top = 10.dp, bottom = 2.dp).semantics { heading() })
                    lastGroup = ch.group
                }
                ChoiceRowItem(ch, botFace, onPick)
                if (i < choices.lastIndex) Box(Modifier.fillMaxWidth().padding(start = 52.dp).height(1.dp).background(c.borderSubtle))
            }
        }
        LibraryLayout.Grid -> Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            choices.chunked(2).forEach { pair ->
                Row(Modifier.fillMaxWidth().height(IntrinsicSize.Min), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    pair.forEach { ch -> ChoiceCard(ch, botFace, Modifier.weight(1f).fillMaxHeight(), onPick) }
                    if (pair.size == 1) Spacer(Modifier.weight(1f))
                }
            }
        }
        LibraryLayout.Deck -> Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            choices.forEach { ch -> ChoiceCard(ch, botFace, Modifier.width(220.dp).heightIn(min = 150.dp), onPick) }
        }
    }
}

private fun choiceDescription(ch: LayerChoice): String =
    ch.name + ", " + ch.detail + (if (ch.badges.isNotEmpty()) ", " + ch.badges.joinToString(", ") { it.first } else "") + (if (!ch.ok) ", not available" else "")

@Composable
private fun ChoiceRowItem(ch: LayerChoice, botFace: @Composable (com.allternit.feature.bots.BotRecord, Dp) -> Unit, onPick: (LayerChoice) -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        Modifier.fillMaxWidth().heightIn(min = 56.dp)
            .background(if (ch.selected) c.fillNeutral else Color.Transparent)
            .clickable(enabled = ch.ok, role = Role.RadioButton) { Haptics.perform(view, Haptics.Kind.Tap); onPick(ch) }
            .semantics(mergeDescendants = true) { contentDescription = choiceDescription(ch); selected = ch.selected }
            .testTag("agentChoice${ch.id}")
            .padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Box(Modifier.size(30.dp), contentAlignment = Alignment.Center) { ch.bot?.let { botFace(it, 30.dp) } ?: Mark(ch.mark, 30.dp) }
        Column(Modifier.weight(1f)) {
            Text(ch.name, color = if (ch.ok) c.textPrimary else c.textTertiary, fontSize = 14.5.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(ch.detail, color = c.textTertiary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        ch.badges.lastOrNull()?.let { Badge(it.first, it.second) }
        if (ch.selected) Icon(painterResource(DesignR.drawable.ph_check), null, tint = c.textPrimary, modifier = Modifier.size(16.dp))
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ChoiceCard(ch: LayerChoice, botFace: @Composable (com.allternit.feature.bots.BotRecord, Dp) -> Unit, modifier: Modifier, onPick: (LayerChoice) -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val shape = RoundedCornerShape(16.dp)
    Column(
        modifier.clip(shape).background(c.bgPanel)
            .border(if (ch.selected) 1.5.dp else 1.dp, if (ch.selected) c.textPrimary else c.borderSubtle, shape)
            .clickable(enabled = ch.ok, role = Role.RadioButton) { Haptics.perform(view, Haptics.Kind.Tap); onPick(ch) }
            .semantics(mergeDescendants = true) { contentDescription = choiceDescription(ch); selected = ch.selected }
            .testTag("agentChoice${ch.id}")
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.Top) {
            Box(Modifier.size(28.dp), contentAlignment = Alignment.Center) { ch.bot?.let { botFace(it, 28.dp) } ?: Mark(ch.mark, 28.dp) }
            Spacer(Modifier.weight(1f))
            if (ch.selected) Box(Modifier.size(20.dp).clip(CircleShape).background(c.textPrimary), contentAlignment = Alignment.Center) {
                Icon(painterResource(DesignR.drawable.ph_check), null, tint = c.bgPrimary, modifier = Modifier.size(12.dp))
            }
        }
        Text(ch.name, color = if (ch.ok) c.textPrimary else c.textTertiary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(ch.detail, color = c.textTertiary, fontSize = 11.5.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
        Spacer(Modifier.weight(1f))
        FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            ch.badges.forEach { Badge(it.first, it.second) }
        }
    }
}

@Composable
private fun Badge(text: String, tone: BadgeTone) {
    val dark = AllternitTheme.colors.bgPrimary.luminance() < 0.4f
    val c = AllternitTheme.colors
    // Tones from the web card (tinted status hues), readable in light and graphite dark.
    val (fg, bg) = when (tone) {
        BadgeTone.Live -> if (dark) Color(0xFF7CC39C) to Color(0x247CC39C) else Color(0xFF1F6F45) to Color(0x1A1F6F45)
        BadgeTone.Warn -> if (dark) Color(0xFFD9B766) to Color(0x24D9B766) else Color(0xFF8A5A00) to Color(0x1FC88C00)
        BadgeTone.Plan -> if (dark) Color(0xFF7CC39C) to Color(0x247CC39C) else Color(0xFF2F6F4F) to Color(0x1A2F6F4F)
        BadgeTone.Key -> if (dark) Color(0xFFD9B766) to Color(0x24D9B766) else Color(0xFF7A5A14) to Color(0x1F967828)
        BadgeTone.Local -> if (dark) Color(0xFF8DB8E2) to Color(0x248DB8E2) else Color(0xFF2B5C8A) to Color(0x1A2B5C8A)
        BadgeTone.Credits -> if (dark) Color(0xFFB9A1E6) to Color(0x24B9A1E6) else Color(0xFF5B3F8F) to Color(0x1A5B3F8F)
        BadgeTone.Neutral -> c.textSecondary to c.fillNeutral
    }
    Box(Modifier.height(20.dp).clip(CircleShape).background(bg).padding(horizontal = 8.dp), contentAlignment = Alignment.Center) {
        Text(text, color = fg, fontSize = 11.sp, fontWeight = FontWeight.Medium, maxLines = 1)
    }
}

/** A runtime or provider mark: its brand logo, else a neutral glyph (iOS ModelProviderMark). */
@Composable
internal fun Mark(key: String, size: Dp) = ProviderLogoMark(key, size)

@Composable
private fun InitialTile(name: String, size: Dp) {
    val c = AllternitTheme.colors
    Box(Modifier.size(size).clip(CircleShape).background(c.fillNeutral), contentAlignment = Alignment.Center) {
        Text(name.take(1).uppercase(), color = c.textPrimary, fontSize = (size.value * 0.42f).sp, fontWeight = FontWeight.SemiBold)
    }
}

/** View: Allternit / Runtime (Split is desktop-only, hidden below 900 px — plan D9). */
@Composable
fun ViewSegmented(view: String, runtimeLabel: String = "Runtime view", onPick: (String) -> Unit) {
    val c = AllternitTheme.colors
    val haptic = LocalView.current
    Row(Modifier.clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(2.dp).semantics { contentDescription = "View" }, horizontalArrangement = Arrangement.spacedBy(2.dp)) {
        listOf(
            Triple("allternit", "Allternit view", DesignR.drawable.ph_chat_circle),
            Triple("runtime", runtimeLabel, DesignR.drawable.ph_terminal_window),
        ).forEach { (v, label, icon) ->
            val on = view == v || (v == "allternit" && view == "split")
            Box(
                Modifier.size(48.dp).clip(RoundedCornerShape(10.dp)).background(if (on) c.bgPrimary else Color.Transparent)
                    .clickable(role = Role.Tab) { Haptics.perform(haptic, Haptics.Kind.Tap); onPick(v) }
                    .semantics { contentDescription = label; selected = on }
                    .testTag("agentView$v"),
                contentAlignment = Alignment.Center,
            ) { Icon(painterResource(icon), null, tint = if (on) c.textPrimary else c.textSecondary, modifier = Modifier.size(18.dp)) }
        }
    }
}

@Composable
private fun SearchField(query: String, onQuery: (String) -> Unit, modifier: Modifier) {
    val c = AllternitTheme.colors
    Row(
        modifier.heightIn(min = 48.dp).clip(CircleShape).background(c.fillNeutral).padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(painterResource(DesignR.drawable.ph_magnifying_glass), null, tint = c.textSecondary, modifier = Modifier.size(15.dp))
        Box(Modifier.weight(1f)) {
            if (query.isEmpty()) Text("Search runtimes, brains and bots", color = c.textTertiary, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            BasicTextField(
                value = query, onValueChange = onQuery, singleLine = true,
                textStyle = TextStyle(color = c.textPrimary, fontSize = 15.sp), cursorBrush = SolidColor(c.textPrimary),
                modifier = Modifier.fillMaxWidth().testTag("agentLayersSearch").semantics { contentDescription = "Search runtimes, brains and bots" },
            )
        }
    }
}

@Composable
internal fun SwitchNote(text: String) {
    val c = AllternitTheme.colors
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(horizontal = 12.dp, vertical = 10.dp)
            .semantics(mergeDescendants = true) {}.testTag("agentSwitchNote"),
        horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Top,
    ) {
        Icon(painterResource(DesignR.drawable.ph_info), null, tint = c.textSecondary, modifier = Modifier.padding(top = 1.dp).size(15.dp))
        Text(text, color = c.textPrimary, fontSize = 13.sp)
    }
}

@Composable
internal fun NoticeLine(text: String, action: String?, onAction: (() -> Unit)?, warn: Boolean) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Top) {
            Icon(painterResource(if (warn) DesignR.drawable.ph_warning_circle else DesignR.drawable.ph_info), null, tint = c.textSecondary, modifier = Modifier.padding(top = 1.dp).size(15.dp))
            Text(text, color = c.textSecondary, fontSize = 13.sp)
        }
        if (action != null && onAction != null) {
            Box(Modifier.heightIn(min = 48.dp).clickable(role = Role.Button, onClick = onAction).padding(start = 23.dp), contentAlignment = Alignment.CenterStart) {
                Text(action, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
            }
        }
    }
}

@Composable
private fun SkeletonTiles(layout: LibraryLayout, label: String) {
    val c = AllternitTheme.colors
    val tile = Modifier.clip(RoundedCornerShape(16.dp)).background(c.fillNeutral)
    Box(Modifier.semantics { contentDescription = "Loading ${label.lowercase()}" }) {
        when (layout) {
            LibraryLayout.List -> Column(verticalArrangement = Arrangement.spacedBy(8.dp)) { repeat(3) { Box(tile.fillMaxWidth().height(52.dp)) } }
            LibraryLayout.Grid -> Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                repeat(2) { Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) { repeat(2) { Box(tile.weight(1f).height(120.dp)) } } }
            }
            LibraryLayout.Deck -> Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) { repeat(2) { Box(tile.width(220.dp).height(150.dp)) } }
        }
    }
}
