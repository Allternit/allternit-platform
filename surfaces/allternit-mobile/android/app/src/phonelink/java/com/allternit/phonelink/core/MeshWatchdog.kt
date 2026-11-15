package com.allternit.phonelink.core

/**
 * One self-check of the embedded tailnet node, parsed from Node.Health()
 * (`state=…;ip=…;serve=ok|dead|off;warn=code,code`). [serve] is "unknown"
 * when the bound Mesh.aar predates Health() and only state/meshIp are known.
 */
data class MeshHealth(
    val state: String,
    val meshIp: String,
    val serve: String,
    val warnings: Set<String> = emptySet(),
) {
    companion object {
        /**
         * Tailscale health warnables that mean peers may not reach this node
         * while it still says `running`: no live map poll with the control
         * server / no netmap for a while (a stale peer list — e.g. a computer
         * that re-registered with a new mesh IP during a control outage), or
         * no working DERP relay. `network-status` (the phone is offline) is
         * deliberately absent: a restart can't fix that.
         */
        val CONNECTIVITY_WARNINGS = setOf(
            "not-in-map-poll", "mapresponse-timeout",
            "no-derp-home", "no-derp-connection", "derp-timed-out",
            "magicsock-receive-func-error",
        )

        /** Parses Node.Health(); null for "" (older Mesh.aar) or a line without a state. */
        fun parse(raw: String): MeshHealth? {
            if (raw.isBlank()) return null
            val m = raw.trim().split(';').mapNotNull { part ->
                val i = part.indexOf('=')
                if (i <= 0) null else part.substring(0, i) to part.substring(i + 1)
            }.toMap()
            val state = m["state"]?.takeIf { it.isNotEmpty() } ?: return null
            return MeshHealth(
                state = state,
                meshIp = m["ip"].orEmpty(),
                serve = m["serve"]?.takeIf { it.isNotEmpty() } ?: "unknown",
                warnings = m["warn"].orEmpty().split(',').map { it.trim() }.filter { it.isNotEmpty() }.toSet(),
            )
        }
    }

    /** Why this node needs a restart, or null when it looks healthy. */
    fun problem(): String? = when {
        serve == "dead" -> "tailnet listener :${MeshController.TAILNET_SERVE_PORT} stopped accepting"
        state != "running" -> "node state is $state"
        meshIp.isEmpty() -> "node is running without a mesh address"
        else -> warnings.intersect(CONNECTIVITY_WARNINGS).sorted().takeIf { it.isNotEmpty() }
            ?.let { "tailscale health: ${it.joinToString(",")}" }
    }
}

/**
 * The `:mesh` process's self-heal loop, pure JVM. MeshService calls [tick]
 * every [INTERVAL_MS]; when the node has looked broken for [STRIKES]
 * consecutive checks (immediately when its tailnet listener died), it
 * restarts the node — same state dir, same node key — no sooner than
 * [BACKOFF_MS] after the previous watchdog restart. A healthy check resets
 * both the strikes and the backoff.
 *
 * What it can see is bounded by what tsnet exposes (see Node.Health in
 * infrastructure/mesh/tsnet-ios/mesh.go): backend state, the mesh address,
 * whether the ServeLocal listener still accepts, and tailscale's own health
 * warnables (not-in-map-poll / mapresponse-timeout stand in for "the last
 * netmap is stale"; tsnet has no netmap timestamp). It cannot see a peer
 * whose packets never arrive while every one of those reads healthy; that
 * case is healed from the computer's side (mesh_restart over LAN/cable).
 */
class MeshWatchdog(
    /** The current node's health, or null while no node is up (the join loop's backoff owns that). */
    private val probe: () -> MeshHealth?,
    /** Restarts the node (MeshController.restart). */
    private val restart: (reason: String) -> Unit,
    private val nowMs: () -> Long = System::currentTimeMillis,
    private val log: (String) -> Unit = {},
) {
    companion object {
        const val INTERVAL_MS = 60_000L
        /** Consecutive unhealthy checks before a restart (a dead listener restarts at once). */
        const val STRIKES = 2
        /** Minimum gap after the 1st, 2nd, … watchdog restart while the node stays unhealthy. */
        val BACKOFF_MS = longArrayOf(60_000, 120_000, 300_000, 600_000, 900_000)
    }

    private var strikes = 0
    private var restarts = 0
    private var lastRestartMs = 0L
    private var loggedNeedsLogin = false

    /** One check; returns the restart reason when it restarted the node, else null. */
    @Synchronized
    fun tick(): String? {
        val h = try { probe() } catch (e: Exception) {
            MeshHealth("error:${(e.message ?: e.javaClass.simpleName).take(80)}", "", "unknown")
        }
        if (h == null) { strikes = 0; return null }
        val reason = h.problem()
        if (reason == null) {
            strikes = 0
            restarts = 0
            loggedNeedsLogin = false
            return null
        }
        if (h.state == "needs_login") {
            // The node key was rejected/expired: only a fresh pre-auth key
            // (re-provisioning) fixes it, so a restart would only loop.
            if (!loggedNeedsLogin) log("mesh watchdog: node needs login; waiting for re-provisioning, not restarting")
            loggedNeedsLogin = true
            strikes = 0
            return null
        }
        strikes++
        if (h.serve != "dead" && strikes < STRIKES) return null
        if (restarts > 0) {
            val gap = BACKOFF_MS[minOf(restarts - 1, BACKOFF_MS.size - 1)]
            if (nowMs() - lastRestartMs < gap) return null
        }
        strikes = 0
        restarts++
        lastRestartMs = nowMs()
        log("mesh watchdog: restarting node ($reason), restart #$restarts")
        restart(reason)
        return reason
    }
}
