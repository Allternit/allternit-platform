package com.allternit.phonelink

import android.view.accessibility.AccessibilityEvent

/**
 * Google Play build's part of [PhoneLinkAccessibilityService]: nothing beyond
 * the shared service. No local server, no adb lane and no error-help reading
 * here; the service only carries out a viewer's taps and keys on the
 * screen-share lane (AppLink). Same members as src/phonelink's version.
 */
class FlavorAccessibility(private val service: PhoneLinkAccessibilityService) {
    fun connected() {}
    fun disconnected() {}
    fun shutdown() {}
    fun screenHeightPx(): Int = ScreenMetrics.real(service).height
    @Suppress("UNUSED_PARAMETER")
    fun onWindowStateChanged(event: AccessibilityEvent) {}
}
