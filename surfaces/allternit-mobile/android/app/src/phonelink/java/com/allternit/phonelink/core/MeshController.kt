package com.allternit.phonelink.core

import java.io.File

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
    /** Node.Health()'s self-check line, or "" when the bound node predates it (see [MeshHealth.parse]). */
    fun health(): String = ""
}

/** Backoff between failed joins: quick at first (boot races Wi-Fi), then every 5 minutes. */
internal val RETRY_DELAYS_MS = longArrayOf(5_000, 15_000, 30_000, 60_000, 120_000, 300_000)

/**
 * Runs the phone's embedded tailnet node — from inside the `:mesh` process
 * (MeshService binds it), so a Go panic there can cost only the mesh, never
 * the phone server in the main process. Given the start [MeshStartParams] at
 * construction, it joins the customer's Headscale mesh and bridges the
 * tailnet's :[TAILNET_SERVE_PORT] onto the main process's internal loopback
 * listener, which marks every such request `viaMesh` (that listener, and
 * every rule about it, lives in the main process — this class holds no HTTP
 * server and no key material). Every state change goes out through
 * [onReport] as a [MeshReport].
 *
 * Lifecycle: start with [start] (idempotent), stop with [stop]. Network
 * changes (Wi-Fi to cellular) are handled inside tsnet/magicsock via
 * [onNetworkChanged]; there is no app-level reconnect loop beyond the join
 * retry backoff above.
 */
class MeshController(
    private val params: MeshStartParams,
    private val nodeFactory: (hostname: String) -> MeshNodeApi,
    private val onReport: (MeshReport) -> Unit = {},
    private val log: (String) -> Unit = {},
) {
    companion object {
        /** The port the phone's server is reachable on inside the tailnet. */
        const val TAILNET_SERVE_PORT = 8200
    }

    private val lock = Any()
    private var node: MeshNodeApi? = null

    @Volatile private var starting = false
    /** Bumped by stop(): a join that finishes afterwards must not publish its node. */
    @Volatile private var generation = 0L

    /** Idempotent. Spawns [runJoin] off the caller's thread (Start blocks for up to 60 s in Go). */
    @Synchronized
    fun start() {
        if (node != null || starting) return
        starting = true
        val g = ++generation
        onReport(MeshReport("starting", null))
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
            reportCurrent()
        } else {
            start()
        }
    }

    @Synchronized
    fun stop() {
        starting = false
        generation++
        synchronized(lock) {
            val n = node
            node = null
            try { n?.close() } catch (e: Exception) { log("mesh close failed: ${e.message}") }
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
        try {
            n = nodeFactory(params.hostname)
            n.refreshInterfaces()
            n.start(params.controlUrl, params.authKey, File(params.meshStateDir))
            n.serveLocal(TAILNET_SERVE_PORT, params.internalPort)
            synchronized(lock) {
                if (generationAtStart != generation) {
                    try { n.close() } catch (_: Exception) {}
                    return true
                }
                node = n
            }
            log("mesh joined, serving tailnet :$TAILNET_SERVE_PORT -> 127.0.0.1:${params.internalPort}")
            reportCurrent()
            return true
        } catch (e: Exception) {
            // The main process keeps the sealed key until a `running` report arrives.
            val why = (e.message ?: e.javaClass.simpleName).take(120)
            log("mesh join failed: ${e.javaClass.simpleName}: $why")
            onReport(MeshReport("error:$why", null))
            try { n?.close() } catch (_: Exception) {}
            return false
        }
    }

    /**
     * Stops and rejoins the node: the old tsnet server closes, a fresh one
     * starts on the same state dir, so the node key (and mesh IP) carry
     * over and no pre-auth key is needed. Used by the watchdog and by the
     * main process's `mesh_restart` (MeshMessages.MSG_RESTART).
     */
    @Synchronized
    fun restart(reason: String) {
        log("mesh restart: $reason")
        stop()
        start()
    }

    /** The current node's self-check, or null while no node is up (joining or retrying). */
    fun healthSnapshot(): MeshHealth? {
        val n = synchronized(lock) { node } ?: return null
        val raw = try { n.health() } catch (e: Exception) { "" }
        MeshHealth.parse(raw)?.let { return it }
        // Older Mesh.aar without Health(): state and address only.
        val st = try { n.state() } catch (e: Exception) { "error:${(e.message ?: e.javaClass.simpleName).take(120)}" }
        val ip = try { n.meshIp() } catch (e: Exception) { "" }
        return MeshHealth(st, ip, "unknown")
    }

    /** The node's current state + mesh address, as a report (network changes, post-join). */
    fun reportCurrent() {
        val n = synchronized(lock) { node } ?: return
        val ip = try { n.meshIp() } catch (e: Exception) { "" }
        val st = try { n.state() } catch (e: Exception) { "error:${(e.message ?: e.javaClass.simpleName).take(120)}" }
        onReport(MeshReport(st, ip.takeIf { it.isNotEmpty() }))
    }
}
