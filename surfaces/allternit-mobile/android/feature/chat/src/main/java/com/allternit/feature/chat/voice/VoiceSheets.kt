package com.allternit.feature.chat.voice

import android.Manifest
import android.content.Context
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitRadii
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR
import kotlinx.coroutines.delay

/** iOS AppPermission: what the priming sheet explains before the system dialog. Shown once per install. */
enum class AppPermission(val icon: Int, val title: String, val message: String) {
    Photos(DesignR.drawable.vc_photo, "Access your photos?", "Pick photos to attach to your chat, and see recent ones right in the composer."),
    Camera(DesignR.drawable.vc_camera, "Use your camera?", "Take a photo and attach it straight to your chat."),
    Microphone(DesignR.drawable.vc_mic, "Dictate messages?", "Allternit transcribes what you say into the message field. Audio is only used while you dictate."),
    Notifications(DesignR.drawable.vc_bell, "Let your bots reach you?", "Know when a task finishes or a bot needs your approval. No marketing messages. Change this in Settings.");

    private val key: String get() = "allternit-priming-shown-${name.lowercase()}"
    fun hasPrimed(context: Context): Boolean = prefs(context).getBoolean(key, false)
    fun markPrimed(context: Context) { prefs(context).edit().putBoolean(key, true).apply() }

    /** The Android runtime permission behind it; null when Android shows no dialog (the photo picker). */
    val manifestPermission: String?
        get() = when (this) {
            Photos -> null
            Camera -> Manifest.permission.CAMERA
            Microphone -> Manifest.permission.RECORD_AUDIO
            Notifications -> if (Build.VERSION.SDK_INT >= 33) Manifest.permission.POST_NOTIFICATIONS else null
        }

    fun isGranted(context: Context): Boolean = manifestPermission?.let { context.checkSelfPermission(it) == android.content.pm.PackageManager.PERMISSION_GRANTED } ?: true

    companion object {
        private fun prefs(context: Context) = context.getSharedPreferences("allternit_priming", Context.MODE_PRIVATE)
        fun resetAll(context: Context) { prefs(context).edit().clear().apply() }
    }
}

/**
 * The iOS flow around a system permission: when the dialog is still pending and the app hasn't primed
 * yet, show [PermissionPrimingSheet] first; Continue opens the system dialog. [then] runs with the result.
 * Call the returned function to start it.
 */
@Composable
fun rememberPrimedPermission(permission: AppPermission, then: (granted: Boolean) -> Unit): () -> Unit {
    val context = LocalContext.current
    var priming by remember { mutableStateOf(false) }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted -> then(granted) }
    if (priming) {
        PermissionPrimingSheet(permission, onDismiss = { priming = false }) {
            priming = false
            permission.manifestPermission?.let { launcher.launch(it) } ?: then(true)
        }
    }
    return {
        when {
            permission.isGranted(context) -> then(true)
            !permission.hasPrimed(context) -> priming = true
            else -> permission.manifestPermission?.let { launcher.launch(it) } ?: then(true)
        }
    }
}

