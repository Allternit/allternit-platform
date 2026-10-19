package com.allternit.phonelink.core

/**
 * The main process's view of the mesh. Pure JVM (unit-tested without Android
 * or Go): owns the state mapping behind `/whoami`'s `mesh_state`/`mesh_host`,
 * the backoff for re-binding the `:mesh` process after it dies, and the
 * internal loopback port computation. The Android-side binding lives in
 * MeshManager, the join itself in MeshController (which runs inside the
 * `:mesh` process).
 */
class MeshSupervisor(
    /** The phone server's port; the mesh bridge targets port + 10000 on loopback. */
    private val serverPort: () -> Int,
    /** Schedules [task] after [delayMs]; carries the re-bind backoff. */
    private val schedule: (delayMs: Long, task: () -> Unit) -> Unit,
    /** Invoked (on a scheduler thread) when a dead mesh process is due a re-bind. */
    private val onRebind: () -> Unit = {},
    private val nowMs: () -> Long = System::currentTimeMillis,
) {
    companion object {
        /** Re-bind after a mesh-process death: 5 s, 15 s, 30 s, 60 s, then every 5 minutes. */
        val REBIND_DELAYS_MS = longArrayOf(5_000, 15_000, 30_000, 60_000, 300_000)
        /** Reported while a death is being retried; the phone server itself is unaffected. */
        const val PROCESS_DEAD = "error:mesh process stopped"
        /** A mesh process healthy this long settles: its next death retries promptly at 5 s. */
        const val HEALTHY_SETTLE_MS = 60_000L
    }

    private val lock = Any()
    private var lastReport: MeshReport? = null
    /** True while the mesh service is bound and healthy. */
    private var bound = false
    /** When the current process connected; a long healthy run resets the backoff. */
    private var boundAtMs = 0L
    /** True after a death, until a re-bind delivers a fresh process. */
    private var died = false
    /** Set by onMeshStopped: a death racing a clean stop must not schedule a re-bind. */
    private var stopped = true
    private var rebindAttempt = 0
    /** Bumped on every lifecycle event; stale scheduled re-binds check it before firing. */
    private var generation = 0L

    /** The loopback port the mesh bridge targets; dedicated so it can be marked viaMesh. */
    fun internalServePort(): Int =
        if (serverPort() + 10000 <= 65535) serverPort() + 10000 else serverPort() - 40000

    /**
     * `stopped|starting|running|needs_login|error:<reason>`. A dead mesh
     * process surfaces as an error, never silently as `stopped`, so /whoami
     * shows the mesh is broken while the phone server keeps running.
     */
    fun state(): String = synchronized(lock) {
        when {
            died -> PROCESS_DEAD
            !bound -> "stopped"
            else -> lastReport?.state ?: "starting"
        }
    }

    /** `<meshIP>:8200` while the mesh process reports a mesh address, else null. */
    fun host(): String? = synchronized(lock) {
        if (died || !bound) null
        else lastReport?.meshIp?.takeIf { it.isNotEmpty() }?.let { "$it:${MeshController.TAILNET_SERVE_PORT}" }
    }

    /** A bind has been initiated; deaths from here on (even before the first connect) count. */
    fun onMeshStarting() = synchronized(lock) { stopped = false }

    /** A healthy mesh process just came up; a fresh join begins, so the state resets. */
    fun onMeshProcessBound() = synchronized(lock) {
        stopped = false
        bound = true
        died = false
        lastReport = null
        boundAtMs = nowMs()
    }

    /**
     * The mesh process died (crash, OOM, Go panic) — possibly before its
     * first onServiceConnected. Reports the error state and schedules a
     * re-bind with backoff; a clean [onMeshStopped] racing this call wins
     * because it marks the supervisor stopped. Quick flaps escalate the
     * backoff, a process that ran for [HEALTHY_SETTLE_MS] settles back to
     * the prompt 5 s retry.
     */
    fun onMeshProcessDied() {
        val g: Long
        val delayMs: Long
        synchronized(lock) {
            if (stopped || died) return
            bound = false
            died = true
            if (nowMs() - boundAtMs >= HEALTHY_SETTLE_MS) rebindAttempt = 0
            g = ++generation
            delayMs = REBIND_DELAYS_MS[minOf(rebindAttempt++, REBIND_DELAYS_MS.size - 1)]
        }
        schedule(delayMs) {
            val fire: Boolean
            synchronized(lock) { fire = g == generation && died }
            if (fire) onRebind()
        }
    }

    /**
     * `mesh_restart` asked the bound mesh process to rejoin. Until its fresh
     * reports arrive the state reads `starting` and no mesh host is offered,
     * so a caller waiting for `running` never sees the old node's report.
     * False when no mesh process is bound (nothing to restart in place).
     */
    fun onRestartRequested(): Boolean = synchronized(lock) {
        if (!bound || died) return false
        lastReport = MeshReport("starting", null)
        true
    }

    /** Clean unbind (server stopped): stop reporting, cancel any pending re-bind. */
    fun onMeshStopped() = synchronized(lock) {
        stopped = true
        bound = false
        died = false
        lastReport = null
        rebindAttempt = 0
        generation++
    }

    /**
     * Stores the mesh process's latest report. Returns true when the main
     * process must now drop the sealed pre-auth key (i.e. the mesh reports
     * `running`: the single-use key has done its job).
     */
    fun onReport(report: MeshReport): Boolean = synchronized(lock) {
        lastReport = report
        report.state == "running"
    }
}
