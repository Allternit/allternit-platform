package com.allternit.feature.settings

import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.chat.voice.DeviceVoice
import com.allternit.feature.chat.voice.KokoroVoices
import com.allternit.feature.chat.voice.VoicePack
import com.allternit.feature.chat.voice.VoicePackManager
import com.allternit.feature.chat.voice.VoicePackState
import com.allternit.feature.chat.voice.VoicePrefs

/*
 * Settings › Voice › "Speech to text" and "Reply voice" (iOS SettingsPages voice page + ReplyVoiceSection): how the
 * on-phone engine listens, the Kokoro voice pack (download, progress, pause, resume, cancel, remove, space used) and
 * the voice bots use when they have none of their own.
 */

/** iOS "Speech to text" radio cards. */
@Composable
internal fun SpeechToTextSection() {
    val context = LocalContext.current
    LaunchedEffect(Unit) { VoicePrefs.load(context) }
    SLabel("Speech to text")
    SNote("How speech becomes text when voice runs on this device. Both stay on this device.")
    SCard {
        listOf(
            Triple(VoicePrefs.LISTENING_AUTOMATIC, "Automatic (recommended)", "Uses Android's on-device speech recognition when this phone has it, otherwise Allternit's."),
            Triple(VoicePrefs.LISTENING_ALLTERNIT, "Allternit on-device", "Always uses Allternit's own recognizer (Moonshine, English)."),
        ).forEachIndexed { i, (id, title, detail) ->
            if (i > 0) SDivider()
            val on = VoicePrefs.listening == id
            SRow(
                title, "voice_listening_$id", detail = detail, chevron = false,
                onClick = { VoicePrefs.setListening(context, id) },
                trailing = ({ Text(if (on) "✓" else "", color = AllternitTheme.colors.accentPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.semantics { selected = on }) }),
            )
        }
    }
}

/** iOS ReplyVoiceSection. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun ReplyVoiceSection() {
    val context = LocalContext.current
    val c = AllternitTheme.colors
    val pack = VoicePackManager.kokoro
    LaunchedEffect(Unit) { VoicePrefs.load(context); pack.refresh(context) }
    val megabytes = VoicePackManager.megabytes(VoicePack.kokoro.totalBytes)
    val voiceId = KokoroVoices.resolve(null, VoicePrefs.kokoroVoice)

    SLabel("Reply voice")
    if (!DeviceVoice.linked || !DeviceVoice.KOKORO_ALLOWED) {
        SNote("Replies use the phone's built-in voice. The on-device Kokoro voice isn't available on Android yet.")
        return
    }
    SNote("Bots answer in Kokoro, a natural voice that runs on this phone. It downloads once ($megabytes, on Wi-Fi unless you tap Download). Until then, and if this phone runs low on memory, the built-in voice reads replies.")
    val state = pack.state
    SCard {
        SRow(
            "Kokoro voice", "voice_kokoroPack",
            detail = when (state) {
                is VoicePackState.Ready -> "On this phone · ${VoicePackManager.megabytes(pack.bytesOnDisk(context))}"
                is VoicePackState.Downloading -> "Downloading · ${VoicePackManager.megabytes((state.progress * VoicePack.kokoro.totalBytes).toLong())} of $megabytes"
                is VoicePackState.Paused -> "Paused · ${VoicePackManager.megabytes((state.progress * VoicePack.kokoro.totalBytes).toLong())} of $megabytes"
                else -> megabytes
            },
            chevron = false,
            trailing = ({
                when (state) {
                    is VoicePackState.Ready -> TextButton(onClick = { pack.remove(context) }, modifier = Modifier.testTag("voice_kokoroRemove")) { Text("Remove", color = c.textSecondary) }
                    is VoicePackState.Downloading -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        val pct = (state.progress * 100).toInt()
                        LinearProgressIndicator(progress = { state.progress.toFloat() }, modifier = Modifier.width(64.dp).semantics { contentDescription = "Downloading, $pct percent" })
                        Text("$pct%", color = c.textSecondary, fontSize = 13.sp, fontFamily = FontFamily.Monospace)
                        TextButton(onClick = { pack.pause() }, modifier = Modifier.testTag("voice_kokoroPause")) { Text("Pause") }
                    }
                    is VoicePackState.Paused -> Row(verticalAlignment = Alignment.CenterVertically) {
                        TextButton(onClick = { pack.cancel(context) }, modifier = Modifier.testTag("voice_kokoroCancel")) { Text("Cancel", color = c.textSecondary) }
                        TextButton(onClick = { pack.install(context, cellular = true) }, modifier = Modifier.testTag("voice_kokoroResume")) { Text("Resume") }
                    }
                    else -> TextButton(onClick = { pack.install(context, cellular = true) }, modifier = Modifier.testTag("voice_kokoroDownload")) { Text("Download") }
                }
            }),
        )
    }
    (state as? VoicePackState.Failed)?.let { SError(it.message) }
    SNote("Voice for bots that don't have their own:")
    FlowRow(
        Modifier.fillMaxWidth().padding(horizontal = 2.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        KokoroVoices.all.forEach { id ->
            val on = id == voiceId
            Box(
                Modifier.width(104.dp).heightIn(min = 36.dp)
                    .border(if (on) 2.dp else 1.dp, if (on) c.accentPrimary else c.borderSubtle, RoundedCornerShape(10.dp))
                    .clickable(role = Role.Button) { VoicePrefs.setKokoroVoice(context, id) }
                    .semantics { selected = on }
                    .testTag("voice_kokoroChip_$id"),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    KokoroVoices.label(id), fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal,
                    color = if (on) c.accentPrimary else c.textPrimary,
                    modifier = Modifier.padding(horizontal = 6.dp),
                )
            }
        }
    }
}
