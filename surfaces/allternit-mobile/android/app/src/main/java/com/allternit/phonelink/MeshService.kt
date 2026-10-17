package com.allternit.phonelink

import android.app.Service
import android.content.Intent
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.os.Message
import android.os.Messenger
import android.util.Log
import com.allternit.phonelink.core.MeshController
import com.allternit.phonelink.core.MeshMessages
import com.allternit.phonelink.core.MeshReport

/**
 * Runs the gomobile tsnet node in the app's private `:mesh` process
 * (declared in the manifest with `android:process=":mesh"`). A Go panic
 * aborts its whole process; keeping the node here means it can cost only the
 * mesh, never the phone server and accessibility service in the main process.
 *
 * The service is never exported: the main process binds it
 * (BIND_AUTO_CREATE|BIND_IMPORTANT) while the phone server runs, hands the
 * start inputs in the bind Intent, and receives `{state, meshIp}` reports on
 * a Messenger — no AIDL. This process has no foreground service of its own;
 * the binding keeps its priority tied to the main process's.
 */
class MeshService : Service() {
    companion object {
        private const val TAG = "PhoneLinkMesh"
    }

    private var handlerThread: HandlerThread? = null
    private var handler: Handler? = null
    private var controller: MeshController? = null
    private var replyTo: Messenger? = null

    private val networkCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) = onNetworkChanged()
        override fun onLost(network: Network) = onNetworkChanged()
        override fun onLinkPropertiesChanged(network: Network, linkProperties: LinkProperties) = onNetworkChanged()
    }

    override fun onBind(intent: Intent): IBinder? {
        val params = MeshMessages.decodeStart(intent.getStringExtra(MeshMessages.EXTRA_PARAMS) ?: return null)
            ?: run {
                Log.e(TAG, "bind without parseable start params; refusing")
                return null
            }
        val thread = HandlerThread("phonelink-mesh-svc").apply { start() }
        handlerThread = thread
        val h = Handler(thread.looper) { msg -> handleCommand(msg) }
        handler = h
        replyTo = replyMessenger(intent)
        val ctl = MeshController(
            params = params,
            nodeFactory = { GomobileMeshNode(it) },
            onReport = { report -> deliver(report) },
            log = { Log.i(TAG, it) },
        )
        controller = ctl
        try {
            getSystemService(ConnectivityManager::class.java)
                ?.registerDefaultNetworkCallback(networkCallback, h)
        } catch (e: Exception) {
            // No network-state feed: joins still retry on their own backoff.
            Log.i(TAG, "network callback unavailable: ${e.message}")
        }
        ctl.start()
        return Messenger(h).binder
    }

    override fun onDestroy() {
        try {
            getSystemService(ConnectivityManager::class.java)?.unregisterNetworkCallback(networkCallback)
        } catch (_: Exception) {
            // Never registered (onBind refused the params) or already gone.
        }
        controller?.stop()
        controller = null
        handlerThread?.quitSafely()
        handlerThread = null
        super.onDestroy()
    }

    private fun handleCommand(msg: Message): Boolean {
        // The main process drives this service through bind/unbind alone; any
        // stray message (e.g. a recycled report) is dropped, never crashed on.
        return true
    }

    /** State changes go back to the main process over the reply Messenger it sent in the bind Intent. */
    private fun deliver(report: MeshReport) {
        val target = replyTo ?: return
        try {
            val msg = Message.obtain(null, MeshMessages.MSG_REPORT)
            // A String in msg.obj can't be marshalled across processes; a Bundle can.
            msg.data = android.os.Bundle().apply { putString(MeshMessages.KEY_REPORT, MeshMessages.encodeReport(report)) }
            target.send(msg)
        } catch (e: Exception) {
            // The main process went away (phone server stopped, app updated);
            // it will re-bind when it comes back. Nothing to do here.
            Log.i(TAG, "report undeliverable: ${e.javaClass.simpleName}")
        }
    }

    private fun onNetworkChanged() {
        handler?.post { controller?.onNetworkChanged() }
    }

    @Suppress("DEPRECATION")
    private fun replyMessenger(intent: Intent): Messenger? =
        if (Build.VERSION.SDK_INT >= 33) intent.getParcelableExtra(MeshMessages.EXTRA_REPLY, Messenger::class.java)
        else intent.getParcelableExtra(MeshMessages.EXTRA_REPLY)
}
