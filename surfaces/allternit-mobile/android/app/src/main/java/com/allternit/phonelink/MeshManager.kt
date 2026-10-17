package com.allternit.phonelink

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.os.Message
import android.os.Messenger
import com.allternit.phonelink.core.HttpRequest
import com.allternit.phonelink.core.HttpResponse
import com.allternit.phonelink.core.HttpServer
import com.allternit.phonelink.core.MeshKeyBox
import com.allternit.phonelink.core.MeshMessages
import com.allternit.phonelink.core.MeshStartParams
import com.allternit.phonelink.core.MeshSupervisor
import java.io.File
import java.net.InetAddress

/**
 * Main-process owner of the mesh. While the phone server runs and a mesh
 * control URL is provisioned, it:
 *
 *  - hosts the INTERNAL loopback listener (127.0.0.1:[MeshSupervisor.internalServePort])
 *    that the mesh bridge forwards onto; every request it serves is marked
 *    `viaMesh`, and every security rule about that marking (no armed grant,
 *    no plaintext pair) stays in this process, in front of the same server;
 *  - binds [MeshService] in the private `:mesh` process (BIND_AUTO_CREATE|
 *    BIND_IMPORTANT, so the mesh shares the foreground service's priority),
 *    feeding it the start inputs in the bind Intent (the opened pre-auth key
 *    only while one is stored);
 *  - keeps the mesh's last report for /whoami and device_info, drops the
 *    sealed key once the mesh reports `running`, and re-binds with backoff
 *    when the mesh process dies — a Go panic there can never stop the server.
 */
class MeshManager(
    private val app: Context,
    private val meshDir: File,
    private val keyBox: MeshKeyBox,
    private val serverPort: () -> Int,
    private val meshControlUrl: () -> String?,
    private val deviceId: () -> String,
    /** The phone server's request handler; the internal listener forwards to it. */
    private val handler: (HttpRequest) -> HttpResponse,
    private val onChanged: () -> Unit,
    private val schedule: (delayMs: Long, task: () -> Unit) -> Unit,
    private val log: (String) -> Unit = {},
) {
    private val supervisor = MeshSupervisor(serverPort, schedule, onRebind = { rebind() })

    @Volatile private var started = false
    @Volatile private var bound = false
    /** True between bindService and the first callback, so stop() can unbind a pending bind. */
    @Volatile private var binding = false
    /** Encoded params of the current binding; a change (new key, new port) forces a re-bind. */
    @Volatile private var lastParams: String? = null
    private var internalHttp: HttpServer? = null
    private var handlerThread: HandlerThread? = null
    private var incoming: Messenger? = null

    private val connection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName, binder: IBinder) {
            bound = true
            binding = false
            supervisor.onMeshProcessBound()
            log("mesh process bound")
            onChanged()
        }

        override fun onServiceDisconnected(name: ComponentName) = onDeath()
        override fun onBindingDied(name: ComponentName) = onDeath()
        override fun onNullBinding(name: ComponentName) = onDeath()
    }

    /** Idempotent. Starts the internal listener and binds the mesh process when provisioned. */
    @Synchronized
    fun start() {
        started = true
        if (meshControlUrl() == null) return
        ensureHandlerThread()
        startInternalListener()
        bindNow()
    }

    /** Unbinds the mesh process and stops the internal listener. */
    @Synchronized
    fun stop() {
        started = false
        lastParams = null
        unbind()
        stopInternalListener()
        supervisor.onMeshStopped()
    }

    fun state(): String = supervisor.state()
    fun host(): String? = supervisor.host()

    private fun buildParams(): MeshStartParams =
        MeshStartParams(
            controlUrl = meshControlUrl()!!,
            internalPort = supervisor.internalServePort(),
            hostname = "phone-" + deviceId().take(8),
            meshStateDir = meshDir.absolutePath,
            authKey = keyBox.read().orEmpty(),
        )

    private fun bindNow() {
        val encoded = MeshMessages.encodeStart(buildParams())
        if (bound) {
            if (encoded == lastParams) return
            // Inputs changed under a live binding (e.g. a fresh pre-auth key
            // arrived over PROVISION): the node must restart to receive them.
            unbind()
        }
        lastParams = encoded
        supervisor.onMeshStarting()
        binding = true
        val intent = Intent(app, MeshService::class.java)
            .putExtra(MeshMessages.EXTRA_PARAMS, encoded)
            .putExtra(MeshMessages.EXTRA_REPLY, incoming)
        try {
            app.bindService(intent, connection, Context.BIND_AUTO_CREATE or Context.BIND_IMPORTANT)
        } catch (e: Exception) {
            binding = false
            log("mesh bind failed: ${e.javaClass.simpleName}: ${e.message}")
        }
    }

    /** Scheduler-thread re-bind after a mesh-process death (backoff lives in MeshSupervisor). */
    private fun rebind() {
        if (!started) return
        if (meshControlUrl() == null) return
        synchronized(this) {
            if (!started) return
            ensureHandlerThread()
            bindNow()
        }
    }

    private fun onDeath() {
        bound = false
        binding = false
        supervisor.onMeshProcessDied()
        onChanged()
        // After onBindingDied Android requires an explicit unbind before the
        // next bind; after onServiceDisconnected the binding is already gone.
        // Both cases: unbind must be idempotent-guarded.
        try { app.unbindService(connection) } catch (_: IllegalArgumentException) {}
    }

    private fun unbind() {
        synchronized(this) {
            if (!bound && !binding) return
            bound = false
            binding = false
        }
        try { app.unbindService(connection) } catch (_: IllegalArgumentException) {}
    }

    /**
     * The dedicated loopback listener the mesh bridge targets. viaMesh stays
     * load-bearing and lives entirely in this process: mesh-forwarded
     * connections arrive from 127.0.0.1, and without the marking they could
     * consume the one-shot armed pairing grant reserved for adb-over-cable.
     */
    @Synchronized
    private fun startInternalListener() {
        if (internalHttp != null) return
        val http = HttpServer(handler, viaMesh = true, log = log)
        try {
            http.start(InetAddress.getByAddress(byteArrayOf(127, 0, 0, 1)), supervisor.internalServePort())
            internalHttp = http
        } catch (e: Exception) {
            log("mesh internal listener failed: ${e.message}")
        }
    }

    @Synchronized
    private fun stopInternalListener() {
        internalHttp?.stop()
        internalHttp = null
    }

    private fun ensureHandlerThread() {
        if (handlerThread != null) return
        val t = HandlerThread("phonelink-mesh-main").apply { start() }
        handlerThread = t
        incoming = Messenger(Handler(t.looper) { msg -> onMeshMessage(msg); true })
    }

    private fun onMeshMessage(msg: Message) {
        if (msg.what != MeshMessages.MSG_REPORT) return
        val report = (msg.obj as? String)?.let { MeshMessages.decodeReport(it) } ?: return
        val dropKey = supervisor.onReport(report)
        if (dropKey) {
            // The pre-auth key is single-use: the mesh is running, so drop it.
            try { keyBox.clear() } catch (e: Exception) { log("mesh key clear failed: ${e.message}") }
        }
        onChanged()
    }
}
