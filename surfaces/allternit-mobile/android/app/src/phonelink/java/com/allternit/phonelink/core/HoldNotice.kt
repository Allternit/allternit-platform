package com.allternit.phonelink.core

import java.time.Instant

/**
 * The "waiting for approval" notice Allternit Desktop asks this phone to show while
 * a sensitive action is held for the owner (`hold_notice`). Words only: what kind
 * of action and which app. Never screen content or typed text.
 */
object HoldNotice {
    /** Longest the notice may stay if Desktop never says `cleared` (holds last 10 minutes). */
    const val MAX_MS = 11 * 60 * 1000L

    /**
     * The link lease. Allternit Desktop repeats `waiting` about every 15 s while a hold is open, and each
     * one restarts the timer. If the link drops (Desktop closed, phone off its network, heartbeat lost)
     * the repeats stop and the notice removes itself within this long, even though the helper service
     * keeps running. When the link comes back, Desktop's next `waiting` shows it again.
     */
    const val LEASE_MS = 45_000L
    private const val MIN_MS = 5_000L

    fun effectPhrase(effect: String?): String = when (effect) {
        "pay" -> "spend money"
        "delete" -> "delete something"
        "send" -> "send a message"
        "call" -> "place a call"
        "credential" -> "enter a password or a code"
        "money_app" -> "open a banking or payment app"
        "security_setting" -> "change a security setting"
        else -> "do something that needs your OK"
    }

    /** Body line, e.g. "Your bot wants to send a message in WhatsApp." */
    fun body(effect: String?, appLabel: String?): String =
        "Your bot wants to ${effectPhrase(effect)}${if (appLabel.isNullOrBlank()) "" else " in $appLabel"}. Answer it in Allternit on your computer or any signed-in device."

    /**
     * How long to keep the notice when Desktop says nothing more: until [expiresAt]
     * (ISO-8601) plus a little, never longer than [MAX_MS] (when the time is missing or
     * unreadable, [MAX_MS]), and never longer than the link lease [LEASE_MS].
     */
    fun timeoutMs(expiresAt: String?, nowMs: Long): Long = minOf(expiryMs(expiresAt, nowMs), LEASE_MS)

    private fun expiryMs(expiresAt: String?, nowMs: Long): Long {
        val at = try { expiresAt?.takeIf { it.isNotBlank() }?.let { Instant.parse(it).toEpochMilli() } } catch (e: Exception) { null }
            ?: return MAX_MS
        return (at - nowMs + 5_000L).coerceIn(MIN_MS, MAX_MS)
    }
}
