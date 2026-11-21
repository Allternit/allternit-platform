package com.allternit.feature.office

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import com.allternit.feature.bots.BotModeStore
import com.allternit.feature.bots.BotStatusAvatar
import com.allternit.feature.bots.BotsOfficeKit
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlin.math.roundToInt

/** The sheet's two tabs (iOS OfficeThreadSheet.Tab). */
enum class OfficeSheetTab(val label: String) { Thread("Thread"), Screen("Screen") }

/** "Answered by <main bot> on autopilot" / "Drafted by <main bot>" (the autopilot line copy, iOS and web). */
internal fun autopilotLine(outcome: String, mainBot: String): String =
    if (outcome == "answer") "Answered by $mainBot on autopilot" else "Drafted by $mainBot"

/**
 * A bot's thread over the Workspace floor (step 26, iOS OfficeThreadSheet; product shape B on phone): a bottom sheet
 * that opens half way so the floor stays visible above it, and drags up to full. Its header carries the bot and its
 * live line, the autopilot answers and drafts for it, then the approval card ([OfficeApprovalCard]) when the bot is
 * waiting on the person, then two tabs: Thread (the bot's main chat from Bots mode, [BotsOfficeKit.Thread]: tool steps,
 * ask cards and the composer) and Screen (the bot's computer, [BotsOfficeKit.Screen]: View desktop, Take over / Hand
 * back). [onInset] reports how much of the floor the sheet covers (dp, = frame CSS px) after each settle.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun OfficeThreadSheet(
    botId: String,
    store: BotModeStore,
    kit: BotsOfficeKit,
    needs: OfficeNeeds,
    mainBotName: String?,
    onInset: (bottomDp: Int) -> Unit,
    onDone: (need: OfficeNeed, bubble: String?) -> Unit,
    onDismiss: () -> Unit,
) {
    val c = AllternitTheme.colors
    val bot = store.bot(botId) ?: return
    val state = rememberModalBottomSheetState(skipPartiallyExpanded = false)
    val density = LocalDensity.current
    var tab by rememberSaveable(botId) { mutableStateOf(OfficeSheetTab.Thread) }
    var refreshKey by remember(botId) { mutableIntStateOf(0) }
    var lines by remember(botId) { mutableStateOf<List<OfficeAutopilotLine>>(emptyList()) }
    val currentInset by rememberUpdatedState(onInset)
    LaunchedEffect(botId) {
        while (true) { lines = autopilotLines(botId); delay(15_000) }
    }

    val threads = store.threads(botId)
    val waiting = threads.firstOrNull { it.group == "waiting" }
    val working = threads.firstOrNull { it.group == "working" }
    val live = when {
        waiting != null -> waiting.statusLine ?: "Needs you"
        working != null -> working.statusLine ?: "Working"
        else -> "Ready"
    }
    val need = needs.firstFor(botId)
    val more = (needs.items.count { it.botId == botId } - 1).coerceAtLeast(0)
    val main = mainBotName ?: "your main bot"

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = state,
        containerColor = c.bgPrimary,
        // A faint scrim: the floor stays readable above the half-open sheet.
        scrimColor = Color.Black.copy(alpha = 0.08f),
        shape = RoundedCornerShape(topStart = 24.dp, topEnd = 24.dp),
    ) {
        // The sheet's full height is fixed (so its stops never move); the column inside is cut to the part on screen,
        // so the composer sits at the visible bottom at half height too.
        val full = (LocalConfiguration.current.screenHeightDp * 0.9f).dp
        val view = LocalView.current
        val navBottom = WindowInsets.navigationBars.getBottom(density)
        var visiblePx by remember { mutableIntStateOf(0) }
        LaunchedEffect(Unit) {
            snapshotFlow { visiblePx }.collectLatest { px ->
                if (px <= 0) return@collectLatest
                delay(200) // after the drag settles
                currentInset(with(density) { (px + navBottom).toDp().value.roundToInt() })
            }
        }
        Box(
            Modifier.fillMaxWidth().height(full).testTag("officeThreadSheet").onGloballyPositioned { lc ->
                val windowH = view.rootView.height
                if (windowH > 0) visiblePx = (windowH - navBottom - lc.positionInWindow().y).roundToInt().coerceIn(0, lc.size.height)
            },
        ) {
            Column(
                Modifier.fillMaxWidth().height(if (visiblePx > 0) with(density) { visiblePx.toDp() } else full).imePadding(),
            ) {
                // Header: the bot, its live line, open the full chat, close.
                Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    BotStatusAvatar(bot, 34.dp, working = working != null && waiting == null, waiting = waiting != null)
                    Column(Modifier.weight(1f)) {
                        Text(bot.displayName, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.semantics { heading() })
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                            if (waiting != null || working != null) Box(Modifier.size(6.dp).clip(CircleShape).background(if (waiting != null) AllternitStatic.StatusWarning else AllternitStatic.StatusSuccess))
                            Text(live, color = c.textSecondary, fontSize = 12.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                    SheetIcon(DesignR.drawable.ph_arrow_square_out, "Open the full chat", "officeSheetOpenChat") { onDismiss(); kit.openFullChat(botId) }
                    SheetIcon(DesignR.drawable.ph_x, "Close", "officeSheetClose", onClick = onDismiss)
                }
                // Autopilot answers and drafts (bot_events autopilot_answer / autopilot_draft via /floor/events).
                lines.forEach { l ->
                    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 2.dp).testTag("officeAutopilotLine"), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Icon(painterResource(DesignR.drawable.ph_sparkle), null, tint = c.textTertiary, modifier = Modifier.size(12.dp))
                        Text(
                            autopilotLine(l.outcome, main) + (l.text?.takeIf { it.isNotBlank() }?.let { ": $it" } ?: ""),
                            color = c.textSecondary, fontSize = 12.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
                need?.let { n ->
                    androidx.compose.runtime.key(n.id) {
                        OfficeApprovalCard(
                            n, bot.displayName, kit.backend, store, more,
                            onDone = { done, bubble -> refreshKey += 1; onDone(done, bubble) },
                            modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
                        )
                    }
                }
                // Thread | Screen.
                Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    OfficeSheetTab.entries.forEach { t ->
                        SheetTab(t.label, on = tab == t, testTag = "officeSheetTab${t.label}") { tab = t }
                    }
                }
                Box(Modifier.weight(1f).fillMaxWidth()) {
                    when (tab) {
                        OfficeSheetTab.Thread -> kit.Thread(botId, refreshKey, Modifier.fillMaxSize())
                        OfficeSheetTab.Screen -> kit.Screen(botId, Modifier.fillMaxSize())
                    }
                }
            }
        }
    }
}

@Composable
private fun SheetIcon(icon: Int, label: String, testTag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        Modifier.size(40.dp).clip(CircleShape).clickable(onClickLabel = label, role = Role.Button, onClick = onClick).testTag(testTag)
            .semantics { this.contentDescription = label },
        contentAlignment = Alignment.Center,
    ) { Icon(painterResource(icon), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(18.dp)) }
}

@Composable
private fun SheetTab(label: String, on: Boolean, testTag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Box(
        Modifier.height(32.dp).clip(CircleShape).background(if (on) c.textPrimary else c.fillNeutral.copy(alpha = 0.7f)).testTag(testTag)
            .semantics { selected = on }
            .clickable(role = Role.Tab) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }.padding(horizontal = 14.dp),
        contentAlignment = Alignment.Center,
    ) { Text(label, color = if (on) c.bgPrimary else c.textPrimary, fontSize = 13.5.sp, fontWeight = FontWeight.Medium) }
}
