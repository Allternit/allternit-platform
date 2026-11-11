package com.allternit.app

import android.content.SharedPreferences

/**
 * When to show the Google Play in-app review sheet. Pure logic over three
 * stored values and an injected clock, so it runs in plain JVM tests.
 *
 * Ask only when all hold:
 *  - the app has been opened at least [MIN_LAUNCHES] times,
 *  - the first open was at least [MIN_DAYS_SINCE_FIRST_LAUNCH] days ago,
 *  - it has never asked, or the last ask was at least [MIN_DAYS_BETWEEN_ASKS] days ago.
 *
 * Play also rate-limits the sheet on its side, so "ask" means "request the
 * flow", never "the sheet will show".
 */
class ReviewPolicy(
    private val store: Store,
    private val now: () -> Long = System::currentTimeMillis,
) {

    /** The persisted values. [SharedPreferencesStore] is the real one. */
    interface Store {
        var launchCount: Int
        /** Epoch millis of the first recorded launch, 0 = none yet. */
        var firstLaunchAt: Long
        /** Epoch millis of the last review request, 0 = never asked. */
        var lastAskedAt: Long
    }

    /** Counts one app open. Sets the first-launch time on the first call. */
    fun recordLaunch() {
        if (store.firstLaunchAt <= 0L) store.firstLaunchAt = now()
        if (store.launchCount < Int.MAX_VALUE) store.launchCount = store.launchCount + 1
    }

    fun shouldAsk(): Boolean {
        val t = now()
        if (store.launchCount < MIN_LAUNCHES) return false
        val first = store.firstLaunchAt
        if (first <= 0L || t - first < MIN_DAYS_SINCE_FIRST_LAUNCH * DAY_MS) return false
        val last = store.lastAskedAt
        return last <= 0L || t - last >= MIN_DAYS_BETWEEN_ASKS * DAY_MS
    }

    /** Records that the review flow was requested now (starts the 120-day wait). */
    fun markAsked() {
        store.lastAskedAt = now()
    }

    class SharedPreferencesStore(private val prefs: SharedPreferences) : Store {
        override var launchCount: Int
            get() = prefs.getInt(KEY_LAUNCH_COUNT, 0)
            set(v) { prefs.edit().putInt(KEY_LAUNCH_COUNT, v).apply() }
        override var firstLaunchAt: Long
            get() = prefs.getLong(KEY_FIRST_LAUNCH_AT, 0L)
            set(v) { prefs.edit().putLong(KEY_FIRST_LAUNCH_AT, v).apply() }
        override var lastAskedAt: Long
            get() = prefs.getLong(KEY_LAST_ASKED_AT, 0L)
            set(v) { prefs.edit().putLong(KEY_LAST_ASKED_AT, v).apply() }
    }

    companion object {
        const val PREFS_NAME = "allternit_review_prompt"
        const val KEY_LAUNCH_COUNT = "launch_count"
        const val KEY_FIRST_LAUNCH_AT = "first_launch_at"
        const val KEY_LAST_ASKED_AT = "last_asked_at"

        const val MIN_LAUNCHES = 5
        const val MIN_DAYS_SINCE_FIRST_LAUNCH = 3L
        const val MIN_DAYS_BETWEEN_ASKS = 120L
        const val DAY_MS = 24L * 60L * 60L * 1000L
    }
}
