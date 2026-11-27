package com.allternit.feature.bots.runtimes

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.ProviderLogoMark
import com.allternit.feature.artifacts.LibraryLayout
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.bots.BotAvatar
import com.allternit.core.design.R as DesignR

/** What the chip's 6dp dot says (decision 8): green = can run now, amber = needs a step, coral = running (Code only). */
enum class RuntimeDot(val label: String) { Ready("ready"), NeedsStep("needs a step"), Running("running") }

/** Maps the existing availability check to the dot. */
fun RuntimeCheck?.toDot(): RuntimeDot = if (this == null || this == RuntimeCheck.Ready) RuntimeDot.Ready else RuntimeDot.NeedsStep

/** The warning triangle: the chosen brain's provider isn't one the runtime runs (the "Not with <Runtime>" rule), or the runtime reports a limited status. */
val AgentLayersModel.misfit: Boolean
    get() = brainProvider?.let { !fits(runtime, it) } == true || (runtime?.let { it.statusTone == "warn" && it.statusLabel != null } == true)

private val DotGreen = Color(0xFF1F6F45)
private val DotAmber = Color(0xFFB7791F)
private val DotCoral = Color(0xFFD9694F)

/**
 * The one composer chip: runtime mark + name | brain, a chevron and a state dot (mockup option 2).
 * Visible height 24dp inside a 48dp touch target. [background] is the surface the chip sits on.
 * A Connected bot shows a locked "Run by <vendor>" and does not open.
 */
@Composable
fun RuntimeBrainChip(
    runtimeKey: String,
    runtimeName: String,
    brainName: String,
    background: Color,
    modifier: Modifier = Modifier,
    dot: RuntimeDot = RuntimeDot.Ready,
    warning: Boolean = false,
    lockedVendor: String? = null,
    iconOnly: Boolean = false,
    /** Start anywhere: the picked computer, shown as one leading segment only when a computer is picked. */
    computerName: String? = null,
    computerCloud: Boolean = false,
    onClick: () -> Unit,
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val locked = lockedVendor != null
    val desc = if (locked) "Runtime and brain: run by $lockedVendor, locked"
    else (computerName?.let { "Computer: $it. " } ?: "") +
        "Runtime and brain: $runtimeName runtime, $brainName brain, ${dot.label}${if (warning) ", this brain may not fit this runtime" else ""}"
    Box(
        modifier.heightIn(min = 48.dp).widthIn(min = 48.dp)
            .clickable(enabled = !locked, role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .semantics(mergeDescendants = true) { contentDescription = desc }
            .testTag("composerChip-runtimeBrain"),
        contentAlignment = Alignment.Center,
    ) {
        Row(
            Modifier.heightIn(min = 24.dp).clip(RoundedCornerShape(12.dp)).background(background)
                .padding(horizontal = 8.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            if (locked) {
                Icon(painterResource(DesignR.drawable.ph_lock), null, tint = c.textSecondary, modifier = Modifier.size(12.dp))
                Text("Run by $lockedVendor", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.Medium, maxLines = 1,
                    overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 160.dp))
            } else {
                if (computerName != null && !iconOnly) {
                    Icon(painterResource(if (computerCloud) DesignR.drawable.ph_cloud else DesignR.drawable.ph_desktop), null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
                    Text(computerName, color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.Medium, maxLines = 1,
                        overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 90.dp).testTag("composerChip-computer"))
                    Text("·", color = c.textTertiary, fontSize = 12.sp)
                }
                ProviderLogoMark(runtimeKey, 16.dp)
                if (!iconOnly) {
                    Text(runtimeName, color = c.textPrimary, fontSize = 12.sp, fontWeight = FontWeight.Medium, maxLines = 1,
                        overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 96.dp))
                    Box(Modifier.size(width = 1.dp, height = 12.dp).background(c.borderStrong))
                    // With a computer segment the brain text shrinks first on overflow (weighted children measure last).
                    Text(brainName, color = c.textSecondary, fontSize = 12.sp, maxLines = 1,
                        overflow = TextOverflow.Ellipsis, modifier = (if (computerName != null) Modifier.weight(1f, fill = false) else Modifier).widthIn(max = 96.dp))
                }
                if (warning) Icon(painterResource(DesignR.drawable.ph_warning), null, tint = DotAmber, modifier = Modifier.size(12.dp))
                if (!iconOnly) Icon(painterResource(DesignR.drawable.ph_caret_down), null, tint = c.textSecondary.copy(alpha = 0.8f), modifier = Modifier.size(10.dp))
                Box(Modifier.size(6.dp).clip(CircleShape).background(when (dot) { RuntimeDot.Ready -> DotGreen; RuntimeDot.NeedsStep -> DotAmber; RuntimeDot.Running -> DotCoral }))
            }
        }
    }
}

