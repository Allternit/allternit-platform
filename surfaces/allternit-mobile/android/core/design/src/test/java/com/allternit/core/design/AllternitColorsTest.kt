package com.allternit.core.design

import androidx.compose.ui.graphics.Color
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Test

/** Guards drift in the iOS colorset names that the generated tokens must carry. */
class AllternitColorsTest {
    private val expected = listOf(
        "AccentBrowser", "AccentChat", "AccentCode", "AccentCowork", "AccentPrimary", "BgPanel",
        "BgPrimary", "BgSecondary", "BgTertiary", "BorderStrong", "BorderSubtle", "BrandAccent",
        "FillNeutral", "OnAccent", "TextPrimary", "TextSecondary", "TextTertiary",
    )

    @Test fun namesMatchTheSeventeenIosColorsets() {
        assertEquals(17, expected.size)
        assertEquals(expected, AllternitColors.names.sorted())
    }

    @Test fun lightAndDarkHaveEveryColor() {
        for ((label, scheme) in listOf("Light" to AllternitColors.Light, "Dark" to AllternitColors.Dark)) {
            val map = scheme.asMap()
            assertEquals("$label keys", expected, map.keys.sorted())
            for (name in expected) assertNotEquals("$label $name unset", Color.Unspecified, map.getValue(name))
        }
    }
}
