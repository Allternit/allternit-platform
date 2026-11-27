package com.allternit.feature.shell

import android.content.Intent
import android.provider.Settings
import android.graphics.Color
import android.os.Build
import android.os.Bundle
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.size
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.testTagsAsResourceId
import androidx.compose.ui.unit.dp
import androidx.lifecycle.lifecycleScope
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.auth.AuthState
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.ThemeMode
import com.allternit.feature.onboarding.SignInScreen
import androidx.compose.runtime.CompositionLocalProvider
import com.allternit.feature.shell.firstrun.FirstRunGate
import com.allternit.feature.shell.firstrun.FirstRunStore
import com.allternit.feature.shell.firstrun.LocalSetupHandoff
import com.allternit.feature.shell.firstrun.SetupHandoffHost
import com.allternit.feature.shell.firstrun.SetupHandoffOverlay
import com.allternit.feature.shell.firstrun.SetupHandoffSpec
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch

/**
 * The native app's only activity (PLAN.md section 2: single MainActivity). Launcher of the
 * `nativeapp` flavor; the play and phonelink builds keep the TWA launcher.
 *
 * Signed out → [SignInScreen]. Signed in → the first-run setup ([FirstRunGate], iOS OnboardingView)
 * until it is done on this phone, then [WorkspaceShell]: home, drawer and mode fan. Like iOS, the
 * setup is a root swap after sign-in and never covers the sign-in screen; the debug "Continue
 * without signing in" path skips it.
 *
 * Debug builds only: `adb shell am start -n com.allternit.app.ondevice/com.allternit.feature.shell.MainActivity
 * --es ticket <sign_in_token>` signs in with a Clerk sign-in token (Maestro, emulator checks).
 * The iOS launch arguments work as extras too ([readDebugLaunch]): `skip-auth`, `open-tab <id>`,
 * `sidebar`, `shell-mode <home|bots|code|aci>`, `open-inbox`, `open-quick-task`,
 * `open-continue-cli`. Release builds compile the checks out (BuildConfig.DEBUG is a constant).
 */
class MainActivity : ComponentActivity() {
    /** Debug "Continue without signing in" (iOS skipAuth): the home without a session. */
    private var skippedAuth by mutableStateOf(false)
    /** A document opened with Allternit from another app (iOS onOpenURL, LocalDocumentView). */
    private var openedDocument by mutableStateOf<android.net.Uri?>(null)

    /** Debug launch extras for the workspace (iOS `-open-tab`, `-sidebar`, …); default in release. */
    private var shellLaunch by mutableStateOf(ShellLaunch())

    /** A Settings section a setup checklist fix asked for, opened once the workspace is up. */
    private var openSettingsOnStart by mutableStateOf(false)

    /** The branded setup hand-off, drawn above everything so it outlives the swap to the workspace. */
    private var handoff by mutableStateOf<SetupHandoffSpec?>(null)
    private val handoffHost = SetupHandoffHost { sources, name, finish ->
        if (handoff != null) false else { handoff = SetupHandoffSpec(sources, name, finish); true }
    }

