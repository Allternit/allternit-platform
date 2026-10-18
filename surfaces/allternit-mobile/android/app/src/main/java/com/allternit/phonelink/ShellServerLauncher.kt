package com.allternit.phonelink

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.provider.Settings
import com.allternit.phonelink.core.AdbClient
import com.allternit.phonelink.core.AdbIdentity
import com.allternit.phonelink.core.AdbKeyBox
import com.allternit.phonelink.core.SecureEnvelope
import com.allternit.phonelink.core.ShellClient
import com.allternit.phonelink.core.ShellServerProtocol
import com.allternit.phonelink.core.toHex
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * Keeps the shell server (ShellServerMain) running — the Shizuku method, owned:
 * with WRITE_SECURE_SETTINGS (granted once over the cable) the app turns on
 * wireless debugging itself, finds adbd's TLS port over NSD, connects to
 * 127.0.0.1 with the per-phone adb key Desktop paired, and launches the server
 * detached. It then puts wireless debugging back the way it was. The server
 * lives until reboot, so this runs at boot, when the phone server starts, when
 * Wi-Fi comes up while the server is down, and on demand from the stream route.
 *
 * Needs Android 13+ (https://r.android.com/2128832 lets WRITE_SECURE_SETTINGS
 * holders toggle adb_wifi_enabled) and a Wi-Fi the phone already trusts for
 * wireless debugging.
 */
