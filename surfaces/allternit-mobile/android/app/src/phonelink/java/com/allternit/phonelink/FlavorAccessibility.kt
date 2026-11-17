package com.allternit.phonelink

import android.os.SystemClock
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo
import com.allternit.phonelink.core.PetNotice
import com.allternit.phonelink.core.PetNoticeGate
import com.allternit.phonelink.core.UiNode
import java.util.concurrent.Executors

/**
 * Sideloaded build's part of [PhoneLinkAccessibilityService] (the service
 * itself is shared with the Google Play build, src/phoneshare): it starts the
 * local Phone Link server when Android connects the service and runs the
 * "Error help" pet offers. The Google Play build has its own, empty
 * FlavorAccessibility (src/play) with the same members.
 */
class FlavorAccessibility(private val service: PhoneLinkAccessibilityService) {
    /** Pet offers read and match off the main thread, one dialog at a time. */
    private val petExecutor = Executors.newSingleThreadExecutor()
    private val petGate by lazy { PetNoticeGate(PetOfferPrefs.settings(service), ownPackage = service.packageName) }

    companion object {
        /** A paired computer that sent a command this recently is driving the phone: pet offers stay quiet. */
        private const val REMOTE_DRIVING_MS = 60_000L
        /** Android's own crash and not-responding dialogs come from this package. */
        private const val SYSTEM_PACKAGE = "android"
        /** Never read here (same screens Phone Link never acts on), besides the block list. */
        private val PET_SKIP_PACKAGES = setOf(
            "com.android.settings",
            "com.android.permissioncontroller",
            "com.google.android.permissioncontroller",
            "com.android.packageinstaller",
            "com.google.android.packageinstaller",
            "com.android.systemui",
        )
    }

    fun connected() = ServerController.onAccessibilityConnected(service, service)

    fun disconnected() = ServerController.onAccessibilityDisconnected(service)

    fun shutdown() { petExecutor.shutdownNow() }

    fun screenHeightPx(): Int = ServerController.staticInfo().heightPx

    // ── Pet offers ──────────────────────────────────────────────────────────

    /**
     * An app shows an error dialog: read it (password and editable fields never), match it against the
     * contract rules and post the "Error help" notification. Order: the switch, lock screen, a paired
     * computer driving the phone, the block list and mute (before any read), the 30 s re-read skip, then
     * the read, the match and the cooldown. Screen text is never logged.
     */
    fun onWindowStateChanged(event: AccessibilityEvent) {
        val className = event.className?.toString()
        if (!PetNotice.isDialogClass(className)) return
        val settings = PetOfferPrefs.settings(service)
        if (!settings.watching) return
        if (service.isScreenLocked() || isRemoteDriving()) return
        val pkg = event.packageName?.toString() ?: return
        if (pkg in PET_SKIP_PACKAGES) return
        val system = pkg == SYSTEM_PACKAGE
        val label = if (system) null else appLabel(pkg)
        if (!system && petGate.blockReason(label, pkg) != null) return
        if (!petGate.shouldRead("$pkg|$className|${event.windowId}", SystemClock.uptimeMillis())) return
        // The event is recycled when this returns: take the dialog's root node now.
        val source = event.source
        val windowId = event.windowId
        petExecutor.execute {
            try { readAndOffer(source, windowId, pkg, label, className!!, system, settings.maxTextChars) } catch (_: Exception) {}
        }
    }

    private fun readAndOffer(source: AccessibilityNodeInfo?, windowId: Int, pkg: String, label: String?, className: String, system: Boolean, maxTextChars: Int) {
        // Re-check on this thread: a command could have arrived since the event.
        if (isRemoteDriving()) return
        // The whole dialog window; the event's own node when the window list doesn't have it.
        val root = (try { service.windows } catch (e: Exception) { emptyList<AccessibilityWindowInfo>() }).firstOrNull { it.id == windowId }?.root
            ?: source
            ?: return
        val nodes = ArrayList<UiNode>()
        service.walk(root, 1, nodes)
        val text = PetNotice.textOf(nodes, maxTextChars)
        val trigger = PetNotice.match(text, dialog = true, maxTextChars = maxTextChars) ?: return
        // A system crash dialog is about the app it names ("Chrome keeps stopping"): check that name too.
        val app = if (system) PetNotice.systemDialogApp(trigger.line) else label
        val appId = if (system) null else pkg
        if (system && petGate.blockReason(app, null) != null) return
        val muteEntry = appId ?: app ?: return
        val windowKey = "${muteEntry.lowercase()}|$className"
        if (!petGate.allowOffer(windowKey, trigger.line, System.currentTimeMillis())) return
        val settings = PetOfferPrefs.settings(service)
        if (!settings.watching) return
        Notifications.showPetOffer(service, PetNotice.fixItUrl(app, appId, trigger, text), muteEntry, settings.cooldownMs)
    }

    private fun appLabel(pkg: String): String? = try {
        val pm = service.packageManager
        pm.getApplicationLabel(pm.getApplicationInfo(pkg, 0)).toString()
    } catch (e: Exception) { null }

    /** True while a paired computer is using the phone (any session sent a command in the last minute). */
    private fun isRemoteDriving(): Boolean = try {
        val now = System.currentTimeMillis()
        ServerController.auth.activeSessions.any { now - it.lastActiveAtMs < REMOTE_DRIVING_MS }
    } catch (e: UninitializedPropertyAccessException) {
        false
    }
}
