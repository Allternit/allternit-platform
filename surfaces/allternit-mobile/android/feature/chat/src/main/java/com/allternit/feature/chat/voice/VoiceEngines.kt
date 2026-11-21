package com.allternit.feature.chat.voice

import android.annotation.SuppressLint
import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.MediaRecorder
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.speech.SpeechRecognizer
import android.util.Base64
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.core.files.KeystoreBox
import com.allternit.core.files.SealedSecret
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONObject
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import kotlin.math.sqrt

/*
 * Gap run GS-D: the voice engines iOS has (Core/Voice VoiceRouter, ByokVoice, LocalVoiceEngine).
 *  - Own key: the person's OpenAI key (Realtime listening with gpt-4o-transcribe, replies read by gpt-4o-mini-tts) or
 *    ElevenLabs key (replies read by eleven_flash_v2_5, listening on this phone). Keys go only to the vendor.
 *  - On this phone: Silero VAD + Moonshine listening and Kokoro replies through sherpa-onnx (LocalVoice.kt), from the
 *    voice packs (VoicePacks.kt), as iOS. "Speech to text: Automatic" listens with the phone's on-device recognizer when
 *    it has one (iOS: Apple's), otherwise Moonshine. Replies use the system voice until the Kokoro pack is on the phone;
 *    the first session fetches it on Wi-Fi in the background (iOS).
 */

/** What voice mode listens with: the system recognizer ([Dictation]) or a vendor stream. */
interface VoiceListener {
    val transcript: String
    val isRecording: Boolean
    var errorMessage: String?
    val micLevel: Float
    var onStopped: (() -> Unit)?
    /** A progress line while the engine gets ready (the on-phone engine's first pack download), else null. */
    val status: String? get() = null
    fun start()
    fun stop()
}

/** What voice mode reads replies with: the system voice ([Speaker]) or a vendor voice. */
interface ReplySpeaker {
    val isSpeaking: Boolean
    var onFinished: (() -> Unit)?
    fun speakChunk(text: String)
    fun stop()
    fun shutdown()
}

/** iOS VoiceRoute. */
enum class VoiceRoute { Device, Legacy, OpenAI, ElevenLabs }

/** iOS VoiceRouter.candidates: which engines to try, in order, for "Where voice runs". */
object VoiceRouter {
    fun candidates(preference: String?, pushToTalk: Boolean): List<VoiceRoute> {
        if (pushToTalk) return listOf(VoiceRoute.Legacy)
        return when (preference) {
            "openai" -> listOf(VoiceRoute.OpenAI, VoiceRoute.Device, VoiceRoute.Legacy)
            "elevenlabs" -> listOf(VoiceRoute.ElevenLabs, VoiceRoute.Device, VoiceRoute.Legacy)
            else -> listOf(VoiceRoute.Device, VoiceRoute.Legacy)
        }
    }
}

/** A saved own voice key. */
class OwnVoiceKey(val apiKey: String, val voiceId: String?)

/**
 * Reads the voice keys Settings › Voice saves (#2669, :feature:settings VoiceKeys) without depending on that module:
 * the same Keystore-sealed secret names and the same "Where voice runs" preference.
 */
interface VoiceKeySource {
    fun key(context: Context, vendor: String): OwnVoiceKey?
    fun route(context: Context): String?
}

object SealedVoiceKeys : VoiceKeySource {
    override fun key(context: Context, vendor: String): OwnVoiceKey? = runCatching {
        val raw = SealedSecret(context.applicationContext, "voice-$vendor", KeystoreBox("voice_keys_v1", "allternit-voice-keys-v1")).read() // gitleaks:allow (Keystore alias + AAD label, not a secret)
            ?: return null
        val o = JSONObject(raw)
        OwnVoiceKey(o.optString("api_key"), o.optString("voice_id").ifEmpty { null }).takeIf { it.apiKey.isNotBlank() }
    }.getOrNull()

