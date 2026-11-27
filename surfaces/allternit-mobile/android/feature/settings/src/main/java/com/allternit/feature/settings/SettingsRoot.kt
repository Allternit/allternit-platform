package com.allternit.feature.settings

import androidx.activity.compose.BackHandler
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalView
import com.allternit.core.design.PolishHaptic
import androidx.compose.foundation.border
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.tween
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.unit.IntOffset
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.drawscope.Stroke
import com.allternit.core.design.AllternitColors
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.auth.Account
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.design.AllternitRadii
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.ThemeMode
import com.allternit.feature.chat.ChatBackend
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR

/** What the shell gives Settings: the account, the relay, and the screens that live in other modules. */
class SettingsHost(
    /** Null for the debug "Continue without signing in" path: no API calls then. */
    val account: Account?,
    val backend: ChatBackend,
    val versionName: String,
    val onClose: () -> Unit,
    /** The connect kit's list (it lives in :feature:shell with the setup flow). */
    val connections: @Composable (onBack: () -> Unit) -> Unit,
    /** "Let your bots use this phone" opens Phone control. */
    val openPhoneControl: () -> Unit,
    /** Models: browse open models (the Models section). */
    val openModels: () -> Unit,
    /** Files & devices → "Open Cowork workspace" (iOS sets the mode to Cowork). */
    val openCowork: () -> Unit = {},
    /** A page to open on top of the root, e.g. "subscriptions" (Connections) from a chat refusal card. Back returns to the root. */
    val startPage: String? = null,
)

/** One page in Settings (iOS SettingsPageID + `phone: {section, order}`). */
internal data class SPageInfo(val id: String, val label: String, val icon: Int, val section: String, val order: Int)

