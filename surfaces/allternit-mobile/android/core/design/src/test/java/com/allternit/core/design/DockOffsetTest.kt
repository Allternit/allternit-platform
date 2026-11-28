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

    @Test
    fun lazyRangeIsKnownOnlyWithBothEndsLaidOut() {
        // Short page: items 0..5 all laid out, content ends 140 px below the viewport end (incl. 24 px after-padding).
        assertEquals(140f, lazyDockMaxScroll(0, 0, 5, 6, contentEnd = 2116, afterContentPadding = 24, viewportEndOffset = 2000), 0f)
        // Scrolled 90 px: the remaining 50 px plus what was scrolled is still the same range.
        assertEquals(140f, lazyDockMaxScroll(0, 90, 5, 6, contentEnd = 2026, afterContentPadding = 24, viewportEndOffset = 2000), 0f)
        // At the bottom the offset is exactly the dock distance.
        assertEquals(dock, dockOffsetFor(140f, lazyDockMaxScroll(0, 140, 5, 6, 1976, 24, 2000)), 0f)
        // The last item is not laid out yet, or item 0 has scrolled away: range unknown, offset unstretched.
        assertEquals(Float.POSITIVE_INFINITY, lazyDockMaxScroll(0, 0, 4, 40, 2100, 0, 2000), 0f)
        assertEquals(Float.POSITIVE_INFINITY, lazyDockMaxScroll(1, 10, 5, 6, 2100, 0, 2000), 0f)
        assertEquals(Float.POSITIVE_INFINITY, lazyDockMaxScroll(0, 0, -1, 0, 0, 0, 2000), 0f)
    }
}