    override fun route(context: Context): String? =
        context.applicationContext.getSharedPreferences("allternit_settings", Context.MODE_PRIVATE)
            .getString("allternit.voice.route", null)?.takeIf { it.isNotEmpty() }
}

/** The on-device voice engine (iOS LocalVoiceEngine on sherpa-onnx): true when sherpa-onnx's native library loads. */
object DeviceVoice {
    val linked: Boolean by lazy { runCatching { System.loadLibrary("sherpa-onnx-jni") }.isSuccess }

    /**
     * Kokoro replies on sherpa-onnx phonemise through espeak-ng (GPL-3.0-or-later). Desktop runs Kokoro in the separate
     * GPL program allternit-tts and iOS runs it on MLX + Misaki; on Android Eoj chose (2026-11-20) to ship it in the app,
     * with the espeak-ng notice shown under Settings › Voice › Reply voice. Listening (Silero + Moonshine) has no GPL part.
     */
    const val KOKORO_ALLOWED = true

    /** iOS ListeningPreference: "automatic" = the phone's on-device recognizer when it has one; "allternit" = Moonshine. */
    fun listener(context: Context, dictation: Dictation): VoiceListener {
        val onDevice = Build.VERSION.SDK_INT >= 31 && runCatching { SpeechRecognizer.isOnDeviceRecognitionAvailable(context) }.getOrDefault(false)
        return if (VoicePrefs.listening == VoicePrefs.LISTENING_AUTOMATIC && onDevice) Dictation(context, onDevice = true) else LocalListener(context)
    }

    /**
     * Kokoro when its pack is on the phone, else the system voice; a missing pack starts downloading on Wi-Fi (iOS).
     * [botVoice] is the bot's own Kokoro voice (iOS VoiceModeViewModel.botVoice), else this phone's pick.
     */
    fun speaker(context: Context, system: Speaker, botVoice: String? = null): ReplySpeaker {
        if (!KOKORO_ALLOWED) return system
        val kokoro = VoicePackManager.kokoro
        if (!kokoro.isInstalled(context)) { kokoro.install(context, cellular = false); return system }
        return KokoroSpeaker(context, KokoroVoices.resolve(botVoice, VoicePrefs.kokoroVoice), system)
    }
}

/** The engines one voice session runs on. [label] is iOS engineLabel; [notice] says why a route fell back. */
class VoiceEngineChoice(val listener: VoiceListener, val speaker: ReplySpeaker, val label: String?, val notice: String?)

object VoiceEngines {
    private const val DEVICE_FALLBACK = "On-device voice is not available on this build. Using the phone's built-in voice for now."

    fun choose(
        context: Context,
        pushToTalk: Boolean,
        dictation: Dictation,
        system: Speaker,
        keys: VoiceKeySource = SealedVoiceKeys,
        botVoice: String? = null,
    ): VoiceEngineChoice {
        var notice: String? = null
        for (route in VoiceRouter.candidates(keys.route(context), pushToTalk)) {
            when (route) {
                VoiceRoute.OpenAI -> {
                    val k = keys.key(context, "openai")
                    if (k == null) { notice = "Your OpenAI key isn't on this phone. Using voice on this phone for now."; continue }
                    return VoiceEngineChoice(OpenAiListener(context, k.apiKey), OwnKeySpeaker(context, "openai", k, system), "OpenAI · your key", null)
                }
                VoiceRoute.ElevenLabs -> {
                    val k = keys.key(context, "elevenlabs")
                    if (k == null) { notice = "Your ElevenLabs key isn't on this phone. Using voice on this phone for now."; continue }
                    // iOS: your ElevenLabs account speaks; the phone still listens.
                    val ear = if (DeviceVoice.linked) DeviceVoice.listener(context, dictation) else dictation
                    return VoiceEngineChoice(ear, OwnKeySpeaker(context, "elevenlabs", k, system), "ElevenLabs · your key", null)
                }
                VoiceRoute.Device -> {
                    if (!DeviceVoice.linked) { notice = notice ?: DEVICE_FALLBACK; continue }
                    return VoiceEngineChoice(DeviceVoice.listener(context, dictation), DeviceVoice.speaker(context, system, botVoice), "On this device", notice)
                }
                VoiceRoute.Legacy -> return VoiceEngineChoice(dictation, system, notice?.let { "Phone voice" }, notice)
            }
        }
        return VoiceEngineChoice(dictation, system, null, notice)
    }
}

