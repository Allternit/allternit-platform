package com.allternit.ondevice

import android.app.Activity
import android.content.ComponentName
import android.content.Context
import android.net.Uri
import android.os.Bundle
import android.util.Log
import androidx.browser.customtabs.CustomTabsCallback
import androidx.browser.customtabs.CustomTabsClient
import androidx.browser.customtabs.CustomTabsService
import androidx.browser.customtabs.CustomTabsServiceConnection
import androidx.browser.customtabs.CustomTabsSession
import androidx.browser.trusted.Token
import androidx.browser.trusted.TrustedWebActivityIntentBuilder
import androidx.browser.trusted.splashscreens.SplashScreenStrategy
import com.google.androidbrowserhelper.trusted.FocusActivity
import com.google.androidbrowserhelper.trusted.SharedPreferencesTokenStore
import org.json.JSONObject
import java.util.concurrent.Executors

/**
 * The app-wide holder of the TWA's CustomTabsSession and the postMessage channel to the
 * page (spike S1 verdict, spec/on-device/android-bridge-spike.md):
 *
 *  - bound once with the application context, so it outlives the launcher activity
 *    (androidbrowserhelper's TwaLauncher keeps its session private and unbinds in onDestroy);
 *  - a new session for every launch (relaunching on an old session only brings its old tab
 *    to the front);
 *  - requestPostMessageChannel(origin, origin) on every NAVIGATION_FINISHED, because Chrome
 *    closes the channel on each cross-document navigation, reloads included;
 *  - `ready` from onMessageChannelReady; the page's `hello` / our `welcome` are the only
 *    "connected" signals (onMessageChannelReady also fires after a failed validation);
 *  - the page sees event.origin == "android-app://<host>" and checks exactly that.
 *
 * Chrome needs `delegate_permission/common.use_as_origin` for this app in the site's
 * assetlinks.json; without it validation fails and the page gets no port (the PWA then has
 * no bridge and falls back to WebGPU).
 */
object TwaBridge {
    private const val TAG = "TwaBridge"

    private val worker = Executors.newSingleThreadExecutor { r -> Thread(r, "twa-bridge") }

    @Volatile private var client: CustomTabsClient? = null
    @Volatile private var connection: CustomTabsServiceConnection? = null
    @Volatile private var boundPackage: String? = null
    @Volatile private var session: CustomTabsSession? = null
    @Volatile private var origin: Uri? = null
    @Volatile var connected = false
        private set
    @Volatile var lastValidation: String = "none"
        private set

    private var pendingLaunch: (() -> Unit)? = null

    /**
     * Launches [builder] as a TWA on a fresh session of our app-wide connection. Mirrors
     * TwaLauncher's TWA path (splash screen hooks, FocusActivity, the notification-delegation
     * token) so LauncherActivity behaves exactly as before. [onFailed] runs the caller's
     * fallback (Custom Tab) when no session can be made.
     */
    fun launch(
        activity: Activity,
        providerPackage: String,
        builder: TrustedWebActivityIntentBuilder,
        qualityCallback: CustomTabsCallback,
        splash: SplashScreenStrategy?,
        onLaunched: Runnable?,
        onFailed: () -> Unit,
    ) {
        val app = activity.applicationContext
        val url = builder.uri
        origin = Uri.parse("${url.scheme}://${url.authority}")
        connected = false
        splash?.onTwaLaunchInitiated(providerPackage, builder)
        val go = go@{
            val c = client
            val s = try { c?.newSession(callbackFor(qualityCallback)) } catch (e: RuntimeException) { Log.w(TAG, e); null }
            if (s == null) { onFailed(); return@go }
            session = s
            val open = {
                val intent = builder.build(s)
                FocusActivity.addToIntent(intent.intent, activity)
                intent.launchTrustedWebActivity(activity)
                onLaunched?.run()
                Log.i(TAG, "TWA launched on our session: $url")
            }
            if (splash != null) splash.configureTwaBuilder(builder, s, open) else open()
        }
        // Notification delegation: remember who may post notifications for us (TwaLauncher does this).
        SharedPreferencesTokenStore(app).store(Token.create(providerPackage, app.packageManager))
        if (client != null && boundPackage == providerPackage) { go(); return }
        pendingLaunch = go
        val conn = object : CustomTabsServiceConnection() {
            override fun onCustomTabsServiceConnected(name: ComponentName, c: CustomTabsClient) {
                // validateRelationship / requestPostMessageChannel need warmup.
                c.warmup(0L)
                client = c
                pendingLaunch?.invoke()
                pendingLaunch = null
            }

            override fun onServiceDisconnected(name: ComponentName?) {
                Log.w(TAG, "Custom Tabs service disconnected")
                client = null
                session = null
                connection = null
                boundPackage = null
                connected = false
            }
        }
        connection?.let { runCatching { app.unbindService(it) } }
        connection = conn
        boundPackage = providerPackage
        if (!CustomTabsClient.bindCustomTabsServicePreservePriority(app, providerPackage, conn)) {
            pendingLaunch = null
            connection = null
            boundPackage = null
            onFailed()
        }
    }

    /** Posts one message to the page. False when Chrome refused it (not in front, no channel). */
    fun post(message: String): Boolean {
        val s = session ?: return false
        return try {
            s.postMessage(message, null) == CustomTabsService.RESULT_SUCCESS
        } catch (e: RuntimeException) {
            Log.w(TAG, "postMessage failed", e)
            false
        }
    }

    fun status(): String = "session=${session != null} origin=$origin connected=$connected validation=$lastValidation"

    private fun requestChannel(reason: String) {
        val s = session ?: return
        val o = origin ?: return
        val ok = s.requestPostMessageChannel(o, o, Bundle())
        Log.i(TAG, "requestPostMessageChannel($o) $reason -> $ok")
    }

    private fun callbackFor(quality: CustomTabsCallback) = object : CustomTabsCallback() {
        override fun onNavigationEvent(navigationEvent: Int, extras: Bundle?) {
            quality.onNavigationEvent(navigationEvent, extras)
            if (navigationEvent == NAVIGATION_FINISHED) {
                connected = false
                requestChannel("navigation")
            }
        }

        override fun extraCallback(callbackName: String, args: Bundle?) = quality.extraCallback(callbackName, args)

        override fun extraCallbackWithResult(callbackName: String, args: Bundle?): Bundle? =
            quality.extraCallbackWithResult(callbackName, args)

        override fun onRelationshipValidationResult(relation: Int, requestedOrigin: Uri, result: Boolean, extras: Bundle?) {
            lastValidation = "relation=$relation origin=$requestedOrigin result=$result"
            Log.i(TAG, "validation $lastValidation")
            quality.onRelationshipValidationResult(relation, requestedOrigin, result, extras)
        }

        override fun onMessageChannelReady(extras: Bundle?) {
            // The app's side is connected now; page -> app messages arrive from here on.
            worker.execute { post(Protocol.msg("ready").toString()) }
        }

        override fun onPostMessage(message: String, extras: Bundle?) {
            worker.execute { BridgeRouter.handle(message) }
        }
    }

    internal fun markConnected() {
        connected = true
    }

    /** The context the router needs (set by the launcher on first use). */
    @Volatile internal var appContext: Context? = null
}
