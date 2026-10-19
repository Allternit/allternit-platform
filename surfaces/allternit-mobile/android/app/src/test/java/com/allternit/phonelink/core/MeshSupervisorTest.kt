package com.allternit.phonelink.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger

/**
 * The main-process mesh state machine: /whoami + device_info mapping, the
 * crash backoff, and the pre-auth key drop rule — all pure JVM.
 */
class MeshSupervisorTest {
    private val scheduled = CopyOnWriteArrayList<Pair<Long, () -> Unit>>()
    private val rebinds = AtomicInteger()
    private var port = 8200
    private var now = 1_000_000L

    private fun newSupervisor() = MeshSupervisor(
        serverPort = { port },
        schedule = { d, t -> scheduled.add(d to t) },
        onRebind = { rebinds.incrementAndGet() },
        nowMs = { now },
    )

    @Test fun stateIsStoppedBeforeAnyBind() {
        val s = newSupervisor()
        assertEquals("stopped", s.state())
        assertNull(s.host())
    }

    @Test fun awaitingFirstReportIsStarting() {
        val s = newSupervisor()
        s.onMeshStarting()
        s.onMeshProcessBound()
        assertEquals("starting", s.state())
        assertNull(s.host())
    }

    @Test fun reportsDriveStateHostAndKeyDrop() {
        val s = newSupervisor()
        s.onMeshProcessBound()
        assertFalse(s.onReport(MeshReport("starting", null)))
        assertEquals("starting", s.state())

        // Only a `running` report licenses dropping the sealed pre-auth key.
        assertFalse(s.onReport(MeshReport("needs_login", null)))
        assertFalse(s.onReport(MeshReport("error:control server unreachable", null)))
        assertTrue(s.onReport(MeshReport("running", "100.64.0.5")))
        assertEquals("running", s.state())
        assertEquals("100.64.0.5:8200", s.host())
    }

    @Test fun internalServePortTracksServerPort() {
        val s = newSupervisor()
        port = 8200; assertEquals(18200, s.internalServePort())
        port = 9000; assertEquals(19000, s.internalServePort())
        port = 55535; assertEquals(65535, s.internalServePort())
        port = 55536; assertEquals(15536, s.internalServePort())
    }

    @Test fun deathReportsErrorAndRebindsWithBackoff() {
        val s = newSupervisor()
        s.onMeshStarting()
        s.onMeshProcessBound()
        val expected = listOf(5_000L, 15_000L, 30_000L, 60_000L, 300_000L, 300_000L)
        for ((i, delay) in expected.withIndex()) {
            s.onMeshProcessDied()
            assertEquals(MeshSupervisor.PROCESS_DEAD, s.state())
            assertNull("host must be gone while the process is dead", s.host())
            assertEquals("one pending re-bind per death", 1, scheduled.size)
            assertEquals(delay, scheduled.last().first)
            // The scheduled re-bind fires, the fresh process connects, then dies again.
            val task = scheduled.last().second
            scheduled.clear()
            task()
            assertEquals(i + 1, rebinds.get())
            s.onMeshProcessBound()
        }
        assertEquals("steady state is a re-bind every 5 min", 6, rebinds.get())
    }

    @Test fun longHealthyRunResetsTheBackoff() {
        val s = newSupervisor()
        s.onMeshStarting()
        s.onMeshProcessBound()
        s.onMeshProcessDied()
        assertEquals(5_000L, scheduled.last().first)
        // The re-bind connects and the mesh runs healthy for over a minute…
        scheduled.last().second()
        s.onMeshProcessBound()
        now += MeshSupervisor.HEALTHY_SETTLE_MS + 1
        // …so its next death retries promptly instead of waiting out the flap ladder.
        s.onMeshProcessDied()
        assertEquals(5_000L, scheduled.last().first)
    }

    @Test fun cleanStopCancelsPendingRebind() {
        val s = newSupervisor()
        s.onMeshStarting()
        s.onMeshProcessBound()
        s.onMeshProcessDied()
        assertEquals(1, scheduled.size)
        s.onMeshStopped()
        assertEquals("stopped", s.state())
        scheduled.last().second()
        assertEquals("no re-bind after a clean stop", 0, rebinds.get())
    }

    @Test fun deathBeforeFirstConnectStillRetries() {
        val s = newSupervisor()
        s.onMeshStarting()
        // The mesh process died between bindService and onServiceConnected.
        s.onMeshProcessDied()
        assertEquals(MeshSupervisor.PROCESS_DEAD, s.state())
        assertEquals(1, scheduled.size)
        assertEquals(5_000L, scheduled.last().first)
    }

    @Test fun staleReportsFromDyingProcessDoNotResurrectState() {
        val s = newSupervisor()
        s.onMeshStarting()
        s.onMeshProcessBound()
        s.onMeshProcessDied()
        // A report in flight from the dying process must not flip state back to running.
        s.onReport(MeshReport("running", "100.64.0.5"))
        assertEquals(MeshSupervisor.PROCESS_DEAD, s.state())
        assertNull(s.host())
    }

    @Test fun restartRequestHidesTheOldReportUntilTheRejoin() {
        val s = newSupervisor()
        assertFalse("nothing bound: nothing to restart in place", s.onRestartRequested())
        s.onMeshStarting()
        s.onMeshProcessBound()
        s.onReport(MeshReport("running", "100.64.0.16"))
        assertEquals("100.64.0.16:8200", s.host())
        assertTrue(s.onRestartRequested())
        assertEquals("starting", s.state())
        assertNull(s.host())
        s.onReport(MeshReport("running", "100.64.0.16"))
        assertEquals("running", s.state())
        s.onMeshProcessDied()
        assertFalse("dead process: re-bind instead", s.onRestartRequested())
    }
}
