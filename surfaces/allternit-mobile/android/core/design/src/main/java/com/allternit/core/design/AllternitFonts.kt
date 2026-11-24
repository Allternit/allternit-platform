package com.allternit.core.design

import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight

/**
 * The serif used for headlines (iOS uses the system serif, New York).
 * Bundled (Noto Serif, OFL) because AllternitSerif falls back to the sans on some phones
 * (seen on the Moto g play 2024: "What are we working on?" rendered in the sans).
 */
val AllternitSerif: FontFamily = FontFamily(
    Font(R.font.noto_serif, FontWeight.Normal),
)