/** Every page, in iOS order, with the web `SETTINGS_NAV_ITEMS` labels. Icons are the closest Phosphor glyphs the design module has. */
internal val ALL_PAGES: List<SPageInfo> = listOf(
    SPageInfo("profile", "Profile", DesignR.drawable.ph_user_circle, "account", 1),
    SPageInfo("appearance", "General", DesignR.drawable.ph_gear, "assistant", 3),
    SPageInfo("signin", "Account", DesignR.drawable.ph_user, "account", 6),
    SPageInfo("organization", "Organization & access", DesignR.drawable.ph_buildings, "account", 7),
    SPageInfo("privacy", "Privacy", DesignR.drawable.ph_lock, "account", 5),
    SPageInfo("subscriptions", "Connections", DesignR.drawable.ph_graph, "bots", 1),
    SPageInfo("billing", "Billing", DesignR.drawable.ph_credit_card, "account", 2),
    SPageInfo("usage", "Usage", DesignR.drawable.ph_gauge, "account", 3),
    SPageInfo("setup", "Setup", DesignR.drawable.ph_list_checks, "account", 8),
    SPageInfo("notifications", "Notifications", DesignR.drawable.ph_bell, "account", 4),
    SPageInfo("status", "Status", DesignR.drawable.ph_heartbeat, "developer", 3),
    SPageInfo("voice", "Voice", DesignR.drawable.ph_microphone, "assistant", 2),
    SPageInfo("memory", "Memory", DesignR.drawable.ph_brain, "assistant", 1),
    SPageInfo("response-style", "Response style", DesignR.drawable.ph_chat_text, "assistant", 4),
    SPageInfo("open-models", "Open models", DesignR.drawable.ph_lock_open, "assistant", 5),
    SPageInfo("gizziio-code", "Gizzi Code", DesignR.drawable.ph_code, "developer", 1),
    SPageInfo("cowork", "Files & devices", DesignR.drawable.ph_briefcase, "computers", 3),
    SPageInfo("devices", "Devices", DesignR.drawable.ph_devices, "computers", 2),
    SPageInfo("computers", "Computers", DesignR.drawable.ph_desktop, "computers", 1),
    SPageInfo("desktop-pet", "Desktop pet", DesignR.drawable.ph_paw_print, "computers", 5),
    SPageInfo("extensions", "Extensions", DesignR.drawable.ph_puzzle_piece, "computers", 6),
    SPageInfo("diagnostics", "Developer", DesignR.drawable.ph_pulse, "developer", 2),
    SPageInfo("agent-rules", "Agent rules", DesignR.drawable.ph_shield_check, "bots", 2),
    SPageInfo("skills", "Skills", DesignR.drawable.ph_sparkle, "bots", 3),
    SPageInfo("subagents", "Subagents", DesignR.drawable.ph_tree_structure, "bots", 4),
    SPageInfo("connectors", "Connectors", DesignR.drawable.ph_plug, "bots", 5),
    SPageInfo("connected-apps", "App approvals", DesignR.drawable.ph_plug, "bots", 7),
    SPageInfo("twin", "Your twin", DesignR.drawable.ph_users_three, "bots", 8),
    SPageInfo("plugins", "Plugins", DesignR.drawable.ph_package, "bots", 6),
    SPageInfo("lens", "Lens Context", DesignR.drawable.ph_database, "bots", 9),
    SPageInfo("models", "Models", DesignR.drawable.ph_cpu, "operator", 1),
    SPageInfo("image-providers", "Image providers", DesignR.drawable.ph_image, "operator", 2),
    SPageInfo("video-providers", "Video providers", DesignR.drawable.ph_film_strip, "operator", 3),
    SPageInfo("mcp", "MCP", DesignR.drawable.ph_plugs, "operator", 4),
    SPageInfo("api-keys", "API Keys", DesignR.drawable.ph_key, "operator", 5),
    SPageInfo("permissions", "Permissions", DesignR.drawable.ph_shield_check, "operator", 6),
    SPageInfo("infrastructure", "Infrastructure", DesignR.drawable.ph_cloud, "operator", 7),
    SPageInfo("security", "Security", DesignR.drawable.ph_shield, "operator", 8),
    SPageInfo("agents", "Agents", DesignR.drawable.ph_gear_six, "operator", 9),
    SPageInfo("webhooks", "Webhooks", DesignR.drawable.ph_git_branch, "operator", 10),
    SPageInfo("about", "About", DesignR.drawable.ph_info, "about", 1),
    SPageInfo("device-access", "Device Access", DesignR.drawable.ph_hand, "bots", 10),
    SPageInfo("remote-control", "Allternit Mobile", DesignR.drawable.ph_device_mobile, "computers", 4),
    SPageInfo("self-hosted", "Self-hosted server", DesignR.drawable.ph_hard_drives, "computers", 7),
    SPageInfo("files", "Files", DesignR.drawable.ph_floppy_disk, "computers", 8),
    SPageInfo("meetings", "Meetings", DesignR.drawable.ph_play_fill, "assistant", 6),
    SPageInfo("platform", "Platform", DesignR.drawable.ph_sliders_horizontal, "developer", 4),
    SPageInfo("brain", "Brain", DesignR.drawable.ph_books, "developer", 5),
)

private fun page(id: String): SPageInfo = ALL_PAGES.first { it.id == id }

/** A root row: a page, or one of the App group's own rows. */
private sealed interface RootRow {
    data class Page(val id: String, val label: String? = null) : RootRow
    data object Customize : RootRow
    data object Feedback : RootRow
    data object Changelog : RootRow
    data object OpenLinks : RootRow
    data object Haptics : RootRow
    data object ReduceMotion : RootRow
}

private val ROOT_GROUPS: List<Pair<String, List<RootRow>>> = listOf(
    "Account" to listOf(RootRow.Page("profile"), RootRow.Page("billing"), RootRow.Page("usage"), RootRow.Page("notifications"), RootRow.Page("privacy"), RootRow.Feedback, RootRow.Changelog),
    "Assistant" to listOf(RootRow.Customize, RootRow.Page("memory"), RootRow.Page("voice", "Voice and dictation")),
    "Bots and computers" to listOf(RootRow.Page("subscriptions"), RootRow.Page("computers", "Computers and devices"), RootRow.Page("agent-rules")),
    "App" to listOf(RootRow.OpenLinks, RootRow.Haptics, RootRow.ReduceMotion),
)

private val ADVANCED_SECTIONS = listOf(
    "account" to "Account", "assistant" to "Assistant", "bots" to "Bots and agents", "computers" to "Computers and devices",
    "developer" to "Developer", "operator" to "Operator", "about" to "About",
)

private val rootPageIds: Set<String> = ROOT_GROUPS.flatMap { it.second }.mapNotNull { (it as? RootRow.Page)?.id }.toSet()

private fun advancedItems(section: String): List<SPageInfo> =
    ALL_PAGES.filter { it.id !in rootPageIds && it.section == section }.sortedBy { it.order }

