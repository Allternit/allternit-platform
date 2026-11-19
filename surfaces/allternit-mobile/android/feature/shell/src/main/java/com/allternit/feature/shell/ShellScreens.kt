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
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
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
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.DpOffset
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
import com.allternit.core.design.R as DesignR
import com.allternit.feature.onboarding.R as OnboardingR
import kotlinx.coroutines.launch

/**
 * The shell's sans face: Roboto, bundled by :feature:onboarding so the app reads the same on
 * every phone (the Moto's system face is a wide geometric one). iOS uses SF Pro at the same sizes.
 */
internal val ShellSans = FontFamily(
    Font(OnboardingR.font.roboto_regular, FontWeight.Normal),
    Font(OnboardingR.font.roboto_semibold, FontWeight.Medium),
    Font(OnboardingR.font.roboto_semibold, FontWeight.SemiBold),
    Font(OnboardingR.font.roboto_bold, FontWeight.Bold),
)

// MARK: - Header

/**
 * Phone top bar (iOS PhoneHeader): ☰ leads, a centered title, trailing actions. 52dp tall, no
 * separator. [wideSides] is iOS `wideSides` (96dp sides), used where ••• and compose sit together.
 */
@Composable
internal fun PhoneHeader(
    title: String,
    subtitle: String? = null,
    wideSides: Boolean = false,
    leading: @Composable () -> Unit,
    trailing: @Composable () -> Unit = {},
) {
    val c = AllternitTheme.colors
    val side = if (wideSides) 96.dp else 52.dp
    Row(
        modifier = Modifier.fillMaxWidth().height(52.dp).padding(horizontal = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(side), contentAlignment = Alignment.CenterStart) { leading() }
        Column(Modifier.weight(1f), horizontalAlignment = Alignment.CenterHorizontally) {
            Text(
                text = title,
                color = c.textPrimary,
                fontSize = 16.sp,
                fontWeight = FontWeight.SemiBold,
                letterSpacing = (-0.2).sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.semantics { heading() },
            )
            if (!subtitle.isNullOrEmpty()) {
                Text(subtitle, color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        Row(Modifier.width(side), horizontalArrangement = Arrangement.spacedBy(2.dp, Alignment.End)) { trailing() }
    }
}

/** A 44dp header button (iOS PhoneIconButton). */
@Composable
internal fun PhoneIconButton(icon: Int, label: String, testTag: String? = null, iconSize: Int = 22, onClick: () -> Unit) {
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
        Icon(painter = painterResource(icon), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(iconSize.dp))
    }
}

// MARK: - Home

/**
 * The signed-in home (iOS ChatView in launch style): the header (☰ with the mode badge,
 * "Allternit", ••• and New chat), the pixel A:// mark over the serif headline, and the composer.
 * Recents live in the drawer, as on iOS. Bots / Code / ACI say plainly that their screens are next.
 */
@Composable
internal fun ShellHome(
    mode: ShellMode,
    store: ShellSessionsStore,
    models: ModelStore,
    menuButton: @Composable () -> Unit,
    onNewChat: (ComposerDraft) -> Unit,
    onNew: () -> Unit,
    onOpenSection: (RailRow) -> Unit,
) {
    var menuOpen by remember { mutableStateOf(false) }
    var phoneArmed by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxSize().statusBarsPadding()) {
        PhoneHeader(
            title = if (mode == ShellMode.Home) "Allternit" else mode.label,
            wideSides = mode == ShellMode.Home,
            leading = menuButton,
            trailing = {
                if (mode == ShellMode.Home) {
                    Box {
                        PhoneIconButton(R.drawable.shell_dots, "Chat options", testTag = "chatOptions", iconSize = 20) { menuOpen = true }
                        ChatOptionsMenu(
                            expanded = menuOpen,
                            phoneArmed = phoneArmed,
                            onDismiss = { menuOpen = false },
                            onRunOnPhone = {
                                phoneArmed = true
                                menuOpen = false
                            },
                        )
                    }
                }
                PhoneIconButton(R.drawable.shell_note_pencil, mode.newLabel, testTag = "chatNewChat", onClick = onNew)
            },
        )
        Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
            if (mode == ShellMode.Home) LaunchGreeting() else ModeArrives(mode)
        }
        if (mode == ShellMode.Home) {
            Composer(
                models = models,
                noComputer = store.noComputer,
                phoneArmed = phoneArmed,
                onClearPhone = { phoneArmed = false },
                onSend = onNewChat,
                onOpenSection = onOpenSection,
            )
        } else {
            Spacer(Modifier.navigationBarsPadding())
        }
    }
}

/**
 * iOS ChatLaunchGreeting on the main chat: the A:// mark (not Gizzi), 22dp, over a 32sp serif
 * headline. Pinned to the reference build's line.
 */
