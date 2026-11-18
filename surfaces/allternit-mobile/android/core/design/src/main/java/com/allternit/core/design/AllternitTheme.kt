package com.allternit.core.design

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.staticCompositionLocalOf

/** Appearance setting. Light is the default (Eoj: light default on phone). */
enum class ThemeMode { Light, Dark, System }

val LocalAllternitColors = staticCompositionLocalOf { AllternitColors.Light }

/**
 * Root theme for every native screen. Colors come from the generated
 * [AllternitColors] (all iOS colorsets; see contracts/design-tokens). The system
 * dark setting is followed only when [themeMode] is [ThemeMode.System].
 * Material3 is only the carrier for text and component defaults; Allternit
 * screens read [AllternitTheme.colors].
 */
@Composable
fun AllternitTheme(
    themeMode: ThemeMode = ThemeMode.Light,
    content: @Composable () -> Unit,
) {
    val dark = when (themeMode) {
        ThemeMode.Light -> false
        ThemeMode.Dark -> true
        ThemeMode.System -> isSystemInDarkTheme()
    }
    val c = if (dark) AllternitColors.Dark else AllternitColors.Light
    val scheme = if (dark) {
        darkColorScheme(
            primary = c.accentPrimary, onPrimary = c.onAccent,
            background = c.bgPrimary, onBackground = c.textPrimary,
            surface = c.bgPrimary, onSurface = c.textPrimary,
            surfaceVariant = c.bgTertiary, onSurfaceVariant = c.textSecondary,
            outline = c.borderStrong, outlineVariant = c.borderSubtle,
        )
    } else {
        lightColorScheme(
            primary = c.accentPrimary, onPrimary = c.onAccent,
            background = c.bgPrimary, onBackground = c.textPrimary,
            surface = c.bgPrimary, onSurface = c.textPrimary,
            surfaceVariant = c.bgTertiary, onSurfaceVariant = c.textSecondary,
            outline = c.borderStrong, outlineVariant = c.borderSubtle,
        )
    }
    CompositionLocalProvider(LocalAllternitColors provides c) {
        MaterialTheme(colorScheme = scheme, content = content)
    }
}

object AllternitTheme {
    val colors: AllternitColorScheme
        @Composable get() = LocalAllternitColors.current
}