private fun pageLabel(id: String): String =
    ROOT_GROUPS.flatMap { it.second }.firstNotNullOfOrNull { (it as? RootRow.Page)?.takeIf { r -> r.id == id }?.label } ?: ALL_PAGES.firstOrNull { it.id == id }?.label ?: "Catalog"

private const val TERMS_URL = "https://allternit.com/terms"
private const val PRIVACY_URL = "https://allternit.com/privacy"
private const val SUPPORT_URL = "https://allternit.com/support"
private const val STATUS_URL = "https://status.allternit.com"
private const val FEEDBACK_URL = "https://allternit.com/feedback"
private const val CHANGELOG_URL = "https://docs.allternit.com/changelog"

/**
 * Settings (iOS SettingsView, phone calm I13): the root with the Accounts card, Account, Assistant, Bots and
 * computers, App, Appearance, one Advanced row and Log out; Advanced lists every other page. Each row pushes its
 * page with a ‹ back. Pages not built yet (Usage, the provider pages) show a placeholder that names the screen.
 */
@Composable
fun SettingsRoot(host: SettingsHost) {
    val stack = remember { mutableStateListOf<String>().apply { host.startPage?.let { add(it) } } }
    val top = stack.lastOrNull()
    BackHandler(enabled = stack.isNotEmpty()) { stack.removeAt(stack.lastIndex) }
    val back: () -> Unit = { if (stack.isNotEmpty()) stack.removeAt(stack.lastIndex) }
    val push: (String) -> Unit = { stack.add(it) }
    Box(Modifier.fillMaxSize().background(AllternitTheme.colors.bgPrimary)) {
        val frame = SettingsFrame(stack.size, top)
        val reduced = rememberReduceMotion()
        AnimatedContent(
            targetState = frame,
            transitionSpec = {
                if (reduced) EnterTransition.None togetherWith ExitTransition.None
                else {
                    val pushing = targetState.depth > initialState.depth
                    val spec = tween<IntOffset>(380, easing = PUSH_EASING)
                    if (pushing) (slideInHorizontally(spec) { it } togetherWith slideOutHorizontally(spec) { -(it * 0.28f).toInt() }).apply { targetContentZIndex = 1f }
                    else (slideInHorizontally(spec) { -(it * 0.28f).toInt() } togetherWith slideOutHorizontally(spec) { it }).apply { targetContentZIndex = -1f }
                }
            },
            label = "settingsPush",
        ) { f ->
        val page = f.id
        CompositionLocalProvider(LocalCrumbIcon provides crumbIconFor(page)) {
        when (page) {
            null -> RootList(host, push)
            "advanced" -> AdvancedPage(host, back, push)
            "open-links" -> OpenLinksPage(back)
            "delete-account" -> DeleteAccountPage(host, back, onGone = { host.onClose() })
            "profile" -> ProfilePage(host, back, push)
            "signin" -> ProfilePage(host, back, push)
            "organization" -> OrganizationPage(host, back)
            "privacy" -> PrivacyPage(host, back, push)
            "connected-apps" -> ConnectedAppsPage(back)
            "notifications" -> NotificationsPage(host, back)
            "subscriptions" -> host.connections(back)
            "device-access" -> DeviceAccessPage(back)
            "devices" -> DevicePairingPage(host, back)
            "response-style" -> ResponseStylePage(host, back, push)
            "memory" -> MemoryPage(host, back, push)
            "memory-drive" -> MemoryDrivePage(host, back, push)
            "memory-history" -> MemoryHistoryPage(host, back)
            "knowledge" -> KnowledgeReviewPage(back, push)
            "knowledge-proposal" -> KnowledgeProposalPage(back)
            "models" -> ModelsPage(host, back, push)
            "subagents" -> SubagentsPage(host, back)
            "mcp" -> McpPage(host, back)
            "webhooks" -> WebhooksPage(host, back)
            "about" -> AboutPage(host, back)
            "customize" -> CustomizeRoot(host, back)
            "connectors" -> ConnectorsRoot(back)
            "skills" -> CapabilitiesPage(host, plugins = false, back = back)
            "plugins" -> CapabilitiesPage(host, plugins = true, back = back)
            "infrastructure" -> InfrastructurePage(back, push)
            "platform" -> PlatformPage(back)
            "cloud-instances" -> ResourcePage(CLOUD_INSTANCES, back)
            "cloud-deploy" -> ResourcePage(CLOUD_DEPLOY, back)
            "vps-servers" -> ResourcePage(VPS_SERVERS, back)
            "nodes" -> ResourcePage(NODES, back, footer = ({ NodeJoinToken() }))
            "cloud-backups" -> CloudBackupsPage(back)
            "byoc" -> ResourcePage(BYOC, back)
            "runtime-ops" -> RuntimeOpsPage(host, back, push)
            "prewarm" -> PrewarmPage(back)
            "replay" -> ResourcePage(REPLAY, back)
            "meetings" -> MeetingsPage(back)
            "twin" -> TwinPage(back)
            "files" -> FilesPage(back, push)
            "files-keys" -> FilesKeysPage(back)
            "voice" -> VoicePage(back)
            "self-hosted" -> SelfHostedPage(back, push)
            "self-hosted-session" -> SelfHostedSessionPage(back)
            "brain" -> BrainPage(back, push)
            "brain-capture" -> BrainCapturePage(back)
            "billing" -> BillingPage(host, back)
            "usage" -> UsagePage(host, back)
            "gizziio-code" -> ProductsPage(ProductsPart.GizziCode, host, back)
            "cowork" -> ProductsPage(ProductsPart.Cowork, host, back)
            "extensions" -> ProductsPage(ProductsPart.Extensions, host, back)
            else -> SPlaceholder(pageLabel(page), back)
        }
        }
        }
    }
}

