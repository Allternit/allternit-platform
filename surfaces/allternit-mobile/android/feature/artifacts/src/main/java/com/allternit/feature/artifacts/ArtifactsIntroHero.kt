package com.allternit.feature.artifacts

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.BlockText
import com.allternit.core.design.IntroKeys
import com.allternit.core.design.LocalIntroSeenStore
import com.allternit.core.design.IntroSheet
import com.allternit.core.design.PolishGlass
import com.allternit.core.design.rememberReducedMotion
import kotlinx.coroutines.launch

/** A small page with a coloured top edge: the neutral-tile art for Docs, Sheets and Slides. [kind] is `doc`, `sheet` or `slide`. */
@Composable
internal fun ArtifactMiniThumb(kind: String, modifier: Modifier = Modifier) {
    val ink = AllternitTheme.colors.textPrimary
    val grey = ink.copy(alpha = 0.16f)
    Canvas(modifier) {
        val w = size.width
        val h = size.height
        val r = CornerRadius(w * 0.06f)
        val accent = when (kind) { "doc" -> Color(0xFF2F6FD0); "sheet" -> Color(0xFF16884F); else -> Color(0xFFD2542F) }
        drawRoundRect(Color.White, Offset.Zero, size, r)
        drawRoundRect(accent, Offset.Zero, Size(w, h * 0.09f), r)
        val pad = w * 0.12f
        when (kind) {
            "doc" -> listOf(0.80f, 0.95f, 0.70f, 0.60f).forEachIndexed { i, f ->
                drawRoundRect(if (i == 0) ink else grey, Offset(pad, h * (0.22f + i * 0.14f)), Size((w - pad * 2) * f, h * 0.07f), CornerRadius(h * 0.03f))
            }
            "sheet" -> {
                val on = setOf(2, 5, 11, 14)
                val cw = (w - pad * 2) / 4f
                val ch = h * 0.15f
                for (i in 0 until 16) drawRoundRect(
                    if (i in on) Color(0xFF15803D) else Color(0xFFE5E5E5),
                    Offset(pad + (i % 4) * cw, h * 0.22f + (i / 4) * (ch + h * 0.04f)), Size(cw * 0.86f, ch), CornerRadius(h * 0.02f),
                )
            }
            else -> {
                drawRoundRect(ink, Offset(pad, h * 0.25f), Size((w - pad * 2) * 0.7f, h * 0.11f), CornerRadius(h * 0.04f))
                drawRoundRect(accent.copy(alpha = 0.18f), Offset(pad, h * 0.45f), Size(w - pad * 2, h * 0.4f), CornerRadius(h * 0.05f))
            }
        }
    }
}

/**
 * The moving copy of the real Artifacts page for the first-visit sheet: the Docs tile is tapped,
 * then three artifacts appear under "Your artifacts". Loops every 5.6 s; shows the end state under reduced motion.
 */
@Composable
internal fun ArtifactsIntroHero(modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    val reduced = rememberReducedMotion()
    val clock = remember { Animatable(if (reduced) 5600f else 0f) }
    LaunchedEffect(reduced) {
        if (reduced) { clock.snapTo(5600f); return@LaunchedEffect }
        while (true) {
            clock.snapTo(0f)
            clock.animateTo(5600f, tween(5600, easing = LinearEasing))
        }
    }
    val mine = listOf(Triple("Q4 plan", "Doc", "doc"), Triple("Budget 2027", "Sheet", "sheet"), Triple("Investor deck", "Slides", "slide"))
    Column(
        modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(Color.White).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        Box(Modifier.padding(bottom = 3.dp)) { BlockText("ARTIFACTS", height = 8.dp) }
        Row { Text("Make something new", color = c.textSecondary, fontSize = 10.sp, fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f)); Text("12 types", color = c.textSecondary, fontSize = 10.sp) }
        Row(horizontalArrangement = Arrangement.spacedBy(7.dp)) {
            listOf("Docs" to "doc", "Sheets" to "sheet", "Slides" to "slide").forEachIndexed { i, (n, kd) ->
                Column(Modifier.weight(1f)) {
                    Box(
                        Modifier.fillMaxWidth().height(34.dp)
                            .graphicsLayer {
                                // Docs: a tap at 0.45 to 0.65 of the first 0.9 s.
                                val t = clock.value / 900f
                                val s = if (i == 0 && t in 0.5f..1f) 1f - 0.1f * (1f - kotlin.math.abs((t - 0.75f) / 0.25f)).coerceIn(0f, 1f) else 1f
                                scaleX = s; scaleY = s
                            }
                            .clip(RoundedCornerShape(10.dp)).background(PolishGlass.track()),
                        contentAlignment = Alignment.Center,
                    ) { ArtifactMiniThumb(kd, Modifier.width(18.dp).fillMaxHeight(0.7f)) }
                    Text(n, color = c.textPrimary, fontSize = 10.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 3.dp))
                }
            }
        }
        Text("Your artifacts", color = c.textSecondary, fontSize = 10.sp, fontWeight = FontWeight.Medium, modifier = Modifier.padding(top = 4.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(7.dp)) {
            mine.forEachIndexed { i, (name, type, kd) ->
                Column(
                    Modifier.weight(1f).graphicsLayer {
                        val p = ((clock.value - (1000f + i * 550f)) / 450f).coerceIn(0f, 1f)
                        alpha = p; translationY = (1f - p) * 14.dp.toPx(); scaleX = 0.9f + 0.1f * p; scaleY = scaleX
                    },
                ) {
                    Box(Modifier.fillMaxWidth().height(44.dp).clip(RoundedCornerShape(10.dp)).background(PolishGlass.track()).padding(5.dp)) {
                        ArtifactMiniThumb(kd, Modifier.fillMaxSize())
                    }
                    Text(name, color = c.textPrimary, fontSize = 10.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(top = 3.dp))
                    Text("$type · just now", color = c.textTertiary, fontSize = 9.sp, maxLines = 1)
                }
            }
        }
    }
}

/**
 * Shows the Artifacts first-visit sheet once per account (`introArtifactsSeenAt`). Nothing shows while signed out,
 * and an intro already seen on this device never waits on the network.
 */
@Composable
internal fun ArtifactsIntroHost(signedIn: Boolean) {
    val scope = rememberCoroutineScope()
    // The shell's one store; null while signed out, and then no intro shows.
    val store = LocalIntroSeenStore.current ?: return
    var show by remember { mutableStateOf(false) }
    LaunchedEffect(signedIn, store) {
        if (!signedIn || store.isSeen(IntroKeys.ARTIFACTS)) return@LaunchedEffect
        store.refresh()
        show = !store.isSeen(IntroKeys.ARTIFACTS)
    }
    if (show) {
        IntroSheet(
            title = "Make real files, not just replies.",
            line = "Docs, sheets, slides and pages you can edit, share and export.",
            eyebrow = { BlockText("ARTIFACTS", height = 8.dp, color = PolishGlass.Coral) },
            hero = { ArtifactsIntroHero() },
            onContinue = { show = false; scope.launch { store.markSeen(IntroKeys.ARTIFACTS) } },
        )
    }
}
