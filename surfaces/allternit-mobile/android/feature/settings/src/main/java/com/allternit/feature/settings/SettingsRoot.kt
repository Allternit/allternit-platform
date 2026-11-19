package com.allternit.feature.settings

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
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
)

/** One page in Settings (iOS SettingsPageID + `phone: {section, order}`). */
internal data class SPageInfo(val id: String, val label: String, val icon: Int, val section: String, val order: Int)

/** Every page, in iOS order, with the web `SETTINGS_NAV_ITEMS` labels. Icons are the closest Phosphor glyphs the design module has. */
internal val ALL_PAGES: List<SPageInfo> = listOf(
    SPageInfo("profile", "Profile", DesignR.drawable.ph_identification_card, "account", 1),
    SPageInfo("appearance", "General", DesignR.drawable.ph_sliders_horizontal, "assistant", 3),
    SPageInfo("signin", "Account", DesignR.drawable.ph_identification_card, "account", 6),
    SPageInfo("organization", "Organization & access", DesignR.drawable.ph_globe, "account", 7),
    SPageInfo("privacy", "Privacy", DesignR.drawable.ph_seal_check, "account", 5),
    SPageInfo("subscriptions", "Connections", DesignR.drawable.ph_plugs_connected, "bots", 1),
    SPageInfo("billing", "Billing", DesignR.drawable.ph_book_open, "account", 2),
    SPageInfo("usage", "Usage", DesignR.drawable.ph_arrows_down_up, "account", 3),
    SPageInfo("setup", "Setup", DesignR.drawable.ph_check_circle_fill, "account", 8),
    SPageInfo("notifications", "Notifications", DesignR.drawable.ph_warning_circle_fill, "account", 4),
    SPageInfo("status", "Status", DesignR.drawable.ph_record_fill, "developer", 3),
    SPageInfo("voice", "Voice", DesignR.drawable.ph_play_fill, "assistant", 2),
    SPageInfo("memory", "Memory", DesignR.drawable.ph_books, "assistant", 1),
    SPageInfo("response-style", "Response style", DesignR.drawable.ph_sliders_horizontal, "assistant", 4),
    SPageInfo("open-models", "Open models", DesignR.drawable.ph_cpu, "assistant", 5),
    SPageInfo("gizziio-code", "Gizzi Code", DesignR.drawable.ph_terminal, "developer", 1),
    SPageInfo("cowork", "Files & devices", DesignR.drawable.ph_desktop, "computers", 3),
    SPageInfo("devices", "Devices", DesignR.drawable.ph_link_simple, "computers", 2),
    SPageInfo("computers", "Computers", DesignR.drawable.ph_desktop, "computers", 1),
    SPageInfo("desktop-pet", "Desktop pet", DesignR.drawable.ph_desktop, "computers", 5),
    SPageInfo("extensions", "Extensions", DesignR.drawable.ph_puzzle_piece, "computers", 6),
    SPageInfo("diagnostics", "Developer", DesignR.drawable.ph_terminal, "developer", 2),
    SPageInfo("agent-rules", "Agent rules", DesignR.drawable.ph_book_open, "bots", 2),
    SPageInfo("skills", "Skills", DesignR.drawable.ph_puzzle_piece, "bots", 3),
    SPageInfo("subagents", "Subagents", DesignR.drawable.ph_cpu, "bots", 4),
    SPageInfo("connectors", "Connectors", DesignR.drawable.ph_plugs_connected, "bots", 5),
    SPageInfo("connected-apps", "App approvals", DesignR.drawable.ph_seal_check, "bots", 7),
    SPageInfo("twin", "Your twin", DesignR.drawable.ph_identification_card, "bots", 8),
    SPageInfo("plugins", "Plugins", DesignR.drawable.ph_puzzle_piece, "bots", 6),
    SPageInfo("lens", "Lens Context", DesignR.drawable.ph_magnifying_glass, "bots", 9),
    SPageInfo("models", "Models", DesignR.drawable.ph_cpu, "operator", 1),
    SPageInfo("image-providers", "Image providers", DesignR.drawable.ph_globe, "operator", 2),
    SPageInfo("video-providers", "Video providers", DesignR.drawable.ph_play_fill, "operator", 3),
    SPageInfo("mcp", "MCP", DesignR.drawable.ph_plugs_connected_thin, "operator", 4),
    SPageInfo("api-keys", "API Keys", DesignR.drawable.ph_link_simple, "operator", 5),
    SPageInfo("permissions", "Permissions", DesignR.drawable.ph_check, "operator", 6),
    SPageInfo("infrastructure", "Infrastructure", DesignR.drawable.ph_cpu, "operator", 7),
    SPageInfo("security", "Security", DesignR.drawable.ph_seal_check, "operator", 8),
    SPageInfo("agents", "Agents", DesignR.drawable.ph_cpu, "operator", 9),
    SPageInfo("webhooks", "Webhooks", DesignR.drawable.ph_webhooks_logo, "operator", 10),
    SPageInfo("about", "About", DesignR.drawable.ph_book_open_thin, "about", 1),
    SPageInfo("device-access", "Device Access", DesignR.drawable.ph_check_circle_fill, "bots", 10),
    SPageInfo("remote-control", "Allternit Mobile", DesignR.drawable.ph_desktop, "computers", 4),
    SPageInfo("self-hosted", "Self-hosted server", DesignR.drawable.ph_desktop, "computers", 7),
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
    data object OpenLinks : RootRow
    data object Haptics : RootRow
    data object ReduceMotion : RootRow
}

