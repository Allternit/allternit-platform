package com.allternit.feature.shell

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
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
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.gen.AgentSession
import com.allternit.core.auth.Account
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.design.AllternitRadii
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.Haptics
import com.allternit.core.design.ModeAccents
import kotlinx.coroutines.launch
import java.time.LocalDateTime
import java.time.format.TextStyle as JavaTextStyle
import java.util.Locale

// MARK: - Header

/**
 * Phone top bar (iOS PhoneHeader): ☰ leads, a centered title, trailing actions. 52dp tall,
 * no separator, light background. [leading] defaults to nothing so sub-pages can put Back there.
 */
@Composable
internal fun PhoneHeader(
    title: String,
    subtitle: String? = null,
    leading: @Composable () -> Unit,
    trailing: @Composable () -> Unit = {},
) {
    val c = AllternitTheme.colors
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .height(52.dp)
            .padding(horizontal = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(52.dp), contentAlignment = Alignment.CenterStart) { leading() }
        Column(Modifier.weight(1f), horizontalAlignment = Alignment.CenterHorizontally) {
            Text(
                text = title,
                color = c.textPrimary,
                fontSize = 16.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.semantics { heading() },
            )
            if (!subtitle.isNullOrEmpty()) {
                Text(subtitle, color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        Box(Modifier.width(52.dp), contentAlignment = Alignment.CenterEnd) { trailing() }
    }
}

/** A 44dp header button (iOS PhoneIconButton). */
@Composable
internal fun PhoneIconButton(icon: Int, label: String, testTag: String? = null, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Box(
        modifier = Modifier
            .size(44.dp)
            .clip(CircleShape)
            .then(if (testTag != null) Modifier.testTag(testTag) else Modifier)
            .clickable(role = Role.Button, onClickLabel = label) {
                Haptics.perform(view, Haptics.Kind.Tap)
                onClick()
            }
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        Icon(painter = painterResource(icon), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(22.dp))
    }
}

// MARK: - Home

/**
 * The signed-in home. Home mode: the greeting (iOS ChatView launch header), the latest chats
 * from the server, and the composer. Bots / Code / ACI: an honest "arrives in the next update"
 * state with the web version one tap away, plus that mode's real recent threads.
 */
@Composable
internal fun ShellHome(
    mode: ShellMode,
    account: Account?,
    store: ShellSessionsStore,
    menuButton: @Composable () -> Unit,
    onNewChat: (String) -> Unit,
    onOpenSession: (AgentSession) -> Unit,
    onOpenDrawer: () -> Unit,
    onRetry: () -> Unit,
    onOpenWeb: () -> Unit,
) {
    Column(Modifier.fillMaxSize().statusBarsPadding()) {
        PhoneHeader(
            title = if (mode == ShellMode.Home) "Allternit" else mode.label,
            leading = menuButton,
            trailing = {
                PhoneIconButton(R.drawable.shell_note_pencil, mode.newLabel, testTag = "chatNewChat") { onNewChat("") }
            },
        )
        Column(
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 20.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            if (mode == ShellMode.Home) {
                Spacer(Modifier.height(56.dp))
                Greeting(account)
                Spacer(Modifier.height(36.dp))
            } else {
                ModeArrives(mode, onOpenWeb)
            }
            RecentPreview(
                mode = mode,
                signedIn = account != null,
                store = store,
                onOpenSession = onOpenSession,
                onViewAll = onOpenDrawer,
                onRetry = onRetry,
            )
            Spacer(Modifier.height(24.dp))
        }
        if (mode == ShellMode.Home) {
            Composer(onSend = onNewChat)
        } else {
            Spacer(Modifier.navigationBarsPadding())
        }
    }
}

@Composable
private fun Greeting(account: Account?) {
    val c = AllternitTheme.colors
    val title = remember(account?.firstName) { greetingTitle(account?.firstName) }
    Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
        // The A:// mark, not Gizzi, on the main chat (Eoj, via iOS ChatView).
        Text("A://", color = c.textPrimary, fontSize = 20.sp, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.Bold)
        Text(
            text = title,
            color = c.textPrimary,
            fontSize = 32.sp,
            lineHeight = 38.sp,
            fontFamily = FontFamily.Serif,
            letterSpacing = (-0.64).sp,
            textAlign = TextAlign.Center,
            modifier = Modifier.testTag("homeGreeting"),
        )
    }
}

/** iOS ChatView.sessionTitle(): one line from a small pool, named when we know the first name. */
internal fun greetingTitle(firstName: String?, now: LocalDateTime = LocalDateTime.now()): String {
    val name = firstName?.trim()?.takeIf { it.isNotEmpty() }
    val weekday = now.dayOfWeek.getDisplayName(JavaTextStyle.FULL, Locale.getDefault())
    val hour = now.hour
    val timeOfDay = when {
        hour < 5 -> "Up late"
        hour < 12 -> "Good morning"
        hour < 18 -> "Good afternoon"
        else -> "Good evening"
    }
    fun withName(base: String) = if (name != null) "$base, $name" else base
    val pool = listOf(
        withName("Happy $weekday"),
        withName(timeOfDay),
        if (name != null) "What's on your mind, $name?" else "What's on your mind?",
        if (name != null) "How can I help, $name?" else "How can I help you today?",
        withName("Welcome back"),
        "What should we get done?",
        if (name != null) "What are we working on, $name?" else "What are we working on?",
    )
    return pool.random()
}

/** A mode whose screen is not built yet: say so, and hand over to the web version. */
@Composable
private fun ModeArrives(mode: ShellMode, onOpenWeb: () -> Unit) {
    val c = AllternitTheme.colors
    Column(
        modifier = Modifier.fillMaxWidth().padding(top = 40.dp, bottom = 12.dp).testTag("modeArrives${mode.raw}"),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(
            modifier = Modifier
                .size(64.dp)
                .clip(RoundedCornerShape(AllternitRadii.XL))
                .background(mode.accent.copy(alpha = ModeAccents.SOFT_ALPHA)),
            contentAlignment = Alignment.Center,
        ) {
            Icon(painter = painterResource(mode.icon), contentDescription = null, tint = mode.accent, modifier = Modifier.size(28.dp))
        }
        Text(
            text = "${mode.label} arrives in the next update",
            color = c.textPrimary,
            fontSize = 20.sp,
            fontFamily = FontFamily.Serif,
            textAlign = TextAlign.Center,
        )
        Text(
            text = modeArrivesMessage(mode),
            color = c.textSecondary,
            fontSize = 14.sp,
            lineHeight = 20.sp,
            textAlign = TextAlign.Center,
            modifier = Modifier.widthIn(max = 340.dp),
        )
        PillButton("Open ${mode.label} in the web version", testTag = "modeArrivesWeb", onClick = onOpenWeb)
    }
}

private fun modeArrivesMessage(mode: ShellMode): String = when (mode) {
    ShellMode.Bots -> "Your bots, their threads and their phones come to the native app next. They all work in the web version today."
    ShellMode.Code -> "Code threads, terminals and reviews come to the native app next. They all work in the web version today."
    ShellMode.Aci -> "Agent runs, computers and Teach come to the native app next. They all work in the web version today."
    ShellMode.Home -> ""
}

/** Up to five of the mode's newest sessions, with loading, error and empty states. */
@Composable
private fun RecentPreview(
    mode: ShellMode,
    signedIn: Boolean,
    store: ShellSessionsStore,
    onOpenSession: (AgentSession) -> Unit,
    onViewAll: () -> Unit,
    onRetry: () -> Unit,
) {
    val c = AllternitTheme.colors
    if (!signedIn) return
    val recents = store.of(mode)
    Column(Modifier.fillMaxWidth().widthIn(max = 520.dp).testTag("homeRecents")) {
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth().padding(bottom = 6.dp)) {
            Text(
                text = if (mode == ShellMode.Home) "Recent chats" else "Your recent ${mode.label} threads",
                color = c.textSecondary,
                fontSize = 14.sp,
                modifier = Modifier.weight(1f).semantics { heading() },
            )
            if (recents.isNotEmpty()) {
                Text(
                    text = "View all",
                    color = c.textSecondary,
                    fontSize = 14.sp,
                    modifier = Modifier
                        .clip(RoundedCornerShape(AllternitRadii.SM))
                        .clickable(role = Role.Button, onClick = onViewAll)
                        .padding(horizontal = 6.dp, vertical = 8.dp),
                )
            }
        }
        when {
            store.isLoading && !store.hasLoaded -> Box(Modifier.fillMaxWidth().padding(vertical = 20.dp), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(
                    modifier = Modifier.size(22.dp).semantics { contentDescription = "Loading chats" },
                    strokeWidth = 2.dp,
                    color = c.textSecondary,
                )
            }
            store.loadError != null && recents.isEmpty() -> FriendlyState(
                style = FriendlyStyle.Offline,
                title = "Couldn't load your chats",
                message = store.loadError.orEmpty(),
                actionTitle = "Retry",
                onAction = onRetry,
                modifier = Modifier.testTag("homeRecentsError"),
            ) {
                Icon(
                    painter = painterResource(com.allternit.core.design.R.drawable.ph_warning),
                    contentDescription = null,
                    tint = c.textSecondary,
                )
            }
            recents.isEmpty() -> Text(
                text = mode.emptyMessage.replace("show up here", "will show up here") + " Start one below.".takeIf { mode == ShellMode.Home }.orEmpty(),
                color = c.textTertiary,
                fontSize = 14.sp,
                modifier = Modifier.padding(vertical = 12.dp).testTag("homeRecentsEmpty"),
            )
            else -> Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(AllternitRadii.LG))
                    .border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(AllternitRadii.LG)),
            ) {
                recents.take(5).forEach { session -> HomeSessionRow(session) { onOpenSession(session) } }
            }
        }
    }
}