private val vendorHttp: OkHttpClient by lazy {
    OkHttpClient.Builder().connectTimeout(15, TimeUnit.SECONDS).readTimeout(60, TimeUnit.SECONDS).build()
}

/**
 * iOS ByokVoice speech: each sentence goes to the vendor's TTS as 24 kHz PCM16 and plays through an AudioTrack.
 * A rejected key, no credit or no network reads the rest with the system voice ([fallback]), as iOS falls back.
 */
class OwnKeySpeaker(private val context: Context, private val vendor: String, private val key: OwnVoiceKey, private val fallback: Speaker) : ReplySpeaker {
    override var isSpeaking by mutableStateOf(false)
        private set
    override var onFinished: (() -> Unit)? = null
    var notice by mutableStateOf<String?>(null)
        private set

    private val main = Handler(Looper.getMainLooper())
    private val queue = LinkedBlockingQueue<Pair<Int, String>>()
    @Volatile private var generation = 0
    @Volatile private var failed = false
    private var track: AudioTrack? = null
    private val worker = thread(name = "own-key-voice", isDaemon = true) { loop() }

    override fun speakChunk(text: String) {
        if (text.isBlank()) return
        if (failed) { fallback.speakChunk(text); return }
        isSpeaking = true
        queue.put(generation to text)
    }

    override fun stop() {
        generation++
        queue.clear()
        runCatching { track?.pause(); track?.flush() }
        fallback.stop()
        isSpeaking = false
    }

    override fun shutdown() {
        stop()
        queue.put(-1 to "")
        runCatching { track?.release() }
        track = null
    }

    private fun loop() {
        while (true) {
            val (gen, text) = queue.take()
            if (gen < 0) return
            if (gen != generation) continue
            val ok = runCatching { speakOne(gen, text) }.getOrElse { e -> fail("${vendorName()} could not be reached. Using the phone's voice for now."); false }
            if (!ok && failed) {
                val rest = buildList { add(text); while (true) { val n = queue.poll() ?: break; if (n.first == gen) add(n.second) } }
                main.post { rest.forEach { fallback.speakChunk(it) }; isSpeaking = false; if (rest.isEmpty()) onFinished?.invoke() }
                fallback.onFinished = { onFinished?.invoke() }
                continue
            }
            if (queue.isEmpty() && gen == generation) {
                // Let the AudioTrack drain what it holds before saying "finished".
                Thread.sleep(250)
                main.post { if (gen == generation && queue.isEmpty()) { isSpeaking = false; onFinished?.invoke() } }
            }
        }
    }

    private fun vendorName() = if (vendor == "openai") "OpenAI" else "ElevenLabs"

    private fun fail(message: String) {
        failed = true
        main.post { notice = message }
    }

    private fun speakOne(gen: Int, text: String): Boolean {
        val json = "application/json".toMediaType()
        val request = if (vendor == "openai") {
            val voice = key.voiceId?.takeIf { it in OPENAI_VOICES } ?: "alloy"
            val body = JSONObject().put("model", "gpt-4o-mini-tts").put("voice", voice).put("input", text).put("response_format", "pcm")
            Request.Builder().url("https://api.openai.com/v1/audio/speech").header("Authorization", "Bearer ${key.apiKey}")
                .post(body.toString().toRequestBody(json)).build()
        } else {
            val voice = key.voiceId ?: DEFAULT_ELEVENLABS_VOICE
            val body = JSONObject().put("text", text).put("model_id", "eleven_flash_v2_5")
            Request.Builder().url("https://api.elevenlabs.io/v1/text-to-speech/$voice/stream?output_format=pcm_24000").header("xi-api-key", key.apiKey)
                .post(body.toString().toRequestBody(json)).build()
        }
        vendorHttp.newCall(request).execute().use { r ->
            if (!r.isSuccessful) {
                fail(when (r.code) {
                    401, 403 -> "${vendorName()} didn't accept your key. Using the phone's voice for now."
                    402, 429 -> "${vendorName()} says the account is out of credit or busy. Using the phone's voice for now."
                    else -> "${vendorName()} answered ${r.code}. Using the phone's voice for now."
                })
                return false
            }
            val t = track ?: buildTrack().also { track = it }
            t.play()
            val input = r.body?.byteStream() ?: return true
            val buf = ByteArray(4800)
            while (gen == generation) {
                val n = input.read(buf)
                if (n < 0) break
                t.write(buf, 0, n - (n % 2))
            }
        }
        return true
    }

