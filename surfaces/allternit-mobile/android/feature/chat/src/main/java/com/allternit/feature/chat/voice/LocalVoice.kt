package com.allternit.feature.chat.voice

import android.annotation.SuppressLint
import android.content.ComponentCallbacks2
import android.content.Context
import android.content.res.Configuration
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.MediaRecorder
import android.os.Handler
import android.os.Looper
import android.util.Log
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.k2fsa.sherpa.onnx.FeatureConfig
import com.k2fsa.sherpa.onnx.OfflineModelConfig
import com.k2fsa.sherpa.onnx.OfflineMoonshineModelConfig
import com.k2fsa.sherpa.onnx.OfflineRecognizer
import com.k2fsa.sherpa.onnx.OfflineRecognizerConfig
import com.k2fsa.sherpa.onnx.OfflineTts
import com.k2fsa.sherpa.onnx.OfflineTtsConfig
import com.k2fsa.sherpa.onnx.OfflineTtsKokoroModelConfig
import com.k2fsa.sherpa.onnx.OfflineTtsModelConfig
import com.k2fsa.sherpa.onnx.SileroVadModelConfig
import com.k2fsa.sherpa.onnx.Vad
import com.k2fsa.sherpa.onnx.VadModelConfig
import java.util.concurrent.LinkedBlockingQueue
import kotlin.concurrent.thread
import kotlin.math.sqrt

/*
 * iOS Core/Voice LocalVoiceEngine + LocalVoiceBackends + KokoroSpeech on sherpa-onnx 1.13.8 (the version iOS and the
 * native engine pin). Listening: Silero VAD (threshold 0.5, 300 ms min silence, 250 ms min speech, 512-sample windows,
 * 20 s max) cuts segments that Moonshine transcribes; a turn ends 700 ms after the last speech. Replies: Kokoro-82M.
 * Android voice mode is turn based (one utterance per VoiceListener.start), so the engine is a listener + a speaker
 * instead of iOS's in-process socket; the models, thresholds and turn rule are the same.
 */

private const val TAG = "LocalVoice"

/** iOS KokoroVoices: Kokoro's stock English voices (same ids as the web and the bot API). */
object KokoroVoices {
    const val DEFAULT_ID = "af_heart"
    val all = listOf(
        "af_heart", "af_alloy", "af_aoede", "af_bella", "af_jessica", "af_kore", "af_nicole", "af_nova",
        "af_river", "af_sarah", "af_sky", "am_adam", "am_echo", "am_eric", "am_fenrir", "am_liam",
        "am_michael", "am_onyx", "am_puck", "am_santa", "bf_alice", "bf_emma", "bf_isabella", "bf_lily",
        "bm_daniel", "bm_fable", "bm_george", "bm_lewis",
    )

    /** kokoro-multi-lang-v1_0 speaker id: the voices in name order (services/voice tts.rs VOICES, sid == index). */
    fun sid(id: String): Int = all.sorted().indexOf(id).coerceAtLeast(0)

    /** The bot's own Kokoro voice, else this phone's pick, else af_heart. */
    fun resolve(bot: String?, setting: String?): String = listOf(bot, setting).firstOrNull { it != null && it in all } ?: DEFAULT_ID

    /** "Heart · American" for a settings chip. */
    fun label(id: String): String {
        val name = id.substringAfterLast('_').replaceFirstChar { it.uppercase() }
        return "$name · ${if (id.startsWith("b")) "British" else "American"}"
    }
}

/**
 * Process-wide models: Moonshine (~30 MB) loads once; Kokoro (~330 MB) loads on the first reply and is dropped when
 * Android asks for memory (iOS drops it on a memory warning). A load failure sends replies to the system voice.
 */
internal object LocalModels {
    private var recognizer: OfflineRecognizer? = null
    private var recognizerPaths: VoicePackPaths? = null
    private var tts: OfflineTts? = null
    @Volatile var ttsFailed = false
        private set
    private var callbacksRegistered = false

