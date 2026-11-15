package com.allternit.app

import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import com.google.android.play.core.review.ReviewManagerFactory
import com.google.androidbrowserhelper.trusted.LauncherActivity

/**
 * The Google Play build's launcher: androidbrowserhelper's LauncherActivity
 * (m.allternit.com as a Trusted Web Activity) plus the Play in-app review sheet.
 * The web content cannot call Play In-App Review, so the ask happens here,
 * natively, before the TWA opens. The phonelink build keeps the stock launcher.
 *
 * Rules:
 *  - Every fresh open counts toward [ReviewPolicy]. The sheet is only requested
 *    on a plain launcher open (ACTION_MAIN), never for a share, a deep link or a
 *    long-press shortcut, so those open their target with no delay.
 *  - [launchTwa] runs exactly once per activity instance: after the review flow
 *    completes, when it fails, or when getting the flow takes longer than
 *    [REQUEST_TIMEOUT_MS]. Once the sheet is on screen, [SHEET_TIMEOUT_MS] is the
 *    safety net (opening the TWA on top would cut off someone mid-rating).
 *  - Recreated (rotation mid-review, or restored after process death): open the
 *    TWA right away and never ask again; the base class already finishes when
 *    its saved state says the browser was launched. The old instance's
 *    callbacks and timeout are dropped once it is destroyed.
 */
class ReviewLauncherActivity : LauncherActivity() {

    private val handler = Handler(Looper.getMainLooper())
    private var twaLaunchRequested = false
    private var reviewInFlight = false
    private var recreated = false
    private val timeout = Runnable {
        if (reviewInFlight) {
            // Don't hold every later open behind a slow Play Store either.
            policy().markAsked()
            Log.d(TAG, "Review flow timed out, opening the app")
        }
        launchTwaOnce()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        recreated = savedInstanceState != null
        // Calls shouldLaunchImmediately(), unless the base class finishes early
        // (app already open, restart in a new task): those are not counted.
        super.onCreate(savedInstanceState)
    }

    override fun shouldLaunchImmediately(): Boolean {
        if (recreated) return true
        val policy = policy()
        policy.recordLaunch()
        if (intent?.action != Intent.ACTION_MAIN || intent?.data != null) return true
        if (!policy.shouldAsk()) return true
        startReviewFlow(policy)
        return false
    }

    private fun startReviewFlow(policy: ReviewPolicy) {
        reviewInFlight = true
        handler.postDelayed(timeout, REQUEST_TIMEOUT_MS)
        val manager = try {
            ReviewManagerFactory.create(this)
        } catch (e: RuntimeException) {
            Log.w(TAG, "Review manager unavailable", e)
            launchTwaOnce()
            return
        }
        manager.requestReviewFlow().addOnCompleteListener { request ->
            if (twaLaunchRequested || isDestroyed || isFinishing) return@addOnCompleteListener
            if (!request.isSuccessful) {
                Log.d(TAG, "requestReviewFlow failed", request.exception)
                launchTwaOnce()
                return@addOnCompleteListener
            }
            // The sheet is about to show: swap the short timeout for the safety net.
            handler.removeCallbacks(timeout)
            handler.postDelayed(timeout, SHEET_TIMEOUT_MS)
            policy.markAsked()
            manager.launchReviewFlow(this, request.result).addOnCompleteListener {
                // Play never says whether the sheet showed or what was chosen.
                if (!isDestroyed) launchTwaOnce()
            }
        }
    }

    /** Single-fire guard around the base class's [launchTwa]. */
    private fun launchTwaOnce() {
        if (twaLaunchRequested || isDestroyed) return
        twaLaunchRequested = true
        reviewInFlight = false
        handler.removeCallbacks(timeout)
        launchTwa()
    }

    override fun onDestroy() {
        handler.removeCallbacks(timeout)
        super.onDestroy()
    }

    private fun policy() = ReviewPolicy(
        ReviewPolicy.SharedPreferencesStore(
            getSharedPreferences(ReviewPolicy.PREFS_NAME, Context.MODE_PRIVATE),
        ),
    )

    private companion object {
        const val TAG = "ReviewLauncher"
        const val REQUEST_TIMEOUT_MS = 3_000L
        const val SHEET_TIMEOUT_MS = 60_000L
    }
}
