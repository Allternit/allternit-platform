package com.allternit.feature.push

import android.Manifest
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import com.allternit.core.api.gen.AllternitApi
import com.allternit.core.api.ApiClients
import com.allternit.core.api.ApiConfig
import com.allternit.core.api.ApiResult
import com.allternit.core.api.gen.RegisterDeviceTokenRequest
import com.allternit.core.api.gen.UnregisterDeviceTokenRequest
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.auth.AuthState
import com.google.firebase.FirebaseApp
import com.google.firebase.messaging.FirebaseMessaging
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/**
 * Native push (FCM) for the Android app, the port of iOS `Core/PushNotificationManager` + `PushTokenClient`.
 *
 * - **Permission** is asked at a moment with context, never on first launch: the "Meet your bots" notifications
 *   step, or Settings > Notifications through the priming sheet (`AppPermission.Notifications`).
 * - **Registration**: once allowed, every launch and every sign-in fetches the FCM token and posts it to cloud-api
 *   `POST /api/v1/device-tokens` with `platform: "android"` (routes/apns.rs `register_route`). A token refresh
 *   ([AllternitMessagingService.onNewToken]) posts it again. Sign-out removes it (the [AllternitAuth] sign-out hook).
 * - **Delivery**: cloud-api sends every push it sends to the web and to iPhones to FCM too (routes/fcm.rs), as a
 *   data message; [PushNotifications] builds the notification (channel and buttons from the iOS category).
 * - **Tap**: [tapIntent] opens MainActivity with the push's `url`, `event`, `inboxId`, `sessionId` and `botId`;
 *   MainActivity reads them back with [readTap] and opens the inbox item, Decisions, Settings or Bots.
 *
 * Firebase is initialised by FirebaseInitProvider from string resources the nativeapp build generates from the local
 * client config (app/build.gradle.kts). Without that config [isConfigured] is false and push stays off: no crash,
 * no token, nothing sent.
 */
object AllternitPush {
    const val EXTRA_URL = "com.allternit.push.url"
    const val EXTRA_EVENT = "com.allternit.push.event"
    const val EXTRA_INBOX_ID = "com.allternit.push.inboxId"
    const val EXTRA_SESSION_ID = "com.allternit.push.sessionId"
    const val EXTRA_BOT_ID = "com.allternit.push.botId"
    /** A `computer_alert` push: the console row id. */
    const val EXTRA_COMPUTER_ID = "com.allternit.push.computerId"

    /** What this build does with a push (cloud `KNOWN_CAPABILITIES`): notification buttons. */
    val CAPABILITIES = listOf("actions")

    private const val TAG = "AllternitPush"
    private const val PREFS = "allternit_push"
    private const val KEY_TOKEN = "fcm-token"
    private const val KEY_REGISTERED = "registered"
    private const val KEY_DEVICE_ID = "device-id"
    private const val KEY_ERROR = "registration-error"

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val started = AtomicBoolean(false)
    private val api by lazy { AllternitApi(ApiClients.okHttp(bearer = AllternitAuth.tokenProvider), ApiConfig()) }

    /** MainActivity.onCreate: channels, the sign-out hook, and a registration after every sign-in. */
    fun start(context: Context) {
        val app = context.applicationContext
        PushNotifications.ensureChannels(app)
        if (!started.compareAndSet(false, true)) return
        AllternitAuth.addSignOutHook { willSignOut(app) }
        BarJobNotification.start(app)
        scope.launch {
            // Sign-in and account switch: post the token for the account now active (cloud moves it).
            AllternitAuth.state.collect { state -> if (state is AuthState.SignedIn) registerIfAuthorized(app) }
        }
    }

    /** True when the build carries a Firebase client config (FirebaseInitProvider made the default app). */
    fun isConfigured(context: Context): Boolean = FirebaseApp.getApps(context.applicationContext).isNotEmpty()

    /** This phone lets Allternit post notifications (Android 13+ permission and the app switch). */
    fun notificationsAllowed(context: Context): Boolean {
        val enabled = context.getSystemService(NotificationManager::class.java)?.areNotificationsEnabled() == true
        val granted = Build.VERSION.SDK_INT < 33 ||
            context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
        return enabled && granted
    }

    /** Last registration failure (Settings > Notifications), cleared on success. */
    fun lastRegistrationError(context: Context): String? = prefs(context).getString(KEY_ERROR, null)

    /** Re-registers without prompting when notifications are allowed: launch, sign-in, after the permission. */
    fun registerIfAuthorized(context: Context) {
        val app = context.applicationContext
        if (!notificationsAllowed(app) || !isConfigured(app)) return
        runCatching {
            FirebaseMessaging.getInstance().token.addOnCompleteListener { task ->
                val token = if (task.isSuccessful) task.result else null
                if (token.isNullOrBlank()) {
                    prefs(app).edit().putString(KEY_ERROR, task.exception?.message ?: "No push token").apply()
                } else {
                    onNewToken(app, token)
                }
            }
        }.onFailure { prefs(app).edit().putString(KEY_ERROR, it.message ?: "Push isn't available").apply() }
    }

    /** A new or refreshed FCM token: keep it and post it (signed out: sign-in posts it). */
    fun onNewToken(context: Context, token: String) {
        val app = context.applicationContext
        prefs(app).edit().putString(KEY_TOKEN, token).apply()
        scope.launch { sendTokenToServer(app, token) }
    }