/**
 * The sheet "Runtime and brain". A bot chat shows the full [AgentLayersPicker] (Runtime, Brain, Bot);
 * a plain chat shows the Runtime and Brain layers only (no Bot row) and a last row that opens the
 * existing model picker for effort and limits. [onChange] gets every pick as a [LayersChange].
 */
@Composable
fun RuntimeBrainSheet(
    model: AgentLayersModel,
    plainChat: Boolean,
    note: String?,
    onDismiss: () -> Unit,
    onChange: (LayersChange) -> Unit,
    registryNotice: RegistryNotice? = null,
    brainsLoading: Boolean = false,
    onOpenConnections: (() -> Unit)? = null,
    onOpenModelPicker: (() -> Unit)? = null,
    extra: (@Composable () -> Unit)? = null,
    /** Start anywhere: the Computer row shown first in a plain chat (mockup E). */
    top: (@Composable () -> Unit)? = null,
    /** When set, the sheet shows this page (the shared device list) instead of the layers. */
    page: (@Composable () -> Unit)? = null,
) {
    val c = AllternitTheme.colors
    var query by remember { mutableStateOf("") }
    LibrarySheet(
        title = "Runtime and brain", testTag = "runtimeBrainSheet", onDismiss = onDismiss,
        trailing = LibrarySheetAction("Done", bold = true, testTag = "agentLayerDone", onClick = onDismiss),
    ) {
        Column(
            Modifier.fillMaxWidth().heightIn(max = 620.dp).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (page != null) {
                page()
            } else if (!plainChat) {
                AgentLayersPicker(
                    model, query, { query = it }, onChange, note, registryNotice = registryNotice, brainsLoading = brainsLoading,
                    onOpenConnections = onOpenConnections, botFace = null, showView = false,
                )
            } else {
                top?.invoke()
                listOf(AgentLayer.Runtime, AgentLayer.Brain).forEach { layer ->
                    Text("${layer.label} · ${layer.hint}", color = c.textSecondary, fontSize = 12.5.sp, fontWeight = FontWeight.SemiBold)
                    if (layer == AgentLayer.Runtime) registryNotice?.let { NoticeLine(it.message, "Try again", it.onRetry, warn = true) }
                    if (layer == AgentLayer.Brain && !brainsLoading && model.brains.isEmpty()) {
                        NoticeLine("No brains yet. Connect a plan, an API key or a local model.", if (onOpenConnections != null) "Open Connections" else null, onOpenConnections, warn = false)
                    }
                    ChoiceLayout(model.choices(layer), LibraryLayout.List, layer.label, botFace = { b, s -> BotAvatar(b, s) }) { ch ->
                        when (layer) {
                            AgentLayer.Runtime -> model.runtimes.firstOrNull { it.id == ch.id }?.let { onChange(model.pickRuntime(it)) }
                            AgentLayer.Brain -> model.brains.firstOrNull { it.key == ch.id }?.let { onChange(model.pickBrain(it)) }
                            AgentLayer.Bot -> Unit
                        }
                    }
                }
                onOpenModelPicker?.let { open ->
                    Row(
                        Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral)
                            .clickable(role = Role.Button) { open() }.padding(horizontal = 12.dp).testTag("runtimeBrainMoreModels"),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text("Model effort and limits", color = c.textPrimary, fontSize = 14.sp, modifier = Modifier.weight(1f))
                        Icon(painterResource(DesignR.drawable.ph_caret_down), null, tint = c.textSecondary, modifier = Modifier.size(10.dp))
                    }
                }
                note?.let { SwitchNote(it) }
                extra?.invoke()
                Text(model.sentence, color = c.textSecondary, fontSize = 13.sp)
            }
        }
    }
}
