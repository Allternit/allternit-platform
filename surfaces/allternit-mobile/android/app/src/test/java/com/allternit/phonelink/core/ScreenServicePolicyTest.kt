package com.allternit.phonelink.core

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ScreenServicePolicyTest {
    private fun enable(up: Boolean = false, key: Boolean = false, wifi: Boolean = true, secure: Boolean = true, on: Boolean = false) =
        ScreenServicePolicy.shouldEnableWirelessDebugging(up, key, wifi, secure, on)

    @Test fun turnsWirelessDebuggingOnOnlyWhenDesktopIsTheWayIn() {
        assertTrue(enable())
        assertFalse("server already up", enable(up = true))
        assertFalse("own key: the app launches it itself", enable(key = true))
        assertFalse("off Wi-Fi", enable(wifi = false))
        assertFalse("no WRITE_SECURE_SETTINGS", enable(secure = false))
        assertFalse("already on (the user's)", enable(on = true))
    }

    @Test fun turnsItBackOffOnlyWhenTheAppTurnedItOn() {
        assertTrue(ScreenServicePolicy.shouldDisableWirelessDebugging(serverUp = true, enabledByUs = true, canWriteSecureSettings = true))
        assertFalse(ScreenServicePolicy.shouldDisableWirelessDebugging(serverUp = true, enabledByUs = false, canWriteSecureSettings = true))
        assertFalse(ScreenServicePolicy.shouldDisableWirelessDebugging(serverUp = false, enabledByUs = true, canWriteSecureSettings = true))
        assertFalse(ScreenServicePolicy.shouldDisableWirelessDebugging(serverUp = true, enabledByUs = true, canWriteSecureSettings = false))
    }
}
