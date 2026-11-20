package com.allternit.feature.settings

import android.Manifest
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.LifecycleOwner
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import com.allternit.core.files.KeystoreBox
import com.allternit.core.files.SealedSecret
import com.allternit.core.files.VoiceHandoffCrypto
import com.google.mlkit.vision.barcode.BarcodeScannerOptions
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.common.InputImage
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import okhttp3.OkHttpClient
import okhttp3.Request
import org.json.JSONObject
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/*
 * Settings > Voice and dictation > Your own provider (iOS SettingsPages VoiceOwnProviderSection) and the voice key
 * hand-off sheet (iOS VoiceKeyScannerView.swift + Core/Voice/VoiceKeyHandoff.swift `VoiceKeyHandoffSession`).
 * The crypto is :core:files VoiceHandoffCrypto. Keys are Keystore-sealed on this phone ([VoiceKeys]), never in prefs
 * or logs, and go only to the vendor (api.openai.com, api.elevenlabs.io).
 */

internal enum class VoiceVendor(val id: String, val title: String, val keyPage: String) {
    OpenAI("openai", "OpenAI", "https://platform.openai.com/api-keys"),
    ElevenLabs("elevenlabs", "ElevenLabs", "https://elevenlabs.io/app/settings/api-keys");

    companion object {
        fun of(id: String) = entries.firstOrNull { it.id == id }
    }
}

/** The user's own voice keys, Keystore-sealed on this phone (iOS VoiceProviderKeys in the Keychain). */
internal object VoiceKeys {
    /** [updatedAt] is the change time in epoch ms (0 = saved before account sync existed), for VoiceKeySync. */
    class Saved(val apiKey: String, val voiceId: String?, val label: String, val updatedAt: Long = 0)

    private fun secret(context: Context, v: VoiceVendor) =
        SealedSecret(context.applicationContext, "voice-${v.id}", KeystoreBox("voice_keys_v1", "allternit-voice-keys-v1"))

    fun load(context: Context, v: VoiceVendor): Saved? = secret(context, v).read()?.let {
        runCatching { JSONObject(it) }.getOrNull()?.let { o ->
            Saved(o.optString("api_key"), o.optString("voice_id").ifEmpty { null }, o.optString("label"), o.optLong("updated_at", 0)).takeIf { s -> s.apiKey.isNotBlank() }
        }
    }

    /**
     * A key connected on this phone (paste or QR hand-off): sealed, Where voice runs switches to it, the vendor's
     * opt-out clears, and with "Save to my Allternit account" on it is sent up (iOS VoiceKeySync.localChanged).
     */
    fun store(context: Context, v: VoiceVendor, apiKey: String, voiceId: String?, label: String) {
        write(context, v, apiKey, voiceId, label, System.currentTimeMillis())
        // Where voice runs (iOS SettingsStore.voiceRoute). Not a secret.
        SettingsPrefs.setString(context, "voice.route", v.id)
        VoiceKeySync.setOptedOut(context, v, false)
        VoiceKeySync.localChanged(context, v)
    }

    /** A key pulled from the account, or the server's change time adopted: never touches Where voice runs. */
    fun write(context: Context, v: VoiceVendor, apiKey: String, voiceId: String?, label: String, updatedAt: Long) {
        secret(context, v).write(
            JSONObject().put("api_key", apiKey.trim()).put("voice_id", voiceId ?: "").put("label", label).put("updated_at", updatedAt).toString(),
        )
    }

    /** Disconnect: this phone only. The vendor is opted out so a pull won't bring it back; the account copy stays. */
    fun remove(context: Context, v: VoiceVendor) {
        secret(context, v).write(null)
        VoiceKeySync.setOptedOut(context, v, true)
        if (SettingsPrefs.string(context, "voice.route") == v.id) SettingsPrefs.setString(context, "voice.route", "")
    }

    /** "sk-…abcd" (iOS VoiceProviderKeys.masked). */
    fun masked(key: String): String {
        val k = key.trim()
        return if (k.length <= 8) "••••" else k.take(3) + "…" + k.takeLast(4)
    }
}

