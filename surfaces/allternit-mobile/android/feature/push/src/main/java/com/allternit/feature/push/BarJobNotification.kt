package com.allternit.feature.push

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.app.PendingIntent
import android.os.Handler
import android.os.Looper
import androidx.glance.appwidget.updateAll
import com.allternit.feature.bar.BarDeepLink
import com.allternit.feature.bar.BarFlag
import com.allternit.feature.bar.BarJobState
import com.allternit.feature.bar.BarJobs
import com.allternit.feature.bar.BarWording
import com.allternit.feature.system.BarWidget
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/**
 * The ongoing notification that follows the island bar's job (Island bar + Team map, lane 37; the Android shape of the
 * iOS Live Activity, #3037). Own channel "Bar jobs", driven by [BarJobs.current]: title, the state line ("Waiting on
 * Noah", "Working"), a 4-step progress bar (Asked, Approval, Running, Done), tap opens the bar. On a terminal state it
 * shows the past-tense receipt ("Refunded EUR 320"), stops being ongoing and dismisses itself after a few seconds.
 * It is a standard ongoing notification: it does not use the Android 16 promoted-ongoing APIs. The status-snapshot
 * notification ([LiveRunNotification]) is a separate surface and is left as it was.
 */
object BarJobNotification {
    const val CHANNEL_BAR = "bar_jobs"
    private const val TAG = "bar-job"
    private const val ID = PushNotifications.NOTIFICATION_ID + 2
    private const val DISMISS_AFTER_MS = 6_000L

    private val started = AtomicBoolean(false)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val main = Handler(Looper.getMainLooper())
    private var dismiss: Runnable? = null

    /** App start (AllternitPush.start): collect the job for the life of the process. Idempotent. */
    fun start(context: Context) {
        if (!started.compareAndSet(false, true)) return
        val app = context.applicationContext
        scope.launch {
            BarJobs.current.collect { job ->
                update(app, job)
                runCatching { BarWidget().updateAll(app) }
            }
        }
    }

    fun update(context: Context, job: BarJobState?) {
        val nm = context.getSystemService(NotificationManager::class.java) ?: return
        dismiss?.let { main.removeCallbacks(it) }
        dismiss = null
        if (job == null || !BarFlag.isOn(context) || !AllternitPush.notificationsAllowed(context)) {
            nm.cancel(TAG, ID)
            return
        }
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_BAR, "Bar jobs", NotificationManager.IMPORTANCE_LOW).apply {
                description = "The thing you started from the island bar, until it is done."
                setShowBadge(false)
            },
        )
        val terminal = job.isTerminal
        val line = job.stateLine
        val builder = Notification.Builder(context, CHANNEL_BAR)
            .setSmallIcon(com.allternit.core.design.R.drawable.ph_bell)
            .setContentTitle(job.title.ifBlank { BarWording.entry })
            .setContentText(line)
            .setCategory(Notification.CATEGORY_PROGRESS)
            .setOngoing(!terminal)
            .setAutoCancel(terminal)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
            .setContentIntent(
                PendingIntent.getActivity(
                    context, ID, BarDeepLink.intent(context),
                    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
                ),
            )
        if (!terminal) {
            // Steps reached 1-4 (Asked, Approval, Running, Done) as a determinate bar.
            builder.setProgress(4, job.stepsReached.coerceIn(1, 4), false)
            job.progressLine?.takeIf { it.isNotBlank() }?.let { builder.setSubText(it) }
        }
        nm.notify(TAG, ID, builder.build())
        if (terminal) {
            val r = Runnable { nm.cancel(TAG, ID) }
            dismiss = r
            main.postDelayed(r, DISMISS_AFTER_MS)
        }
    }
}