    private fun buildTrack(): AudioTrack {
        val min = AudioTrack.getMinBufferSize(24_000, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT)
        return AudioTrack.Builder()
            .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_ASSISTANT).setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
            .setAudioFormat(AudioFormat.Builder().setSampleRate(24_000).setEncoding(AudioFormat.ENCODING_PCM_16BIT).setChannelMask(AudioFormat.CHANNEL_OUT_MONO).build())
            .setBufferSizeInBytes(maxOf(min, 24_000))
            .setTransferMode(AudioTrack.MODE_STREAM)
            .build()
    }

    companion object {
        val OPENAI_VOICES = setOf("alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer", "verse")
        const val DEFAULT_ELEVENLABS_VOICE = "21m00Tcm4TlvDq8ikWAM"
    }
}

/**
 * iOS ByokVoice OpenAI Realtime listening: a short-lived client secret from the person's key, then the transcription
 * socket (gpt-4o-transcribe, server VAD 600 ms), 24 kHz PCM16 from the mic in 100 ms frames. One utterance per
 * [start], like [Dictation], so voice mode's turn loop is the same.
 */
class OpenAiListener(private val context: Context, private val apiKey: String) : VoiceListener {
    override var transcript by mutableStateOf("")
        private set
    override var isRecording by mutableStateOf(false)
        private set
    override var errorMessage by mutableStateOf<String?>(null)
    override var micLevel by mutableFloatStateOf(0f)
        private set
    override var onStopped: (() -> Unit)? = null

    private val main = Handler(Looper.getMainLooper())
    @Volatile private var socket: WebSocket? = null
    @Volatile private var recording = false
    private val partial = StringBuilder()
    private val noSpeech = Runnable { stop() }

    private fun session(): JSONObject {
        val transcription = JSONObject().put("model", "gpt-4o-transcribe")
        VoicePrefs.speechLanguage?.tag?.substringBefore('-')?.let { transcription.put("language", it) }
        return JSONObject().put("type", "transcription").put("audio", JSONObject().put("input", JSONObject()
            .put("format", JSONObject().put("type", "audio/pcm").put("rate", RATE))
            .put("noise_reduction", JSONObject().put("type", "near_field"))
            .put("transcription", transcription)
            .put("turn_detection", JSONObject().put("type", "server_vad").put("silence_duration_ms", 600))))
    }

    override fun start() {
        if (isRecording) return
        errorMessage = null
        transcript = ""
        partial.setLength(0)
        if (!hasMicPermission(context)) {
            errorMessage = "Microphone access is off. Enable it in Settings to talk to Allternit."
            return
        }
        isRecording = true
        main.postDelayed(noSpeech, 10_000)
        thread(name = "openai-realtime-open", isDaemon = true) { open() }
    }

