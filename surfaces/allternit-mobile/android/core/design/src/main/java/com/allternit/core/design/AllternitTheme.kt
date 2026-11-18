package com.allternit.core.design

import android.os.Build
import android.view.HapticFeedbackConstants
import android.view.View
import androidx.compose.animation.core.SpringSpec
import androidx.compose.animation.core.spring
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color

/**
 * The three core colors, copied from the iOS asset catalog
 * (`ios/Assets.xcassets/{BgPrimary,TextPrimary,AccentPrimary}.colorset`).
 * Step 5 replaces this with the generated `AllternitTokens.gen.kt` (all 17 colorsets).
 */
@Immutable
data class AllternitColors(
    val bgPrimary: Color,
    val textPrimary: Color,
    val accentPrimary: Color,
) {
    companion object {
        val Light = AllternitColors(
            bgPrimary = Color(0xFFFFFFFF),
            textPrimary = Color(0xFF1F1E1D),
            accentPrimary = Color(0xFF1F1E1D),
        )
        val Dark = AllternitColors(
            bgPrimary = Color(0xFF1F1F1E),
            textPrimary = Color(0xFFECECEC),
            accentPrimary = Color(0xFFECECEC),
        )
    }
}

val LocalAllternitColors = staticCompositionLocalOf { AllternitColors.Light }

/**
 * Root theme for every native screen. Light is the default (Eoj: light default on
 * phone); callers pass `darkTheme = true` from the user's appearance setting.
 * Material3 is only the carrier for text and component defaults; Allternit
 * screens read [AllternitTheme.colors].
 */
@Composable
fun AllternitTheme(
    darkTheme: Boolean = false,
    content: @Composable () -> Unit,
) {
    val colors = if (darkTheme) AllternitColors.Dark else AllternitColors.Light
    val scheme = if (darkTheme) {
        darkColorScheme(
            primary = colors.accentPrimary,
            onPrimary = colors.bgPrimary,
            background = colors.bgPrimary,
            onBackground = colors.textPrimary,
            surface = colors.bgPrimary,
            onSurface = colors.textPrimary,
        )
    } else {
        lightColorScheme(
            primary = colors.accentPrimary,
            onPrimary = colors.bgPrimary,
            background = colors.bgPrimary,
            onBackground = colors.textPrimary,
            surface = colors.bgPrimary,
            onSurface = colors.textPrimary,
        )
    }
    CompositionLocalProvider(LocalAllternitColors provides colors) {
        MaterialTheme(colorScheme = scheme, content = content)
    }
}

object AllternitTheme {
    val colors: AllternitColors
        @Composable get() = LocalAllternitColors.current
}

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
