package com.allternit.phonelink

import android.app.Activity
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import com.allternit.phonelink.core.AutoStart
import com.allternit.phonelink.core.ProvisionCommand

/** Brings the server back after a reboot or an app update, with no user action. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            Intent.ACTION_BOOT_COMPLETED, Intent.ACTION_MY_PACKAGE_REPLACED, "android.intent.action.QUICKBOOT_POWERON" -> Unit
            else -> return
        }
        ServerController.init(context)
        if (!AutoStart.shouldStart(ServerController.settings)) {
            ServerController.log("boot: not starting (provisioned=${ServerController.settings.provisioned}, stopped=${ServerController.settings.userStopped})")
            return
        }
        ServerController.log("boot: starting (${intent.action})")
        // BOOT_COMPLETED is an allowed context for starting a foreground service (also on
        // Android 15 for the specialUse type). The accessibility service, which the system
        // rebinds on its own after unlock, starts the server too, whichever comes first.
        PhoneLinkService.start(context)
    }
}

/**
 * One-time setup over the cable. Only senders holding android.permission.DUMP
 * can reach it, i.e. `adb shell` and the system; no app can arm a pairing.
 *
 *   adb shell am broadcast -a com.allternit.phonelink.PROVISION \
 *     -n com.allternit.phonelink/.ProvisionReceiver \
 *     --ez arm_pairing true --ez allow_remote_access true --es device_name "Joe's moto"
 */
class ProvisionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != ProvisionCommand.ACTION) return
        ServerController.init(context)
        val extras = HashMap<String, Any?>()
        intent.extras?.let { b ->
            for (k in b.keySet()) {
                @Suppress("DEPRECATION")
                extras[k] = b.get(k)
            }
        }
        val now = System.currentTimeMillis()
        val cmd = ProvisionCommand.parse(extras).getOrElse { e ->
            ServerController.log("provision rejected: ${e.message}")
            if (isOrderedBroadcast) { resultCode = Activity.RESULT_CANCELED; resultData = "error: ${e.message}" }
            return
        }
        val s = ServerController.settings
        val wasRemote = s.allowRemote
        val oldPort = s.port
        cmd.applyTo(s, now)
        cmd.meshAuthKey?.let { ServerController.storeMeshAuthKey(it) }
        if (ServerController.isRunning && (wasRemote != s.allowRemote || oldPort != s.port)) {
            ServerController.restartServer()
        } else {
            ServerController.startServer()
            ServerController.syncMesh()
        }
        PhoneLinkService.start(context)
        val summary = cmd.summary(s, now)
        ServerController.log(summary)
        if (isOrderedBroadcast) { resultCode = Activity.RESULT_OK; resultData = summary }
        ServerController.notifyChanged()
    }
}

/** The notification's Stop: kills the server immediately and keeps it off. */
class StopReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        ServerController.stopByUser(context)
    }
}