/** iOS VoiceVendorClient.checkKey: one GET to the vendor with the key. */
internal object VoiceVendorCheck {
    sealed class Result {
        data class Ok(val label: String) : Result()
        data class Rejected(val message: String) : Result()
        data class Network(val message: String) : Result()
    }

    private val http by lazy { OkHttpClient.Builder().callTimeout(15, TimeUnit.SECONDS).build() }

    private fun get(v: VoiceVendor, path: String, key: String): Pair<Int, String>? = runCatching {
        val base = if (v == VoiceVendor.OpenAI) "https://api.openai.com/" else "https://api.elevenlabs.io/"
        val b = Request.Builder().url(base + path).header("Accept", "application/json")
        if (v == VoiceVendor.OpenAI) b.header("Authorization", "Bearer $key") else b.header("xi-api-key", key)
        http.newCall(b.build()).execute().use { it.code to (it.body?.string() ?: "") }
    }.getOrNull()

    private fun rejected(v: VoiceVendor) = Result.Rejected("${v.title} didn't accept this key. Copy it again from your ${v.title} account.")
    private fun unreachable(v: VoiceVendor) = Result.Network("${v.title} could not be reached. Check your connection and try again.")

    suspend fun check(v: VoiceVendor, raw: String): Result = withContext(Dispatchers.IO) {
        val key = raw.trim()
        if (key.isEmpty()) return@withContext Result.Rejected("Paste your ${v.title} key first.")
        when (v) {
            VoiceVendor.OpenAI -> {
                val (status, _) = get(v, "v1/models", key) ?: return@withContext unreachable(v)
                when (status) {
                    in 200..299 -> Result.Ok(VoiceKeys.masked(key))
                    401 -> rejected(v)
                    403 -> Result.Ok("restricted key")
                    429 -> Result.Ok("${VoiceKeys.masked(key)} · check your OpenAI credit")
                    else -> Result.Network("OpenAI could not check the key right now (error $status). Try again.")
                }
            }
            VoiceVendor.ElevenLabs -> {
                val (status, body) = get(v, "v1/user", key) ?: return@withContext unreachable(v)
                val missing = { b: String -> b.contains("missing_permissions") }
                when {
                    status in 200..299 -> Result.Ok("ElevenLabs key")
                    status == 403 || missing(body) -> {
                        val (vs, vb) = get(v, "v1/voices", key) ?: return@withContext unreachable(v)
                        if (vs in 200..299 || vs == 403 || missing(vb)) Result.Ok("restricted key")
                        else if (vs == 401) rejected(v) else Result.Network("ElevenLabs could not check the key right now (error $vs). Try again.")
                    }
                    status == 401 -> rejected(v)
                    else -> Result.Network("ElevenLabs could not check the key right now (error $status). Try again.")
                }
            }
        }
    }
}

/** One hand-off, from scanning QR 1 to a key saved on this phone (iOS VoiceKeyHandoffSession). */
internal class VoiceHandoffSession(private val context: Context, private val scope: CoroutineScope) {
    sealed class Phase {
        data object Scanning : Phase()
        data object Registering : Phase()
        data class Confirm(val code: String) : Phase()
        data class Checking(val vendor: VoiceVendor) : Phase()
        data class Done(val vendor: VoiceVendor, val label: String) : Phase()
        data class Failed(val message: String) : Phase()
    }

    var phase by mutableStateOf<Phase>(Phase.Scanning)
        private set
    private var sid: String? = null
    private var senderKey: ByteArray? = null
    private var ephemeral: VoiceHandoffCrypto.Ephemeral? = null
    private var timeoutJob: Job? = null
    private var pollJob: Job? = null

    private fun forgetKey() {
        ephemeral?.close()
        ephemeral = null
    }

    fun restart() {
        timeoutJob?.cancel(); pollJob?.cancel(); forgetKey()
        sid = null; senderKey = null
        phase = Phase.Scanning
    }

