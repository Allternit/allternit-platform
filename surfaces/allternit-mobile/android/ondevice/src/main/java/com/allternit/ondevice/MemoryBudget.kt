package com.allternit.ondevice

import android.app.ActivityManager
import android.content.Context

/**
 * The phone's model budget, plan section 3 "Android" row:
 *   budget = min(availMem - threshold, 0.45 x totalMem)
 * from ActivityManager.MemoryInfo. A model this engine already holds is counted
 * as available (it is freed before another loads), so the number does not
 * shrink just because a model is running. Reported to the page as a `unified`
 * DeviceBudget; the page runs the shared TS fit (src/lib/on-device/fit.ts) on it.
 */
object MemoryBudget {
    data class Reading(
        val totalBytes: Long,
        val availBytes: Long,
        val thresholdBytes: Long,
        val lowMemory: Boolean,
        val heldBytes: Long,
        val budgetBytes: Long,
    )

    /** Pure formula, JVM-tested. Never negative. */
    fun budget(availBytes: Long, thresholdBytes: Long, totalBytes: Long, heldBytes: Long = 0): Long {
        val free = availBytes + heldBytes - thresholdBytes
        val cap = totalBytes * 45 / 100
        return maxOf(0L, minOf(free, cap))
    }

    fun read(context: Context, heldBytes: Long = 0): Reading {
        val am = context.getSystemService(ActivityManager::class.java)
        val mi = ActivityManager.MemoryInfo()
        am.getMemoryInfo(mi)
        return Reading(
            totalBytes = mi.totalMem,
            availBytes = mi.availMem,
            thresholdBytes = mi.threshold,
            lowMemory = mi.lowMemory,
            heldBytes = heldBytes,
            budgetBytes = budget(mi.availMem, mi.threshold, mi.totalMem, heldBytes),
        )
    }

    /** Memory a GGUF needs: plan section 3, `size x 1.2 + 0.4 GB` (no ram_est in manifests). */
    fun needBytes(fileBytes: Long): Long = fileBytes * 12 / 10 + 400_000_000L
}