/** iOS PermissionPrimingSheet: the notifications layout (example rows) or the standard one (icon, copy, Continue / Not Now). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PermissionPrimingSheet(permission: AppPermission, onDismiss: () -> Unit, onContinue: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val state = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = state, containerColor = c.bgPrimary) {
        Column(
            Modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 24.dp).padding(bottom = 16.dp).testTag("permissionPriming-${permission.name.lowercase()}"),
            horizontalAlignment = if (permission == AppPermission.Notifications) Alignment.Start else Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            if (permission == AppPermission.Notifications) {
                ExampleRow(Color(0xFFDBE8F7), "Research finished", "Sources gathered, ready to read")
                ExampleRow(Color(0xFFE0EDD1), "Code task done", "Changes ready for review")
                ExampleRow(Color(0xFFF5DBDB), "A bot needs your approval", "It waits until you answer")
                Spacer(Modifier.height(8.dp))
            } else {
                Box(Modifier.size(64.dp).clip(RoundedCornerShape(AllternitRadii.LG)).background(c.accentPrimary.copy(alpha = 0.12f))
                    .border(1.dp, c.accentPrimary.copy(alpha = 0.28f), RoundedCornerShape(AllternitRadii.LG)), contentAlignment = Alignment.Center) {
                    Icon(painterResource(permission.icon), null, tint = c.accentPrimary, modifier = Modifier.size(28.dp))
                }
            }
            Text(permission.title, color = c.textPrimary, fontSize = 22.sp, fontWeight = FontWeight.SemiBold,
                textAlign = if (permission == AppPermission.Notifications) TextAlign.Start else TextAlign.Center)
            Text(permission.message, color = c.textSecondary, fontSize = 15.sp, lineHeight = 21.sp,
                textAlign = if (permission == AppPermission.Notifications) TextAlign.Start else TextAlign.Center)
            Spacer(Modifier.height(8.dp))
            PrimaryButton(if (permission == AppPermission.Notifications) "Turn on notifications" else "Continue", "permissionPrimingContinue") {
                permission.markPrimed(context)
                onContinue()
            }
            Text(
                if (permission == AppPermission.Notifications) "Not now" else "Not Now",
                color = c.textSecondary, fontSize = 16.sp, textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth().clip(CircleShape).clickable { permission.markPrimed(context); onDismiss() }.padding(vertical = 12.dp).testTag("permissionPrimingNotNow"),
            )
        }
    }
}

@Composable
private fun ExampleRow(tint: Color, title: String, detail: String) {
    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(Color.White).border(1.dp, Color.Black.copy(alpha = 0.06f), RoundedCornerShape(14.dp)).padding(12.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Box(Modifier.size(32.dp).clip(RoundedCornerShape(8.dp)).background(tint), contentAlignment = Alignment.Center) {
            Icon(painterResource(DesignR.drawable.vc_bell), null, tint = Color.Black.copy(alpha = 0.6f), modifier = Modifier.size(16.dp))
        }
        Column {
            Text(title, color = Color.Black.copy(alpha = 0.88f), fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
            Text(detail, color = Color.Black.copy(alpha = 0.55f), fontSize = 12.5.sp)
        }
    }
}

@Composable
private fun PrimaryButton(label: String, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(Modifier.fillMaxWidth().height(50.dp).clip(RoundedCornerShape(AllternitRadii.MD)).background(c.accentPrimary).clickable(onClick = onClick).testTag(tag),
        contentAlignment = Alignment.Center) { Text(label, color = c.onAccent, fontSize = 16.sp, fontWeight = FontWeight.SemiBold) }
}

/** iOS DictationOnboardingSheet: shown before the first dictation; the speech language list; Continue. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DictationOnboardingSheet(onDismiss: () -> Unit, onContinue: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    LaunchedEffect(Unit) { VoicePrefs.load(context) }
    var expanded by remember { mutableStateOf(false) }
    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = c.bgPrimary) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 24.dp).padding(bottom = 16.dp).testTag("dictationOnboarding")) {
            Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState()), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(20.dp)) {
                Box(Modifier.size(64.dp).clip(CircleShape).background(c.accentPrimary.copy(alpha = 0.12f)).border(1.dp, c.accentPrimary.copy(alpha = 0.28f), CircleShape), contentAlignment = Alignment.Center) {
                    Icon(painterResource(DesignR.drawable.vc_mic), null, tint = c.accentPrimary, modifier = Modifier.size(28.dp))
                }
                Text("Send messages to Allternit using your voice.", color = c.textPrimary, fontSize = 22.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center)
                Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                    Bullet(DesignR.drawable.ph_globe, "Choose a language to speak in")
                    Bullet(DesignR.drawable.vc_clock, "Dictate for up to 10 minutes")
                    Bullet(DesignR.drawable.vc_chat, "Chat more quickly and naturally")
                }
                Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(AllternitRadii.MD)).border(1.dp, c.borderSubtle, RoundedCornerShape(AllternitRadii.MD))) {
                    Row(Modifier.fillMaxWidth().clickable { expanded = !expanded }.padding(14.dp).testTag("dictationLanguage"), verticalAlignment = Alignment.CenterVertically) {
                        Text("Speech language", color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
                        Text(VoicePrefs.speechLanguage?.label ?: "System default", color = c.textSecondary, fontSize = 15.sp)
                    }
                    if (expanded) {
                        LanguageOption("System default", VoicePrefs.speechLanguage == null) { VoicePrefs.setLanguage(context, null); expanded = false }
                        SpeechLanguage.entries.forEach { l ->
                            LanguageOption(l.label, VoicePrefs.speechLanguage == l) { VoicePrefs.setLanguage(context, l); expanded = false }
                        }
                    }
                }
            }
            Spacer(Modifier.height(16.dp))
            PrimaryButton("Continue", "dictationOnboardingContinue") {
                markDictationOnboardingShown(context)
                onContinue()
            }
        }
    }
}

fun dictationOnboardingShown(context: Context): Boolean =
    context.getSharedPreferences("allternit_priming", Context.MODE_PRIVATE).getBoolean("allternit-dictation-onboarding-shown", false)

fun markDictationOnboardingShown(context: Context) {
    context.getSharedPreferences("allternit_priming", Context.MODE_PRIVATE).edit().putBoolean("allternit-dictation-onboarding-shown", true).apply()
}

@Composable
private fun Bullet(icon: Int, text: String) {
    val c = AllternitTheme.colors
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Icon(painterResource(icon), null, tint = c.textSecondary, modifier = Modifier.size(18.dp))
        Text(text, color = c.textPrimary, fontSize = 15.sp)
    }
}

@Composable
private fun LanguageOption(label: String, selected: Boolean, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 14.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(label, color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
        if (selected) Icon(painterResource(DesignR.drawable.ph_check), null, tint = c.accentPrimary, modifier = Modifier.size(16.dp))
    }
}

/** iOS VoiceSettingsSheet: the phone's voices, language, speed and mode. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun VoiceSettingsSheet(speaker: Speaker, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    var voices by remember { mutableStateOf<List<Pair<String, String>>>(emptyList()) }
    LaunchedEffect(VoicePrefs.speechLanguage) {
        // TextToSpeech binds asynchronously; retry briefly until it reports its voices.
        repeat(20) {
            voices = speaker.voices()
            if (voices.isNotEmpty()) return@LaunchedEffect
            delay(250)
        }
    }
    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = c.bgPrimary) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).padding(bottom = 20.dp).testTag("voiceSettingsSheet"),
            verticalArrangement = Arrangement.spacedBy(24.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Voice settings", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                Text("Done", color = c.accentPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.clickable(onClick = onDismiss).padding(8.dp).testTag("voiceSettingsDone"))
            }
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("Voice", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                if (voices.isEmpty()) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp, color = c.textSecondary)
                        Text("Loading voices…", color = c.textSecondary, fontSize = 13.sp)
                    }
                } else {
                    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        voices.forEachIndexed { i, (id, detail) ->
                            val selected = VoicePrefs.voiceIdentifier == id || (VoicePrefs.voiceIdentifier == null && i == 0)
                            Column(
                                Modifier.width(140.dp).height(110.dp).clip(RoundedCornerShape(AllternitRadii.MD)).background(c.fillNeutral)
                                    .border(if (selected) 2.dp else 1.dp, if (selected) c.accentPrimary else c.borderSubtle, RoundedCornerShape(AllternitRadii.MD))
                                    .clickable { VoicePrefs.setVoice(context, id) }.padding(12.dp).testTag("voiceCard-$i"),
                            ) {
                                Icon(painterResource(DesignR.drawable.vc_waveform), null, tint = c.textSecondary, modifier = Modifier.size(16.dp))
                                Spacer(Modifier.weight(1f))
                                Text(id.substringBefore('#').take(22), color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
                                Text(detail, color = c.textSecondary, fontSize = 11.sp, maxLines = 1)
                            }
                        }
                    }
                }
            }
            ChoiceRow("Language", "Speech recognition and playback language",
                listOf<Pair<String, () -> Unit>>("System default" to { VoicePrefs.setLanguage(context, null) }) + SpeechLanguage.entries.map { l -> l.label to { VoicePrefs.setLanguage(context, l) } },
                VoicePrefs.speechLanguage?.label ?: "System default")
            ChoiceRow("Speed", "Voice playback speed",
                VoicePrefs.speechSpeeds.map { s -> speedLabel(s) to { VoicePrefs.setSpeed(context, s) } }, speedLabel(VoicePrefs.speechSpeed))
            ChoiceRow("Mode", "Hands free listens after each reply; push to talk records while you hold the mic",
                VoiceInteractionMode.entries.map { m -> m.label to { VoicePrefs.setMode(context, m) } }, VoicePrefs.interactionMode.label)
        }
    }
}

private fun speedLabel(s: Float): String = if (s == 1.0f) "1×" else "${s.toString().trimEnd('0').trimEnd('.')}×"

/** iOS settingsRow with a menu Picker; Android shows the choices as tappable chips (no menus as the main way). */
@Composable
private fun ChoiceRow(title: String, detail: String, options: List<Pair<String, () -> Unit>>, current: String) {
    val c = AllternitTheme.colors
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Column {
            Text(title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            Text(detail, color = c.textSecondary, fontSize = 12.sp)
        }
        Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            options.forEach { (label, pick) ->
                val on = label == current
                Text(label, color = if (on) c.onAccent else c.textPrimary, fontSize = 13.sp,
                    modifier = Modifier.clip(CircleShape).background(if (on) c.accentPrimary else c.fillNeutral).clickable { pick() }.padding(horizontal = 12.dp, vertical = 7.dp))
            }
        }
    }
}
