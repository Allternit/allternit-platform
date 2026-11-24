package com.allternit.feature.shell.firstrun

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.R as DesignR

/**
 * The ChatGPT account shown as two separate connections (contract section 9.9, D5; design pick: Connections,
 * option B grouped). "ChatGPT subscription" is the device sign-in (chat models); "Codex CLI" is its
 * own sign-in (coding runs). They can use the same ChatGPT account, but each one signs in and reports on its own.
 * Mirrors iOS `AccountPair` and the web `accountPair.tsx`.
 */
internal object AccountPair {
    const val CHATGPT_ID = "chatgpt"
    const val CODEX_ID = "codex"
    const val PAIR_TITLE = "ChatGPT account · two separate connections"
    const val PAIR_NOTE = "They can use the same ChatGPT account, but each one signs in and reports on its own."

    /** Both rows are in the list, so they group. One alone (or neither) does not. */
    fun pairsUp(ids: Collection<String>) = CHATGPT_ID in ids && CODEX_ID in ids

    /** What a row says about itself once it carries a role. */
    data class Extras(
        val title: String,
        val badge: String,
        val powers: String,
        val connectLabel: String,
        val disconnectLabel: String,
        val statusOn: String,
        val statusOff: String,
        val modelCount: Int?,
    )

    /**
     * The extras for [id]; null for any other row. [paired] adds the "signing out of one does not sign out the
     * other" line to Codex. Android words: the chat is signed in once through the account's device sign-in, so the
     * status reads "Signed in" with no device named (iOS says "this iPhone" because its sign-in runs on the phone).
     */
    fun extras(id: String, row: ConnectionWire, paired: Boolean): Extras? = when (id) {
        CHATGPT_ID -> Extras(
            title = "ChatGPT subscription", badge = "Device sign-in",
            powers = "Powers: chat models.",
            connectLabel = "Connect", disconnectLabel = "Disconnect",
            statusOn = "Signed in", statusOff = "Not signed in",
            modelCount = row.modelCount.takeIf { it > 0 },
        )
        CODEX_ID -> Extras(
            title = "Codex CLI", badge = "Own sign-in",
            powers = if (paired) "Powers: coding runs. Signing out of one does not sign out the other." else "Powers: coding runs.",
            connectLabel = "Set up", disconnectLabel = "Sign out",
            statusOn = row.found?.let { "Signed in · $it" } ?: "Signed in", statusOff = "Not set up",
            modelCount = null,
        )
        else -> null
    }

    /** The one status line for a role row. Needs-you and connecting keep the store's own wording ([fallback]), which carries the real reason. */
    fun statusLine(extras: Extras, state: ConnState, fallback: String): String = when (state) {
        ConnState.On -> extras.statusOn + (extras.modelCount?.let { " · $it model${if (it == 1) "" else "s"}" } ?: "")
        ConnState.Off -> extras.statusOff
        ConnState.Needs, ConnState.Connecting -> fallback
    }
}

/** The one card for both rows, with the heading above and the note under. */
@Composable
internal fun AccountPairCard(
    entries: List<ConnEntry>,
    store: ConnectionsStore,
    onConnect: (String) -> Unit,
    onDisconnect: (ConnEntry) -> Unit,
    onFallback: (String) -> Unit,
) {
    val c = setupColors
    Column(Modifier.fillMaxWidth().testTag("conn-account-pair").semantics { contentDescription = AccountPair.PAIR_TITLE }) {
        Text(
            AccountPair.PAIR_TITLE.uppercase(), color = c.mut, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.6.sp,
            modifier = Modifier.fillMaxWidth().background(c.ink.copy(alpha = 0.03f)).padding(horizontal = 16.dp, vertical = 6.dp),
        )
        entries.forEachIndexed { i, e ->
            if (i > 0) HorizontalDivider(thickness = 0.5.dp, color = c.line)
            AccountRoleRow(e, store, paired = true, onConnect = onConnect, onDisconnect = onDisconnect, onFallback = onFallback)
        }
        HorizontalDivider(thickness = 0.5.dp, color = c.line)
        Text(AccountPair.PAIR_NOTE, color = c.mut, fontSize = 12.sp, lineHeight = 16.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 10.dp))
    }
}

/** A connection row with its role: own mark, title, badge, status, a "Powers:" line and its own buttons. */
@Composable
private fun AccountRoleRow(e: ConnEntry, store: ConnectionsStore, paired: Boolean, onConnect: (String) -> Unit, onDisconnect: (ConnEntry) -> Unit, onFallback: (String) -> Unit) {
    val c = setupColors
    val row = store.row(e.id)
    val extras = AccountPair.extras(e.id, row, paired) ?: return
    val available = store.isAvailable(e.id)
    val status = if (!available) store.unavailableReason(e.id) else AccountPair.statusLine(extras, row.state, store.statusLine(e))
    Row(
        Modifier.fillMaxWidth().heightIn(min = 60.dp).padding(horizontal = 12.dp, vertical = 10.dp).testTag("conn-row-${e.id}"),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        AccountMark(e.id)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(extras.title, color = c.ink, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1, modifier = Modifier.weight(1f, fill = false))
                Text(extras.badge, color = c.sub, fontSize = 11.sp, fontWeight = FontWeight.Medium, maxLines = 1, modifier = Modifier.border(1.dp, c.ink.copy(alpha = 0.18f), CircleShape).padding(horizontal = 7.dp, vertical = 1.5.dp))
            }
            Text(status, fontSize = 12.5.sp, color = if (row.state == ConnState.Needs) c.errFg else c.sub, maxLines = 2)
            if (available) Text(extras.powers, color = c.sub, fontSize = 12.5.sp, lineHeight = 16.sp)
        }
        when {
            row.state == ConnState.Connecting -> CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.sub)
            !available -> row.fallbacks.firstOrNull()?.let { f -> SetupQuietButton(ConnectionsCatalog.fallbackLabel(f)) { onFallback(f) } }
            row.state == ConnState.On -> SetupQuietButton(extras.disconnectLabel, Modifier.semantics { contentDescription = "${extras.disconnectLabel} ${extras.title}" }) { onDisconnect(e) }
            row.state == ConnState.Needs -> SetupQuietButton("Reconnect", Modifier.semantics { contentDescription = "Reconnect ${extras.title}" }) { onConnect(e.id) }
            else -> SetupQuietButton(extras.connectLabel, Modifier.semantics { contentDescription = "${extras.connectLabel} ${extras.title}" }) { onConnect(e.id) }
        }
    }
}

/** Round chat bubble (subscription) vs outlined terminal (CLI): two shapes, not only two words, so the rows read as different things. */
@Composable
internal fun AccountMark(id: String) {
    val c = setupColors
    if (id == AccountPair.CODEX_ID) {
        Box(Modifier.size(36.dp).border(1.5.dp, c.ink.copy(alpha = 0.35f), RoundedCornerShape(9.dp)), contentAlignment = Alignment.Center) {
            Icon(painterResource(DesignR.drawable.ph_terminal), contentDescription = null, tint = c.sub, modifier = Modifier.size(16.dp))
        }
    } else {
        Box(Modifier.size(36.dp).clip(CircleShape).background(c.ink.copy(alpha = 0.09f)), contentAlignment = Alignment.Center) {
            Icon(painterResource(DesignR.drawable.ph_chat_circle_dots_fill), contentDescription = null, tint = c.ink, modifier = Modifier.size(16.dp))
        }
    }
}
