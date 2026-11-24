package com.allternit.feature.chat.voice

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import java.util.Locale
import java.util.UUID

/** iOS SpeechLanguage (SettingsStore): the languages dictation and voice mode can use. */
enum class SpeechLanguage(val tag: String, val label: String) {
    EnglishUS("en-US", "English (US)"),
    EnglishUK("en-GB", "English (UK)"),
    Spanish("es-ES", "Español"),
    French("fr-FR", "Français"),
    German("de-DE", "Deutsch"),
    Italian("it-IT", "Italiano"),
    Portuguese("pt-BR", "Português (BR)"),
    Japanese("ja-JP", "日本語"),
}

/** iOS VoiceInteractionMode. */
enum class VoiceInteractionMode(val raw: String, val label: String) {
    HandsFree("handsFree", "Hands free"),
    PushToTalk("pushToTalk", "Push to talk"),
}

/**
 * The voice settings iOS keeps in SettingsStore (speechLanguage, speechSpeed, voiceIdentifier,
 * voiceInteractionMode), on this device. Compose state, so the sheets and voice mode follow changes.
 */
object VoicePrefs {
    private const val FILE = "allternit_voice"
    val speechSpeeds = listOf(0.75f, 1.0f, 1.25f, 1.5f)

    var speechLanguage by mutableStateOf<SpeechLanguage?>(null)
        private set
    var speechSpeed by mutableFloatStateOf(1.0f)
        private set
    var voiceIdentifier by mutableStateOf<String?>(null)
        private set
    var interactionMode by mutableStateOf(VoiceInteractionMode.HandsFree)
        private set
    /** iOS SettingsStore.listening: [LISTENING_AUTOMATIC] or [LISTENING_ALLTERNIT] (Moonshine on this phone). */
    var listening by mutableStateOf(LISTENING_AUTOMATIC)
        private set
    /** iOS KokoroVoices.settingKey: the Kokoro voice for bots without their own. */
    var kokoroVoice by mutableStateOf<String?>(null)
        private set
    const val LISTENING_AUTOMATIC = "automatic"
    const val LISTENING_ALLTERNIT = "allternit"
    private var loaded = false

    fun load(context: Context) {
        if (loaded) return
        loaded = true
        val p = context.getSharedPreferences(FILE, Context.MODE_PRIVATE)
        speechLanguage = p.getString("speechLanguage", null)?.let { t -> SpeechLanguage.entries.firstOrNull { it.tag == t } }
        speechSpeed = p.getFloat("speechSpeed", 1.0f)
        voiceIdentifier = p.getString("voiceIdentifier", null)
        interactionMode = VoiceInteractionMode.entries.firstOrNull { it.raw == p.getString("voiceInteractionMode", null) } ?: VoiceInteractionMode.HandsFree
        listening = p.getString("listening", null)?.takeIf { it == LISTENING_ALLTERNIT } ?: LISTENING_AUTOMATIC
        kokoroVoice = p.getString("allternit-settings-voice-kokoro", null)
    }

    private fun prefs(context: Context) = context.getSharedPreferences(FILE, Context.MODE_PRIVATE)

    fun setLanguage(context: Context, value: SpeechLanguage?) {
        speechLanguage = value
        prefs(context).edit().putString("speechLanguage", value?.tag).apply()
    }

    fun setSpeed(context: Context, value: Float) {
        speechSpeed = value
        prefs(context).edit().putFloat("speechSpeed", value).apply()
    }

    fun setVoice(context: Context, value: String?) {
        voiceIdentifier = value
        prefs(context).edit().putString("voiceIdentifier", value).apply()
    }

    fun setListening(context: Context, value: String) {
        listening = value
        prefs(context).edit().putString("listening", value).apply()
    }

    fun setKokoroVoice(context: Context, value: String) {
        kokoroVoice = value
        prefs(context).edit().putString("allternit-settings-voice-kokoro", value).apply()
    }

    fun setMode(context: Context, value: VoiceInteractionMode) {
        interactionMode = value
        prefs(context).edit().putString("voiceInteractionMode", value.raw).apply()
    }

    /** The recognition / playback locale: the chosen language, else the system's. */
    fun locale(): Locale = speechLanguage?.let { Locale.forLanguageTag(it.tag) } ?: Locale.getDefault()
}

fun hasMicPermission(context: Context): Boolean =
    context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED

/**
 * iOS DictationController on Android SpeechRecognizer (iOS uses on-device SFSpeechRecognizer, so this is
 * the platform recognizer too, not a server route). Live partial results into [transcript], a 2.5 s
 * silence stop, and the same in-place error copy. Main thread only (SpeechRecognizer requires it).
 */
