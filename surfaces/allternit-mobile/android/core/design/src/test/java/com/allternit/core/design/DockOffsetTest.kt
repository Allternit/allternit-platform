package com.allternit.core.design

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * A page that overflows by less than the dock distance must still finish docking at its bottom
 * (TestFlight feedback 2026-11-27: "COMPUTER" docked with "S" stuck mid-flight at max scroll).
 */
class DockOffsetTest {
    private val dock = DockSpec.DOCK

    @Test
    fun shortOverflowDocksExactlyAtTheBottom() {
        for (max in listOf(1.5f, 13f, 50.33f, 100f, 150f, 169.9f)) {
            val y = dockOffsetFor(max, max)
            assertEquals("maxScroll $max", dock, y, 0f)
            val frame = dockFrame(y, 9, reduced = false)
            assertTrue("maxScroll $max", frame.docked)
            assertEquals("maxScroll $max", 9, frame.landed)
            assertEquals("last letter, maxScroll $max", 1f, dockLetterProgress(frame.t, 9, 8), 0f)
        }
    }

    @Test
    fun shortOverflowStretchesTheScrollOverTheDockDistance() {
        assertEquals(dock / 2f, dockOffsetFor(50f, 100f), 1e-4f)
        assertEquals(0f, dockOffsetFor(0f, 100f), 0f)
        // Overscroll past either end stays inside the dock.
        assertEquals(0f, dockOffsetFor(-12f, 100f), 0f)
        assertEquals(dock, dockOffsetFor(130f, 100f), 0f)
    }

    @Test
    fun noOverflowNeverDocks() {
        assertEquals(0f, dockOffsetFor(0f, 0f), 0f)
        assertEquals(0f, dockOffsetFor(20f, 0f), 0f)
        assertEquals(0f, dockOffsetFor(20f, -5f), 0f)
        // Up to 1 dp of rounding is not overflow.
        assertEquals(0f, dockOffsetFor(0.7f, 0.7f), 0f)
    }

    @Test
    fun longAndUnknownRangesAreUnchanged() {
        assertEquals(150f, dockOffsetFor(150f, dock), 0f)
        assertEquals(150f, dockOffsetFor(150f, 900f), 0f)
        assertEquals(240f, dockOffsetFor(240f, 900f), 0f)
        assertEquals(150f, dockOffsetFor(150f, Float.POSITIVE_INFINITY), 0f)
        assertFalse(dockFrame(dockOffsetFor(150f, 900f), 9, reduced = false).docked)
    }
}