class ShellServerLauncher(
    private val app: Context,
    private val keyBox: AdbKeyBox,
    private val log: (String) -> Unit,
    private val onChanged: () -> Unit,
) {
    enum class State { NOT_SET_UP, STARTING, RUNNING, WAITING_FOR_WIFI, ERROR }

    @Volatile var state: State = State.NOT_SET_UP; private set
    @Volatile var lastError: String? = null; private set

    private val port = ShellServerProtocol.DEFAULT_PORT
    val client = ShellClient(port = { port }, token = { token() })
    private val worker = Executors.newSingleThreadExecutor { r -> Thread(r, "phonelink-shell").apply { isDaemon = true } }
    private var wifiCallbackRegistered = false

    private fun token(): String = keyBox.shellToken { SecureEnvelope.randomBytes(32).toHex() }

    val isSetUp: Boolean get() = keyBox.hasKey() && hasSecureSettings() && Build.VERSION.SDK_INT >= 33

    private fun hasSecureSettings() =
        app.checkSelfPermission(Manifest.permission.WRITE_SECURE_SETTINGS) == PackageManager.PERMISSION_GRANTED

    /** Idempotent and non-blocking: makes sure the server is up, if this phone is set up for it. */
    fun ensureRunning() {
        worker.execute { ensureRunningBlocking() }
    }

    /** Blocking form for the stream route. Returns true when the server answers. */
    @Synchronized
    fun ensureRunningBlocking(): Boolean {
        if (!isSetUp) {
            set(State.NOT_SET_UP, null)
            return false
        }
        if (client.ping()) {
            set(State.RUNNING, null)
            return true
        }
        set(State.STARTING, null)
        return try {
            launch()
            val up = (1..20).any { Thread.sleep(250); client.ping() }
            if (up) set(State.RUNNING, null) else set(State.ERROR, "The shell server didn't answer after starting.")
            up
        } catch (e: NoWifi) {
            set(State.WAITING_FOR_WIFI, e.message)
            watchForWifi()
            false
        } catch (e: Exception) {
            set(State.ERROR, e.message ?: e.javaClass.simpleName)
            false
        }
    }

    /** The user's Stop: the screen service goes with the phone server. Restarts only via ensureRunning. */
    fun stop() {
        worker.execute {
            if (client.quit()) log("shell server stopped")
            if (isSetUp) set(State.ERROR, "Stopped") else set(State.NOT_SET_UP, null)
        }
    }

    private class NoWifi(message: String) : Exception(message)

    private fun launch() {
        val identity = AdbIdentity.fromPkcs8(keyBox.read() ?: throw IllegalStateException("No adb key; run Phone Link setup again."))
        val cr = app.contentResolver
        val wasOn = Settings.Global.getInt(cr, "adb_wifi_enabled", 0) == 1
        if (!wifiConnected()) throw NoWifi("Waiting for Wi-Fi to start the screen service.")
        // Keys never expire from disuse (default: 7 days), or a quiet week breaks the restart.
        Settings.Global.putLong(cr, "adb_allowed_connection_time", 0L)
        if (!wasOn) Settings.Global.putInt(cr, "adb_wifi_enabled", 1)
        try {
            val ports = discoverAdbPorts(timeoutMs = 8_000)
            if (ports.isEmpty()) throw NoWifi("Wireless debugging didn't start on this Wi-Fi. Open Wireless debugging once on this network and tap Always allow.")
            var lastFailure: Exception? = null
            for (p in ports) {
                try {
                    AdbClient(identity).use { adb ->
                        adb.connect("127.0.0.1", p)
                        val apk = app.applicationInfo.sourceDir
                        // A server left over from an older token would hold the port. Its own
                        // `sh -c` call: pkill -f would otherwise match the launch line below.
                        adb.shell("pkill -f 'phonelink[.]shell[.][S]hellServerMain'")
                        adb.shell("setsid app_process -Djava.class.path=$apk / com.allternit.phonelink.shell.ShellServerMain ${token()} $port " +
                            "</dev/null >/data/local/tmp/allternit-shell.log 2>&1 &")
                    }
                    log("shell server launched via adbd port $p")
                    return
                } catch (e: Exception) {
                    lastFailure = e
                }
            }
            throw lastFailure ?: IllegalStateException("Couldn't reach wireless debugging.")
        } finally {
            if (!wasOn) Settings.Global.putInt(cr, "adb_wifi_enabled", 0)
        }
    }

    private fun wifiConnected(): Boolean {
        val cm = app.getSystemService(ConnectivityManager::class.java)
        return cm.allNetworks.any { n -> cm.getNetworkCapabilities(n)?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) == true }
    }

    /** adbd's `_adb-tls-connect._tcp` ports seen on the LAN; ours is the one that answers on 127.0.0.1. */
    @Suppress("DEPRECATION")
    private fun discoverAdbPorts(timeoutMs: Long): List<Int> {
        val nsd = app.getSystemService(NsdManager::class.java)
        val ports = ConcurrentLinkedQueue<Int>()
        val first = CountDownLatch(1)
        val listener = object : NsdManager.DiscoveryListener {
            override fun onServiceFound(info: NsdServiceInfo) {
                nsd.resolveService(info, object : NsdManager.ResolveListener {
                    override fun onServiceResolved(resolved: NsdServiceInfo) {
                        if (resolved.port > 0) { ports.add(resolved.port); first.countDown() }
                    }
                    override fun onResolveFailed(info: NsdServiceInfo, errorCode: Int) = Unit
                })
            }
            override fun onDiscoveryStarted(serviceType: String) = Unit
            override fun onDiscoveryStopped(serviceType: String) = Unit
            override fun onServiceLost(info: NsdServiceInfo) = Unit
            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) { first.countDown() }
            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) = Unit
        }
        nsd.discoverServices("_adb-tls-connect._tcp", NsdManager.PROTOCOL_DNS_SD, listener)
        try {
            first.await(timeoutMs, TimeUnit.MILLISECONDS)
            Thread.sleep(500) // let other answers (other phones on the LAN) arrive too
        } finally {
            try { nsd.stopServiceDiscovery(listener) } catch (_: Exception) {}
        }
        return ports.distinct()
    }

    @Synchronized
    private fun watchForWifi() {
        if (wifiCallbackRegistered) return
        val cm = app.getSystemService(ConnectivityManager::class.java)
        val request = NetworkRequest.Builder().addTransportType(NetworkCapabilities.TRANSPORT_WIFI).build()
        cm.registerNetworkCallback(request, object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                if (state == State.WAITING_FOR_WIFI) ensureRunning()
            }
        })
        wifiCallbackRegistered = true
    }

    private fun set(s: State, error: String?) {
        val changed = s != state || error != lastError
        state = s
        lastError = error
        if (changed) {
            log("shell server: $s${error?.let { " ($it)" } ?: ""}")
            onChanged()
        }
    }
}