class Dictation(
    private val context: Context,
    /** On this phone's on-device recognizer only (voice mode's "Speech to text: Automatic", API 31+). */
    private val onDevice: Boolean = false,
) : VoiceListener {
    override var transcript by mutableStateOf("")
        private set
    override var isRecording by mutableStateOf(false)
        private set
    override var errorMessage by mutableStateOf<String?>(null)
    /** 0…1 microphone level for the speaker ring (iOS VoiceLevels.pushMic). */
    override var micLevel by mutableFloatStateOf(0f)
        private set

    /** Called once each time recording stops (silence, final result, stop(), error). */
    override var onStopped: (() -> Unit)? = null

    private val main = Handler(Looper.getMainLooper())
    private var recognizer: SpeechRecognizer? = null
    private val silenceStop = Runnable { teardown() }

    val isAvailable: Boolean get() = SpeechRecognizer.isRecognitionAvailable(context)

    override fun start() {
        if (isRecording) return
        errorMessage = null
        transcript = ""
        if (!hasMicPermission(context)) {
            errorMessage = "Microphone access is off. Enable it in Settings to dictate messages."
            return
        }
        if (!onDevice && !isAvailable) {
            errorMessage = "Speech recognition isn't available right now."
            return
        }
        val r = runCatching {
            if (onDevice && Build.VERSION.SDK_INT >= 31) SpeechRecognizer.createOnDeviceSpeechRecognizer(context)
            else SpeechRecognizer.createSpeechRecognizer(context)
        }.getOrNull()
        if (r == null) {
            errorMessage = "Couldn't start dictation: no speech service on this phone."
            return
        }
        recognizer = r
        r.setRecognitionListener(listener)
        val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH)
            .putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
            .putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
            .putExtra(RecognizerIntent.EXTRA_LANGUAGE, VoicePrefs.locale().toLanguageTag())
            .putExtra(RecognizerIntent.EXTRA_SPEECH_INPUT_COMPLETE_SILENCE_LENGTH_MILLIS, 2500L)
        runCatching { r.startListening(intent) }.onFailure {
            teardown()
            errorMessage = "Couldn't start dictation: ${it.localizedMessage ?: "the recognizer failed"}"
            return
        }
        isRecording = true
        scheduleSilenceStop()
    }

    override fun stop() {
        if (!isRecording) return
        teardown()
    }

    private fun teardown() {
        main.removeCallbacks(silenceStop)
        recognizer?.let { runCatching { it.cancel() }; runCatching { it.destroy() } }
        recognizer = null
        micLevel = 0f
        if (isRecording) {
            isRecording = false
            onStopped?.invoke()
        }
    }

    private fun scheduleSilenceStop() {
        main.removeCallbacks(silenceStop)
        main.postDelayed(silenceStop, 2_500L + 4_000L)
    }

    private val listener = object : RecognitionListener {
        override fun onReadyForSpeech(params: Bundle?) {}
        override fun onBeginningOfSpeech() { scheduleSilenceStop() }
        override fun onRmsChanged(rmsdB: Float) { micLevel = ((rmsdB + 2f) / 12f).coerceIn(0f, 1f) }
        override fun onBufferReceived(buffer: ByteArray?) {}
        override fun onEndOfSpeech() {}
        override fun onEvent(eventType: Int, params: Bundle?) {}

        override fun onPartialResults(partialResults: Bundle?) {
            val text = partialResults?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull()
            if (!isRecording || text.isNullOrEmpty()) return
            transcript = text
            scheduleSilenceStop()
        }

        override fun onResults(results: Bundle?) {
            val text = results?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull()
            if (!text.isNullOrEmpty()) transcript = text
            teardown()
        }

        override fun onError(error: Int) {
            if (!isRecording) return
            // iOS treats "no speech" / cancelled as benign; the same here.
            val benign = error == SpeechRecognizer.ERROR_NO_MATCH || error == SpeechRecognizer.ERROR_SPEECH_TIMEOUT ||
                error == SpeechRecognizer.ERROR_CLIENT
            errorMessage = when {
                benign -> null
                error == SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS -> "Microphone access is off. Enable it in Settings to dictate messages."
                Build.VERSION.SDK_INT >= 31 && error == SpeechRecognizer.ERROR_LANGUAGE_NOT_SUPPORTED ->
                    "Speech recognition doesn't support the current language."
                error == SpeechRecognizer.ERROR_NETWORK || error == SpeechRecognizer.ERROR_NETWORK_TIMEOUT ->
                    "Dictation failed: the speech service is offline."
                else -> "Dictation failed (code $error)."
            }
            teardown()
        }
    }
}

/**
 * iOS SpeechSpeaker on Android TextToSpeech: sentence chunks queue up, [isSpeaking] drops when the
 * queue drains, speed and voice follow [VoicePrefs]. iOS voice mode's system path uses
 * AVSpeechSynthesizer; this is its Android twin.
 */
class Speaker(context: Context) : ReplySpeaker {
    override var isSpeaking by mutableStateOf(false)
        private set
    /** Called when the queue drains (iOS `speaker.isSpeaking` → false). */
    override var onFinished: (() -> Unit)? = null

    private val main = Handler(Looper.getMainLooper())
    private var ready = false
    private var pending = 0
    private val backlog = ArrayList<String>()
    private val tts: TextToSpeech = TextToSpeech(context.applicationContext) { status ->
        main.post {
            ready = status == TextToSpeech.SUCCESS
            if (ready) {
                apply()
                val queued = backlog.toList()
                backlog.clear()
                queued.forEach { speakChunk(it) }
            } else if (backlog.isNotEmpty()) {
                backlog.clear()
                finish()
            }
        }
    }

