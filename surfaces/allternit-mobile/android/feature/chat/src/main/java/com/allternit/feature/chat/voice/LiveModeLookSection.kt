package com.allternit.feature.chat.voice

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics

/**
 * Live mode's look in settings (iOS LiveModeLookSection, allternit-ai SpeakerLookPicker.tsx): the speaker style
 * tiles (live previews, a "Motion | Allternit" switch), the ring colours and the caption size. With a [bot] it edits
 * that bot's own look, with "Same as Settings" first in each section; without one it edits the defaults.
 *
 * Shown in Settings › Voice and dictation and in voice mode's settings sheet, as on iOS.
 */
@Composable
fun LiveModeLookSection(bot: VoiceBot? = null, modifier: Modifier = Modifier, inset: Dp = 0.dp) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    SpeakerLookStore.load(context)
    val store = SpeakerLookStore
    val botColor = bot?.color
    val ownStyle = bot?.let { store.ownStyle(it.id) }
    val ownPalette = bot?.let { store.ownPalette(it.id) }
    val currentStyle: SpeakerStyle? = if (bot == null) store.style else ownStyle
    val currentPalette: RingPalette? = if (bot == null) store.palette else ownPalette
    var showMotion by remember(bot?.id) { mutableStateOf((currentStyle ?: store.style).isMotion) }
    val tagBase = if (bot == null) "liveModeLook" else "liveModeLook-bot"

    Column(modifier.fillMaxWidth().padding(horizontal = inset).testTag(tagBase), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(bot?.let { "${it.name}'s live mode look" } ?: "Live mode", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            Text("What moves around a bot's avatar while you talk by voice.", color = c.textSecondary, fontSize = 12.sp)
        }

        Segmented(listOf(true to "Motion", false to "Allternit"), showMotion, "${tagBase}_set", Modifier.widthIn(max = 260.dp)) { showMotion = it }

        Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(vertical = 2.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            if (bot != null) {
                StyleTile("Same as Settings", store.style.title, ownStyle == null, "${tagBase}_style_settings", null, onClick = { store.setOwnStyle(bot.id, null) }) {
                    Text("Settings", color = c.textSecondary, fontSize = 11.sp)
                }
            }
            SpeakerStyle.offered(showMotion).forEach { style ->
                StyleTile(style.title, null, currentStyle == style, "${tagBase}_style_${style.raw}", style.note, onClick = {
                    if (bot != null) store.setOwnStyle(bot.id, style) else store.updateStyle(style)
                }) {
                    VoiceSpeakerView(
                        phase = SpeakerPhase.Speaking, muted = false, style = style, palette = currentPalette ?: store.palette,
                        botColor = botColor, sizeDp = 30f, compact = true, still = currentStyle != style,
                    ) { PreviewFace(bot, 30f) }
                }
            }
        }

        Text("Colour", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
        ChipGrid(
            buildList {
                if (bot != null) add(ChipSpec("Same as Settings", null, ownPalette == null, "${tagBase}_palette_settings") { store.setOwnPalette(bot.id, null) })
                RingPalette.entries.forEach { p ->
                    add(ChipSpec(p.title, p.colors(botColor) ?: RingPalette.spectrumColors, currentPalette == p, "${tagBase}_palette_${p.raw}") {
                        if (bot != null) store.setOwnPalette(bot.id, p) else store.updatePalette(p)
                    })
                }
            },
        )
        Text("Bot's colour follows whichever bot you talk with. Muted and stopped calls turn grey.", color = c.textSecondary, fontSize = 12.sp)

        if (bot == null) {
            Text("Caption size", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
            Segmented(CaptionSize.entries.map { it to it.title }, store.captionSize, "${tagBase}_caption", Modifier.fillMaxWidth()) { store.updateCaptionSize(it) }
        }
    }
}

/** The settings tiles' face: the bot's avatar, or a plain disc for the defaults (iOS previewFace). */
@Composable
private fun PreviewFace(bot: VoiceBot?, sizeDp: Float) {
    if (bot != null) bot.face(sizeDp)
    else Box(Modifier.size(sizeDp.dp).clip(CircleShape).background(AllternitTheme.colors.bgTertiary))
}

@Composable
private fun StyleTile(
    title: String,
    subtitle: String?,
    selected: Boolean,
    tag: String,
    hint: String?,
    onClick: () -> Unit,
    preview: @Composable () -> Unit,
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val shape = RoundedCornerShape(10.dp)
    Column(
        Modifier.width(96.dp).clip(RoundedCornerShape(12.dp))
            .clickable(role = Role.Button, onClickLabel = title) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .semantics { this.selected = selected; contentDescription = listOfNotNull(title, subtitle, hint).joinToString(". ") }
            .testTag(tag),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Box(
            Modifier.width(96.dp).height(60.dp).clip(shape).background(c.bgPanel)
                .border(if (selected) 2.dp else 1.dp, if (selected) c.textPrimary else c.borderSubtle, shape),
            contentAlignment = Alignment.Center,
        ) { preview() }
        Text(title, color = c.textPrimary, fontSize = 12.sp, fontWeight = if (selected) FontWeight.SemiBold else FontWeight.Normal, maxLines = 1, overflow = TextOverflow.Ellipsis)
        if (subtitle != null) Text(subtitle, color = c.textSecondary, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

private class ChipSpec(val name: String, val colors: List<Color>?, val selected: Boolean, val tag: String, val onClick: () -> Unit)

/** iOS LazyVGrid(.adaptive(minimum: 116)): as many 116 dp+ columns as fit, each chip stretched to its column. */
@Composable
private fun ChipGrid(chips: List<ChipSpec>) {
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        val spacing = 8.dp
        val cols = maxOf(1, ((maxWidth + spacing) / (116.dp + spacing)).toInt())
        Column(verticalArrangement = Arrangement.spacedBy(spacing)) {
            chips.chunked(cols).forEach { row ->
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(spacing)) {
                    row.forEach { Chip(it, Modifier.weight(1f)) }
                    repeat(cols - row.size) { Spacer(Modifier.weight(1f)) }
                }
            }
        }
    }
}

@Composable
private fun Chip(spec: ChipSpec, modifier: Modifier) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val neutral = c.bgTertiary
    Row(
        modifier.height(30.dp).clip(CircleShape).background(c.bgPanel)
            .border(if (spec.selected) 1.5.dp else 1.dp, if (spec.selected) c.textPrimary else c.borderSubtle, CircleShape)
            .clickable(role = Role.Button, onClickLabel = spec.name) { Haptics.perform(view, Haptics.Kind.Tap); spec.onClick() }
            .semantics { selected = spec.selected }
            .testTag(spec.tag)
            .padding(start = 5.dp, end = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Canvas(Modifier.size(20.dp)) {
            val w = 5.dp.toPx()
            val r = size.minDimension / 2 - w / 2
            val colors = spec.colors
            if (colors == null || colors.size < 2) drawCircle(colors?.firstOrNull() ?: neutral, radius = r, style = Stroke(w))
            else drawCircle(Brush.sweepGradient(colors + colors.first(), center = center), radius = r, style = Stroke(w))
        }
        Text(spec.name, color = c.textPrimary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** A two-or-three-way switch (iOS segmented Picker). */
@Composable
private fun <T> Segmented(options: List<Pair<T, String>>, value: T, tag: String, modifier: Modifier, onPick: (T) -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(modifier.clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).padding(2.dp), horizontalArrangement = Arrangement.spacedBy(2.dp)) {
        options.forEachIndexed { i, (v, label) ->
            val on = v == value
            Box(
                Modifier.weight(1f).heightIn(min = 34.dp).clip(RoundedCornerShape(8.dp)).background(if (on) c.bgPrimary else Color.Transparent)
                    .clickable(role = Role.Tab, onClickLabel = label) { if (!on) { Haptics.perform(view, Haptics.Kind.Tap); onPick(v) } }
                    .semantics { selected = on }
                    .testTag("${tag}_$i"),
                contentAlignment = Alignment.Center,
            ) { Text(label, color = c.textPrimary, fontSize = 13.sp, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal) }
        }
    }
}