    @Synchronized
    fun recognizer(paths: VoicePackPaths): OfflineRecognizer {
        recognizer?.takeIf { recognizerPaths == paths }?.let { return it }
        recognizer?.release()
        val model = OfflineModelConfig(
            moonshine = OfflineMoonshineModelConfig(encoder = paths.encoder, mergedDecoder = paths.decoder),
            tokens = paths.tokens,
            numThreads = 2,
        )
        return OfflineRecognizer(config = OfflineRecognizerConfig(featConfig = FeatureConfig(), modelConfig = model)).also {
            recognizer = it
            recognizerPaths = paths
        }
    }

    @Synchronized
    fun transcribe(paths: VoicePackPaths, samples: FloatArray): String {
        val r = recognizer(paths)
        val stream = r.createStream()
        return try {
            stream.acceptWaveform(samples, LocalListener.SAMPLE_RATE)
            r.decode(stream)
            r.getResult(stream).text
        } finally {
            stream.release()
        }
    }

    fun newVad(model: String): Vad = Vad(
        config = VadModelConfig(
            sileroVadModelConfig = SileroVadModelConfig(
                model = model, threshold = 0.5f, minSilenceDuration = 0.3f, minSpeechDuration = 0.25f,
                windowSize = LocalListener.WINDOW, maxSpeechDuration = 20f,
            ),
            sampleRate = LocalListener.SAMPLE_RATE,
            numThreads = 1,
        ),
    )

    @Synchronized
    fun tts(context: Context): OfflineTts? {
        tts?.let { return it }
        if (ttsFailed) return null
        val paths = VoicePackManager.kokoro.kokoroPaths(context) ?: return null
        registerTrim(context)
        return try {
            val kokoro = OfflineTtsKokoroModelConfig(
                model = paths.model, voices = paths.voices, tokens = paths.tokens, dataDir = paths.dataDir,
                lexicon = paths.lexicon ?: "", lang = if (paths.lexicon != null) "en-us" else "",
            )
            OfflineTts(config = OfflineTtsConfig(model = OfflineTtsModelConfig(kokoro = kokoro, numThreads = 2))).also { tts = it }
        } catch (t: Throwable) {
            Log.i(TAG, "Kokoro unavailable, using the system voice: ${t.message}")
            ttsFailed = true
            null
        }
    }

    @Synchronized
    fun releaseTts() {
        tts?.release()
        tts = null
    }

    private fun registerTrim(context: Context) {
        if (callbacksRegistered) return
        callbacksRegistered = true
        context.applicationContext.registerComponentCallbacks(object : ComponentCallbacks2 {
            override fun onTrimMemory(level: Int) {
                @Suppress("DEPRECATION")
                if (level >= ComponentCallbacks2.TRIM_MEMORY_RUNNING_LOW) thread(isDaemon = true) { releaseTts() }
            }
            override fun onConfigurationChanged(newConfig: Configuration) {}
            @Deprecated("Deprecated in Java")
            override fun onLowMemory() { thread(isDaemon = true) { releaseTts() } }
        })
    }
}

/**
 * iOS LocalVoiceEngine's listening side: the echo-cancelled mic at 16 kHz → Silero VAD → Moonshine, one turn per
 * [start]. Downloads the listening pack first when it isn't on the phone yet ("Downloading the voice model…", iOS
 * VoiceModeViewModel), with progress in [status].
 */
class LocalListener(private val context: Context) : VoiceListener {
    override var transcript by mutableStateOf("")
        private set
    override var isRecording by mutableStateOf(false)
        private set
    override var errorMessage by mutableStateOf<String?>(null)
    override var micLevel by mutableFloatStateOf(0f)
        private set
    override var status by mutableStateOf<String?>(null)
        private set
    override var onStopped: (() -> Unit)? = null

    private val main = Handler(Looper.getMainLooper())
    @Volatile private var running = false
    @Volatile private var session = 0