@Composable
private fun LaunchGreeting() {
    val c = AllternitTheme.colors
    Column(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp).padding(bottom = 24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        AProtocolMark(height = 22.dp, modifier = Modifier.testTag("homeMark"))
        Text(
            text = "What are we working on?",
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

/** The chat ⋯ menu on a new chat (iOS ChatMenuPopover with chatMenuItems): Run on my phone. */
@Composable
private fun ChatOptionsMenu(expanded: Boolean, phoneArmed: Boolean, onDismiss: () -> Unit, onRunOnPhone: () -> Unit) {
    val c = AllternitTheme.colors
    DropdownMenu(
        expanded = expanded,
        onDismissRequest = onDismiss,
        offset = DpOffset(0.dp, 4.dp),
        shape = RoundedCornerShape(16.dp),
        containerColor = c.bgPrimary,
        modifier = Modifier.width(300.dp),
    ) {
        MenuRow(
            icon = R.drawable.shell_device_mobile,
            title = "Run on my phone",
            checked = phoneArmed,
            testTag = "chatMenurunOnPhone",
            onClick = onRunOnPhone,
        )
    }
}

/** One 44dp menu row: 24dp glyph, 15sp label, an optional check (iOS ChatMenuPopover row). */
@Composable
internal fun MenuRow(icon: Int?, title: String, checked: Boolean = false, testTag: String? = null, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 8.dp)
            .clip(RoundedCornerShape(10.dp))
            .then(if (testTag != null) Modifier.testTag(testTag) else Modifier)
            .clickable(role = Role.Button, onClick = onClick)
            .heightIn(min = 44.dp)
            .padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (icon != null) {
            Icon(painterResource(icon), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(20.dp))
        }
        Text(title, color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
        if (checked) Icon(painterResource(DesignR.drawable.ph_check), contentDescription = "On", tint = c.textPrimary, modifier = Modifier.size(16.dp))
    }
}

/** A mode whose screen is not built yet: say so plainly. */
@Composable
private fun ModeArrives(mode: ShellMode) {
    val c = AllternitTheme.colors
    Column(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp).testTag("modeArrives${mode.raw}"),
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
            text = when (mode) {
                ShellMode.Bots -> "Your bots, their threads and their phones come to this app next."
                ShellMode.Code -> "Code threads, terminals and reviews come to this app next."
                ShellMode.Aci -> "Agent runs, computers and Teach come to this app next."
                ShellMode.Home -> ""
            },
            color = c.textSecondary,
            fontSize = 14.sp,
            lineHeight = 20.sp,
            textAlign = TextAlign.Center,
            modifier = Modifier.widthIn(max = 340.dp),
        )
    }
}

// MARK: - Sub-pages

@Composable
private fun BackButton(onBack: () -> Unit) {
    PhoneIconButton(DesignR.drawable.ph_caret_left, "Back", testTag = "shellBack", onClick = onBack)
}

/**
 * A chat until step 8 brings the real chat screen. A new chat shows what the person sent, with
 * its brain and attachments; an existing one shows its size. Both say replies come next.
 */
@Composable
internal fun ChatStub(session: AgentSession?, draft: ComposerDraft, onBack: () -> Unit, onNewChat: () -> Unit) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("chatStub")) {
        PhoneHeader(
            title = session?.title ?: "Allternit",
            leading = { BackButton(onBack) },
            trailing = { PhoneIconButton(R.drawable.shell_note_pencil, "New chat", testTag = "chatNewChat", onClick = onNewChat) },
        )
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
            if (draft.text.isNotEmpty() || draft.attachments.isNotEmpty()) {
                Column(Modifier.fillMaxWidth().padding(top = 16.dp), horizontalAlignment = Alignment.End) {
                    draft.attachments.forEach { AttachmentChip(it, onRemove = null) }
                    if (draft.text.isNotEmpty()) {
                        Text(
                            text = draft.text,
                            color = c.textPrimary,
                            fontSize = 16.sp,
                            modifier = Modifier
                                .padding(top = 6.dp)
                                .widthIn(max = 300.dp)
                                .clip(RoundedCornerShape(20.dp))
                                .background(c.fillNeutral)
                                .padding(horizontal = 14.dp, vertical = 10.dp),
                        )
                    }
                    val meta = listOfNotNull(draft.modelLabel, draft.agentMode, "On this phone".takeIf { draft.onPhone })
                    if (meta.isNotEmpty()) Text(meta.joinToString(" · "), color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.padding(top = 6.dp))
                }
            }
            FriendlyState(
                style = FriendlyStyle.Empty,
                title = "Replies arrive in the next update",
                message = if (session != null) {
                    "This chat opens here once the chat screen comes to this app."
                } else {
                    "Your message is kept here. The chat screen comes to this app next."
                },
            ) {
                Icon(painter = painterResource(R.drawable.shell_chat), contentDescription = null, tint = c.textSecondary)
            }
        }
    }
}