    private fun open() {
        val secret = try {
            val body = JSONObject().put("expires_after", JSONObject().put("anchor", "created_at").put("seconds", 600)).put("session", session())
            vendorHttp.newCall(Request.Builder().url("https://api.openai.com/v1/realtime/client_secrets")
                .header("Authorization", "Bearer $apiKey").post(body.toString().toRequestBody("application/json".toMediaType())).build()).execute().use { r ->
                if (!r.isSuccessful) {
                    fail(when (r.code) {
                        401, 403 -> "OpenAI didn't accept your key. Check it in Settings › Voice."
                        402, 429 -> "OpenAI says the account is out of credit or busy."
                        else -> "OpenAI answered ${r.code}. Try again."
                    })
                    return
                }
                JSONObject(r.body?.string().orEmpty()).optString("value").ifEmpty { null }
            }
        } catch (e: Exception) {
            null
        }
        if (secret == null) { fail("OpenAI could not be reached. Check your connection and try again."); return }
        if (!isRecordingNow()) return
        val req = Request.Builder().url("wss://api.openai.com/v1/realtime?intent=transcription")
            .header("Sec-WebSocket-Protocol", "realtime, openai-insecure-api-key.$secret").build()
        socket = vendorHttp.newWebSocket(req, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                webSocket.send(JSONObject().put("type", "session.update").put("session", session()).toString())
                capture(webSocket)
            }
            override fun onMessage(webSocket: WebSocket, text: String) = onEvent(text)
            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                if (isRecordingNow()) fail("The OpenAI connection dropped. Try again.")
            }
        })
    }

    private fun isRecordingNow() = recordingFlag()
    private fun recordingFlag(): Boolean = isRecording

    @SuppressLint("MissingPermission")
    private fun capture(ws: WebSocket) {
        thread(name = "openai-realtime-mic", isDaemon = true) {
            val min = AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
            val rec = runCatching { AudioRecord(MediaRecorder.AudioSource.VOICE_RECOGNITION, RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT, maxOf(min, FRAME_BYTES * 4)) }.getOrNull()
            if (rec == null || rec.state != AudioRecord.STATE_INITIALIZED) { rec?.release(); fail("The microphone could not start."); return@thread }
            recording = true
            rec.startRecording()
            val buf = ByteArray(FRAME_BYTES)
            try {
                while (recording && socket === ws) {
                    val n = rec.read(buf, 0, buf.size)
                    if (n <= 0) continue
                    var sum = 0.0
                    var i = 0
                    while (i + 1 < n) { val s = ((buf[i + 1].toInt() shl 8) or (buf[i].toInt() and 0xff)).toShort().toDouble(); sum += s * s; i += 2 }
                    val rms = sqrt(sum / (n / 2).coerceAtLeast(1)) / 32768.0
                    main.post { micLevel = (rms * 6).toFloat().coerceIn(0f, 1f) }
                    ws.send(JSONObject().put("type", "input_audio_buffer.append").put("audio", Base64.encodeToString(buf, 0, n, Base64.NO_WRAP)).toString())
                }
            } finally {
                runCatching { rec.stop() }
                rec.release()
            }
        }
    }

    private fun onEvent(raw: String) {
        val ev = runCatching { JSONObject(raw) }.getOrNull() ?: return
        when (ev.optString("type")) {
            "input_audio_buffer.speech_started" -> main.post { main.removeCallbacks(noSpeech) }
            "conversation.item.input_audio_transcription.delta" -> {
                val d = ev.optString("delta")
                main.post { partial.append(d); transcript = partial.toString() }
            }
            "conversation.item.input_audio_transcription.completed" -> {
                val t = ev.optString("transcript")
                main.post { if (t.isNotBlank()) transcript = t; stop() }
            }
            "error" -> fail(ev.optJSONObject("error")?.optString("message")?.ifEmpty { null } ?: "OpenAI reported an error.")
        }
    }

    private fun fail(message: String) {
        main.post {
            if (!isRecording) return@post
            errorMessage = message
            stop()
        }
    }

    override fun stop() {
        main.removeCallbacks(noSpeech)
        recording = false
        socket?.close(1000, null)
        socket = null
        micLevel = 0f
        if (isRecording) {
            isRecording = false
            onStopped?.invoke()
        }
    }

    companion object {
        const val RATE = 24_000
        /** 100 ms of PCM16 at 24 kHz. */
        const val FRAME_BYTES = 4_800
    }
}