    override fun start() {
        if (isRecording) return
        errorMessage = null
        transcript = ""
        if (!hasMicPermission(context)) {
            errorMessage = "Microphone access is off. Enable it in Settings to talk to Allternit."
            return
        }
        isRecording = true
        running = true
        val mine = ++session
        thread(name = "local-voice-listen", isDaemon = true) {
            try { run(mine) } catch (t: Throwable) {
                Log.w(TAG, "listening failed", t)
                fail(mine, "On-device listening stopped. Try again.")
            }
        }
    }

    override fun stop() {
        running = false
        micLevel = 0f
        if (isRecording) {
            isRecording = false
            onStopped?.invoke()
        }
    }

    private fun live(mine: Int) = running && session == mine

    private fun fail(mine: Int, message: String) {
        main.post {
            if (session != mine || !isRecording) return@post
            status = null
            errorMessage = message
            stop()
        }
    }

    private fun run(mine: Int) {
        val packs = VoicePackManager.listening
        if (!packs.isInstalled(context)) {
            main.post { status = "Downloading the voice model…" }
            val ok = packs.installBlocking(context, cellular = true) { p ->
                main.post { if (session == mine) status = "Downloading the voice model… ${(p * 100).toInt()}%" }
            }
            main.post { if (session == mine) status = null }
            if (!ok) {
                val message = (packs.state as? VoicePackState.Failed)?.message ?: "The on-device voice model isn't installed."
                fail(mine, message)
                return
            }
        }
        if (!live(mine)) return
        val paths = packs.listeningPaths(context) ?: return fail(mine, "The on-device voice model isn't installed.")
        runCatching { LocalModels.recognizer(paths) }.onFailure { return fail(mine, "The on-device voice model couldn't start.") }
        val vad = runCatching { LocalModels.newVad(paths.vad) }.getOrElse { return fail(mine, "The on-device voice model couldn't start.") }
        try { capture(mine, paths, vad) } finally { vad.release() }
    }

    @SuppressLint("MissingPermission")
    private fun capture(mine: Int, paths: VoicePackPaths, vad: Vad) {
        val min = AudioRecord.getMinBufferSize(SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        // VOICE_COMMUNICATION turns on the phone's echo canceller, so a reply still playing isn't transcribed (iOS).
        val rec = runCatching {
            AudioRecord(MediaRecorder.AudioSource.VOICE_COMMUNICATION, SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT, maxOf(min, WINDOW * 2 * 8))
        }.getOrNull()
        if (rec == null || rec.state != AudioRecord.STATE_INITIALIZED) { rec?.release(); return fail(mine, "The microphone could not start.") }
        val pcm = ShortArray(WINDOW)
        val window = FloatArray(WINDOW)
        val texts = ArrayList<String>()
        var samples = 0
        var inSpeech = false
        var heard = false
        var lastSegmentEnd = 0
        rec.startRecording()
        try {
            while (live(mine)) {
                var filled = 0
                while (filled < WINDOW && live(mine)) {
                    val n = rec.read(pcm, filled, WINDOW - filled)
                    if (n <= 0) break
                    filled += n
                }
                if (filled < WINDOW) { if (!live(mine)) break else continue }
                var sum = 0.0
                for (i in 0 until WINDOW) { val f = pcm[i] / 32768f; window[i] = f; sum += f * f }
                samples += WINDOW
                val level = (sqrt(sum / WINDOW) * 6).toFloat().coerceIn(0f, 1f)
                main.post { if (session == mine) micLevel = level }

                vad.acceptWaveform(window.copyOf())
                val speech = vad.isSpeechDetected()
                if (speech && !inSpeech) { inSpeech = true; heard = true } else if (!speech && inSpeech) inSpeech = false
                while (!vad.empty()) {
                    val segment = vad.front().samples
                    vad.pop()
                    val text = LocalModels.transcribe(paths, segment).trim()
                    lastSegmentEnd = samples
                    if (text.isEmpty()) continue
                    texts.add(text)
                    val joined = texts.joinToString(" ")
                    main.post { if (session == mine) transcript = joined }
                }
                val silentMs = ms(samples - lastSegmentEnd)
                // iOS turn rule: 700 ms after the last speech, the first 300 of which the VAD already waited.
                if (texts.isNotEmpty() && !inSpeech && silentMs >= TURN_SILENCE_MS - VAD_SILENCE_MS) break
                // Nothing said for 10 s, or speech that held no words: end this listen (Dictation does the same).
                if (!heard && ms(samples) >= 10_000) break
                if (heard && texts.isEmpty() && !inSpeech && silentMs >= 3_000) break
            }
        } finally {
            runCatching { rec.stop() }
            rec.release()
        }
        main.post { if (session == mine) stop() }
    }

    private fun ms(n: Int) = n * 1000 / SAMPLE_RATE

    companion object {
        const val SAMPLE_RATE = 16_000
        const val WINDOW = 512
        const val TURN_SILENCE_MS = 700
        const val VAD_SILENCE_MS = 300
    }
}

/**
 * iOS KokoroSpeechSynthesizer: each sentence goes to Kokoro and streams to an AudioTrack as it is made. If Kokoro
 * can't load (memory, a damaged pack) the rest is read by the system voice ([fallback]), as on iOS.
 */
class KokoroSpeaker(private val context: Context, private val voiceId: String, private val fallback: Speaker) : ReplySpeaker {
    override var isSpeaking by mutableStateOf(false)
        private set
    override var onFinished: (() -> Unit)? = null