private val ROOT_GROUPS: List<Pair<String, List<RootRow>>> = listOf(
    "Account" to listOf(RootRow.Page("profile"), RootRow.Page("billing"), RootRow.Page("usage"), RootRow.Page("notifications"), RootRow.Page("privacy")),
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
    ROOT_GROUPS.flatMap { it.second }.firstNotNullOfOrNull { (it as? RootRow.Page)?.takeIf { r -> r.id == id }?.label } ?: ALL_PAGES.firstOrNull { it.id == id }?.label ?: "Customize"

private const val TERMS_URL = "https://allternit.com/terms"
private const val PRIVACY_URL = "https://allternit.com/privacy"
private const val SUPPORT_URL = "https://allternit.com/support"
private const val STATUS_URL = "https://status.allternit.com"

/**
 * Settings (iOS SettingsView, phone calm I13): the root with the Accounts card, Account, Assistant, Bots and
 * computers, App, Appearance, one Advanced row and Log out; Advanced lists every other page. Each row pushes its
 * page with a ‹ back. Pages not built yet (Usage, the provider pages) show a placeholder that names the screen.
 */
@Composable
fun SettingsRoot(host: SettingsHost) {
    val stack = remember { mutableStateListOf<String>() }
    val top = stack.lastOrNull()
    BackHandler(enabled = stack.isNotEmpty()) { stack.removeAt(stack.lastIndex) }
    val back: () -> Unit = { if (stack.isNotEmpty()) stack.removeAt(stack.lastIndex) }
    val push: (String) -> Unit = { stack.add(it) }
    Box(Modifier.fillMaxSize().background(AllternitTheme.colors.bgPrimary)) {
        when (top) {
            null -> RootList(host, push)
            "advanced" -> AdvancedPage(host, back, push)
            "open-links" -> OpenLinksPage(back)
            "delete-account" -> DeleteAccountPage(host, back, onGone = { host.onClose() })
            "profile" -> ProfilePage(host, back, push)
            "signin" -> ProfilePage(host, back, push)
            "organization" -> OrganizationPage(host, back)
            "privacy" -> PrivacyPage(host, back)
            "notifications" -> NotificationsPage(host, back)
            "subscriptions" -> host.connections(back)
            "device-access" -> DeviceAccessPage(back)
            "devices" -> DevicePairingPage(host, back)
            "memory" -> MemoryPage(host, back, push)
            "memory-drive" -> MemoryDrivePage(host, back, push)
            "memory-history" -> MemoryHistoryPage(host, back)
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
            "nodes" -> ResourcePage(NODES, back)
            "cloud-backups" -> CloudBackupsPage(back)
            "byoc" -> ResourcePage(BYOC, back)
            "runtime-ops" -> RuntimeOpsPage(back, push)
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
            "gizziio-code" -> ProductsPage(ProductsPart.GizziCode, host, back)
            "cowork" -> ProductsPage(ProductsPart.Cowork, host, back)
            "extensions" -> ProductsPage(ProductsPart.Extensions, host, back)
            else -> SPlaceholder(pageLabel(top), back)
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun RootList(host: SettingsHost, push: (String) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val openLink = rememberLinkOpener()
    var info by remember { mutableStateOf(false) }
    var haptics by remember { mutableStateOf(SettingsPrefs.hapticsOn(context)) }
    var reduceMotion by remember { mutableStateOf(SettingsPrefs.reduceMotion(context)) }
    var inApp by remember { mutableStateOf(SettingsPrefs.openLinksInApp(context)) }
    var signOutError by remember { mutableStateOf<String?>(null) }
    val account = host.account
    SPage(
        "Settings", tag = "settingsScreen",
        leading = { SRoundButton(DesignR.drawable.ph_x, "Close settings", "settingsClose", host.onClose) },
        trailing = { SRoundButton(DesignR.drawable.ph_book_open_thin, "About Allternit", "settingsInfoMenu") { info = true } },
    ) {
        // Accounts card: the person (opens Profile) and their organization.
        SCard {
            if (account != null) {
                AccountRow(account.displayName, account.email ?: "", "settingsAccountRow") { push("profile") }
                if (account.organizationId != null) {
                    SDivider()
                    SRow("Organization", "settingsOrgRow", icon = DesignR.drawable.ph_globe, detail = "Bots, chats and billing follow the active account.", onClick = { push("organization") })
                }
            } else {
                AccountRow("Account", "Sign in to sync across devices", "settingsAccountRow") { push("profile") }
            }
        }
        for ((label, rows) in ROOT_GROUPS) {
            SLabel(label)
            SCard {
                rows.forEachIndexed { index, row ->
                    if (index > 0) SDivider()
                    when (row) {
                        is RootRow.Page -> SRow(row.label ?: page(row.id).label, "settingsRow_${row.id}", icon = page(row.id).icon, onClick = { push(row.id) })
                        RootRow.Customize -> SRow("Customize", "settingsRow_customize", icon = DesignR.drawable.ph_sliders_horizontal, onClick = { push("customize") })
                        RootRow.OpenLinks -> SRow("Open links in", "settingsRow_openLinks", icon = DesignR.drawable.ph_arrow_square_out, value = if (inApp) "In app" else "Browser", onClick = { push("open-links") })
                        RootRow.Haptics -> SToggleRow("Haptic feedback", "settingsRow_haptics", haptics, icon = DesignR.drawable.ph_dots_three_vertical, onChange = { haptics = it; SettingsPrefs.setHapticsOn(context, it) })
                        RootRow.ReduceMotion -> SToggleRow("Reduce motion", "settingsRow_reduceMotion", reduceMotion, icon = DesignR.drawable.ph_arrows_down_up, onChange = { reduceMotion = it; SettingsPrefs.setReduceMotion(context, it) })
                    }
                }
            }
        }
        SLabel("Appearance")
        SCard { AppearanceTiles() }
        SCard(Modifier.padding(top = 18.dp)) {
            val count = ADVANCED_SECTIONS.sumOf { advancedItems(it.first).size }
            SRow("Advanced", "settingsAdvancedRow", icon = DesignR.drawable.ph_terminal, value = "$count pages", onClick = { push("advanced") })
        }
        if (account != null) {
            SCard(Modifier.padding(top = 22.dp)) {
                SRow("Log out", "settingsSignOut", icon = DesignR.drawable.ph_arrow_up, destructive = true, chevron = false, onClick = {
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

/** iOS AppearanceTiles: Light, Dark, System. The workspace follows [AppearancePrefs]. */
@Composable
private fun AppearanceTiles() {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val mode = AppearancePrefs.mode
    Row(Modifier.fillMaxWidth().padding(12.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        for ((value, label) in listOf(ThemeMode.Light to "Light", ThemeMode.Dark to "Dark", ThemeMode.System to "System")) {
            val selected = mode == value
            Box(
                Modifier.weight(1f).heightIn(min = 64.dp).clip(RoundedCornerShape(AllternitRadii.MD))
                    .background(c.bgPrimary)
                    .border(if (selected) 2.dp else 1.dp, if (selected) c.brandAccent else c.borderSubtle, RoundedCornerShape(AllternitRadii.MD))
                    .testTag("appearance_${label.lowercase()}")
                    .clickable(role = androidx.compose.ui.semantics.Role.RadioButton) { AppearancePrefs.set(context, value) },
                contentAlignment = Alignment.Center,
            ) { Text(label, color = c.textPrimary, fontSize = 15.sp, fontWeight = if (selected) FontWeight.SemiBold else FontWeight.Normal) }
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
                        SToggleRow("Waiting games", "settingsRow_waitingGames", games, icon = DesignR.drawable.ph_play_fill, onChange = { games = it; SettingsPrefs.setWaitingGames(context, it) })
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
