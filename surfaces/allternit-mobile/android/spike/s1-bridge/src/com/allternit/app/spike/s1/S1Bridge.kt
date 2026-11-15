package com.allternit.app.spike.s1

import android.content.ComponentName
import android.content.Context
import android.net.Uri
import android.os.Bundle
import android.os.SystemClock
import android.util.Log
import androidx.browser.customtabs.CustomTabsCallback
import androidx.browser.customtabs.CustomTabsClient
import androidx.browser.customtabs.CustomTabsService
import androidx.browser.customtabs.CustomTabsServiceConnection
import androidx.browser.customtabs.CustomTabsSession
import androidx.browser.trusted.TrustedWebActivityIntentBuilder
import org.json.JSONObject
import java.util.concurrent.Executors

/**
 * SPIKE S1: candidate 1, the TWA postMessage channel.
 *
 * Holds ONE CustomTabsSession for the whole process (bound with the application
 * context, so it outlives the launcher activity, which finishes right after the
 * launch). androidbrowserhelper's LauncherActivity cannot be used for this: its
 * TwaLauncher keeps the session private and unbinds it in onDestroy, which runs
 * as soon as the TWA is on screen.
 *
 * Flow (Chrome 115+, androidx.browser 1.6+):
 *   bind -> warmup(0) -> newSession(callback) -> launch TWA with that session
 *   -> NAVIGATION_FINISHED -> requestPostMessageChannel(origin, origin)
 *   -> Chrome verifies use_as_origin (DAL, cached result offline, or the
 *      --disable-digital-asset-link-verification-for-url dev switch)
 *   -> Chrome posts an empty message carrying a MessagePort to the page
 *   -> onMessageChannelReady -> postMessage() / onPostMessage().
 * Every cross-document navigation (reload included) closes the channel in
 * Chrome; the next NAVIGATION_FINISHED requests a new one.
 *
 * Wire protocol: JSON strings. Page -> app: hello, ping, stream, result.
 * App -> page: ready, welcome, pong, tok, end.
 * Handshake: the page sends hello on the port and repeats it every 250 ms
 * until welcome; the app also sends ready on onMessageChannelReady, which
 * makes the page send hello at once. Page messages sent before that callback
 * can be lost (Chrome binds the app's PostMessageService after it hands the
 * page the port).
 */
object S1Bridge {
    const val TAG = "S1Bridge"

    private val worker = Executors.newSingleThreadExecutor { r -> Thread(r, "s1-bridge") }

    @Volatile private var client: CustomTabsClient? = null
    @Volatile private var session: CustomTabsSession? = null
    @Volatile private var connection: CustomTabsServiceConnection? = null
    @Volatile private var origin: Uri? = null
    @Volatile private var autoRequest = true
    @Volatile var channelsReady = 0; private set
    @Volatile var navigationsFinished = 0; private set
    @Volatile var lastValidation: String = "none"; private set
    @Volatile var providerPackage: String? = null; private set

    /** Binds (once) and launches [url] as a TWA on our own session. */
    fun launch(context: Context, url: Uri, autoRequestChannel: Boolean) {
        val app = context.applicationContext
        origin = Uri.parse("${url.scheme}://${url.authority}")
        autoRequest = autoRequestChannel
        // A new session per launch: re-launching on an old session only brings
        // that session's tab to the front, without loading the new URL.
        client?.let { c ->
            val s = c.newSession(callback)
            Log.i(TAG, "new session on bound client (${s != null}), launching $url")
            session = s
            if (s != null) openTwa(context, url, s)
            return
        }
        val pkg = CustomTabsClient.getPackageName(app, listOf("com.android.chrome"), false)
            ?: CustomTabsClient.getPackageName(app, null)
        providerPackage = pkg
        Log.i(TAG, "binding CustomTabsService of $pkg for $url (origin $origin)")
        if (pkg == null) {
            Log.e(TAG, "FAIL: no Custom Tabs provider installed")
            return
        }
        val conn = object : CustomTabsServiceConnection() {
            override fun onCustomTabsServiceConnected(name: ComponentName, client: CustomTabsClient) {
                // validateRelationship / requestPostMessageChannel require warmup.
                val warm = client.warmup(0L)
                this@S1Bridge.client = client
                val s = client.newSession(callback)
                Log.i(TAG, "service connected, warmup=$warm session=${s != null}")
                session = s
                if (s != null) openTwa(app, url, s)
            }

            override fun onServiceDisconnected(name: ComponentName?) {
                Log.w(TAG, "service disconnected")
                session = null
                client = null
                connection = null
            }
        }
        connection = conn
        val bound = CustomTabsClient.bindCustomTabsService(app, pkg, conn)
        Log.i(TAG, "bindCustomTabsService=$bound")
    }

