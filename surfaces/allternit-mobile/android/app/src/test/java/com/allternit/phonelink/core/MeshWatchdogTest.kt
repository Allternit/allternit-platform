package com.allternit.phonelink.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MeshWatchdogTest {
    private var now = 0L
    private var health: MeshHealth? = HEALTHY
    private val restarts = ArrayList<String>()
    private val logs = ArrayList<String>()
    private val dog = MeshWatchdog(probe = { health }, restart = { restarts.add(it) }, nowMs = { now }, log = { logs.add(it) })

    companion object {
        val HEALTHY = MeshHealth("running", "100.64.0.16", "ok")
    }

    /** One watchdog interval later. */
    private fun tick(): String? { now += MeshWatchdog.INTERVAL_MS; return dog.tick() }

    @Test fun parsesNodeHealthLines() {
        assertEquals(
            MeshHealth("running", "100.64.0.16", "ok", setOf("no-derp-home", "not-in-map-poll")),
            MeshHealth.parse("state=running;ip=100.64.0.16;serve=ok;warn=no-derp-home,not-in-map-poll"),
        )
        assertEquals(MeshHealth("stopped", "", "off"), MeshHealth.parse("state=stopped;ip=;serve=off;warn="))
        assertNull("older Mesh.aar", MeshHealth.parse(""))
        assertNull(MeshHealth.parse("ip=1.2.3.4"))
    }

    @Test fun problemCoversStateAddressListenerAndConnectivityWarnings() {
        assertNull(HEALTHY.problem())
        assertNull("non-connectivity warnings are ignored", HEALTHY.copy(warnings = setOf("update-available", "network-status")).problem())
        assertTrue(HEALTHY.copy(state = "starting").problem()!!.contains("starting"))
        assertTrue(HEALTHY.copy(meshIp = "").problem()!!.contains("without a mesh address"))
        assertTrue(HEALTHY.copy(serve = "dead").problem()!!.contains("stopped accepting"))
        assertTrue(HEALTHY.copy(warnings = setOf("not-in-map-poll")).problem()!!.contains("not-in-map-poll"))
    }

    @Test fun healthyNodeIsLeftAlone() {
        repeat(10) { assertNull(tick()) }
        assertTrue(restarts.isEmpty())
    }

    @Test fun noNodeIsTheJoinLoopsJob() {
        health = null
        repeat(5) { assertNull(tick()) }
        assertTrue(restarts.isEmpty())
    }

    @Test fun staleNetmapRestartsAfterTwoConsecutiveChecks() {
        health = HEALTHY.copy(warnings = setOf("not-in-map-poll"))
        assertNull("one bad check is not enough", tick())
        assertEquals("tailscale health: not-in-map-poll", tick())
        assertEquals(1, restarts.size)
    }

    @Test fun aBlipBetweenBadChecksResetsTheStrikes() {
        health = HEALTHY.copy(state = "stopped")
        assertNull(tick())
        health = HEALTHY
        assertNull(tick())
        health = HEALTHY.copy(state = "stopped")
        assertNull(tick())
        assertTrue(restarts.isEmpty())
    }

    @Test fun deadListenerRestartsAtOnce() {
        health = HEALTHY.copy(serve = "dead")
        assertTrue(tick()!!.contains("stopped accepting"))
    }

    @Test fun needsLoginIsNeverRestarted() {
        health = HEALTHY.copy(state = "needs_login")
        repeat(5) { assertNull(tick()) }
        assertTrue(restarts.isEmpty())
        assertEquals(1, logs.count { it.contains("needs login") })
    }

    @Test fun repeatedRestartsBackOff() {
        health = HEALTHY.copy(meshIp = "")
        assertNull(tick()); assertNotNull(tick())   // #1 at 120 s
        assertNull(tick()); assertNotNull(tick())   // #2 at 240 s (60 s gap already passed)
        assertNull(tick()); assertNotNull(tick())   // #3 at 360 s (120 s gap)
        // After #3 the gap is 300 s: checks at 420..600 s wait, 660 s fires.
        repeat(4) { assertNull(tick()) }
        assertNotNull(tick())
        assertEquals(4, restarts.size)
        // A healthy check resets the backoff: two bad checks restart again.
        health = HEALTHY
        assertNull(tick())
        health = HEALTHY.copy(meshIp = "")
        assertNull(tick()); assertNotNull(tick())
        assertEquals(5, restarts.size)
    }

    @Test fun aThrowingProbeCountsAsUnhealthy() {
        val d = MeshWatchdog(probe = { throw IllegalStateException("go panic") }, restart = { restarts.add(it) }, nowMs = { now })
        now += 60_000; assertNull(d.tick())
        now += 60_000; assertTrue(d.tick()!!.contains("go panic"))
    }
}