@Composable
private fun HomeSessionRow(session: AgentSession, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("homeRow${session.id}")
            .clickable(role = Role.Button) {
                Haptics.perform(view, Haptics.Kind.Tap)
                onClick()
            }
            .heightIn(min = 52.dp)
            .padding(horizontal = 14.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(painter = painterResource(R.drawable.shell_chat), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(18.dp))
        Column(Modifier.weight(1f)) {
            Text(session.title, color = c.textPrimary, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            session.description?.trim()?.takeIf { it.isNotEmpty() }?.let {
                Text(it, color = c.textSecondary, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        val time = relativeTime(session.timestampMillis)
        if (time.isNotEmpty()) Text(time, color = c.textTertiary, fontSize = 13.sp)
    }
}

/** The home composer (iOS ComposerView, text only until step 8). Test id `composerInput`. */
@Composable
private fun Composer(onSend: (String) -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    var text by rememberSaveable { mutableStateOf("") }
    val canSend = text.isNotBlank()
    fun send() {
        if (!canSend) return
        Haptics.perform(view, Haptics.Kind.Tap)
        val draft = text.trim()
        text = ""
        onSend(draft)
    }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .navigationBarsPadding()
            .imePadding()
            .padding(start = 12.dp, end = 12.dp, top = 6.dp, bottom = 10.dp)
            .clip(RoundedCornerShape(24.dp))
            .background(c.bgSecondary)
            .border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(24.dp))
            .padding(start = 16.dp, end = 6.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.weight(1f).padding(vertical = 8.dp)) {
            if (text.isEmpty()) Text("Ask anything", color = c.textTertiary, fontSize = 16.sp)
            BasicTextField(
                value = text,
                onValueChange = { text = it },
                textStyle = TextStyle(color = c.textPrimary, fontSize = 16.sp),
                cursorBrush = SolidColor(c.textPrimary),
                maxLines = 5,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
                keyboardActions = KeyboardActions(onSend = { send() }),
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag("composerInput")
                    .semantics { contentDescription = "Message" },
            )
        }
        Box(
            modifier = Modifier
                .size(36.dp)
                .clip(CircleShape)
                .background(if (canSend) c.textPrimary else c.fillNeutral)
                .testTag("composerSend")
                .clickable(enabled = canSend, role = Role.Button, onClickLabel = "Send") { send() }
                .semantics { contentDescription = "Send" },
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                painter = painterResource(com.allternit.core.design.R.drawable.ph_arrow_up),
                contentDescription = null,
                tint = if (canSend) c.bgPrimary else c.textTertiary,
                modifier = Modifier.size(18.dp),
            )
        }
    }
}

@Composable
internal fun PillButton(title: String, testTag: String? = null, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        modifier = Modifier
            .clip(RoundedCornerShape(22.dp))
            .border(BorderStroke(1.dp, c.borderStrong), RoundedCornerShape(22.dp))
            .then(if (testTag != null) Modifier.testTag(testTag) else Modifier)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .heightIn(min = 44.dp)
            .padding(horizontal = 18.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(title, color = if (enabled) c.textPrimary else c.textTertiary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
    }
}

// MARK: - Sub-pages

@Composable
private fun BackButton(onBack: () -> Unit) {
    PhoneIconButton(com.allternit.core.design.R.drawable.ph_caret_left, "Back", testTag = "shellBack", onClick = onBack)
}

/**
 * A chat until step 8 brings the real chat screen: an existing chat shows its name and size;
 * a new one shows what the person typed. Both say plainly that replies arrive next, and open
 * the web version on request.
 */
@Composable
internal fun ChatStub(session: AgentSession?, draft: String, onBack: () -> Unit, onOpenWeb: () -> Unit) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("chatStub")) {
        PhoneHeader(title = session?.title ?: "New chat", leading = { BackButton(onBack) })
        Column(
            modifier = Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            if (session != null) {
                val parts = buildList {
                    session.messageCount?.let { add(if (it == 1L) "1 message" else "$it messages") }
                    relativeTime(session.timestampMillis).takeIf { it.isNotEmpty() }?.let { add("updated $it ago") }
                }
                if (parts.isNotEmpty()) {
                    Text(parts.joinToString(" · "), color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(top = 8.dp))
                }
            }
            if (draft.isNotEmpty()) {
                Box(Modifier.fillMaxWidth().padding(top = 16.dp), contentAlignment = Alignment.CenterEnd) {
                    Text(
                        text = draft,
                        color = c.textPrimary,
                        fontSize = 16.sp,
                        modifier = Modifier
                            .widthIn(max = 300.dp)
                            .clip(RoundedCornerShape(20.dp))
                            .background(c.bgTertiary)
                            .padding(horizontal = 14.dp, vertical = 10.dp),
                    )
                }
            }
            FriendlyState(
                style = FriendlyStyle.Empty,
                title = "Chats open here in the next update",
                message = if (session != null) {
                    "This chat and everything in it is in the web version today."
                } else {
                    "Replies come to the native app next. Send this from the web version for now."
                },
                actionTitle = "Open web version",
                onAction = onOpenWeb,
            ) {
                Icon(painter = painterResource(R.drawable.shell_chat), contentDescription = null, tint = c.textSecondary)
            }
        }
    }
}