    private fun applySystemBars(darkContent: Boolean) {
        val style = if (darkContent) {
            SystemBarStyle.dark(Color.TRANSPARENT)
        } else {
            SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT)
        }
        enableEdgeToEdge(statusBarStyle = style, navigationBarStyle = style)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) window.isNavigationBarContrastEnforced = false
    }

    @OptIn(ExperimentalComposeUiApi::class)
    override fun onCreate(savedInstanceState: Bundle?) {
        AllternitAuth.start(applicationContext)
        // Gap run GS-B: the Files drive (DocumentsProvider, Share > Save to computer, Files key) signs in with this
        // session instead of its own Custom Tab hand-off. Every Files route accepts the Clerk session.
        com.allternit.core.files.FilesSession.bearer = {
            if (AllternitAuth.state.value is AuthState.SignedIn) AllternitAuth.tokenProvider.bearerToken() else null
        }
        // Wave I1: notification channels, push registration after each sign-in, token removal at sign-out.
        com.allternit.feature.push.AllternitPush.start(applicationContext)
        // Sign-out clears the person's Files key material, Files drive and voice keys (iOS clearsPersonData).
        com.allternit.feature.settings.SettingsPersonData.start(applicationContext)
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContent {
            val state by AllternitAuth.state.collectAsState()
            val signedIn = state is AuthState.SignedIn || skippedAuth
            // Wave H1 (iOS StorePurchaseManager.start at launch): load the store catalog and post every open Play purchase,
            // again after each sign-in. Nothing is bought here.
            LaunchedEffect(state is AuthState.SignedIn) {
                if (state is AuthState.SignedIn) com.allternit.feature.billing.PlayPurchases.start(applicationContext)
            }
            val firstRun = remember { FirstRunStore(applicationContext) }
            LaunchedEffect(firstRun) { firstRun.load() }
            // Sign-in is always dark, like iOS LoginGateView; the workspace is light by default.
            // System bar icons follow the screen: light icons on the dark sign-in, dark icons on
            // the light workspace. Bars stay transparent (no grey 3-button nav scrim).
            val appearance = com.allternit.feature.settings.AppearancePrefs.mode
            DisposableEffect(signedIn, appearance) {
                applySystemBars(darkContent = !signedIn || appearance == ThemeMode.Dark)
                onDispose {}
            }
            // Settings > Appearance picks the workspace theme (light by default); sign-in is always dark.
            LaunchedEffect(Unit) { com.allternit.feature.settings.AppearancePrefs.load(applicationContext) }
            AllternitTheme(themeMode = if (signedIn) com.allternit.feature.settings.AppearancePrefs.mode else ThemeMode.Dark) {
                Box(
                    modifier = Modifier
                        .fillMaxSize()
                        .background(AllternitTheme.colors.bgPrimary)
                        // Maestro selects Modifier.testTag ids as resource ids (maestro/ids.md).
                        .semantics { testTagsAsResourceId = true },
                    contentAlignment = Alignment.Center,
                ) {
                    CompositionLocalProvider(LocalSetupHandoff provides handoffHost) {
                    when (val s = state) {
                        is AuthState.SignedIn -> when {
                            !firstRun.loaded -> Unit
                            !firstRun.isComplete -> FirstRunGate(firstRun, s.account.firstName, onDone = ::openSetupFix)
                            else -> WorkspaceShell(account = s.account, openSettingsOnStart = openSettingsOnStart, launch = shellLaunch)
                        }
                        else -> if (skippedAuth) {
                            WorkspaceShell(account = null, launch = shellLaunch)
                        } else {
                            when (s) {
                                AuthState.Starting, AuthState.Finishing -> CircularProgressIndicator(
                                    modifier = Modifier.size(28.dp),
                                    strokeWidth = 2.5.dp,
                                    color = AllternitTheme.colors.textSecondary,
                                )
                                is AuthState.Unavailable -> FriendlyState(
                                    style = FriendlyStyle.Offline,
                                    title = "Can't reach sign-in",
                                    message = s.message,
                                    actionTitle = "Try again",
                                    onAction = AllternitAuth::retry,
                                ) {
                                    Icon(
                                        painter = painterResource(com.allternit.core.design.R.drawable.ph_globe),
                                        contentDescription = null,
                                        tint = AllternitTheme.colors.textSecondary,
                                    )
                                }
                                else -> SignInScreen(
                                    onSkipAuth = if (BuildConfig.DEBUG) ({ skippedAuth = true }) else null,
                                )
                            }
                        }
                    }
                    }
                    openedDocument?.let { uri ->
                        com.allternit.feature.office.LocalDocumentScreen(uri, onClose = { openedDocument = null })
                    }
                    // Privacy > "Lock with screen lock" (iOS AppLockStore): covers the app at launch and after it was in the background.
                    if (signedIn) com.allternit.feature.settings.AppLockGate()
                    // Over setup and then over the freshly mounted workspace (iOS runs it in its own window).
                    handoff?.let { spec ->
                        SetupHandoffOverlay(spec, dark = !signedIn, onDone = { handoff = null })
                    }
                }
            }
        }
        if (BuildConfig.DEBUG) readDebugLaunch(intent)
        handleDebugTicket(intent)
        readOpenedDocument(intent)
        readPushTap(intent)
        readSystemLink(intent)
    }

    /** Foreground: refresh the status snapshot and redraw the widget, tiles, shortcuts and the ongoing run notification. */
    override fun onStart() {
        super.onStart()
        lifecycleScope.launch {
            val snap = runCatching { com.allternit.feature.system.StatusSync.refresh(applicationContext) }.getOrNull()
            com.allternit.feature.push.LiveRunNotification.update(applicationContext, snap)
        }
    }

    /**
     * An `allternit://` link from the widget, a Quick Settings tile or a shortcut (iOS DeepLinkRouter): `inbox`,
     * `inbox/<id>`, `bot/<id>/chat`, `bot/main/chat`, `call/main`, `handoff`, `routines`. The workspace applies it
     * through [ShellLaunch.nonce] like a notification tap.
     */
    private fun readSystemLink(intent: Intent) {
        val uri = intent.data ?: return
        if (intent.action != Intent.ACTION_VIEW || uri.scheme != com.allternit.feature.system.SystemLinks.SCHEME) return
        val segments = listOfNotNull(uri.host) + uri.pathSegments
        // Island bar (lane 36): allternit://bar?app=&tool=&v.<key>= opens the translucent bar over the app.
        // With `bar.island` off the link falls through to Home like any unknown link.
        if (segments.firstOrNull() == com.allternit.feature.bar.BarDeepLink.HOST && com.allternit.feature.bar.BarFlag.isOn(this)) {
            startActivity(com.allternit.feature.bar.BarDeepLink.intent(this, com.allternit.feature.bar.BarDeepLink.parse(uri) ?: com.allternit.feature.bar.BarLinkRequest()))
            intent.data = null
            return
        }
        val nonce = System.nanoTime()
        shellLaunch = when (segments.firstOrNull()) {
            "inbox" -> ShellLaunch(sheet = "inbox", inboxItem = segments.getOrNull(1), nonce = nonce)
            // Bots mode opens the bot's chat; "main" is the first bot of the roster (BotsLaunch.openBotId null opens the launchpad).
            "bot" -> ShellLaunch(mode = ShellMode.Bots, botId = segments.getOrNull(1)?.takeIf { it != "main" }, nonce = nonce)
            // iOS DeepLinkHost .callMain: the main bot's chat in Bots mode (its voice button starts the call).
            "call" -> ShellLaunch(mode = ShellMode.Bots, openMain = true, nonce = nonce)
            "handoff" -> ShellLaunch(mode = ShellMode.Bots, nonce = nonce)
            "routines", "routine" -> ShellLaunch(tab = "automation-tasks", nonce = nonce)
            // iOS DeepLink.computerConsole: allternit://computers/<id> opens that computer's console detail.
            "computers" -> ShellLaunch(mode = ShellMode.Aci, tab = "computers", computerId = segments.getOrNull(1), nonce = nonce)
            else -> ShellLaunch(mode = ShellMode.Home, nonce = nonce)
        }
        intent.data = null
    }

    /** Android 8-9 screen-lock confirmation for agent approvals (DeviceAuth). Newer versions use BiometricPrompt. */
    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (com.allternit.core.auth.DeviceAuth.onActivityResult(requestCode, resultCode)) return
        @Suppress("DEPRECATION")
        super.onActivityResult(requestCode, resultCode, data)
    }

    /**
     * A tapped notification (or its Open / Review button) opens what it points at (iOS
     * PushNotificationManager.handleTap → DeepLinkRouter): the inbox item, Decisions for an approval, Settings, or
     * Bots for a bot's message. The workspace applies it when it is already up ([ShellLaunch.nonce]).
     */
    private fun readPushTap(intent: Intent) {
        val tap = com.allternit.feature.push.AllternitPush.readTap(intent) ?: return
        val nonce = System.nanoTime()
        val url = tap.url.orEmpty()
        shellLaunch = when {
            // A computer_alert push (or a /computers/<id> url): that computer's console detail.
            tap.computer != null -> ShellLaunch(mode = ShellMode.Aci, tab = "computers", computerId = tap.computer, nonce = nonce)
            tap.inboxItem != null -> ShellLaunch(sheet = "inbox", inboxItem = tap.inboxItem, nonce = nonce)
            tap.event == "approval.requested" -> ShellLaunch(sheet = "inbox", nonce = nonce)
            url.contains("settings") -> ShellLaunch(tab = "settings", nonce = nonce)
            tap.botId != null -> ShellLaunch(mode = ShellMode.Bots, botId = tap.botId, sessionId = tap.sessionId, nonce = nonce)
            tap.sessionId != null -> ShellLaunch(mode = ShellMode.Bots, nonce = nonce)
            else -> ShellLaunch(mode = ShellMode.Home, nonce = nonce)
        }
    }

    /** An ACTION_VIEW on a content or file Uri (a PDF, text, Word or Excel file) opens in the local document viewer. */
    private fun readOpenedDocument(intent: Intent) {
        val uri = intent.data
        if (intent.action == Intent.ACTION_VIEW && uri != null && (uri.scheme == "content" || uri.scheme == "file")) openedDocument = uri
    }

    /**
     * iOS opens the fix's Settings page through DeepLinkRouter once the workspace is up. The
     * notifications fix opens Android's own notification settings for the app; every other
     * section opens the workspace's Settings page.
     */
    private fun openSetupFix(fix: String?) {
        when (fix) {
            null -> Unit
            "notifications" -> runCatching {
                startActivity(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, packageName))
            }
            else -> openSettingsOnStart = true
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleDebugTicket(intent)
        readOpenedDocument(intent)
        readPushTap(intent)
        readSystemLink(intent)
    }

    /**
     * iOS LaunchArguments for Maestro and screenshots, as intent extras. Maestro sends
     * `launchApp: arguments:` as extras; keys may carry leading dashes and values may be
     * strings or booleans, so both are read loosely (iOS launchArgumentEnabled / Value).
     */
    private fun readDebugLaunch(intent: Intent) {
        if (!BuildConfig.DEBUG) return
        val extras = intent.extras ?: return
        val values = extras.keySet().associate { key ->
            @Suppress("DEPRECATION")
            key.trimStart('-') to extras.get(key)?.toString()
        }
        fun enabled(key: String) = values.containsKey(key) && values[key]?.lowercase() != "false"
        fun value(key: String) = values[key]?.takeIf { it.isNotBlank() && it != "true" }
        if (enabled("skip-auth")) skippedAuth = true
        val mode = value("shell-mode")?.let { ShellMode.fromRaw(it) } ?: when {
            enabled("chat") -> ShellMode.Home
            enabled("code") -> ShellMode.Code
            enabled("browser") || enabled("open-aci") -> ShellMode.Aci
            enabled("bots") || enabled("open-bots") -> ShellMode.Bots
            else -> null
        }
        val tab = value("open-tab") ?: when {
            enabled("open-settings") || enabled("calm-settings") -> "settings"
            enabled("calm-search") -> "search"
            enabled("open-projects") -> "projects"
            enabled("open-artifacts") -> "artifacts-library"
            enabled("open-automation") -> "automation-tasks"
            enabled("open-customize") -> "customize"
            enabled("open-models") -> "models"
            else -> null
        }
        val sheet = when {
            enabled("open-inbox") -> "inbox"
            enabled("open-quick-task") -> "quick-task"
            enabled("open-continue-cli") -> "continue-cli"
            else -> null
        }
        shellLaunch = ShellLaunch(
            tab = tab,
            sidebar = enabled("sidebar") || enabled("calm-drawer-open"),
            mode = mode,
            sheet = sheet,
            inboxItem = value("inbox-item"),
        )
    }

    private fun handleDebugTicket(intent: Intent) {
        if (!BuildConfig.DEBUG) return
        val ticket = intent.getStringExtra(EXTRA_TICKET)?.takeIf { it.isNotBlank() } ?: return
        intent.removeExtra(EXTRA_TICKET)
        lifecycleScope.launch {
            val ready = AllternitAuth.state.first { it !is AuthState.Starting }
            if (ready !is AuthState.SignedOut) {
                Log.i(TAG, "ticket ignored: already $ready")
                return@launch
            }
            val result = AllternitAuth.signInWithTicket(ticket)
            Log.i(TAG, "ticket sign-in: ${result.error ?: "ok"}")
        }
    }

    private companion object {
        const val TAG = "AllternitMain"
        const val EXTRA_TICKET = "ticket"
    }
}
