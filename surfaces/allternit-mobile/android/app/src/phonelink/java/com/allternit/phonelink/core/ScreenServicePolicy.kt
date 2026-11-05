package com.allternit.phonelink.core

/**
 * When the app touches Wireless debugging for the screen service (the shell
 * server, ShellServerMain). Pure, so it is unit-tested; ShellServerLauncher
 * applies it.
 *
 * Without its own trusted adb key the app can't start the server itself:
 * Allternit Desktop starts it over its existing adb connection (USB, or home
 * Wireless debugging with the Mac's trusted key). After a reboot the server is
 * gone, so while it is down, off the cable and on Wi-Fi, an app holding
 * WRITE_SECURE_SETTINGS turns Wireless debugging on so Desktop's adb lane can
 * reconnect at home. Once the server runs again it turns Wireless debugging
 * back off, but only when it was the one that turned it on.
 */
object ScreenServicePolicy {
    /** Prefs flag: the app turned Wireless debugging on and owes the user turning it back off. */
    const val PREF_WIFI_ADB_BY_US = "screen_wifi_adb_by_us"

    /** Seconds between local pings while waiting for Desktop to start the server. */
    const val WAIT_POLL_SECONDS = 30L

    fun shouldEnableWirelessDebugging(
        serverUp: Boolean,
        hasOwnKey: Boolean,
        onWifi: Boolean,
        canWriteSecureSettings: Boolean,
        wirelessDebuggingOn: Boolean,
    ): Boolean = !serverUp && !hasOwnKey && onWifi && canWriteSecureSettings && !wirelessDebuggingOn

    fun shouldDisableWirelessDebugging(serverUp: Boolean, enabledByUs: Boolean, canWriteSecureSettings: Boolean): Boolean =
        serverUp && enabledByUs && canWriteSecureSettings
}