    init {
        tts.setOnUtteranceProgressListener(object : UtteranceProgressListener() {
            override fun onStart(utteranceId: String?) {}
            override fun onDone(utteranceId: String?) { main.post { drained() } }
            @Deprecated("Deprecated in Java")
            override fun onError(utteranceId: String?) { main.post { drained() } }
            override fun onStop(utteranceId: String?, interrupted: Boolean) { main.post { drained() } }
        })
    }

    private fun apply() {
        runCatching { tts.language = VoicePrefs.locale() }
        tts.setSpeechRate(VoicePrefs.speechSpeed)
        VoicePrefs.voiceIdentifier?.let { id -> runCatching { tts.voices?.firstOrNull { it.name == id }?.let { tts.voice = it } } }
    }

    /**
     * The phone's voices for voice mode's Voice carousel (iOS VoiceSettingsSheet.onDeviceVoices): the speech
     * language's voices first, then the better ones, at most 12. Voices the engine hasn't downloaded are left out.
     */
    fun voices(): List<PhoneVoice> {
        if (!ready) return emptyList()
        val all = runCatching { tts.voices?.toList() }.getOrNull().orEmpty()
            .filter { v -> v.features?.contains(TextToSpeech.Engine.KEY_FEATURE_NOT_INSTALLED) != true }
            .map { v -> PhoneVoiceFacts(v.name, v.locale, v.quality, v.isNetworkConnectionRequired) }
        return phoneVoiceOptions(all, VoicePrefs.locale().language)
    }

    override fun speakChunk(text: String) {
        if (text.isBlank()) return
        if (!ready) {
            backlog += text
            isSpeaking = true
            return
        }
        if (pending == 0) apply()
        pending += 1
        isSpeaking = true
        tts.speak(text, TextToSpeech.QUEUE_ADD, null, UUID.randomUUID().toString())
    }

    override fun stop() {
        backlog.clear()
        pending = 0
        runCatching { tts.stop() }
        isSpeaking = false
    }

    override fun shutdown() {
        stop()
        runCatching { tts.shutdown() }
    }

    private fun drained() {
        if (pending == 0) return
        pending -= 1
        if (pending == 0) finish()
    }

    private fun finish() {
        if (!isSpeaking) return
        isSpeaking = false
        onFinished?.invoke()
    }
}

/** One card in voice mode's Voice carousel (iOS VoiceSettingsSheet.VoiceOption): [id] is the TTS voice name. */
data class PhoneVoice(val id: String, val name: String, val detail: String)

/** What the carousel reads from an android.speech.tts.Voice. */
internal data class PhoneVoiceFacts(val id: String, val locale: Locale, val quality: Int, val network: Boolean)

/**
 * Readable labels for the phone's voices. Android voices have no human name (iOS shows "Samantha"), only ids such as
 * "en-us-x-tpf-local", so a card is named by its language and region ("English (Australia)"), numbered when several
 * share one ("English (Australia) · Voice 2"), with a caption of its quality: "Enhanced" at QUALITY_HIGH or better,
 * and "Offline" or "Network" (Voice.isNetworkConnectionRequired).
 *
 * Ranked as iOS ranks them: +10 for the speech language, +3 very high (iOS premium), +2 high (iOS enhanced); Android
 * adds +1 for a voice that works offline, since voice mode should not stop when the network does. Top 12.
 */
internal fun phoneVoiceOptions(all: List<PhoneVoiceFacts>, preferredLanguage: String, display: Locale = Locale.getDefault()): List<PhoneVoice> {
    fun rank(v: PhoneVoiceFacts): Int {
        var score = if (v.locale.language == preferredLanguage) 10 else 0
        score += when {
            v.quality >= android.speech.tts.Voice.QUALITY_VERY_HIGH -> 3
            v.quality >= android.speech.tts.Voice.QUALITY_HIGH -> 2
            else -> 0
        }
        if (!v.network) score += 1
        return score
    }
    val top = all.sortedWith(compareByDescending<PhoneVoiceFacts> { rank(it) }.thenBy { it.locale.toLanguageTag() }.thenBy { it.id }).take(12)
    val perLocale = top.groupingBy { it.locale.toLanguageTag() }.eachCount()
    val seen = HashMap<String, Int>()
    return top.map { v ->
        val tag = v.locale.toLanguageTag()
        val base = v.locale.getDisplayName(display).ifBlank { tag }
        val n = (seen[tag] ?: 0) + 1
        seen[tag] = n
        val name = if ((perLocale[tag] ?: 0) > 1) "$base · Voice $n" else base
        val detail = listOfNotNull(
            if (v.quality >= android.speech.tts.Voice.QUALITY_HIGH) "Enhanced" else null,
            if (v.network) "Network" else "Offline",
        ).joinToString(" · ")
        PhoneVoice(v.id, name, detail)
    }
}
