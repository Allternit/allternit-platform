package com.allternit.feature.artifacts

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.res.painterResource

/**
 * The "Make something new" tile art (iOS ArtifactCreateTileArt): a cream stage with a dot grid and
 * the rest-state scene of Docs, Sheets and Slides, drawn in code from the same coordinates (220 x 154
 * stage, scaled to the tile). Other kinds show their untinted deck icon on the same stage.
 */
@Composable
internal fun ArtifactTileArt(kind: StudioKind, modifier: Modifier = Modifier) {
    if (kind != StudioKind.Docs && kind != StudioKind.Sheets && kind != StudioKind.Slides) {
        Image(painterResource(kind.deck), null, modifier)
        return
    }
    Canvas(modifier.fillMaxSize()) {
        val sx = size.width / 220f
        val sy = size.height / 154f
        scale(sx, sy, pivot = Offset.Zero) {
            drawRect(CREAM, size = Size(220f, 154f))
            var y = 6f
            while (y < 154f) { var x = 6f; while (x < 220f) { drawCircle(INK.copy(alpha = 0.07f), 1.1f, Offset(x, y)); x += 12f }; y += 12f }
            when (kind) {
                StudioKind.Docs -> {
                    val k = Color(0xFF2F6FD0)
                    page(36f, 16f, 120f, 150f, k)
                    listOf(38f to 70f, 51f to 86f, 64f to 78f, 77f to 82f, 90f to 52f).forEachIndexed { i, (yy, w) ->
                        box(48f, yy, w, 5f, if (i == 0) INK else INK.copy(alpha = 0.2f))
                    }
                }
                StudioKind.Sheets -> {
                    val k = Color(0xFF16884F)
                    page(30f, 18f, 130f, 118f, k)
                    val filled = setOf(1 to 0, 3 to 1, 0 to 2, 4 to 3, 2 to 4, 1 to 5)
                    for (r in 0 until 6) for (q in 0 until 5) box(40f + q * 24f, 32f + r * 16f, 18f, 10f, if ((q to r) in filled) k else INK.copy(alpha = 0.2f))
                }
                else -> {
                    val k = Color(0xFFD2542F)
                    listOf(48f to 34f, 58f to 44f, 68f to 54f).forEach { (x, yy) ->
                        page(x, yy, 104f, 64f, k)
                        box(x + 10f, yy + 14f, 46f, 5f, INK)
                        box(x + 10f, yy + 26f, 84f, 26f, k.copy(alpha = 0.25f))
                    }
                }
            }
        }
    }
}

private val CREAM = Color(0xFFFDF8F3)
private val INK = Color(0xFF141413)

private fun DrawScope.box(x: Float, y: Float, w: Float, h: Float, color: Color) =
    drawRoundRect(color, Offset(x, y), Size(w, h), CornerRadius(3f))

/** A white page with a soft shadow and the coloured top edge. */
private fun DrawScope.page(x: Float, y: Float, w: Float, h: Float, k: Color) {
    drawRoundRect(Color.Black.copy(alpha = 0.05f), Offset(x, y + 3f), Size(w, h), CornerRadius(8f))
    drawRoundRect(Color.White, Offset(x, y), Size(w, h), CornerRadius(8f))
    drawRoundRect(k, Offset(x, y), Size(w, 5f), CornerRadius(8f, 8f))
    drawRect(k, Offset(x, y + 3f), Size(w, 2f))
}