/** A drawer section whose screen is not built yet (Projects, Artifacts, …). */
@Composable
internal fun SectionStub(row: RailRow, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("sectionStub${row.id}")) {
        PhoneHeader(title = row.label, leading = { BackButton(onBack) })
        FriendlyState(
            style = FriendlyStyle.Empty,
            title = "${row.label} arrives in the next update",
            message = "Everything you already have there stays where it is, and shows up here when this screen comes to the app.",
            modifier = Modifier.padding(horizontal = 20.dp),
        ) {
            Icon(painter = painterResource(row.icon), contentDescription = null, tint = c.textSecondary)
        }
    }
}

/**
 * Every chat of the mode (iOS "View all ›", the session overview in list layout). Grid and
 * deck layouts and selection come with the overview port.
 */
@Composable
internal fun AllChatsScreen(mode: ShellMode, store: ShellSessionsStore, onBack: () -> Unit, onOpen: (AgentSession) -> Unit) {
    val list = store.of(mode)
    Column(Modifier.fillMaxSize().statusBarsPadding().testTag("allChats")) {
        PhoneHeader(title = if (mode == ShellMode.Home) "Chats" else "${mode.label} threads", leading = { BackButton(onBack) })
        if (list.isEmpty()) {
            FriendlyState(style = FriendlyStyle.Empty, title = mode.emptyTitle, message = mode.emptyMessage, modifier = Modifier.padding(20.dp)) {
                Icon(painterResource(R.drawable.shell_chat), contentDescription = null, tint = AllternitTheme.colors.textSecondary)
            }
        } else {
            LazyColumn(Modifier.fillMaxSize()) {
                items(list, key = { it.id }) { s -> RecentRow(s) { onOpen(s) } }
                item { Spacer(Modifier.navigationBarsPadding().height(16.dp)) }
            }
        }
    }
}

/** Drawer search (iOS ShellSearchView): chats by title. Projects join when that screen lands. */
@Composable
internal fun ShellSearchScreen(store: ShellSessionsStore, onBack: () -> Unit, onOpen: (AgentSession) -> Unit) {
    val c = AllternitTheme.colors
    var query by remember { mutableStateOf("") }
    val q = query.trim().lowercase()
    val hits = if (q.isEmpty()) emptyList() else store.sessions.filter { !it.archived && it.title.lowercase().contains(q) }
    Column(Modifier.fillMaxSize().statusBarsPadding().testTag("shellSearch")) {
        PhoneHeader(title = "Search", leading = { BackButton(onBack) })
        Row(
            modifier = Modifier
                .padding(horizontal = 16.dp, vertical = 4.dp)
                .fillMaxWidth()
                .clip(CircleShape)
                .background(c.fillNeutral)
                .padding(horizontal = 12.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Icon(painterResource(DesignR.drawable.ph_magnifying_glass), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(16.dp))
            Box(Modifier.weight(1f)) {
                if (query.isEmpty()) Text("Search chats and projects", color = c.textTertiary, fontSize = 16.sp)
                BasicTextField(
                    value = query,
                    onValueChange = { query = it },
                    singleLine = true,
                    textStyle = TextStyle(color = c.textPrimary, fontSize = 16.sp, fontFamily = ShellSans),
                    cursorBrush = SolidColor(c.textPrimary),
                    modifier = Modifier.fillMaxWidth().testTag("shellSearchField"),
                )
            }
        }
        when {
            q.isEmpty() -> Text(
                "Search chats and projects.",
                color = c.textSecondary,
                fontSize = 14.sp,
                modifier = Modifier.fillMaxWidth().padding(top = 32.dp),
                textAlign = TextAlign.Center,
            )
            hits.isEmpty() -> FriendlyState(style = FriendlyStyle.Empty, title = "No results", message = "Nothing matches “$query”.", modifier = Modifier.padding(20.dp)) {
                Icon(painterResource(DesignR.drawable.ph_magnifying_glass), contentDescription = null, tint = c.textSecondary)
            }
            else -> LazyColumn(Modifier.fillMaxSize()) {
                items(hits, key = { it.id }) { s -> RecentRow(s) { onOpen(s) } }
            }
        }
    }
}

/** Settings, account part (iOS Settings sheet top): who is signed in, and sign out. */
@Composable
internal fun SettingsScreen(account: Account?, onBack: () -> Unit) {
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
                    .background(c.fillNeutral)
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
                "Models, memory, connectors and appearance come to this screen next.",
                color = c.textTertiary,
                fontSize = 13.sp,
                lineHeight = 18.sp,
            )
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
            .background(c.fillNeutral)
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