    fun cancel() {
        timeoutJob?.cancel(); pollJob?.cancel(); forgetKey()
    }

    /** Every QR string the scanner reads lands here; anything else is ignored. */
    fun scanned(text: String) {
        val qr = VoiceHandoffCrypto.QR.parse(text) ?: return
        val p = phase
        when {
            p is Phase.Scanning && qr is VoiceHandoffCrypto.QR.Offer -> { phase = Phase.Registering; scope.launch { register(qr.sid, qr.senderKey) } }
            p is Phase.Confirm && qr is VoiceHandoffCrypto.QR.Sealed && qr.sid == sid -> scope.launch { receive(qr.nonce, qr.ciphertext) }
        }
    }

    private suspend fun register(sid: String, sender: ByteArray) {
        val eph = try { withContext(Dispatchers.Default) { VoiceHandoffCrypto.Ephemeral.create() } } catch (e: Exception) {
            phase = Phase.Failed("This phone couldn't make a key for the handoff. Try again."); return
        }
        val body = buildJsonObject { put("pubkey", VoiceHandoffCrypto.b64url(eph.publicKey)) }
        when (val r = Net.cloud("PUT", "api/v1/voice/handoff/${enc(sid)}/receiver", body)) {
            is ApiResult.Ok, is ApiResult.Decode -> Unit
            is ApiResult.HttpError -> { eph.close(); phase = Phase.Failed(relayMessage(r.status)); return }
            is ApiResult.Network -> { eph.close(); phase = Phase.Failed("No connection. Check your internet and try again."); return }
        }
        this.sid = sid
        this.senderKey = sender
        this.ephemeral = eph
        phase = Phase.Confirm(VoiceHandoffCrypto.confirmCode(sid, sender, eph.publicKey))
        timeoutJob?.cancel()
        timeoutJob = scope.launch {
            delay(TIMEOUT_MS)
            if (phase is Phase.Confirm) { forgetKey(); phase = Phase.Failed(EXPIRED) }
        }
        pollJob?.cancel()
        pollJob = scope.launch {
            while (phase is Phase.Confirm) {
                val r = Net.cloud("GET", "api/v1/voice/handoff/${enc(sid)}/sealed")
                if (phase !is Phase.Confirm) return@launch
                when {
                    r is ApiResult.Ok && r.value is JsonObject -> {
                        val o = r.value as JsonObject
                        val nonce = o["nonce"]?.jsonPrimitive?.content?.let(VoiceHandoffCrypto::unb64url)
                        val ct = o["ciphertext"]?.jsonPrimitive?.content?.let(VoiceHandoffCrypto::unb64url)
                        if (nonce != null && ct != null) { receive(nonce, ct); return@launch }
                    }
                    r is ApiResult.HttpError && r.status == 404 -> { timeoutJob?.cancel(); forgetKey(); phase = Phase.Failed(EXPIRED); return@launch }
                    // 409: already taken; QR 2 still works.
                    r is ApiResult.HttpError && r.status == 409 -> return@launch
                    else -> Unit // 204 waiting (empty body), or a network blip: keep polling.
                }
                delay(POLL_MS)
            }
        }
    }

    private suspend fun receive(nonce: ByteArray, ciphertext: ByteArray) {
        val eph = ephemeral ?: return
        val sender = senderKey ?: return
        val sid = sid ?: return
        val payload = try {
            withContext(Dispatchers.Default) {
                val key = VoiceHandoffCrypto.sessionKey(eph, sender, sid)
                try { VoiceHandoffCrypto.open(nonce, ciphertext, key) } finally { key.fill(0) }
            }
        } catch (e: Exception) {
            phase = Phase.Failed("This phone couldn't open the key. Start again on your computer."); return
        }
        timeoutJob?.cancel(); pollJob?.cancel()
        forgetKey()
        val vendor = VoiceVendor.of(payload.provider) ?: return
        phase = Phase.Checking(vendor)
        val label = when (val check = VoiceVendorCheck.check(vendor, payload.apiKey)) {
            is VoiceVendorCheck.Result.Rejected -> { phase = Phase.Failed(check.message); return }
            is VoiceVendorCheck.Result.Ok -> check.label
            is VoiceVendorCheck.Result.Network -> "Saved, not checked"
        }
        withContext(Dispatchers.IO) { VoiceKeys.store(context, vendor, payload.apiKey, payload.voiceId, label) }
        phase = Phase.Done(vendor, label)
    }

