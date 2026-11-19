package com.allternit.feature.shell

import android.content.Intent
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
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
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
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch

/**
 * The native app's only activity (PLAN.md section 2: single MainActivity). Launcher of the
 * `nativeapp` flavor; the play and phonelink builds keep the TWA launcher.
 *
 * Signed out → [SignInScreen]. Signed in → [WorkspaceShell]: home, drawer and mode fan (step 7c-1).
 * Search, the session overview and the real chat screen arrive in later steps.
 *
 * Debug builds only: `adb shell am start -n com.allternit.app.ondevice/com.allternit.feature.shell.MainActivity
 * --es ticket <sign_in_token>` signs in with a Clerk sign-in token (Maestro, emulator checks).
 * Release builds compile the check out (BuildConfig.DEBUG is a constant).
 */
class MainActivity : ComponentActivity() {
    /** Debug "Continue without signing in" (iOS skipAuth): the home without a session. */
    private var skippedAuth by mutableStateOf(false)

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
                    when (val s = state) {
                        is AuthState.SignedIn -> WorkspaceShell(account = s.account, onOpenWeb = ::openWebVersion)
                        else -> if (skippedAuth) {
                            WorkspaceShell(account = null, onOpenWeb = ::openWebVersion)
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
            }
        }
        handleDebugTicket(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleDebugTicket(intent)
    }

    /**
     * "Use web version" (PLAN.md section 3 rollback switch, first cut): the retained TWA
     * launcher in the same package opens m.allternit.com.
     */
    private fun openWebVersion() {
        // NEW_TASK is required: without it androidbrowserhelper's LauncherActivity restarts
        // itself in a new task, the second copy sees the first still alive
        // (sLauncherActivitiesAlive > 1) and finishes, so nothing opens (Moto, 2026-11-18).
        startActivity(Intent().setClassName(packageName, TWA_LAUNCHER).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
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
        const val TWA_LAUNCHER = "com.google.androidbrowserhelper.trusted.LauncherActivity"
    }
}
