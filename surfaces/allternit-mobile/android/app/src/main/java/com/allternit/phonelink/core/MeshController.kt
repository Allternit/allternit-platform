package com.allternit.phonelink.core

import java.io.File
import java.net.InetAddress

/**
 * The seam between this app and the gomobile-bound tsnet node (package `mesh`
 * in `infrastructure/mesh/tsnet-ios`, shipped as `app/libs/Mesh.aar`). Keeps
 * every mesh.* reference out of the unit-test classpath: the real
 * implementation loads Go native code, the tests substitute a fake.
 */
interface MeshNodeApi {
    fun start(controlUrl: String, authKey: String, dataDir: File)
    fun serveLocal(port: Int, localPort: Int)
    fun state(): String
    fun meshIp(): String
    fun close()
    /** Re-read the device's interfaces (Android denies Go the netlink dump; see GomobileMeshNode). */
    fun refreshInterfaces() {}
}

/**
 * Owns the phone's embedded tailnet node. When the app is provisioned with a
 * mesh control URL, the node joins the customer's Headscale mesh and serves
 * the phone's iCTRL server on the tailnet at [TAILNET_SERVE_PORT]: a mesh
 * peer dialing `<meshIP>:8200` is bridged by tsnet onto a dedicated INTERNAL
 * loopback listener (not the app's public port), which marks every such
 * request `viaMesh`. That marking is a security property: mesh-forwarded
 * connections arrive from 127.0.0.1, and without it they could consume the
 * one-shot armed pairing grant that is reserved for adb-over-cable.
 *
 * Lifecycle mirrors the HTTP server: start with [start] (called from the
 * server's start path, which the foreground service and boot receiver both
 * funnel into), stop with [stop]. Network changes (Wi-Fi to cellular) are
 * handled inside tsnet/magicsock; there is no app-level reconnect loop.
 */
/** Backoff between failed joins: quick at first (boot races Wi-Fi), then every 5 minutes. */
internal val RETRY_DELAYS_MS = longArrayOf(5_000, 15_000, 30_000, 60_000, 120_000, 300_000)

class MeshController(
    private val settings: AppSettings,
    private val keyBox: MeshKeyBox,
    private val meshDir: File,
    private val handler: (HttpRequest) -> HttpResponse,
    private val nodeFactory: (hostname: String) -> MeshNodeApi,
    private val log: (String) -> Unit = {},
) {
    companion object {
        /** The port the phone's server is reachable on inside the tailnet. */
        const val TAILNET_SERVE_PORT = 8200
    }

    private val lock = Any()
    private var node: MeshNodeApi? = null
    private var meshHttp: HttpServer? = null
    private var internalPort = 0

    @Volatile private var starting = false
    @Volatile private var lastError: String? = null
    /** Bumped by stop(): a join that finishes afterwards must not publish its node. */
    @Volatile private var generation = 0L

    /** The loopback port the mesh bridge targets; dedicated so it can be marked viaMesh. */
    fun internalServePort(): Int =
        if (settings.port + 10000 <= 65535) settings.port + 10000 else settings.port - 40000

    fun state(): String = synchronized(lock) {
        node?.state() ?: lastError?.let { "error:$it" } ?: if (starting) "starting" else "stopped"
    }

    /** `<meshIP>:8200` while the node is up, else null. */
    fun host(): String? = synchronized(lock) {
        node?.meshIp()?.takeIf { it.isNotEmpty() }?.let { "$it:$TAILNET_SERVE_PORT" }
    }

    /** Idempotent. Spawns [runJoin] off the caller's thread (Start blocks for up to 60 s in Go). */
    @Synchronized
    fun start() {
        if (node != null || starting) return
        if (settings.meshControlUrl == null) return
        starting = true
        val g = ++generation
        Thread({
            try { runJoin(g) } finally { if (g == generation) starting = false }
        }, "phonelink-mesh").apply { isDaemon = true }.start()
    }

    /**
     * The default network changed (Wi-Fi joined or lost, cellular took over).
     * tsnet re-reads interfaces through the hook; a join that failed earlier
     * (e.g. no network yet at boot) is retried now instead of waiting.
     */
    fun onNetworkChanged() {
        val n = synchronized(lock) { node }
        if (n != null) {
            try { n.refreshInterfaces() } catch (e: Exception) { log("mesh interface refresh failed: ${e.message}") }
        } else if (lastError != null) {
            start()
        }
    }

    @Synchronized
    fun stop() {
        starting = false
        generation++
        lastError = null
        synchronized(lock) {
            val n = node
            node = null
            val h = meshHttp
            meshHttp = null
            try { n?.close() } catch (e: Exception) { log("mesh close failed: ${e.message}") }
            h?.stop()
        }
    }

    /** Joins, and while this generation is current retries a failed join with backoff. */
    private fun runJoin(generationAtStart: Long) {
        var attempt = 0
        while (true) {
            if (joinOnce(generationAtStart)) return
            if (generationAtStart != generation) return
            val delayMs = RETRY_DELAYS_MS[minOf(attempt++, RETRY_DELAYS_MS.size - 1)]
            log("mesh join retry in ${delayMs / 1000} s")
            try { Thread.sleep(delayMs) } catch (_: InterruptedException) { return }
            if (generationAtStart != generation) return
        }
    }

    /** One join attempt; true when the node is up (or this generation was superseded). */
    private fun joinOnce(generationAtStart: Long): Boolean {
        var n: MeshNodeApi? = null
        var http: HttpServer? = null
        try {
            val port = internalServePort()
            http = HttpServer(handler, viaMesh = true, log = log)
            http.start(InetAddress.getByAddress(byteArrayOf(127, 0, 0, 1)), port)
            internalPort = http.localPort

            n = nodeFactory("phone-" + settings.deviceId.take(8))
            n.refreshInterfaces()
            // First join uses the sealed pre-auth key; later joins pass "" and
            // tsnet re-authenticates with the node key already in meshDir.
            n.start(settings.meshControlUrl!!, keyBox.read().orEmpty(), meshDir)
            n.serveLocal(TAILNET_SERVE_PORT, internalPort)
            // The join succeeded: the pre-auth key is single-use, drop it.
            keyBox.clear()
            synchronized(lock) {
                if (generationAtStart != generation) {
                    try { n.close() } catch (_: Exception) {}
                    http.stop()
                    return true
                }
                node = n
                meshHttp = http
                lastError = null
            }
            log("mesh joined, serving tailnet :$TAILNET_SERVE_PORT -> 127.0.0.1:$internalPort")
            return true
        } catch (e: Exception) {
            // Keep the sealed key: the join did not succeed, so it is still needed.
            val why = (e.message ?: e.javaClass.simpleName).take(120)
            lastError = why
            log("mesh join failed: ${e.javaClass.simpleName}: $why")
            try { n?.close() } catch (_: Exception) {}
            http?.stop()
            return false
        }
    }
}