    private fun enc(s: String) = java.net.URLEncoder.encode(s, "UTF-8").replace("+", "%20")

    companion object {
        const val TIMEOUT_MS = 120_000L
        const val POLL_MS = 1_500L
        const val EXPIRED = "The code expired. Start again on your computer."

        fun relayMessage(status: Int) = when (status) {
            401, 403 -> "Sign in to Allternit on this phone, with the same account as your computer, then try again."
            404 -> "This code has expired or belongs to another Allternit account. Sign in to the same account on both devices and start again on your computer."
            409 -> "This code was already used. Start again on your computer."
            else -> "Allternit couldn't start the handoff (error $status). Try again."
        }
    }
}

/** Settings > Voice and dictation: Your own provider (vendor rows, Scan from my computer). */
@Composable
internal fun VoicePage(back: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val c = AllternitTheme.colors
    val openLink = rememberLinkOpener()
    var revision by remember { mutableStateOf(0) }
    var scanning by remember { mutableStateOf(false) }
    var pasting by remember { mutableStateOf<VoiceVendor?>(null) }
    var draft by remember { mutableStateOf("") }
    var checking by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val saved = remember(revision) { VoiceVendor.entries.associateWith { VoiceKeys.load(context, it) } }
    val sync by VoiceKeySync.state
    val signedIn = com.allternit.core.auth.AllternitAuth.state.value is com.allternit.core.auth.AuthState.SignedIn
    // iOS: pull once and merge on Voice page open (the newer copy of each vendor wins).
    LaunchedEffect(Unit) { if (signedIn && VoiceKeySync.sync(context)) revision++ }

    SPage("Voice and dictation", tag = "settingsVoice", onBack = back) {
        SLabel("Your own provider")
        SNote(
            if (sync.enabled) "Optional. For a different voice, connect your own account. Your key is kept on this phone, sealed by Android's keystore, and in your Allternit account, encrypted. Voice calls go straight to that company. You pay them directly."
            else "Optional. For a different voice, connect your own account. Your key is kept only on this phone, sealed by Android's keystore, and goes straight to that company, never to Allternit. You pay them directly.",
        )
        SCard {
            VoiceVendor.entries.forEachIndexed { i, v ->
                if (i > 0) SDivider()
                val s = saved[v]
                SRow(
                    v.title, "voice_vendor_${v.id}",
                    detail = if (s != null) "● Connected · ${s.label}" else "Not connected",
                    chevron = false,
                    trailing = ({
                        if (s != null) TextButton(onClick = { VoiceKeys.remove(context, v); revision++ }) { Text("Disconnect", color = Danger) }
                        else TextButton(onClick = { openLink(v.keyPage); draft = ""; error = null; pasting = v }) { Text("Connect") }
                    }),
                )
            }
        }
        SCard(Modifier.padding(top = 12.dp)) {
            SRow("Scan from my computer", "voice_scanRow", detail = "Already set up on ai.allternit.com or Allternit Desktop? Show the QR code there and scan it. The key is sent end-to-end encrypted. Allternit can't read it.", onClick = { scanning = true })
        }
        SCard(Modifier.padding(top = 12.dp)) {
            SToggleRow(
                "Save to my Allternit account", "voice_accountSync", sync.enabled,
                onChange = { on -> scope.launch { VoiceKeySync.setEnabled(context, on); revision++ } },
                detail = if (signedIn) "Off by default. Your other signed-in devices get your keys." else "Sign in to Allternit to save keys to your account.",
                enabled = signedIn && !sync.busy,
            )
        }
        sync.error?.let { SError(it) }
        SNote("Disconnect removes a key from this phone only. To cut a key off everywhere, revoke it in your OpenAI or ElevenLabs account.")
        // iOS order: own provider, speech to text, reply voice (VoicePackSections.kt), then the live mode look.
        SpeechToTextSection()
        ReplyVoiceSection()
        com.allternit.feature.chat.voice.LiveModeLookSection(modifier = Modifier.padding(top = 18.dp), inset = 6.dp)
    }

    pasting?.let { v ->
        AlertDialog(
            onDismissRequest = { if (!checking) pasting = null },
            title = { Text("Your ${v.title} key") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("Copy a key from your ${v.title} account, then paste it here. It is checked with ${v.title} and kept on this phone.", color = c.textSecondary, fontSize = 13.sp)
                    SField("API key", draft, "voice_keyField", onChange = { draft = it }, enabled = !checking)
                    error?.let { Text(it, color = Danger, fontSize = 13.sp) }
                }
            },
            confirmButton = {
                TextButton(enabled = !checking && draft.isNotBlank(), onClick = {
                    checking = true
                    scope.launch {
                        val label = when (val r = VoiceVendorCheck.check(v, draft)) {
                            is VoiceVendorCheck.Result.Rejected -> { error = r.message; null }
                            is VoiceVendorCheck.Result.Network -> { error = r.message; null }
                            is VoiceVendorCheck.Result.Ok -> r.label
                        }
                        if (label != null) {
                            withContext(Dispatchers.IO) { VoiceKeys.store(context, v, draft, null, label) }
                            draft = ""
                            pasting = null
                            revision++
                        }
                        checking = false
                    }
                }) { Text(if (checking) "Checking…" else "Save") }
            },
            dismissButton = { TextButton(enabled = !checking, onClick = { draft = ""; pasting = null }) { Text("Cancel") } },
        )
    }
    if (scanning) VoiceKeyHandoffSheet(onClose = { scanning = false; revision++ })
}

