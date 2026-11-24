package com.allternit.feature.models

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
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
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.MeFlagsClient
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.BlockText
import com.allternit.core.design.IntroKeys
import com.allternit.core.design.IntroSeenStore
import com.allternit.core.design.IntroSheet
import com.allternit.core.design.PolishGlass
import com.allternit.core.design.rememberReducedMotion
import com.allternit.feature.artifacts.LibraryNet
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR

/** A round tag (licence, kind, size, "On this phone"). [green] is the success tint. */
@Composable
internal fun FwTag(text: String, modifier: Modifier = Modifier, tint: Color? = null, dot: Boolean = false, icon: Int? = null, fontSize: Float = 11f, bg: Color = AllternitTheme.colors.bgPrimary) {
    val c = AllternitTheme.colors
    val fg = tint ?: c.textSecondary
    Row(
        modifier.clip(CircleShape).background(if (tint != null) tint.copy(alpha = 0.14f) else bg)
            .padding(horizontal = 8.dp, vertical = 2.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        if (dot) Box(Modifier.size(5.dp).clip(CircleShape).background(Color(0xFF22A06B)))
        if (icon != null) Icon(painterResource(icon), null, tint = fg, modifier = Modifier.size((fontSize + 1).dp))
        Text(text, color = fg, fontSize = fontSize.sp, fontWeight = if (tint != null) FontWeight.SemiBold else FontWeight.Normal, maxLines = 1)
    }
}

/**
 * The moving copy of the real Freeweights rows for the first-visit sheet: Florence-2-base downloads
 * on a progress ring and gets an "On this phone" tag, then gpt2 is pulled to the computer (Pull turns
 * into Sent). Loops every 5.6 s; shows the end state under reduced motion.
 */
@Composable
internal fun FreeweightsIntroHero(modifier: Modifier = Modifier) {
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
    val t = clock.value
    val pct = ((t - 700f) / 1200f).coerceIn(0f, 1f)
    val sent = t >= 3000f
    val green = Color(0xFF15803D)
    Column(
        modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(Color.White).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        Box(Modifier.padding(bottom = 3.dp)) { BlockText("FREEWEIGHTS", height = 8.dp) }
        Row(
            Modifier.fillMaxWidth().clip(RoundedCornerShape(9.dp)).background(PolishGlass.track()).padding(horizontal = 8.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp),
        ) {
            Icon(painterResource(DesignR.drawable.ph_magnifying_glass), null, tint = c.textTertiary, modifier = Modifier.size(11.dp))
            Text("Search Freeweights", color = c.textTertiary, fontSize = 10.sp)
        }
        HeroRow("Florence-2-base", "microsoft", listOf("MIT", "text", "930.3 MB"), "FL", extra = {
            if (pct >= 1f) FwTag("On this phone", tint = green, icon = DesignR.drawable.ph_device_mobile, fontSize = 9f, bg = PolishGlass.track())
        }) {
            if (pct < 1f) ProgressRing(pct)
            else Icon(painterResource(DesignR.drawable.ph_caret_right), null, tint = c.textTertiary, modifier = Modifier.size(13.dp))
        }
        HeroRow("gpt2", "openai-community", listOf("MIT", "text", "5.63 GB"), "GP") {
            if (sent) Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(3.dp)) {
                Icon(painterResource(DesignR.drawable.ph_laptop), null, tint = green, modifier = Modifier.size(11.dp))
                Text("Sent", color = green, fontSize = 10.sp, fontWeight = FontWeight.SemiBold)
                Icon(painterResource(DesignR.drawable.ph_check), null, tint = green, modifier = Modifier.size(10.dp))
            } else Row(
                Modifier.clip(CircleShape).background(PolishGlass.track()).padding(horizontal = 8.dp, vertical = 3.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(3.dp),
            ) {
                Icon(painterResource(DesignR.drawable.ph_download_simple), null, tint = c.textPrimary, modifier = Modifier.size(10.dp))
                Text("Pull", color = c.textPrimary, fontSize = 10.sp, fontWeight = FontWeight.SemiBold)
            }
        }
    }
}

@Composable
private fun HeroRow(name: String, owner: String, tags: List<String>, mono: String, extra: @Composable () -> Unit = {}, trailing: @Composable () -> Unit) {
    val c = AllternitTheme.colors
    Row(Modifier.fillMaxWidth().padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Box(Modifier.size(26.dp).clip(RoundedCornerShape(8.dp)).background(PolishGlass.track()), Alignment.Center) {
            Text(mono, color = c.textPrimary, fontSize = 9.sp, fontWeight = FontWeight.Bold)
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
            Text(name, color = c.textPrimary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
            Text(owner, color = c.textSecondary, fontSize = 9.sp, maxLines = 1)
            Row(Modifier.padding(top = 2.dp), horizontalArrangement = Arrangement.spacedBy(3.dp), verticalAlignment = Alignment.CenterVertically) {
                tags.forEachIndexed { i, tag -> FwTag(tag, dot = i == 0, fontSize = 8.5f, bg = PolishGlass.track()) }
                extra()
            }
        }
        trailing()
    }
}

@Composable
private fun ProgressRing(progress: Float, size: Dp = 22.dp) {
    val c = AllternitTheme.colors
    val track = PolishGlass.track()
    val ink = c.textPrimary
    Box(Modifier.size(size), Alignment.Center) {
        Canvas(Modifier.size(size)) {
            val w = 2.5.dp.toPx()
            val inset = w / 2f
            val d = Size(this.size.width - w, this.size.height - w)
            drawArc(track, 0f, 360f, false, Offset(inset, inset), d, style = Stroke(w))
            drawArc(ink, -90f, 360f * progress, false, Offset(inset, inset), d, style = Stroke(w, cap = StrokeCap.Round))
        }
        Icon(painterResource(DesignR.drawable.ph_download_simple), null, tint = c.textPrimary, modifier = Modifier.size(9.dp))
    }
}

/**
 * Shows the Freeweights first-visit sheet once per account (`introFreeweightsSeenAt`). Nothing shows while signed out,
 * and an intro already seen on this device never waits on the network. The store is built the way the Artifacts
 * lane (#2848) builds it, from [MeFlagsClient]; swap for the shared one once it is wired.
 */
@Composable
internal fun FreeweightsIntroHost(signedIn: Boolean) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val store = remember {
        val client = MeFlagsClient(LibraryNet.okHttp, LibraryNet.config.cloudBaseUrl)
        IntroSeenStore(context, fetch = { client.read() }, push = { k, iso -> client.write(k, iso) })
    }
    var show by remember { mutableStateOf(false) }
    LaunchedEffect(signedIn) {
        if (!signedIn || store.isSeen(IntroKeys.FREEWEIGHTS)) return@LaunchedEffect
        store.refresh()
        show = !store.isSeen(IntroKeys.FREEWEIGHTS)
    }
    if (show) {
        IntroSheet(
            title = "Open models you can keep.",
            line = "Download one to this phone, or pull it to your computer. Every license is checked.",
            eyebrow = { BlockText("FREEWEIGHTS", height = 8.dp, color = PolishGlass.Coral) },
            hero = { FreeweightsIntroHero() },
            onContinue = { show = false; scope.launch { store.markSeen(IntroKeys.FREEWEIGHTS) } },
        )
    }
}