/** One Settings screen in the push stack; [depth] tells push from pop. */
private data class SettingsFrame(val depth: Int, val id: String?)

/** `cubic-bezier(.32,.72,0,1)` from the approved phone push. */
private val PUSH_EASING = CubicBezierEasing(0.32f, 0.72f, 0f, 1f)

/** The page's icon for the back crumb tile; null shows the plain back button. */
private fun crumbIconFor(id: String?): Int? = when (id) {
    null -> null
    "advanced" -> DesignR.drawable.ph_terminal
    "open-links" -> DesignR.drawable.ph_arrow_square_out
    "knowledge", "knowledge-proposal" -> DesignR.drawable.ph_books
    else -> ALL_PAGES.firstOrNull { it.id == id }?.icon
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun RootList(host: SettingsHost, push: (String) -> Unit) {
    val context = LocalContext.current
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    val openLink = rememberLinkOpener()
    var info by remember { mutableStateOf(false) }
    var haptics by remember { mutableStateOf(SettingsPrefs.hapticsOn(context)) }
    var reduceMotion by remember { mutableStateOf(SettingsPrefs.reduceMotion(context)) }
    var inApp by remember { mutableStateOf(SettingsPrefs.openLinksInApp(context)) }
    var signOutError by remember { mutableStateOf<String?>(null) }
    val account = host.account
    remember(context) { ResponseFontPrefs.load(context); 0 }
    SPage(
        "Settings", tag = "settingsScreen", animateIn = false,
        leading = { SRoundButton(DesignR.drawable.ph_x, "Close settings", "settingsClose", host.onClose) },
        trailing = { SRoundButton(DesignR.drawable.ph_info, "About Allternit", "settingsInfoMenu") { info = true } },
    ) {
        // Accounts card: the person (opens Profile) and their organization.
        SCard {
            if (account != null) {
                AccountRow(account.displayName, account.email ?: "", "settingsAccountRow") { push("profile") }
                if (account.organizationId != null) {
                    SDivider()
                    SRow("Organization", "settingsOrgRow", icon = DesignR.drawable.ph_globe, detail = "Bots, chats and billing follow the active account.", onClick = { push("organization") })
                    OrgSwitcher(account.organizationId)
                }
            } else {
                AccountRow("Account", "Sign in to sync across devices", "settingsAccountRow") { push("profile") }
            }
        }
        for ((label, rows) in ROOT_GROUPS) {
            RootLabel(label)
            SCard {
                rows.forEachIndexed { index, row ->
                    if (index > 0) SDivider()
                    Box(Modifier.pressScale98()) {
                    when (row) {
                        is RootRow.Page -> SRow(row.label ?: page(row.id).label, "settingsRow_${row.id}", icon = page(row.id).icon, onClick = { push(row.id) })
                        // E8: the row is "Catalog" (nav id stays `customize`); it opens the same screen.
                        RootRow.Customize -> SRow("Catalog", "settingsRow_customize", icon = DesignR.drawable.ph_sliders_horizontal, onClick = { push("customize") })
                        RootRow.Feedback -> SRow("Give feedback", "settingsRow_feedback", icon = DesignR.drawable.ph_chat_text, onClick = { openLink(FEEDBACK_URL) })
                        RootRow.Changelog -> SRow("View changelog", "settingsRow_changelog", icon = DesignR.drawable.ph_list_bullets, onClick = { openLink(CHANGELOG_URL) }, trailing = {
                            androidx.compose.material3.Icon(androidx.compose.ui.res.painterResource(DesignR.drawable.ph_arrow_up_right), contentDescription = "Opens outside Settings", tint = AllternitTheme.colors.textTertiary, modifier = Modifier.size(14.dp))
                        })
                        RootRow.OpenLinks -> SRow("Open links in", "settingsRow_openLinks", icon = DesignR.drawable.ph_arrow_square_out, value = if (inApp) "In app" else "Browser", onClick = { push("open-links") })
                        RootRow.Haptics -> SToggleRow("Haptic feedback", "settingsRow_haptics", haptics, icon = DesignR.drawable.ph_vibrate, onChange = { haptics = it; SettingsPrefs.setHapticsOn(context, it); if (it) PolishHaptic.perform(view, PolishHaptic.Kind.Selection) })
                        RootRow.ReduceMotion -> SToggleRow("Reduce motion", "settingsRow_reduceMotion", reduceMotion, icon = DesignR.drawable.ph_arrows_down_up, onChange = { reduceMotion = it; SettingsPrefs.setReduceMotion(context, it); if (haptics) PolishHaptic.perform(view, PolishHaptic.Kind.Selection) })
                    }
                    }
                }
            }
        }
        RootLabel("Appearance")
        SCard {
            AppearanceTiles()
            SDivider()
            // Response font (E5: Serif by default): the face of assistant replies in chat.
            SRow("Response font", "settingsRow_responseFont", icon = DesignR.drawable.ph_chat_text, chevron = false, trailing = {
                SSegmented(listOf("Sans", "Serif"), if (ResponseFontPrefs.serif) 1 else 0, "responseFont") { ResponseFontPrefs.set(context, it == 1) }
            })
        }
        SCard(Modifier.padding(top = 18.dp)) {
            val count = ADVANCED_SECTIONS.sumOf { advancedItems(it.first).size }
            SRow("Advanced", "settingsAdvancedRow", icon = DesignR.drawable.ph_terminal, value = "$count pages", onClick = { push("advanced") })
        }
        if (account != null) {
            SCard(Modifier.padding(top = 22.dp)) {
                SRow("Log out", "settingsSignOut", icon = DesignR.drawable.ph_sign_out, destructive = true, chevron = false, onClick = {
                    scope.launch {
                        val result = AllternitAuth.signOut()
                        // Success flips AllternitAuth.state, and MainActivity shows sign-in.
                        signOutError = result.error
                    }
                })
            }
            if (signOutError != null) SError("Couldn't log out. $signOutError")
        } else {
            SCard(Modifier.padding(top = 22.dp)) { SRow("Sign in", "settingsSignIn", chevron = false, onClick = host.onClose) }
        }
    }
    if (info) {
        ModalBottomSheet(onDismissRequest = { info = false }, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = AllternitTheme.colors.bgPrimary) {
            Column(Modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 12.dp).padding(bottom = 12.dp).testTag("settingsInfoSheet")) {
                SRow("Allternit ${host.versionName}", "infoAbout", icon = DesignR.drawable.ph_book_open_thin, onClick = { info = false; push("about") })
                SRow("Terms", "infoTerms", icon = DesignR.drawable.ph_arrow_square_out, onClick = { info = false; openLink(TERMS_URL) })
                SRow("Privacy Policy", "infoPrivacy", icon = DesignR.drawable.ph_arrow_square_out, onClick = { info = false; openLink(PRIVACY_URL) })
                SRow("Help and support", "infoSupport", icon = DesignR.drawable.ph_arrow_square_out, onClick = { info = false; openLink(SUPPORT_URL) })
                SRow("Status", "infoStatus", icon = DesignR.drawable.ph_arrow_square_out, onClick = { info = false; openLink(STATUS_URL) })
            }
        }
    }
}

