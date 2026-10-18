package com.allternit.phonelink

import android.content.Context
import android.os.Build
import android.util.DisplayMetrics
import android.util.Log
import android.view.WindowManager
import com.allternit.phonelink.core.AdbKeyBox
import com.allternit.phonelink.core.AppSettings
import com.allternit.phonelink.core.AuthManager
import com.allternit.phonelink.core.AutoStart
import com.allternit.phonelink.core.DeviceOps
import com.allternit.phonelink.core.EncryptedFileSessionStore
import com.allternit.phonelink.core.HttpServer
import com.allternit.phonelink.core.MeshKeyBox
import com.allternit.phonelink.core.PhoneServer
import com.allternit.phonelink.core.RpcDispatcher
import com.allternit.phonelink.core.ServerConfig
import com.allternit.phonelink.core.ServerHooks
import com.allternit.phonelink.core.StaticDeviceInfo
import com.allternit.phonelink.core.StreamOpener
import java.io.File
import java.net.Inet4Address
import java.net.InetAddress
import java.net.NetworkInterface
import java.util.UUID
import java.util.concurrent.CopyOnWriteArraySet
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * Process-wide owner of the HTTP server. Both the foreground service and the
 * accessibility service start it, so it comes back after a reboot even if one
 * of the two start paths is refused by the OS.
 */
object ServerController {
    private const val TAG = "PhoneLink"

    @Volatile private var initialized = false
    private lateinit var app: Context
    lateinit var settings: AppSettings; private set
    lateinit var auth: AuthManager; private set
    lateinit var pairing: PairingNotifier; private set
    private lateinit var dispatcher: RpcDispatcher
    private lateinit var http: HttpServer

    @Volatile var ops: DeviceOps? = null; private set
    @Volatile var lastError: String? = null; private set
    private lateinit var meshKeyBox: MeshKeyBox
    private lateinit var meshManager: MeshManager
    private lateinit var adbKeyBox: AdbKeyBox
    lateinit var shell: ShellServerLauncher; private set
    private val listeners = CopyOnWriteArraySet<() -> Unit>()
    private val scheduler = Executors.newSingleThreadScheduledExecutor { r -> Thread(r, "phonelink-ctl").apply { isDaemon = true } }

    fun log(msg: String) { Log.i(TAG, msg) }

    @Synchronized
    fun init(context: Context) {
        if (initialized) return
        app = context.applicationContext
        val kv = PrefsStore(app.getSharedPreferences("phonelink", Context.MODE_PRIVATE))
        settings = AppSettings(kv, defaultName = { "${Build.MANUFACTURER} ${Build.MODEL}".trim() }, newId = { UUID.randomUUID().toString() })
        val store = EncryptedFileSessionStore(File(app.filesDir, "sessions.bin"), KeystoreSealer(), ::log)
        auth = AuthManager(store, consumeArmedGrant = { local -> settings.consumeArmedGrant(local, System.currentTimeMillis()) })
        pairing = PairingNotifier(app)
        auth.approver = pairing
        auth.onSessionsChanged = { notifyChanged() }
        dispatcher = RpcDispatcher(
            ops = { ops },
            info = { staticInfo() },
            auth = auth,
            hooks = hooks,
            ownPackage = app.packageName,
        )
        meshKeyBox = MeshKeyBox(kv, KeystoreSealer())
        adbKeyBox = AdbKeyBox(kv, KeystoreSealer())
        shell = ShellServerLauncher(app, adbKeyBox, log = ::log, onChanged = { notifyChanged() })
        val streams = StreamOpener { command ->
            if (!shell.ensureRunningBlocking()) throw java.io.IOException(shell.lastError ?: "The screen service isn't set up on this phone.")
            shell.client.openRaw(command)
        }
        val phoneServer = PhoneServer(auth, dispatcher, config = { meshAwareConfig() }, log = ::log, streams = streams)
        http = HttpServer(phoneServer::handle, log = ::log)
        meshManager = MeshManager(
            app = app,
            meshDir = File(app.filesDir, "mesh"),
            keyBox = meshKeyBox,
            serverPort = { settings.port },
            meshControlUrl = { settings.meshControlUrl },
            deviceId = { settings.deviceId },
            handler = phoneServer::handle,
            onChanged = { notifyChanged() },
            schedule = { d, t -> scheduler.schedule(t, d, TimeUnit.MILLISECONDS) },
            log = ::log,
        )
        initialized = true
    }

    /** ServerConfig plus the live mesh facts, for /whoami and device_info. */
    private fun meshAwareConfig(): ServerConfig =
        settings.serverConfig().copy(meshState = meshManager.state(), meshHost = meshManager.host(), screenState = shell.state.name.lowercase())

    // ── State ───────────────────────────────────────────────────────────────

    val isRunning: Boolean get() = initialized && http.isRunning

    fun addListener(l: () -> Unit) { listeners.add(l) }
    fun removeListener(l: () -> Unit) { listeners.remove(l) }
    fun notifyChanged() {
        listeners.forEach { try { it() } catch (_: Exception) {} }
        if (initialized && http.isRunning) PhoneLinkService.refreshNotification(app)
    }

    fun onAccessibilityConnected(service: DeviceOps, context: Context) {
        init(context)
        ops = service
        log("accessibility connected")
        if (AutoStart.shouldStart(settings)) {
            startServer()
            PhoneLinkService.start(context)
        }
        notifyChanged()
    }

