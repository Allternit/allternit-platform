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
 *
 * Every build: the TWA → native sign-in hand-off ([TwaHandoff]) on the first signed-out launch
 * after the update from the old Play app, and its callback link.
 */
class MainActivity : ComponentActivity() {
    /** Debug "Continue without signing in" (iOS skipAuth): the home without a session. */
    private var skippedAuth by mutableStateOf(false)

    /** Debug launch extras for the workspace (iOS `-open-tab`, `-sidebar`, …); default in release. */
    private var shellLaunch by mutableStateOf(ShellLaunch())

    private val twaHandoff by lazy { TwaHandoff(applicationContext) }
    private var twaHandoffChecked = false

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
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        setContent {
            val state by AllternitAuth.state.collectAsState()
            val signedIn = state is AuthState.SignedIn || skippedAuth
            val firstRun = remember { FirstRunStore(applicationContext) }
            LaunchedEffect(firstRun) { firstRun.load() }
            // Sign-in is always dark, like iOS LoginGateView; the workspace is light by default.
            // System bar icons follow the screen: light icons on the dark sign-in, dark icons on
            // the light workspace. Bars stay transparent (no grey 3-button nav scrim).
            DisposableEffect(signedIn) {
                applySystemBars(darkContent = !signedIn)
                onDispose {}
            }
            AllternitTheme(themeMode = if (signedIn) ThemeMode.Light else ThemeMode.Dark) {
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
                    // Over setup and then over the freshly mounted workspace (iOS runs it in its own window).
                    handoff?.let { spec ->
                        SetupHandoffOverlay(spec, dark = !signedIn, onDone = { handoff = null })
                    }
                }
            }
        }
        if (BuildConfig.DEBUG) readDebugLaunch(intent)
        handleDebugTicket(intent)
        handleHandoffCallback(intent)
        maybeStartTwaHandoff()
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
        handleHandoffCallback(intent)
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

    /** First signed-out launch after the TWA app: try the sign-in hand-off once ([TwaHandoff]). */
    private fun maybeStartTwaHandoff() {
        if (twaHandoffChecked || skippedAuth) return
        twaHandoffChecked = true
        lifecycleScope.launch {
            val ready = AllternitAuth.state.first { it !is AuthState.Starting }
            if (ready is AuthState.SignedIn) {
                // Already signed in natively: the hand-off never applies to this install.
                twaHandoff.markTried()
                return@launch
            }
            if (ready !is AuthState.SignedOut || !twaHandoff.shouldTry()) return@launch
            twaHandoff.markTried()
            if (!twaHandoff.isEnabled(ShellApi.okHttp)) {
                Log.i(TAG, "twa hand-off: not available, normal sign-in")
                return@launch
            }
            if (!twaHandoff.launch(this@MainActivity)) Log.i(TAG, "twa hand-off: no browser, normal sign-in")
        }
    }

    /** The hand-off page's callback link: sign in with the token, else stay on sign-in. */
    private fun handleHandoffCallback(intent: Intent) {
        val uri = intent.data
        if (!twaHandoff.isCallback(uri)) return
        intent.data = null
        lifecycleScope.launch {
            val ticket: String? = try {
                twaHandoff.complete(uri!!)
            } catch (e: Exception) {
                Log.i(TAG, "twa hand-off refused: ${e.message}")
                null
            }
            if (ticket == null) return@launch
            val ready = AllternitAuth.state.first { it !is AuthState.Starting }
            if (ready !is AuthState.SignedOut) return@launch
            val result = AllternitAuth.signInWithTicket(ticket)
            Log.i(TAG, "twa hand-off sign-in: ${result.error ?: "ok"}")
        }
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
