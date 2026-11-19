package com.allternit.feature.shell

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.gen.AgentSession
import com.allternit.core.auth.Account
import com.allternit.core.design.AllternitRadii
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.Haptics

/**
 * The drawer (iOS HistorySidebarView, phone Arc drawer): header (mode dial · wordmark), the
 * mode's rail rows, Use web version, Recents from `GET /agent-sessions`, and a footer with the
 * account (opens Settings) and the mode's New pill.
 */
@Composable
internal fun ShellDrawer(
    mode: ShellMode,
    account: Account?,
    store: ShellSessionsStore,
    onModeDial: () -> Unit,
    onRow: (RailRow) -> Unit,
    onOpenSession: (AgentSession) -> Unit,
    onNew: () -> Unit,
    onSettings: () -> Unit,
    onRetry: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = AllternitTheme.colors
    val recents = store.of(mode)
    Box(modifier.fillMaxSize().statusBarsPadding()) {
        Column(Modifier.fillMaxSize()) {
            DrawerHeader(mode = mode, onModeDial = onModeDial)
            LazyColumn(
                modifier = Modifier.fillMaxWidth().weight(1f),
            ) {
                items(railRows(mode), key = { "rail-${it.id}" }) { row -> DrawerNavRow(row) { onRow(row) } }
                item(key = "rail-web") { DrawerNavRow(WebVersionRow) { onRow(WebVersionRow) } }
                item(key = "recents-label") {
                    Text(
                        text = "Recents",
                        color = c.textSecondary,
                        fontSize = 14.sp,
                        modifier = Modifier
                            .padding(start = 20.dp, end = 14.dp, top = 18.dp, bottom = 4.dp)
                            .semantics { heading() },
                    )
                }
                when {
                    account == null -> item(key = "recents-signed-out") {
                        DrawerNote("Sign in to see your chats here.")
                    }
                    store.isLoading && !store.hasLoaded -> item(key = "recents-loading") {
                        Box(Modifier.fillMaxWidth().padding(vertical = 24.dp), contentAlignment = Alignment.Center) {
                            CircularProgressIndicator(
                                modifier = Modifier.size(22.dp).semantics { contentDescription = "Loading chats" },
                                strokeWidth = 2.dp,
                                color = c.textSecondary,
                            )
                        }
                    }
                    store.loadError != null && recents.isEmpty() -> item(key = "recents-error") {
                        FriendlyState(
                            style = FriendlyStyle.Offline,
                            title = "Couldn't load conversations",
                            message = store.loadError.orEmpty(),
                            actionTitle = "Retry",
                            onAction = onRetry,
                            modifier = Modifier.padding(horizontal = 16.dp).testTag("drawerRecentsError"),
                        ) {
                            Icon(
                                painter = painterResource(com.allternit.core.design.R.drawable.ph_warning),
                                contentDescription = null,
                                tint = c.textSecondary,
                            )
                        }
                    }
                    recents.isEmpty() -> item(key = "recents-empty") {
                        FriendlyState(
                            style = FriendlyStyle.Empty,
                            title = mode.emptyTitle,
                            message = mode.emptyMessage,
                            actionTitle = mode.newLabel,
                            onAction = onNew,
                            modifier = Modifier.padding(horizontal = 16.dp),
                        ) {
                            Icon(
                                painter = painterResource(R.drawable.shell_chat),
                                contentDescription = null,
                                tint = c.textSecondary,
                            )
                        }
                    }
                    else -> items(recents, key = { "s-${it.id}" }) { session ->
                        RecentRow(session) { onOpenSession(session) }
                    }
                }
                // Room for the floating footer.
                item(key = "foot-space") { Spacer(Modifier.height(92.dp)) }
            }
        }
        DrawerFooter(
            mode = mode,
            account = account,
            onSettings = onSettings,
            onNew = onNew,
            modifier = Modifier.align(Alignment.BottomCenter),
        )
    }
}

/** `phone-drawer-top-modes`: the mode dial and the wordmark (search arrives with step 7c-2). */
@Composable
private fun DrawerHeader(mode: ShellMode, onModeDial: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        modifier = Modifier.fillMaxWidth().padding(start = 12.dp, end = 14.dp, top = 6.dp, bottom = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        // ModeDialButton: the active mode as a round filled icon; tap closes the drawer and fans the modes.
        Box(
            modifier = Modifier
                .size(44.dp)
                .clip(CircleShape)
                .testTag("modeDial")
                .clickable(role = Role.Button, onClickLabel = "Switch mode") {
                    Haptics.perform(view, Haptics.Kind.Tap)
                    onModeDial()
                }
                .semantics { contentDescription = "${mode.label} mode. Switch mode" },
            contentAlignment = Alignment.Center,
        ) {
            Box(
                Modifier.size(34.dp).clip(CircleShape).background(c.textPrimary),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    painter = painterResource(mode.icon),
                    contentDescription = null,
                    tint = c.bgPrimary,
                    modifier = Modifier.size(17.dp),
                )
            }
        }
        // ShellWordmark (iOS draws the pixel mark; plain monospace until the pixel mark is ported).
        Text(
            text = mode.wordmark,
            color = c.textPrimary,
            fontSize = 15.sp,
            fontFamily = FontFamily.Monospace,
            fontWeight = FontWeight.Bold,
            letterSpacing = 1.sp,
            maxLines = 1,
            overflow = TextOverflow.Clip,
            modifier = Modifier
                .padding(start = 10.dp)
                .weight(1f)
                .testTag("shellWordmark${mode.raw}"),
        )
    }
}

