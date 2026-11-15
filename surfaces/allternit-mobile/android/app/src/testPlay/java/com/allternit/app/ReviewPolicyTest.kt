package com.allternit.app

import com.allternit.app.ReviewPolicy.Companion.DAY_MS
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ReviewPolicyTest {

    private class MemoryStore : ReviewPolicy.Store {
        override var launchCount = 0
        override var firstLaunchAt = 0L
        override var lastAskedAt = 0L
    }

    private val start = 1_700_000_000_000L
    private var clock = start
    private val store = MemoryStore()
    private val policy = ReviewPolicy(store) { clock }

    private fun launches(n: Int) = repeat(n) { policy.recordLaunch() }

    @Test
    fun firstLaunchTimeIsKeptAcrossLaunches() {
        policy.recordLaunch()
        clock += 2 * DAY_MS
        policy.recordLaunch()
        assertEquals(start, store.firstLaunchAt)
        assertEquals(2, store.launchCount)
    }

    @Test
    fun underFiveLaunchesDoesNotAsk() {
        launches(4)
        clock += 30 * DAY_MS
        assertFalse(policy.shouldAsk())
        policy.recordLaunch()
        assertTrue(policy.shouldAsk())
    }

    @Test
    fun underThreeDaysSinceFirstLaunchDoesNotAsk() {
        launches(20)
        clock += 3 * DAY_MS - 1
        assertFalse(policy.shouldAsk())
        clock += 1
        assertTrue(policy.shouldAsk())
    }

    @Test
    fun insideOneHundredTwentyDaysOfLastAskDoesNotAsk() {
        launches(5)
        clock += 3 * DAY_MS
        policy.markAsked()
        clock += 120 * DAY_MS - 1
        assertFalse(policy.shouldAsk())
        clock += 1
        assertTrue(policy.shouldAsk())
    }

    @Test
    fun happyPathAsksOnceThenWaits() {
        launches(5)
        clock += 3 * DAY_MS
        assertTrue(policy.shouldAsk())
        policy.markAsked()
        policy.recordLaunch()
        assertFalse(policy.shouldAsk())
    }
}