    private val main = Handler(Looper.getMainLooper())
    private val queue = LinkedBlockingQueue<Pair<Int, String>>()
    @Volatile private var generation = 0
    @Volatile private var failed = false
    private var track: AudioTrack? = null
    private var trackRate = 0
    private val worker = thread(name = "kokoro-voice", isDaemon = true) { loop() }

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
            val tts = if (failed) null else LocalModels.tts(context)
            if (tts == null) {
                failed = true
                val rest = buildList { add(text); while (true) { val n = queue.poll() ?: break; if (n.first == gen) add(n.second) } }
                fallback.onFinished = { onFinished?.invoke() }
                main.post { if (gen == generation) { rest.forEach { fallback.speakChunk(it) }; isSpeaking = false } }
                continue
            }
            runCatching { speakOne(tts, gen, text) }.onFailure { Log.w(TAG, "Kokoro sentence failed", it) }
            if (queue.isEmpty() && gen == generation) {
                // Let the AudioTrack drain what it holds before saying "finished".
                Thread.sleep(250)
                main.post { if (gen == generation && queue.isEmpty()) { isSpeaking = false; onFinished?.invoke() } }
            }
        }
    }

    private fun speakOne(tts: OfflineTts, gen: Int, text: String) {
        val rate = tts.sampleRate()
        val t = track?.takeIf { trackRate == rate } ?: buildTrack(rate).also { track?.release(); track = it; trackRate = rate }
        t.play()
        tts.generateWithCallback(text, sid = KokoroVoices.sid(voiceId), speed = VoicePrefs.speechSpeed) { samples ->
            if (gen != generation) 0 else { t.write(samples, 0, samples.size, AudioTrack.WRITE_BLOCKING); 1 }
        }
    }

    private fun buildTrack(rate: Int): AudioTrack {
        val min = AudioTrack.getMinBufferSize(rate, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_FLOAT)
        return AudioTrack.Builder()
            .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_ASSISTANT).setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
            .setAudioFormat(AudioFormat.Builder().setSampleRate(rate).setEncoding(AudioFormat.ENCODING_PCM_FLOAT).setChannelMask(AudioFormat.CHANNEL_OUT_MONO).build())
            .setBufferSizeInBytes(maxOf(min, rate * 4 / 2))
            .setTransferMode(AudioTrack.MODE_STREAM)
            .build()
    }
}
