package com.allternit.feature.computer

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AciConsoleDetailTest {
    private fun obj(s: String) = Json.parseToJsonElement(s) as JsonObject

    @Test fun bucketAveragesIntoAtMostN() {
        val out = consoleBucket((1..96).map { it.toDouble() }, 48)
        assertEquals(48, out.size)
        assertEquals(1.5, out.first(), 0.0)
        assertEquals(95.5, out.last(), 0.0)
        assertEquals(listOf(1.0, 2.0), consoleBucket(listOf(1.0, 2.0), 48))
    }

    @Test fun metricSeriesReadsValuesFromPointPairs() {
        val s = MetricSeries.parse(obj("""{"metric":"cpu","points":[["2026-11-20T10:00:00Z",12.5],["2026-11-20T10:05:00Z",90]]}"""))
        assertEquals(listOf(12.5, 90.0), s?.values)
        assertNull(MetricSeries.parse(obj("""{"points":[]}""")))
    }

    @Test fun logLineNeedsAMessageAndFlagsProblems() {
        assertEquals(true, ConsoleLogLine.parse("""{"ts":"x","unit":"nginx","level":"warn","message":"slow"}""")?.isProblem)
        assertEquals(false, ConsoleLogLine.parse("""{"level":"info","message":"ok"}""")?.isProblem)
        assertNull(ConsoleLogLine.parse("not json"))
        assertNull(ConsoleLogLine.parse("""{"level":"info"}"""))
    }

    @Test fun hostSnapshotNeedsCpuAndMemory() {
        val ok = HostSnapshot.parse(Json.parseToJsonElement("""{"cpu":{"percent":40,"cores":8},"memory":{"usedBytes":1,"totalBytes":4},"disk":[],"uptimeSec":3700,"os":{"name":"Ubuntu","arch":"x86_64"}}"""))
        assertEquals(25.0, ok?.sample?.memoryPercent ?: -1.0, 0.0)
        assertEquals("1h 1m", consoleDuration(ok?.uptimeSec ?: 0.0))
        assertNull(HostSnapshot.parse(Json.parseToJsonElement("""{"cpu":{"percent":40}}""")))
    }
}

class AciConsoleManageTest {
    private fun obj(s: String) = kotlinx.serialization.json.Json.parseToJsonElement(s) as kotlinx.serialization.json.JsonObject
    private val now = java.time.Instant.parse("2026-11-20T12:00:00Z")

    @Test fun freshBackupIsNewestUnder24Hours() {
        val old = CloudBackup("a", "2026-11-18T12:00:00Z", null)
        val fresh = CloudBackup("b", "2026-11-20T01:00:00Z", 10.0)
        val fresher = CloudBackup("c", "2026-11-20T09:00:00Z", null)
        assertEquals("c", consoleFreshBackup(listOf(old, fresh, fresher), now)?.id)
        assertNull(consoleFreshBackup(listOf(old), now))
        assertNull(consoleFreshBackup(emptyList(), now))
    }

    @Test fun keyValidationRefusesPrivateKeysAndNeedsPublicPrefix() {
        assertEquals(true, consoleKeyValid("Mac", "ssh-ed25519 AAAA"))
        assertEquals(true, consoleKeyValid("Mac", "sk-ssh-ed25519@openssh.com AAAA"))
        assertEquals(false, consoleKeyValid("", "ssh-ed25519 AAAA"))
        assertEquals(false, consoleKeyValid("Mac", "-----BEGIN OPENSSH PRIVATE KEY-----"))
        assertEquals(false, consoleKeyValid("Mac", "hello"))
    }

    @Test fun findingNeedsTitleAndReadsSuggestedAction() {
        val f = HostFinding.parse(obj("""{"id":"1","severity":"warning","title":"Disk 91% full","detail":"d","suggestedAction":{"type":"clean_apt_cache","target":"/var/cache/apt","label":"Clear package cache"}}"""))
        assertEquals("clean_apt_cache /var/cache/apt", f?.suggestedAction?.commandLine)
        assertNull(HostFinding.parse(obj("""{"detail":"x"}""")))
    }

    @Test fun actionResultNeedsChangeIdAndMayCarryUndo() {
        val r = HostActionResult.parse(obj("""{"changeId":"chg_1","undo":{"type":"restore","target":"chg_1"}}"""))
        assertEquals("chg_1", r?.changeId)
        assertEquals("restore chg_1", r?.undo?.commandLine)
        assertNull(HostActionResult.parse(obj("""{"note":"x"}""")))
    }

    @Test fun agentKeysHaveNoRemoveAndRowsAcceptBareArrayOrEnvelope() {
        assertEquals(true, SshKey.parse(obj("""{"id":"k","label":"agent","kind":"agent"}"""))?.isAgent)
        val env = kotlinx.serialization.json.Json.parseToJsonElement("""{"keys":[{"id":"k","label":"a"}]}""")
        val bare = kotlinx.serialization.json.Json.parseToJsonElement("""[{"id":"k","label":"a"}]""")
        assertEquals(1, env.rows("keys").size)
        assertEquals(1, bare.rows("keys").size)
    }

    @Test fun locationKeyMatchesCodeOrName() {
        assertEquals("chi", consoleLocationKey("Chicago"))
        assertEquals("lon", consoleLocationKey("lon"))
        assertNull(consoleLocationKey("mars"))
    }
}