private fun Context.lifecycleOwner(): LifecycleOwner? {
    var c: Context? = this
    while (c != null) {
        if (c is LifecycleOwner) return c
        c = (c as? ContextWrapper)?.baseContext
    }
    return null
}

/** Settings > Voice > Scan from my computer (iOS VoiceKeyHandoffSheet): the camera reads QR 1, shows the code, then the key arrives. */
@Composable
internal fun VoiceKeyHandoffSheet(onClose: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val c = AllternitTheme.colors
    val session = remember { VoiceHandoffSession(context.applicationContext, scope) }
    var granted by remember { mutableStateOf(ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) }
    var asked by remember { mutableStateOf(false) }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted = it; asked = true }
    val hasCamera = remember { context.packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY) }
    LaunchedEffect(Unit) { if (!granted && hasCamera) ask.launch(Manifest.permission.CAMERA) }
    DisposableEffect(Unit) { onDispose { session.cancel() } }
    val phase = session.phase
    val done = phase is VoiceHandoffSession.Phase.Done
    val close = { session.cancel(); onClose() }

    AlertDialog(
        onDismissRequest = close,
        modifier = Modifier.testTag("voiceHandoffSheet"),
        title = { Text("Scan from my computer") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(14.dp)) {
                Text(
                    if (phase is VoiceHandoffSession.Phase.Confirm) "Check the code matches, then press Codes match on your computer. The key arrives here by itself."
                    else "On your computer, open Settings › Voice in Allternit and click Send to my phone. Then point this camera at the QR code.",
                    color = c.textSecondary, fontSize = 15.sp,
                )
                Box(
                    Modifier.fillMaxWidth().aspectRatio(1f).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral)
                        .border(1.dp, c.borderSubtle, RoundedCornerShape(12.dp)),
                    contentAlignment = Alignment.Center,
                ) {
                    when {
                        done -> Text("✓", color = c.textPrimary, fontSize = 44.sp)
                        !hasCamera -> Text("This device has no camera. Paste or type the key instead.", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(24.dp))
                        granted -> QrScanner { session.scanned(it) }
                        asked -> Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.padding(24.dp)) {
                            Text("Camera access is off. Turn it on in Settings › Apps › Allternit to scan the code, or paste the key instead.", color = c.textSecondary, fontSize = 14.sp)
                            TextButton(onClick = {
                                context.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                            }) { Text("Open Settings") }
                        }
                        else -> Text("…", color = c.textSecondary)
                    }
                }
                when (phase) {
                    VoiceHandoffSession.Phase.Scanning -> Unit
                    VoiceHandoffSession.Phase.Registering -> Text("Connecting to your computer…", color = c.textSecondary, fontSize = 14.sp)
                    is VoiceHandoffSession.Phase.Confirm -> Column(
                        Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).padding(14.dp),
                        verticalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        Text("Check this matches your computer", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                        Text("${phase.code.take(3)} ${phase.code.takeLast(3)}", color = c.textPrimary, fontSize = 34.sp, fontWeight = FontWeight.SemiBold, fontFamily = FontFamily.Monospace, modifier = Modifier.testTag("voiceHandoff_code"))
                        Text("If the numbers differ, cancel. Someone else may be showing a code.", color = c.textSecondary, fontSize = 13.sp)
                    }
                    is VoiceHandoffSession.Phase.Checking -> Text("Checking your ${phase.vendor.title} key…", color = c.textSecondary, fontSize = 14.sp)
                    is VoiceHandoffSession.Phase.Done -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Text("● Connected · ${phase.label}", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                        Text("Your ${phase.vendor.title} key is saved on this phone, and voice now runs on your own ${phase.vendor.title}.", color = c.textSecondary, fontSize = 13.sp)
                    }
                    is VoiceHandoffSession.Phase.Failed -> Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                        SError(phase.message)
                        TextButton(onClick = { session.restart() }, modifier = Modifier.testTag("voiceHandoff_retry")) { Text("Try again") }
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = close, modifier = Modifier.testTag("voiceHandoff_close")) { Text(if (done) "Done" else "Cancel") } },
    )
}

