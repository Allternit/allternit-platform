package com.allternit.ondevice

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class BudgetAndManifestTest {
    private val gb = 1_000_000_000L

    @Test fun budgetIsCappedAtFortyFivePercentOfTotal() {
        assertEquals(3_600_000_000L, MemoryBudget.budget(6 * gb, 400_000_000L, 8 * gb))
    }

    @Test fun budgetIsAvailMinusThresholdWhenLow() {
        assertEquals(1_100_000_000L, MemoryBudget.budget(1_500_000_000L, 400_000_000L, 8 * gb))
    }

    @Test fun heldModelCountsAsAvailableAndBudgetNeverNegative() {
        assertEquals(2_100_000_000L, MemoryBudget.budget(500_000_000L, 400_000_000L, 8 * gb, 2 * gb))
        assertEquals(0L, MemoryBudget.budget(100_000_000L, 400_000_000L, 8 * gb))
    }

    @Test fun needIsSizeTimesOnePointTwoPlusFourHundredMb() {
        assertEquals(1_600_000_000L, MemoryBudget.needBytes(1 * gb))
    }

    private val sha = "a".repeat(64)
    private fun manifest(sources: String, sha256: String = sha, size: Long = 100) = JSONObject(
        """{"files":[{"path":"dir/Q 4.gguf","size":$size,"sha256":"$sha256"}],"sources":$sources}"""
    )

    @Test fun picksHostedBeforeUpstreamAndEncodesPath() {
        val m = manifest("""[{"type":"hf","url":"https://hf.co/x","priority":1},{"type":"hosted","url":"https://fw/x/","priority":1},{"type":"magnet","url":"magnet:?x","priority":0}]""")
        val ref = FreeweightsManifest.pick(m, "o/n", "dir/Q 4.gguf")
        assertEquals("https://fw/x/dir/Q%204.gguf", ref.url)
        assertEquals(sha, ref.sha256)
    }

    @Test fun rejectsBadShaMissingFileAndNoHttpsSource() {
        for ((m, path, code) in listOf(
            Triple(manifest("""[{"type":"hosted","url":"https://fw/"}]""", "zz"), "dir/Q 4.gguf", "bad_manifest"),
            Triple(manifest("""[{"type":"hosted","url":"https://fw/"}]"""), "other.gguf", "no_file"),
            Triple(manifest("""[{"type":"hosted","url":"http://fw/"}]"""), "dir/Q 4.gguf", "no_source"),
        )) {
            try {
                FreeweightsManifest.pick(m, "o/n", path)
                fail("expected $code")
            } catch (e: Protocol.Failure) {
                assertTrue(e.toString(), e.code == code)
            }
        }
    }
}
