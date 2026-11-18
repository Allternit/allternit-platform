package com.allternit.core.design

import android.os.Build
import android.view.HapticFeedbackConstants
import android.view.View
import androidx.compose.animation.core.SpringSpec
import androidx.compose.animation.core.spring

/**
 * Motion shared with iOS. iOS `spring(response: 0.35, dampingFraction: 0.86)` is
 * stiffness (2π / 0.35)² ≈ 322 with the same damping ratio.
 */
object Motion {
    const val STANDARD_DAMPING_RATIO = 0.86f
    const val STANDARD_STIFFNESS = 322f

    /** The default spring for moves, sheets and drawer settles. */
    fun <T> standardSpring(): SpringSpec<T> =
        spring(dampingRatio = STANDARD_DAMPING_RATIO, stiffness = STANDARD_STIFFNESS)
}

/** Haptics shared with iOS (light impact, success and warning notifications). */
object Haptics {
    enum class Kind { Tap, Success, Warning }

    /**
     * The [HapticFeedbackConstants] value for [kind]. CONFIRM and REJECT exist from
     * API 30; older phones (minSdk 26) get the closest older constants.
     */
    fun constantFor(kind: Kind, sdkInt: Int = Build.VERSION.SDK_INT): Int = when (kind) {
        Kind.Tap -> HapticFeedbackConstants.VIRTUAL_KEY
        Kind.Success ->
            if (sdkInt >= Build.VERSION_CODES.R) HapticFeedbackConstants.CONFIRM
            else HapticFeedbackConstants.KEYBOARD_TAP
        Kind.Warning ->
            if (sdkInt >= Build.VERSION_CODES.R) HapticFeedbackConstants.REJECT
            else HapticFeedbackConstants.LONG_PRESS
    }

    /** Plays [kind] on [view] (in Compose: `LocalView.current`). Honors the system haptics setting. */
    fun perform(view: View, kind: Kind): Boolean = view.performHapticFeedback(constantFor(kind))
}