/** CameraX preview + ML Kit QR reading (bundled model). Calls [onCode] on the main thread for every QR it reads. */
@Composable
private fun QrScanner(onCode: (String) -> Unit) {
    val context = LocalContext.current
    val owner = remember { context.lifecycleOwner() }
    val executor = remember { Executors.newSingleThreadExecutor() }
    val scanner = remember { BarcodeScanning.getClient(BarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build()) }
    var provider by remember { mutableStateOf<ProcessCameraProvider?>(null) }
    DisposableEffect(Unit) {
        onDispose {
            provider?.unbindAll()
            scanner.close()
            executor.shutdown()
        }
    }
    if (owner == null) {
        Text("The camera isn't available here. Paste or type the key instead.", color = AllternitTheme.colors.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(24.dp))
        return
    }
    AndroidView(
        modifier = Modifier.fillMaxWidth().aspectRatio(1f).testTag("voiceHandoff_camera"),
        factory = { ctx ->
            val view = PreviewView(ctx)
            val future = ProcessCameraProvider.getInstance(ctx)
            future.addListener({
                val p = future.get()
                provider = p
                val preview = Preview.Builder().build().also { it.setSurfaceProvider(view.surfaceProvider) }
                val analysis = ImageAnalysis.Builder().setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST).build()
                analysis.setAnalyzer(executor) { proxy ->
                    val image = InputImage.fromBitmap(proxy.toBitmap(), proxy.imageInfo.rotationDegrees)
                    scanner.process(image)
                        .addOnSuccessListener { codes -> codes.firstNotNullOfOrNull { it.rawValue }?.let(onCode) }
                        .addOnCompleteListener { proxy.close() }
                }
                runCatching {
                    p.unbindAll()
                    p.bindToLifecycle(owner, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis)
                }
            }, ContextCompat.getMainExecutor(ctx))
            view
        },
    )
}
