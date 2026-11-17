package com.allternit.phonelink

import android.app.ActivityManager
import android.content.Context
import android.os.Build
import android.util.Log
import com.allternit.phonelink.core.AppInfo
import com.allternit.phonelink.core.AppPhoneCredentials
import com.allternit.phonelink.core.AppPhoneLink
import com.allternit.phonelink.core.Consent
import com.allternit.phonelink.core.DeviceOps
import com.allternit.phonelink.core.GlobalAction
import com.allternit.phonelink.core.InputEvents
import com.allternit.phonelink.core.LinkListener
import com.allternit.phonelink.core.PairFailure
import com.allternit.phonelink.core.ProjectionReport
import com.allternit.phonelink.core.RpcException
import com.allternit.phonelink.core.ScreenHost
import com.allternit.phonelink.core.ScreenShareSession
import com.allternit.phonelink.core.Screenshot
import com.allternit.phonelink.core.SharePhase
import com.allternit.phonelink.core.UiNode
import org.json.JSONObject
import java.io.File
import java.util.concurrent.CopyOnWriteArraySet

/**
 * The screen-share lane in the app (both builds): owns the [AppPhoneLink]
 * (claim code, owner pairing, control socket), the share [session] and what
 * the link asks of the app ([ScreenHost]). No adb, no local server: the phone
 * dials out to the account's runtime through api.allternit.com.
 *
 * The control socket runs while the process lives: while the Accessibility
 * service is bound (Android keeps that process up), while a share runs (its
 * foreground service), or while the app is open. A screen_request then posts
 * the "tap to share" notification; capture itself only ever starts from the
 * person's tap on Android's own screen-share prompt (ProjectionConsentActivity).
 */
object AppLink : ScreenHost, LinkListener {
    private const val TAG = "AppLink"
    /** The cloud API (same host as the iPhone app). Never taken from a link or a code. */
    const val CLOUD_BASE_URL = "https://api.allternit.com"
    /** At most one "tap to share" notification per this many ms (the node also limits screen_request). */
    private const val REQUEST_NOTICE_GAP_MS = 10_000L

    @Volatile private var initialized = false
    private lateinit var app: Context
    private lateinit var linkImpl: AppPhoneLink
    val link: AppPhoneLink get() = linkImpl

    val session = ScreenShareSession()

    /** The connected accessibility service (taps), or null. */
    @Volatile var ops: DeviceOps? = null; private set
    @Volatile var pairCode: String? = null; private set
    @Volatile var pairing = false; private set
    @Volatile var pairError: String? = null; private set
    @Volatile var controlConnected = false; private set
    @Volatile private var lastRequestNoticeAt = 0L

    private val listeners = CopyOnWriteArraySet<() -> Unit>()

    @Synchronized
    fun init(context: Context) {
        if (initialized) return
        app = context.applicationContext
        val store = KeystoreCredentialStore(File(app.filesDir, "app_phone.bin"), KeystoreSealer("app_phone_v1"))
        val input = InputEvents(CurrentOps { ops }, geometry = { session.geometry() })
        linkImpl = AppPhoneLink(
            cloudBaseUrl = CLOUD_BASE_URL,
            http = OkHttpLink.http,
            sockets = OkHttpLink.sockets,
            store = store,
            runtime = OkHttpLink.runtime,
            host = this,
            input = input,
            deviceModel = "${Build.MANUFACTURER} ${Build.MODEL}".trim(),
            listener = this,
            // Phone Use `bt-android`: Desktop's hid.* commands, sent to the iPhone as Bluetooth keyboard/mouse reports.
            hid = BluetoothRemote.also { it.init(app) }.commands,
        )
        initialized = true
        // The node lists this phone's Bluetooth link on the iPhone's routes (`bt-android`).
        BluetoothRemote.addListener { linkImpl.stateChanged() }
    }

    val isPaired: Boolean get() = initialized && linkImpl.isPaired
    fun credentials(): AppPhoneCredentials? = if (initialized) linkImpl.credentials() else null

    // ── UI ──────────────────────────────────────────────────────────────────

    fun addListener(l: () -> Unit) { listeners.add(l) }
    fun removeListener(l: () -> Unit) { listeners.remove(l) }
    private fun notifyChanged() { listeners.forEach { try { it() } catch (_: Exception) {} } }

    fun pair(code: String, name: String?) {
        pairError = null
        pairCode = null
        pairing = true
        notifyChanged()
        linkImpl.pair(code, name)
    }

    fun cancelPairing() {
        linkImpl.cancelPairing()
        pairing = false
        pairCode = null
        notifyChanged()
    }

    fun unpair(context: Context) {
        ProjectionService.stopShare(context, "revoked")
        linkImpl.unpair()
        controlConnected = false
        notifyChanged()
    }

    /** Keep the control socket up when paired (idempotent). */
    fun ensureControl() { if (isPaired) linkImpl.startControl() }