    private suspend fun sendTokenToServer(context: Context, token: String) {
        if (AllternitAuth.state.value !is AuthState.SignedIn) return
        val body = RegisterDeviceTokenRequest(
            token = token,
            platform = "android",
            environment = "production",
            deviceId = deviceId(context),
            osVersion = Build.VERSION.RELEASE,
            appBuild = appBuild(context),
            capabilities = CAPABILITIES,
        )
        when (val result = api.registerDeviceToken(body)) {
            is ApiResult.Ok -> {
                prefs(context).edit().putBoolean(KEY_REGISTERED, true).remove(KEY_ERROR).apply()
                // Gap run GS-B: the read token for private notification bodies, Keystore-sealed (never prefs or logs).
                PrivateBodies.storeReadToken(context, result.value.readToken)
            }
            is ApiResult.HttpError -> prefs(context).edit().putString(KEY_ERROR, result.code ?: "HTTP ${result.status}").apply()
            else -> prefs(context).edit().putString(KEY_ERROR, "Couldn't reach Allternit").apply()
        }
        if (BuildConfig.DEBUG) Log.i(TAG, "device token registration: ${lastRegistrationError(context) ?: "ok"}")
    }

    /** Sign-out (runs before the session is dropped, so the DELETE still has the leaving account's bearer). */
    private suspend fun willSignOut(context: Context) {
        context.getSystemService(NotificationManager::class.java)?.cancelAll()
        // Wave I2: the widget, tiles and shortcuts drop the leaving account's snapshot.
        com.allternit.feature.system.StatusSync.clear(context)
        PrivateBodies.storeReadToken(context, null)
        val p = prefs(context)
        val token = p.getString(KEY_TOKEN, null)
        if (!p.getBoolean(KEY_REGISTERED, false) || token.isNullOrBlank()) return
        api.unregisterDeviceToken(UnregisterDeviceTokenRequest(token))
        p.edit().putBoolean(KEY_REGISTERED, false).apply()
    }

    /** MainActivity with the push's link, from a tap or the Open / Review button. Null if there is no launcher. */
    fun tapIntent(context: Context, data: Map<String, String>): Intent? =
        context.packageManager.getLaunchIntentForPackage(context.packageName)?.apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
            data["url"]?.let { putExtra(EXTRA_URL, it) }
            data["event"]?.let { putExtra(EXTRA_EVENT, it) }
            data["inboxId"]?.let { putExtra(EXTRA_INBOX_ID, it) }
            (data["sessionId"] ?: data["threadSessionId"])?.let { putExtra(EXTRA_SESSION_ID, it) }
            data["botId"]?.let { putExtra(EXTRA_BOT_ID, it) }
            data["computerId"]?.let { putExtra(EXTRA_COMPUTER_ID, it) }
        }

    /** The push a notification tap carried, once (the extras are removed). Null for any other intent. */
    fun readTap(intent: Intent): PushTap? {
        val keys = listOf(EXTRA_URL, EXTRA_EVENT, EXTRA_INBOX_ID, EXTRA_SESSION_ID, EXTRA_BOT_ID, EXTRA_COMPUTER_ID)
        if (keys.none { intent.hasExtra(it) }) return null
        val tap = PushTap(
            url = intent.getStringExtra(EXTRA_URL),
            event = intent.getStringExtra(EXTRA_EVENT),
            inboxId = intent.getStringExtra(EXTRA_INBOX_ID)?.takeIf { it.isNotBlank() },
            sessionId = intent.getStringExtra(EXTRA_SESSION_ID)?.takeIf { it.isNotBlank() },
            botId = intent.getStringExtra(EXTRA_BOT_ID)?.takeIf { it.isNotBlank() },
            computerId = intent.getStringExtra(EXTRA_COMPUTER_ID)?.takeIf { it.isNotBlank() },
        )
        keys.forEach { intent.removeExtra(it) }
        return tap
    }

    private fun deviceId(context: Context): String {
        val p = prefs(context)
        return p.getString(KEY_DEVICE_ID, null) ?: UUID.randomUUID().toString().also { p.edit().putString(KEY_DEVICE_ID, it).apply() }
    }

    private fun appBuild(context: Context): String = runCatching {
        val info = context.packageManager.getPackageInfo(context.packageName, 0)
        if (Build.VERSION.SDK_INT >= 28) info.longVersionCode.toString() else @Suppress("DEPRECATION") info.versionCode.toString()
    }.getOrDefault("")

    private fun prefs(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
}

/** What a tapped notification points at (iOS PushNotificationManager.handleTap arguments). */
data class PushTap(
    val url: String?,
    val event: String?,
    val inboxId: String?,
    val sessionId: String?,
    val botId: String?,
    val computerId: String? = null,
) {
    /** The computer a `computer_alert` points at: `computerId`, else the url `/computers/<id>` (iOS DeepLink.computerConsole). */
    val computer: String?
        get() = computerId ?: url?.let { Regex("""^/computers/([^/?#]+)""").find(it)?.groupValues?.get(1) }

    /** The run an `aci.attention` push names: the `run` query of `/computers/<id>?run=<run_id>`. Null for other pushes. */
    val run: String?
        get() = url?.let { runCatching { android.net.Uri.parse("https://app$it").getQueryParameter("run") }.getOrNull() }?.takeIf { it.isNotBlank() }

    /** The inbox item: `inboxId`, else the `allternit_inbox` query of the url (`/?allternit_inbox=approval%3Aap1`). */
    val inboxItem: String?
        get() = inboxId ?: url?.let { runCatching { android.net.Uri.parse("https://app$it").getQueryParameter("allternit_inbox") }.getOrNull() }
            ?.takeIf { it.isNotBlank() }
}
