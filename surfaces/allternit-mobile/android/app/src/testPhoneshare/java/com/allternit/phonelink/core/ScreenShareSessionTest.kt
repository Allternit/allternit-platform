package com.allternit.phonelink.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Runs in both builds: testPhonelink and testPlay include src/testPhoneshare. */
class ScreenShareSessionTest {
    private val portrait = DisplayInfo(720, 1600, 280, 0)
    private val landscape = DisplayInfo(1600, 720, 280, 90)
    private val minute = 60_000L

    @Test fun fitCapsTheLongSideAndAlignsToEight() {
        assertEquals(VideoSize(576, 1280), ScreenShareSession.fit(720, 1600))
        assertEquals(VideoSize(1280, 576), ScreenShareSession.fit(1600, 720))
        assertEquals(VideoSize(720, 1280), ScreenShareSession.fit(1080, 1920))
        // Already small: kept, rounded down to a multiple of 8.
        assertEquals(VideoSize(480, 800), ScreenShareSession.fit(483, 803))
    }

    @Test fun startsPausedAndCountsIdleFromTheStart() {
        val s = ScreenShareSession()
        s.start(portrait, 0)
        assertEquals(SharePhase.PAUSED, s.phase)
        assertFalse(s.shouldStopForIdle(10 * minute - 1))
        assertTrue(s.shouldStopForIdle(10 * minute))
    }

    @Test fun aViewerResetsTheIdleTimerAndLeavingStartsItAgain() {
        val s = ScreenShareSession()
        s.start(portrait, 0)
        assertEquals(EncoderAction.RESUME, s.control("resume", 5 * minute))
        assertFalse(s.shouldStopForIdle(60 * minute))
        assertEquals(EncoderAction.PAUSE, s.control("pause", 20 * minute))
        assertFalse(s.shouldStopForIdle(29 * minute))
        assertTrue(s.shouldStopForIdle(30 * minute))
    }

    @Test fun losingTheIngestPausesALiveShare() {
        val s = ScreenShareSession()
        s.start(portrait, 0)
        s.control("resume", 1)
        assertEquals(EncoderAction.PAUSE, s.ingestLost(100))
        assertEquals(SharePhase.PAUSED, s.phase)
        assertEquals(EncoderAction.NONE, s.ingestLost(200))
        assertTrue(s.shouldStopForIdle(100 + 10 * minute))
    }

    @Test fun nodeHintsMapToEncoderActions() {
        val s = ScreenShareSession()
        assertEquals(EncoderAction.NONE, s.control("resume", 0)) // off: nothing to resume
        s.start(portrait, 0)
        assertEquals(EncoderAction.NONE, s.control("keyframe", 0)) // paused: no frames to key
        assertEquals(EncoderAction.NONE, s.control("pause", 0))
        assertEquals(EncoderAction.RESUME, s.control("resume", 0))
        assertEquals(EncoderAction.KEYFRAME, s.control("resume", 0)) // second viewer while live
        assertEquals(EncoderAction.KEYFRAME, s.control("keyframe", 0))
        assertEquals(EncoderAction.NONE, s.control("bogus", 0))
    }

    @Test fun rotationThatChangesTheSizeNeedsANewConfig() {
        val s = ScreenShareSession()
        s.start(portrait, 0)
        assertEquals(VideoSize(1280, 576), s.displayChanged(landscape))
        assertEquals(VideoSize(1280, 576), s.video)
        assertEquals(InputGeometry(1280, 576, 1600, 720), s.geometry())
        // Upside-down portrait: same size, the display already shows it upright.
        assertEquals(VideoSize(576, 1280), s.displayChanged(portrait))
        assertNull(s.displayChanged(DisplayInfo(720, 1600, 280, 180)))
    }

    @Test fun rotationWhileOffDoesNothing() {
        val s = ScreenShareSession()
        assertNull(s.displayChanged(landscape))
        assertNull(s.geometry())
    }

    @Test fun consentFollowsThePromptAndTheShare() {
        val s = ScreenShareSession()
        assertEquals(Consent.NEEDED, s.consent())
        s.consentDenied()
        assertEquals(Consent.DENIED, s.consent())
        s.consentRequested()
        assertEquals(Consent.NEEDED, s.consent())
        s.start(portrait, 0)
        assertEquals(Consent.GRANTED, s.consent())
        s.stop("stopped")
        assertEquals(Consent.NEEDED, s.consent())
    }

    @Test fun reportCarriesStateSizeIdleAndTheLastEnd() {
        val s = ScreenShareSession()
        val off = s.report(0, paired = false, controlConnected = false, canTap = false).toJson()
        assertEquals("off", off.getString("state"))
        assertEquals("needed", off.getString("consent"))
        assertTrue(off.isNull("width"))
        assertTrue(off.isNull("lastEnded"))

        s.start(portrait, 1_000)
        val paused = s.report(4_000, paired = true, controlConnected = true, canTap = true).toJson()
        assertEquals("paused", paused.getString("state"))
        assertEquals("granted", paused.getString("consent"))
        assertEquals(576, paused.getInt("width"))
        assertEquals(1280, paused.getInt("height"))
        assertEquals(3_000L, paused.getLong("idleMs"))
        assertTrue(paused.getBoolean("canTap"))

        s.control("resume", 5_000)
        val live = s.report(6_000, paired = true, controlConnected = true, canTap = false).toJson()
        assertEquals("live", live.getString("state"))
        assertTrue(live.isNull("idleMs"))

        s.stop("idle")
        val ended = s.report(7_000, paired = true, controlConnected = true, canTap = false).toJson()
        assertEquals("off", ended.getString("state"))
        assertEquals("idle", ended.getString("lastEnded"))
        assertFalse(s.shouldStopForIdle(Long.MAX_VALUE))
    }
}
