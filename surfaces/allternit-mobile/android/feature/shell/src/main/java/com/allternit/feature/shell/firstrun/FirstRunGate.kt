package com.allternit.feature.shell.firstrun

import android.content.Context
import androidx.compose.animation.Crossfade
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import com.allternit.core.api.ApiConfig
import com.allternit.core.design.ThinkingOrb
import com.allternit.core.design.ThinkingOrbSize
import com.allternit.core.design.ThinkingOrbState
import com.allternit.feature.chat.RuntimeRelay
import com.allternit.feature.shell.ModelStore
import com.allternit.feature.shell.ShellApi
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeout

/**
 * The first-run gate after sign-in (iOS OnboardingView, the m.allternit.com phone setup in
 * allternit-ai `PhoneFirstRun.tsx`). MainActivity shows it instead of the workspace while
 * [FirstRunStore.isComplete] is false; it is never shown over the sign-in screen.
 *
 * The server's setup config decides (`user.onboardingComplete`). When that check can't answer
 * (no computer online to relay it), setup shows only for an account with no computer paired at
 * all, so an existing user whose computer is asleep is never stopped here. The check gives up
 * after 8 s and the computer list settles after 6 s, so the phone is never held on the splash.
 *
 * [onDone] gets the Settings section a checklist fix asked to open, or null.
 */
@Composable
internal fun FirstRunGate(store: FirstRunStore, accountFirstName: String?, onDone: (fix: String?) -> Unit) {
    val context = LocalContext.current
    val relay = remember { RuntimeRelay(ShellApi.okHttp, ApiConfig(), context) }
    var server by remember { mutableStateOf(SetupServerAnswer.Pending) }
    var runtimesLoaded by remember { mutableStateOf(false) }
    var runtimeCount by remember { mutableIntStateOf(0) }

    val decision = if (store.forceRun) PhoneSetupDecision.Run else decidePhoneSetup(
        completedLocally = store.completedOnDevice, usedAppBefore = store.usedAppBefore,
        server = server, runtimesLoaded = runtimesLoaded, runtimeCount = runtimeCount,
    )

    LaunchedEffect(Unit) {
        if (store.forceRun) return@LaunchedEffect
        val answer = try {
            if (withTimeout(8_000) { SetupApi.onboardingComplete() }) SetupServerAnswer.Done else SetupServerAnswer.Run
        } catch (e: kotlinx.coroutines.CancellationException) {
            // withTimeout's own cancellation is the "took too long" case; a real cancel rethrows below.
            if (e is kotlinx.coroutines.TimeoutCancellationException) SetupServerAnswer.Failed else throw e
        } catch (e: Exception) {
            SetupServerAnswer.Failed
        }
        server = answer
        if (answer == SetupServerAnswer.Done) store.complete()
    }
    LaunchedEffect(Unit) {
        coroutineScope {
            val deadline = launch { delay(6_000); runtimesLoaded = true }
            runtimeCount = relay.listDevices()?.size ?: 0
            runtimesLoaded = true
            deadline.cancel()
        }
    }
    LaunchedEffect(decision) {
        // An existing user goes straight in; the server is asked again next launch.
        if (decision == PhoneSetupDecision.Skip && !store.isComplete) store.skipThisLaunch()
        if (decision == PhoneSetupDecision.Skip) onDone(null)
    }

    Crossfade(decision == PhoneSetupDecision.Run, animationSpec = tween(250), label = "first-run") { run ->
        if (run) {
            FirstRunSetupScreen(
                relay = relay,
                accountFirstName = accountFirstName,
                onFinish = { _, data, fix ->
                    finish(context, data)
                    // Persist first: onDone swaps the root and cancels this scope (iOS marks complete before leaving).
                    store.complete()
                    // The fix is queued before the root swaps to the workspace.
                    onDone(fix)
                },
            )
        } else {
            Box(
                Modifier.fillMaxSize().background(setupColors.bg).testTag("setup-splash").semantics { contentDescription = "Loading" },
                contentAlignment = Alignment.Center,
            ) { ThinkingOrb(state = ThinkingOrbState.Connecting, size = ThinkingOrbSize.Avatar, paused = true, label = "Loading") }
        }
    }
}

/**
 * Finish (web completeFirstRun, phone): saves the setup config with the wizard-completion flag
 * (best-effort: offline must not trap the person), then makes the picked brain the composer's
 * model when the catalog has it. The no-computer path works: Allternit Cloud's
 * `allternit/<alias>` matches the cloud catalog the composer offers without a computer.
 */
private suspend fun finish(context: Context, data: SetupWizardData) = coroutineScope {
    val version = runCatching { context.packageManager.getPackageInfo(context.packageName, 0).versionName }.getOrNull() ?: "android"
    val save = async { runCatching { withTimeout(8_000) { SetupApi.saveConfig(data.defaultModel, data.infraType, version) } } }
    val wanted = data.defaultModel
    if (wanted != null) {
        val models = ModelStore(context)
        runCatching { withTimeout(10_000) { models.load(force = true) } }
        val all = models.chatModels
        val match = all.firstOrNull { it.id == wanted }
            ?: data.defaultModelId?.let { id -> all.firstOrNull { it.id == id || it.id.endsWith("/$id") } }
        if (match != null) models.select(match.id)
    }
    save.await()
}