/** A drawer section whose screen is not built yet (Projects, Artifacts, …). */
@Composable
internal fun SectionStub(row: RailRow, onBack: () -> Unit, onOpenWeb: () -> Unit) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("sectionStub${row.id}")) {
        PhoneHeader(title = row.label, leading = { BackButton(onBack) })
        FriendlyState(
            style = FriendlyStyle.Empty,
            title = "${row.label} arrives in the next update",
            message = "It works in the web version today, with everything you already have there.",
            actionTitle = "Open web version",
            onAction = onOpenWeb,
            modifier = Modifier.padding(horizontal = 20.dp),
        ) {
            Icon(painter = painterResource(row.icon), contentDescription = null, tint = c.textSecondary)
        }
    }
}

/** Settings, account part (iOS Settings sheet top): who is signed in, web version, sign out. */
@Composable
internal fun SettingsScreen(account: Account?, onBack: () -> Unit, onOpenWeb: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var signingOut by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("settingsScreen")) {
        PhoneHeader(title = "Settings", leading = { BackButton(onBack) })
        Column(
            modifier = Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(top = 8.dp)
                    .clip(RoundedCornerShape(AllternitRadii.LG))
                    .background(c.bgSecondary)
                    .padding(16.dp)
                    .semantics(mergeDescendants = true) {},
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                AccountAvatar(account, size = 52)
                Column(Modifier.weight(1f)) {
                    Text(account?.displayName ?: "Not signed in", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                    Text(account?.email ?: "Debug session without an account", color = c.textSecondary, fontSize = 13.sp)
                }
            }
            Text(
                "More settings (models, memory, connectors, appearance) come to the native app next. They all work in the web version today.",
                color = c.textTertiary,
                fontSize = 13.sp,
                lineHeight = 18.sp,
            )
            SettingsRow(WebVersionRow.label, WebVersionRow.icon, "settingsWebVersion", onClick = onOpenWeb)
            if (account != null) {
                SettingsRow(
                    title = if (signingOut) "Signing out…" else "Sign out",
                    icon = R.drawable.shell_sign_out,
                    testTag = "settingsSignOut",
                    enabled = !signingOut,
                ) {
                    signingOut = true
                    error = null
                    scope.launch {
                        val result = AllternitAuth.signOut()
                        // Success flips AllternitAuth.state, and MainActivity shows sign-in.
                        error = result.error
                        signingOut = false
                    }
                }
            }
            error?.let { Text(it, color = c.textSecondary, fontSize = 13.sp) }
        }
    }
}

@Composable
private fun SettingsRow(title: String, icon: Int, testTag: String, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(AllternitRadii.MD))
            .background(c.bgSecondary)
            .testTag(testTag)
            .clickable(enabled = enabled, role = Role.Button) {
                Haptics.perform(view, Haptics.Kind.Tap)
                onClick()
            }
            .heightIn(min = 52.dp)
            .padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Icon(painter = painterResource(icon), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(20.dp))
        Text(title, color = if (enabled) c.textPrimary else c.textTertiary, fontSize = 16.sp)
    }
}