    fun onAccessibilityDisconnected(service: DeviceOps) {
        if (ops === service) ops = null
        log("accessibility disconnected")
        notifyChanged()
    }

    // ── Server lifecycle ────────────────────────────────────────────────────

    /** Socket work off the caller's thread (receivers and services call in on the main thread). */
    private val net = Executors.newSingleThreadExecutor { r -> Thread(r, "phonelink-net").apply { isDaemon = true } }

    @Synchronized
    fun startServer(): Boolean {
        if (http.isRunning) return true
        val bind: InetAddress? = if (settings.allowRemote) null else InetAddress.getByAddress(byteArrayOf(127, 0, 0, 1))
        val port = settings.port
        val ok = try {
            net.submit<Unit> { http.start(bind, port) }.get(5, TimeUnit.SECONDS)
            lastError = null
            true
        } catch (e: Exception) {
            val cause = (e as? java.util.concurrent.ExecutionException)?.cause ?: e
            lastError = "Could not listen on port $port: ${cause.message}"
            log(lastError!!)
            false
        }
        if (ok) {
            meshManager.start()
            shell.ensureRunning()
        }
        notifyChanged()
        return ok
    }

    /** Seal and persist a mesh pre-auth key from a PROVISION broadcast. Never logged. */
    fun storeMeshAuthKey(authKey: String) {
        init(app)
        meshKeyBox.store(authKey)
    }

    /** Seal and persist the per-phone adb key from a PROVISION broadcast, then start the shell server. Never logged. */
    fun storeAdbKey(pkcs8: ByteArray) {
        init(app)
        adbKeyBox.store(pkcs8)
        shell.ensureRunning()
    }

    /** (Re)start the mesh join if provisioned with a mesh control URL. Idempotent. */
    fun syncMesh() {
        if (initialized) meshManager.start()
    }

    fun meshState(): String = if (initialized) meshManager.state() else "stopped"
    fun meshHost(): String? = if (initialized) meshManager.host() else null

    @Synchronized
    fun restartServer() {
        http.stop()
        meshManager.stop()
        startServer()
    }

    /** Stop now. [byUser] keeps it off across reboots until Start or re-provisioning. */
    @Synchronized
    fun stopServer(byUser: Boolean) {
        if (byUser) {
            settings.userStopped = true
            shell.stop()
        }
        http.stop()
        meshManager.stop()
        log("server stopped (byUser=$byUser)")
        notifyChanged()
    }

    /** The notification's Stop and the app's Stop button. */
    fun stopByUser(context: Context) {
        init(context)
        stopServer(byUser = true)
        PhoneLinkService.stop(context)
    }

    /** The app's Start button. */
    fun startByUser(context: Context) {
        init(context)
        settings.userStopped = false
        settings.provisioned = true
        startServer()
        PhoneLinkService.start(context)
    }

    private val hooks = object : ServerHooks {
        override fun config(): ServerConfig = meshAwareConfig()

        override fun applyConfig(allowRemote: Boolean?, port: Int?, deviceName: String?) {
            deviceName?.let { settings.deviceName = it }
            val needsRestart = (allowRemote != null && allowRemote != settings.allowRemote) || (port != null && port != settings.port)
            allowRemote?.let { settings.allowRemote = it }
            port?.let { settings.port = it }
            // After the reply has gone out on the current socket.
            if (needsRestart) scheduler.schedule({ restartServer() }, 1, TimeUnit.SECONDS)
            notifyChanged()
        }

        override fun scheduleStop() {
            scheduler.schedule({ stopByUser(app) }, 1, TimeUnit.SECONDS)
        }
    }

    // ── Device facts ────────────────────────────────────────────────────────

    @Suppress("DEPRECATION")
    fun staticInfo(): StaticDeviceInfo {
        val wm = app.getSystemService(WindowManager::class.java)
        val dm = app.resources.displayMetrics
        val (w, h) = if (Build.VERSION.SDK_INT >= 30) {
            val b = wm.maximumWindowMetrics.bounds
            b.width() to b.height()
        } else {
            val real = DisplayMetrics()
            wm.defaultDisplay.getRealMetrics(real)
            real.widthPixels to real.heightPixels
        }
        return StaticDeviceInfo(Build.MANUFACTURER, Build.MODEL, Build.VERSION.RELEASE, Build.VERSION.SDK_INT, w, h, dm.densityDpi, dm.density)
    }

    /** Site-local IPv4 addresses (what a computer on the same Wi-Fi dials). */
    fun lanAddresses(): List<String> = try {
        NetworkInterface.getNetworkInterfaces().toList()
            .filter { it.isUp && !it.isLoopback }
            .flatMap { it.inetAddresses.toList() }
            .filter { it is Inet4Address && it.isSiteLocalAddress }
            .mapNotNull { it.hostAddress }
    } catch (e: Exception) { emptyList() }

    fun statusLine(): String {
        if (!initialized) return "Starting…"
        if (!http.isRunning) return lastError ?: "Stopped"
        val where = if (settings.allowRemote) (lanAddresses().firstOrNull() ?: "this phone") else "127.0.0.1 (cable only)"
        val n = auth.activeSessions.size
        val mesh = meshHost()?.let { " · mesh $it" } ?: ""
        val screen = if (shell.state == ShellServerLauncher.State.RUNNING) " · screen on" else ""
        return "Listening on $where:${settings.port} · $n paired$mesh$screen"
    }
}