    private fun openTwa(context: Context, url: Uri, s: CustomTabsSession) {
        val intent = TrustedWebActivityIntentBuilder(url).build(s)
        if (context !is android.app.Activity) {
            intent.intent.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        intent.launchTrustedWebActivity(context)
        Log.i(TAG, "TWA launched: $url")
    }

    /** adb trigger (mode B: real origin + console-injected client). */
    fun requestChannel(reason: String): Boolean {
        val s = session ?: run { Log.w(TAG, "requestChannel($reason): no session"); return false }
        val o = origin ?: return false
        val ok = s.requestPostMessageChannel(o, o, Bundle())
        Log.i(TAG, "requestPostMessageChannel($o) reason=$reason -> $ok")
        return ok
    }

    fun status(): String =
        "session=${session != null} provider=$providerPackage origin=$origin " +
            "navFinished=$navigationsFinished channelsReady=$channelsReady validation=$lastValidation"

    private val callback = object : CustomTabsCallback() {
        override fun onNavigationEvent(navigationEvent: Int, extras: Bundle?) {
            if (navigationEvent != NAVIGATION_FINISHED) return
            navigationsFinished++
            Log.i(TAG, "NAVIGATION_FINISHED #$navigationsFinished")
            if (autoRequest) requestChannel("navigation#$navigationsFinished")
        }

        override fun onRelationshipValidationResult(
            relation: Int, requestedOrigin: Uri, result: Boolean, extras: Bundle?,
        ) {
            lastValidation = "relation=$relation origin=$requestedOrigin result=$result"
            Log.i(TAG, "validation $lastValidation")
        }

        override fun onMessageChannelReady(extras: Bundle?) {
            channelsReady++
            Log.i(TAG, "CHANNEL_READY #$channelsReady")
            // Chrome gives the page its port BEFORE its binding to our
            // PostMessageService is connected; a page message sent in that gap
            // is dropped (seen on the Moto after a reload). This callback comes
            // through that binding, so page -> app works from here on: tell the
            // page to (re)send hello. The page also retries hello until welcome.
            val n = channelsReady
            worker.execute { post("""{"t":"ready","chan":$n}""") }
        }

        override fun onPostMessage(message: String, extras: Bundle?) {
            worker.execute { handle(message) }
        }
    }

    private fun handle(raw: String) {
        val msg = try { JSONObject(raw) } catch (e: Exception) {
            Log.w(TAG, "non-JSON message ignored (${raw.take(60)})"); return
        }
        when (msg.optString("t")) {
            "hello" -> {
                Log.i(TAG, "HELLO run=${msg.optInt("run")} href=${msg.optString("href")} online=${msg.optBoolean("online")}")
                post(JSONObject().put("t", "welcome").put("chan", channelsReady).put("nav", navigationsFinished).toString())
            }
            "ping" -> post(JSONObject().put("t", "pong").put("id", msg.optInt("id")).toString())
            "stream" -> stream(msg.optInt("s"), msg.optInt("n", 500), msg.optInt("bytes", 16))
            "result" -> Log.i(TAG, "RESULT $raw")
            else -> Log.w(TAG, "unknown message type: ${msg.optString("t")}")
        }
    }

    /** Pushes [n] token-like messages as fast as Chrome accepts them. */
    private fun stream(streamId: Int, n: Int, bytes: Int) {
        val pad = "x".repeat(bytes.coerceIn(0, 65536))
        var fail = 0
        var firstFailCode = 0
        val t0 = SystemClock.elapsedRealtimeNanos()
        for (i in 0 until n) {
            val code = post("""{"t":"tok","s":$streamId,"i":$i,"p":"$pad"}""")
            if (code != CustomTabsService.RESULT_SUCCESS) {
                fail++
                if (firstFailCode == 0) firstFailCode = code
            }
        }
        val postMs = (SystemClock.elapsedRealtimeNanos() - t0) / 1e6
        post(JSONObject().put("t", "end").put("s", streamId).put("n", n).put("fail", fail)
            .put("failCode", firstFailCode).put("postMs", postMs).toString())
        Log.i(TAG, "STREAM s=$streamId n=$n bytes=$bytes fail=$fail firstFailCode=$firstFailCode nativePostMs=%.1f".format(postMs))
    }

    private fun post(m: String): Int =
        session?.postMessage(m, null) ?: CustomTabsService.RESULT_FAILURE_MESSAGING_ERROR
}