    /** The share started, paused, resumed or ended: tell the node and the UI. */
    fun shareStateChanged() {
        if (initialized) linkImpl.stateChanged()
        notifyChanged()
    }

    fun consentDenied() {
        session.consentDenied()
        shareStateChanged()
    }

    fun consentRequested() { session.consentRequested() }

    fun onAccessibility(context: Context, service: DeviceOps, connected: Boolean) {
        init(context)
        if (connected) ops = service else if (ops === service) ops = null
        if (connected) ensureControl()
        shareStateChanged()
    }

    fun report(): ProjectionReport =
        session.report(System.currentTimeMillis(), paired = isPaired, controlConnected = controlConnected, canTap = ops != null)

    // ── ScreenHost ──────────────────────────────────────────────────────────

    override fun onScreenRequest() {
        // Already sharing: the node resumes the encoder itself.
        if (session.phase != SharePhase.OFF) return
        val now = System.currentTimeMillis()
        if (now - lastRequestNoticeAt < REQUEST_NOTICE_GAP_MS) return
        lastRequestNoticeAt = now
        ShareNotifications.showRequest(app)
    }

    override fun canTap(): Boolean = ops != null

    override fun consent(): Consent = session.consent()

    override fun keyboard(): JSONObject {
        val on = BluetoothRemote.isOn
        val s = BluetoothRemote.state()
        return JSONObject()
            .put("on", on)
            .put("paired", on && s.paired)
            .put("connected", on && s.connected)
            .apply { if (on) s.name?.trim()?.takeIf { it.isNotEmpty() }?.let { put("host", it) } }
    }

    override fun foreground(): Boolean = try {
        val info = ActivityManager.RunningAppProcessInfo()
        ActivityManager.getMyMemoryState(info)
        info.importance <= ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND
    } catch (e: Exception) { false }

    // ── LinkListener ────────────────────────────────────────────────────────

    override fun onPairCode(code: String) { pairCode = code; notifyChanged() }

    override fun onPaired(credentials: AppPhoneCredentials) {
        pairing = false
        pairCode = null
        pairError = null
        linkImpl.startControl()
        notifyChanged()
    }

    override fun onPairFailed(reason: PairFailure, message: String) {
        pairing = false
        pairCode = null
        pairError = message
        notifyChanged()
    }

    override fun onControlConnected(connected: Boolean) {
        controlConnected = connected
        Log.i(TAG, "control ${if (connected) "connected" else "disconnected"}")
        notifyChanged()
    }

    override fun onUnpaired() {
        controlConnected = false
        ProjectionService.stopShare(app, "revoked")
        notifyChanged()
    }
}

/**
 * The accessibility service as InputEvents sees it: whichever one is connected
 * now. Without one, every call fails as `accessibility_off` (InputEvents counts
 * those events as ignored).
 */
private class CurrentOps(private val current: () -> DeviceOps?) : DeviceOps {
    private fun o(): DeviceOps = current() ?: throw RpcException.typed(
        "accessibility_off", "Taps are off on this phone. Turn on Allternit in Accessibility settings.")
    override fun isScreenOn() = o().isScreenOn()
    override fun isScreenLocked() = o().isScreenLocked()
    override fun foregroundPackage() = o().foregroundPackage()
    override fun snapshot(): List<UiNode> = o().snapshot()
    override fun tap(x: Float, y: Float) = o().tap(x, y)
    override fun doubleTap(x: Float, y: Float) = o().doubleTap(x, y)
    override fun press(x: Float, y: Float, durationMs: Long) = o().press(x, y, durationMs)
    override fun swipe(x1: Float, y1: Float, x2: Float, y2: Float, durationMs: Long, holdStartMs: Long) =
        o().swipe(x1, y1, x2, y2, durationMs, holdStartMs)
    override fun click(node: UiNode) = o().click(node)
    override fun scrollNode(node: UiNode, forward: Boolean) = o().scrollNode(node, forward)
    override fun focusedInput() = o().focusedInput()
    override fun setText(node: UiNode, text: String) = o().setText(node, text)
    override fun paste(node: UiNode, text: String) = o().paste(node, text)
    override fun imeEnter(node: UiNode) = o().imeEnter(node)
    override fun globalAction(action: GlobalAction) = o().globalAction(action)
    override fun isInstalled(packageName: String) = o().isInstalled(packageName)
    override fun launchApp(packageName: String) = o().launchApp(packageName)
    override fun terminateApp(packageName: String) = o().terminateApp(packageName)
    override fun listApps(): List<AppInfo> = o().listApps()
    override fun resolveUrl(url: String) = o().resolveUrl(url)
    override fun openUrl(url: String) = o().openUrl(url)
    override fun screenshot(format: String, quality: Float, scale: Float): Screenshot = o().screenshot(format, quality, scale)
    override fun waitForIdle(quietMs: Long, timeoutMs: Long) = o().waitForIdle(quietMs, timeoutMs)
    override fun systemMetrics(): JSONObject = o().systemMetrics()
}
