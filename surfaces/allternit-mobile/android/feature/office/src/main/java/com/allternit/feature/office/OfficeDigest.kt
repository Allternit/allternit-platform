package com.allternit.feature.office

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.relativeTime
import com.allternit.feature.bots.BotAvatar
import com.allternit.feature.bots.BotModeStore

/** Away at least this long before the digest shows (plan D13). */
internal const val DIGEST_AWAY_MS = 30 * 60_000L

/** Whether a `POST /floor/seen` result means the digest should be fetched: a previous visit 30 minutes or more ago. */
internal fun digestDue(seen: FloorSeen): Boolean = seen.previous != null && seen.now - seen.previous >= DIGEST_AWAY_MS

/**
 * "While you were away" (step 27, iOS OfficeDigestView), over the floor when the Office opens after 30 minutes or more
 * away with at least one item: what the main bot answered on autopilot, what is waiting, what finished, and new
 * suggestions, each a bot face and a line. Tapping a line opens that bot's thread sheet. The server already marked
 * the visit seen (`POST /floor/seen`), so it does not show again on any device.
 */
@Composable
fun OfficeDigestCard(
    digest: FloorDigest,
    store: BotModeStore,
    mainBotName: String?,
    onOpenBot: (String) -> Unit,
    onDismiss: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = AllternitTheme.colors
    val main = mainBotName ?: "your main bot"
    Column(
        modifier.fillMaxWidth().shadow(12.dp, RoundedCornerShape(20.dp), clip = false).clip(RoundedCornerShape(20.dp)).background(c.bgPrimary)
            .border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(20.dp)).testTag("officeDigest")
            .semantics { liveRegion = LiveRegionMode.Polite },
    ) {
        Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 6.dp, top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(painterResource(DesignR.drawable.ph_sparkle), null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
            Column(Modifier.weight(1f).padding(start = 8.dp)) {
                Text("While you were away", color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.semantics { heading() })
                relativeTime(digest.since).takeIf { it.isNotEmpty() }?.let { Text("Last here $it", color = c.textTertiary, fontSize = 12.sp) }
            }
            Box(
                Modifier.size(44.dp).clip(CircleShape).clickable(onClickLabel = "Close", role = Role.Button, onClick = onDismiss)
                    .semantics { contentDescription = "Close" }.testTag("officeDigestClose"),
                contentAlignment = Alignment.Center,
            ) { Icon(painterResource(DesignR.drawable.ph_x), null, tint = c.textSecondary, modifier = Modifier.size(16.dp)) }
        }
        Column(Modifier.heightIn(max = 380.dp).verticalScroll(rememberScrollState()).padding(bottom = 10.dp)) {
            DigestSection("Answered by $main on autopilot", digest.answered, store, onOpenBot)
            DigestSection("Waiting for you", digest.waiting, store, onOpenBot)
            DigestSection("Finished", digest.finished, store, onOpenBot)
            DigestSection("Suggestions", digest.suggestions, store, onOpenBot)
        }
    }
}

@Composable
private fun DigestSection(title: String, items: List<FloorDigestItem>, store: BotModeStore, onOpenBot: (String) -> Unit) {
    if (items.isEmpty()) return
    val c = AllternitTheme.colors
    val view = LocalView.current
    Text(title, color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 10.dp, bottom = 2.dp).semantics { heading() })
    val shown = items.take(3)
    shown.forEach { item ->
        val bot = store.bot(item.botId)
        val name = bot?.displayName ?: "A bot"
        Row(
            Modifier.fillMaxWidth()
                .clickable(enabled = bot != null, onClickLabel = "Open $name", role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onOpenBot(item.botId) }
                .padding(horizontal = 16.dp, vertical = 7.dp).testTag("officeDigestItem"),
            verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (bot != null) BotAvatar(bot, 26.dp) else Box(Modifier.size(26.dp).clip(CircleShape).background(c.fillNeutral))
            Column(Modifier.weight(1f)) {
                Text(name, color = c.textPrimary, fontSize = 13.5.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(item.text, color = c.textSecondary, fontSize = 13.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
            relativeTime(item.at).takeIf { it.isNotEmpty() }?.let { Text(it, color = c.textTertiary, fontSize = 11.5.sp) }
        }
    }
    if (items.size > shown.size) Text("+${items.size - shown.size} more", color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.padding(start = 52.dp, top = 2.dp))
}