/** Root section heading: the grey 14sp label over a flat group (web phone frame), with its "<label> section" description. */
@Composable
private fun RootLabel(text: String) = SLabel(text)

/** Row press feedback (.98) without taking the tap from the row: watches the pointer on the initial pass only. */
@Composable
private fun Modifier.pressScale98(): Modifier {
    var pressed by remember { mutableStateOf(false) }
    val s by animateFloatAsState(if (pressed) 0.98f else 1f, label = "rowPress")
    return this
        .graphicsLayer { scaleX = s; scaleY = s }
        .pointerInput(Unit) {
            awaitEachGesture {
                awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
                pressed = true
                waitForUpOrCancellation(PointerEventPass.Initial)
                pressed = false
            }
        }
}

@Composable
private fun AccountRow(name: String, detail: String, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        Modifier.fillMaxWidth().heightIn(min = 60.dp).testTag(tag).clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(Modifier.size(36.dp).clip(CircleShape).background(c.borderStrong), contentAlignment = Alignment.Center) {
            Text(name.firstOrNull()?.uppercaseChar()?.toString() ?: "?", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
        }
        Column(Modifier.weight(1f)) {
            Text(name, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
            if (detail.isNotBlank()) Text(detail, color = c.textSecondary, fontSize = 13.sp)
        }
    }
}

/** Approved look: three window pictures (sidebar lines + composer pill; System split on the diagonal), a ring that eases to the choice, bold label. */
@Composable
private fun AppearanceTiles() {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val mode = AppearancePrefs.mode
    val reduced = rememberReduceMotion()
    Row(Modifier.fillMaxWidth().padding(12.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        for ((value, label) in listOf(ThemeMode.Light to "Light", ThemeMode.Dark to "Dark", ThemeMode.System to "System")) {
            val selected = mode == value
            val ring by animateFloatAsState(if (selected) 1f else 0f, if (reduced) androidx.compose.animation.core.snap() else androidx.compose.animation.core.tween(220), label = "ring")
            val ringShape = RoundedCornerShape(AllternitRadii.MD + 3.dp)
            Column(
                Modifier.weight(1f).testTag("appearance_${label.lowercase()}")
                    .clickable(role = androidx.compose.ui.semantics.Role.RadioButton) { AppearancePrefs.set(context, value) },
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Box(Modifier.fillMaxWidth().border(2.dp, c.textPrimary.copy(alpha = ring), ringShape).padding(3.dp)) {
                    WindowThumb(value, Modifier.fillMaxWidth().aspectRatio(1.4f).clip(RoundedCornerShape(AllternitRadii.MD)).border(1.dp, c.borderSubtle, RoundedCornerShape(AllternitRadii.MD)))
                }
                Text(label, color = c.textPrimary, fontSize = 13.sp, fontWeight = if (selected) FontWeight.Bold else FontWeight.Normal, modifier = Modifier.padding(top = 8.dp))
            }
        }
    }
}

/** A tiny app window in [mode]'s colors; System draws Light with Dark on the lower-right of the diagonal. */
@Composable
private fun WindowThumb(mode: ThemeMode, modifier: Modifier) {
    androidx.compose.foundation.Canvas(modifier) {
        fun window(k: com.allternit.core.design.AllternitColorScheme) {
            val w = size.width; val h = size.height
            drawRect(k.bgPrimary)
            drawRect(k.fillNeutral, topLeft = Offset.Zero, size = Size(w * 0.28f, h))
            for (n in 0 until 3) {
                drawRoundRect(k.textTertiary, Offset(w * 0.05f, h * (0.16f + n * 0.14f)), Size(w * (if (n == 1) 0.14f else 0.18f), h * 0.05f), CornerRadius(h * 0.025f))
            }
            drawRoundRect(k.bgPanel, Offset(w * 0.36f, h * 0.72f), Size(w * 0.56f, h * 0.17f), CornerRadius(h * 0.085f))
            drawRoundRect(k.borderSubtle, Offset(w * 0.36f, h * 0.72f), Size(w * 0.56f, h * 0.17f), CornerRadius(h * 0.085f), style = Stroke(1.dp.toPx()))
        }
        when (mode) {
            ThemeMode.Light -> window(AllternitColors.Light)
            ThemeMode.Dark -> window(AllternitColors.Dark)
            else -> {
                window(AllternitColors.Light)
                val tri = Path().apply { moveTo(size.width, 0f); lineTo(size.width, size.height); lineTo(0f, size.height); close() }
                clipPath(tri) { window(AllternitColors.Dark) }
            }
        }
    }
}

@Composable
private fun AdvancedPage(host: SettingsHost, back: () -> Unit, push: (String) -> Unit) {
    val context = LocalContext.current
    val openLink = rememberLinkOpener()
    var drafts by remember { mutableStateOf(SettingsPrefs.saveDrafts(context)) }
    var games by remember { mutableStateOf(SettingsPrefs.waitingGames(context)) }
    SPage("Advanced", tag = "settingsAdvanced", onBack = back) {
        for ((section, label) in ADVANCED_SECTIONS) {
            SLabel(label)
            SCard {
                val items = advancedItems(section)
                items.forEachIndexed { index, p ->
                    if (index > 0) SDivider()
                    SRow(p.label, "settingsRow_${p.id}", icon = p.icon, onClick = { push(p.id) })
                }
                when (section) {
                    "assistant" -> {
                        SDivider()
                        SToggleRow("Save chat drafts", "settingsRow_saveDrafts", drafts, icon = DesignR.drawable.ph_floppy_disk, onChange = { drafts = it; SettingsPrefs.setSaveDrafts(context, it) })
                        SDivider()
                        SToggleRow("Waiting games", "settingsRow_waitingGames", games, icon = DesignR.drawable.ph_game_controller, onChange = { games = it; SettingsPrefs.setWaitingGames(context, it) })
                    }
                    "computers" -> {
                        SDivider()
                        SRow("Let your bots use this phone", "settingsRow_phoneControl", icon = DesignR.drawable.ph_desktop, onClick = host.openPhoneControl)
                    }
                    "about" -> {
                        SDivider()
                        SRow("System status", "settingsRow_systemStatus", icon = DesignR.drawable.ph_record_fill, onClick = { openLink(STATUS_URL) })
                    }
                }
            }
        }
        SCard(Modifier.padding(top = 18.dp)) {
            SRow("Open on the web", "settingsRow_web", icon = DesignR.drawable.ph_arrow_square_out, onClick = { openLink("https://ai.allternit.com") })
            SDivider()
            SRow("Courses", "settingsRow_courses", icon = DesignR.drawable.ph_book_open, onClick = { openLink("https://labs.allternit.com") })
        }
    }
}

/** iOS OpenLinksSettingsPage. */
@Composable
private fun OpenLinksPage(back: () -> Unit) {
    val context = LocalContext.current
    var inApp by remember { mutableStateOf(SettingsPrefs.openLinksInApp(context)) }
    SPage("Open links in", tag = "settingsOpenLinks", onBack = back) {
        SCard {
            SRow("In app", "openLinks_app", detail = "Links open in a window over Allternit.", chevron = false, onClick = { inApp = true; SettingsPrefs.setOpenLinksInApp(context, true) },
                trailing = { if (inApp) CheckMark() })
            SDivider()
            SRow("Browser", "openLinks_browser", detail = "Links open in your browser app.", chevron = false, onClick = { inApp = false; SettingsPrefs.setOpenLinksInApp(context, false) },
                trailing = { if (!inApp) CheckMark() })
        }
    }
}

@Composable
internal fun CheckMark() {
    androidx.compose.material3.Icon(
        androidx.compose.ui.res.painterResource(DesignR.drawable.ph_check), contentDescription = "Selected",
        tint = AllternitTheme.colors.textPrimary, modifier = Modifier.size(20.dp),
    )
}

@Composable
private fun AboutPage(host: SettingsHost, back: () -> Unit) {
    val openLink = rememberLinkOpener()
    SPage("About", tag = "settingsAbout", onBack = back) {
        SCard {
            SRow("Version", "about_version", value = host.versionName, chevron = false)
            SDivider()
            SRow("Terms", "about_terms", onClick = { openLink(TERMS_URL) })
            SDivider()
            SRow("Privacy Policy", "about_privacy", onClick = { openLink(PRIVACY_URL) })
            SDivider()
            SRow("Help and support", "about_support", onClick = { openLink(SUPPORT_URL) })
        }
    }
}
