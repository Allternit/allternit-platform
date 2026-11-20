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