/** `phone-nav-row`: 46dp, 22dp glyph, 17sp label. */
@Composable
private fun DrawerNavRow(row: RailRow, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 8.dp)
            .clip(RoundedCornerShape(AllternitRadii.MD))
            .testTag("drawerRow${row.id}")
            .clickable(role = Role.Button) {
                Haptics.perform(view, Haptics.Kind.Tap)
                onClick()
            }
            .heightIn(min = 46.dp)
            .padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Icon(
            painter = painterResource(row.icon),
            contentDescription = null,
            tint = c.textPrimary,
            modifier = Modifier.size(22.dp),
        )
        Text(row.label, color = c.textPrimary, fontSize = 17.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

@Composable
private fun RecentRow(session: AgentSession, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val time = relativeTime(session.timestampMillis)
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 8.dp)
            .clip(RoundedCornerShape(AllternitRadii.MD))
            .testTag("recentRow${session.id}")
            .clickable(role = Role.Button) {
                Haptics.perform(view, Haptics.Kind.Tap)
                onClick()
            }
            .heightIn(min = 44.dp)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Icon(
            painter = painterResource(R.drawable.shell_chat),
            contentDescription = null,
            tint = c.textSecondary,
            modifier = Modifier.size(18.dp),
        )
        Text(
            text = session.title,
            color = c.textPrimary,
            fontSize = 16.5.sp,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        if (session.isTerminal) {
            Text(
                text = "TERMINAL",
                color = c.textSecondary,
                fontSize = 10.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier
                    .clip(RoundedCornerShape(4.dp))
                    .background(c.borderStrong.copy(alpha = 0.5f))
                    .padding(horizontal = 5.dp, vertical = 1.dp),
            )
        }
        if (time.isNotEmpty()) Text(time, color = c.textTertiary, fontSize = 13.sp)
    }
}

@Composable
private fun DrawerNote(text: String) {
    Text(
        text = text,
        color = AllternitTheme.colors.textSecondary,
        fontSize = 14.sp,
        modifier = Modifier.padding(horizontal = 20.dp, vertical = 12.dp),
    )
}

/** `phone-rail-foot`: account avatar + name (Settings), and the mode's New pill. */
@Composable
private fun DrawerFooter(
    mode: ShellMode,
    account: Account?,
    onSettings: () -> Unit,
    onNew: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val name = account?.displayName ?: "Not signed in"
    Row(
        modifier = modifier
            .fillMaxWidth()
            .background(Brush.verticalGradient(0f to c.bgPrimary.copy(alpha = 0f), 0.42f to c.bgPrimary))
            .navigationBarsPadding()
            .padding(start = 10.dp, end = 14.dp, top = 28.dp, bottom = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(
            modifier = Modifier
                .weight(1f)
                .clip(RoundedCornerShape(22.dp))
                .testTag("drawerSettings")
                .clickable(role = Role.Button, onClickLabel = "Open settings") {
                    Haptics.perform(view, Haptics.Kind.Tap)
                    onSettings()
                }
                .semantics(mergeDescendants = true) { contentDescription = "$name. Settings" }
                .padding(4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            AccountAvatar(account, size = 36)
            Column(Modifier.weight(1f)) {
                Text(name, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                account?.email?.let {
                    Text(it, color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
        }
        Row(
            modifier = Modifier
                .clip(RoundedCornerShape(22.dp))
                .background(c.textPrimary)
                .testTag("drawerNew")
                .clickable(role = Role.Button) {
                    Haptics.perform(view, Haptics.Kind.Tap)
                    onNew()
                }
                .heightIn(min = 44.dp)
                .padding(horizontal = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Icon(
                painter = painterResource(R.drawable.shell_note_pencil),
                contentDescription = null,
                tint = c.bgPrimary,
                modifier = Modifier.size(16.dp),
            )
            Text(mode.newLabel, color = c.bgPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
        }
    }
}

/** The account initial in a neutral circle (iOS accountAvatar, without the usage ring yet). */
@Composable
internal fun AccountAvatar(account: Account?, size: Int) {
    val c = AllternitTheme.colors
    val initial = account?.displayName?.firstOrNull()?.uppercaseChar()?.toString() ?: "?"
    Box(
        modifier = Modifier.size(size.dp).clip(CircleShape).background(c.borderStrong),
        contentAlignment = Alignment.Center,
    ) {
        Text(initial, color = c.textPrimary, fontSize = (size * 0.38f).sp, fontWeight = FontWeight.SemiBold)
    }
}

/** Width helper so the shell and the drawer agree: min(92% of the screen, 440dp). */
internal fun drawerWidth(screenWidth: androidx.compose.ui.unit.Dp): androidx.compose.ui.unit.Dp =
    minOf(screenWidth * 0.92f, 440.dp)
