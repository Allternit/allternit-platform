package com.allternit.ondevice

import android.app.Activity
import android.content.Context
import androidx.browser.customtabs.CustomTabsCallback
import androidx.browser.trusted.TrustedWebActivityIntentBuilder
import com.google.androidbrowserhelper.trusted.splashscreens.SplashScreenStrategy
import com.google.androidbrowserhelper.trusted.LauncherActivity
import com.google.androidbrowserhelper.trusted.TwaLauncher
import com.google.androidbrowserhelper.trusted.TwaProviderPicker

/**
 * androidbrowserhelper's LauncherActivity with one change: the TWA is launched on
 * [TwaBridge]'s app-wide session, so the on-device engine can talk to the page. Everything
 * else stays LauncherActivity's: manifest metadata (colors, display mode, share target),
 * share intents, deep links and shortcuts, the splash screen, the Chrome update prompt, the
 * Custom Tab fallback when no TWA-capable browser is installed, and notification delegation.
 * The Play build's ReviewLauncherActivity extends this.
 */
open class BridgedLauncherActivity : LauncherActivity() {
    override fun createTwaLauncher(): TwaLauncher {
        TwaBridge.appContext = applicationContext
        OnDeviceEngine.get(applicationContext).sink = TwaBridge::post
        return BridgedTwaLauncher(this)
    }
}

/**
 * TwaLauncher whose TWA path goes through [TwaBridge]. The fallback path (no TWA provider,
 * or no session) is TwaLauncher's own.
 */
internal class BridgedTwaLauncher(private val activity: Activity) : TwaLauncher(activity) {
    private val pick = TwaProviderPicker.pickProvider(activity.packageManager)

    override fun launch(
        twaBuilder: TrustedWebActivityIntentBuilder,
        customTabsCallback: CustomTabsCallback,
        splashScreenStrategy: SplashScreenStrategy?,
        completionCallback: Runnable?,
        fallbackStrategy: FallbackStrategy,
    ) {
        val provider = pick.provider
        if (pick.launchMode != TwaProviderPicker.LaunchMode.TRUSTED_WEB_ACTIVITY || provider == null) {
            super.launch(twaBuilder, customTabsCallback, splashScreenStrategy, completionCallback, fallbackStrategy)
            return
        }
        TwaBridge.launch(activity, provider, twaBuilder, customTabsCallback, splashScreenStrategy, completionCallback) {
            fallbackStrategy.launch(activity as Context, twaBuilder, provider, completionCallback)
        }
    }
}
