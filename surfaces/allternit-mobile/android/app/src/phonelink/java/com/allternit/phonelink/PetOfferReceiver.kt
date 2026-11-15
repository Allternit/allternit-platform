package com.allternit.phonelink

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import com.allternit.phonelink.core.PetOfferSettings

/** Pet-offer settings live in their own device-local prefs file (never synced, never backed up). */
object PetOfferPrefs {
    const val FILE = "pet_offers"

    fun settings(context: Context): PetOfferSettings =
        PetOfferSettings(PrefsStore(context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE)))
}

/**
 * The offer notification's Mute: adds the app (package, or the app's name when the offer came from a
 * system crash dialog) to the muted list and removes the notification. Not exported: only this app's
 * own PendingIntent reaches it.
 */
class PetOfferReceiver : BroadcastReceiver() {
    companion object {
        const val ACTION_MUTE = "com.allternit.phonelink.PET_OFFER_MUTE"
        private const val EXTRA_ENTRY = "entry"
        private const val EXTRA_NOTIFICATION_ID = "notification_id"

        fun muteIntent(context: Context, entry: String, notificationId: Int): Intent =
            Intent(context, PetOfferReceiver::class.java)
                .setAction(ACTION_MUTE)
                .putExtra(EXTRA_ENTRY, entry)
                .putExtra(EXTRA_NOTIFICATION_ID, notificationId)
    }

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != ACTION_MUTE) return
        val entry = intent.getStringExtra(EXTRA_ENTRY)?.takeIf { it.isNotBlank() }
        if (entry != null) PetOfferPrefs.settings(context).mute(entry)
        val id = intent.getIntExtra(EXTRA_NOTIFICATION_ID, -1)
        if (id >= 0) Notifications.cancelPetOffer(context, id)
        ServerController.notifyChanged()
    }
}
