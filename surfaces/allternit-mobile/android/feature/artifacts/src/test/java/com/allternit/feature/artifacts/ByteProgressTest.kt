package com.allternit.feature.artifacts

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Downloads, option B (step 11): the size and ETA rules, the status words, the poster blur and the poster rule. Mirrors iOS ByteProgressTests. */
class ByteProgressTest {
    private val mb = 1_048_576L

    @Test fun totalIsKnownOnlyForAnUnencodedLength() {
        assertNull(ByteProgressTracker.totalBytes(-1, null))
        assertNull(ByteProgressTracker.totalBytes(0, null))
        assertNull(ByteProgressTracker.totalBytes(100, "gzip"))
        assertNull(ByteProgressTracker.totalBytes(100, "br"))
        assertEquals(100L, ByteProgressTracker.totalBytes(100, "identity"))
        assertEquals(100L, ByteProgressTracker.totalBytes(100, " "))
    }

    @Test fun noEtaUntilAfterOneSecondThenASmoothedRate() {
        val t = ByteProgressTracker(1000, startedAt = 0.0)
        assertNull(t.add(100, now = 0.5).etaSeconds)
        // dt 1 s, instant 100 B/s, alpha 1-e^-0.5 against the first 200 B/s: about 160.6 B/s, 800 B left, 5 s.
        assertEquals(5, t.add(100, now = 1.5).etaSeconds)
        assertEquals(200L, t.finished().bytes)
    }

    @Test fun unknownTotalNeverGuessesASizeOrAnEta() {
        val t = ByteProgressTracker(null, startedAt = 0.0)
        val p = t.add(500, now = 3.0)
        assertNull(p.totalBytes); assertNull(p.etaSeconds); assertNull(p.fraction)
        assertEquals(ByteProgress(500, 500), t.finished())
    }

    @Test fun aTotalSmallerThanTheBytesGrowsToThem() {
        val p = ByteProgressTracker(100, startedAt = 0.0).add(150, now = 2.0)
        assertEquals(150L, p.totalBytes)
        assertEquals(1.0, p.fraction!!, 0.0)
        assertNull(p.etaSeconds)
    }

    @Test fun statusWordsGiveBytesPercentAndEta() {
        assertEquals("Loading 50% · 3.0 of 6.0 MB · ETA 3 s", ByteProgressText.status("Loading", ByteProgress(3 * mb, 6 * mb, 3)))
        assertEquals("Loading · 3.0 MB · — ETA", ByteProgressText.status("Loading", ByteProgress(3 * mb)))
        assertEquals("Saving 13% · 400 KB of 3.0 MB · — ETA", ByteProgressText.status("Saving", ByteProgress(400 * 1024, 3 * mb)))
        assertEquals("Loading · 0 B · — ETA", ByteProgressText.status("Loading", ByteProgress(0)))
    }

    @Test fun posterBlursFromTwentyEightToNoneAndHoldsAtSixteenWhenTheSizeIsUnknown() {
        assertEquals(28, ByteProgressText.blur(null))
        assertEquals(28, ByteProgressText.blur(ByteProgress(0, 100)))
        assertEquals(14, ByteProgressText.blur(ByteProgress(50, 100)))
        assertEquals(0, ByteProgressText.blur(ByteProgress(100, 100)))
        assertEquals(16, ByteProgressText.blur(ByteProgress(50)))
    }

    @Test fun posterIsOnlyAnInlineImageOrAnHttpsLink() {
        fun meta(poster: String) = Json.parseToJsonElement("""{"media":{"poster":"$poster"}}""").jsonObject
        assertEquals("data:image/png;base64,AA", ByteLoading.poster(meta("data:image/png;base64,AA")))
        assertEquals("https://x.test/p.jpg", ByteLoading.poster(meta("https://x.test/p.jpg")))
        assertNull(ByteLoading.poster(meta("http://x.test/p.jpg")))
        assertNull(ByteLoading.poster(meta("data:text/html;base64,AA")))
        assertNull(ByteLoading.poster(Json.parseToJsonElement("""{"media":{}}""").jsonObject))
        assertNull(ByteLoading.poster(null))
    }

    @Test fun reportsGoOutAboutEightTimesASecondAndTheLastAlwaysDoes() {
        assertTrue(ByteLoading.shouldSend(now = 1.0, last = Double.NEGATIVE_INFINITY, minGap = 0.12, complete = false))
        assertFalse(ByteLoading.shouldSend(now = 1.05, last = 1.0, minGap = 0.12, complete = false))
        assertTrue(ByteLoading.shouldSend(now = 1.05, last = 1.0, minGap = 0.12, complete = true))
        assertTrue(ByteLoading.shouldSend(now = 1.2, last = 1.0, minGap = 0.12, complete = false))
    }

    @Test fun sniffsTheImageType() {
        assertEquals("image/jpeg", sniffImageMime(byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0, 0)))
        assertEquals("image/png", sniffImageMime(byteArrayOf(0x89.toByte(), 'P'.code.toByte(), 'N'.code.toByte(), 'G'.code.toByte())))
        assertEquals("image/gif", sniffImageMime("GIF89a".toByteArray()))
        assertEquals("image/webp", sniffImageMime("RIFF....WEBPVP8 ".toByteArray()))
    }
}
