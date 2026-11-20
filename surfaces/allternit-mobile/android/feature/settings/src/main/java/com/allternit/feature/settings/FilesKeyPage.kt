package com.allternit.feature.settings

import android.content.Context
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import com.allternit.core.files.FilesKeyRing
import com.allternit.feature.artifacts.friendly
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.coroutines.suspendCancellableCoroutine
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import kotlin.coroutines.resume
import java.io.IOException

/**
 * Settings > Files > Files key, the port of iOS `Features/Settings/FilesKeysView.swift` on the shared
 * [FilesKeyRing] (:core:files): this phone's state (ready, waiting with its code, ask, set up), the requests waiting
 * for approval (code recomputed here; a mismatched code can only be declined), approve after a confirm that names
 * the code, decline, remove a device after a confirm, and "Make a new key" when a removal left one needed. While the
 * code shows it checks every 5 seconds for the approval (iOS pollWhileWaiting).
 */
internal object FilesKeyNet {
    /** One cloud-api call for the ring (blocking: the ring runs on Dispatchers.IO). */
    fun call(method: String, path: String, body: String?): String = runBlocking {
        val json = body?.let { Json.parseToJsonElement(it) }
        when (val r = Net.cloud(method, path.trimStart('/'), json)) {
            is ApiResult.Ok -> r.value.toString()
            is ApiResult.HttpError -> throw FilesKeyRing.HttpException(r.status, r.message?.takeIf { it.isNotBlank() } ?: r.friendly("change your Files key"))
            is ApiResult.Network -> throw IOException("You're offline. Check your connection and try again.")
            else -> throw IOException("Allternit sent a reply this app couldn't read.")
        }
    }

    fun ring(context: Context) = FilesKeyRing(context, android.os.Build.MODEL ?: "Android phone", ::call)
}

/**
 * Files v2 calls to one computer over the owner's relay (`POST /api/v1/runtime-devices/:id/proxy`, the same
 * envelope :feature:computer's AciRelay sends), for "Give key" (iOS FilesKeysView.give / deliverUnlocking:
 * `GET /replicas/key`, then `PUT /replicas/key` with the sealed envelope). The envelope is sealed to the computer's
 * own X25519 key by [FilesKeyRing]; the relay only carries it. Blocking: the ring runs on Dispatchers.IO.
 */
internal object FilesKeyRelay {
    /** The computer holds a different key, so the runtime wants a fresh signed unlock token. */
    class Locked : IOException("This computer holds a different Files key.")

    fun call(cid: String, deviceId: String, unlock: String?, method: String, path: String, body: String?): String = runBlocking {
        val envelope = buildJsonObject {
            put("method", method)
            put("path", "/api/v1/files/v2$path")
            put("headers", buildJsonObject {
                put("accept", "application/json")
                put("x-allternit-device-id", deviceId)
                put("x-allternit-device-platform", "android")
                if (body != null) put("content-type", "application/json")
                if (unlock != null) put("x-allternit-unlock", unlock)
            })
            put("body", body ?: "")
            put("bodyEncoding", "utf8")
        }
        when (val r = Net.cloud("POST", "api/v1/runtime-devices/${FilesCloud.seg(cid)}/proxy", envelope)) {
            is ApiResult.Ok -> r.value.toString()
            is ApiResult.HttpError -> when {
                r.code == "protected_root_locked" -> throw Locked()
                r.status == 428 || r.code == "compute_required" -> throw IOException("This computer isn't connected. Turn it on, then try again.")
                r.status == 503 -> throw IOException("This computer isn't reachable right now. Wake it, then try again.")
                else -> throw FilesKeyRing.HttpException(r.status, r.message?.takeIf { it.isNotBlank() } ?: r.friendly("give this computer the Files key"))
            }
            is ApiResult.Network -> throw IOException("You're offline. Check your connection and try again.")
            else -> throw IOException("Allternit sent a reply this app couldn't read.")
        }
    }

    /** A signed unlock token (at most 5 minutes) from `POST /computer-files/:id/unlock`, after the owner check. Never logged or stored. */
    fun unlockToken(cid: String, deviceId: String): String = runBlocking {
        when (val r = Net.cloud("POST", "api/v1/computer-files/${FilesCloud.seg(cid)}/unlock", buildJsonObject { put("device_id", deviceId) })) {
            is ApiResult.Ok -> r.value.obj.str("token") ?: throw IOException("Allternit sent a reply this app couldn't read.")
            is ApiResult.HttpError -> throw FilesKeyRing.HttpException(r.status, r.message?.takeIf { it.isNotBlank() } ?: r.friendly("unlock this computer"))
            else -> throw IOException("You're offline. Check your connection and try again.")
        }
    }
}

/**
 * The owner check before an unlock token (iOS: Face ID / Touch ID or the passcode through LocalAuthentication):
 * the platform BiometricPrompt with the screen lock allowed. False when the user cancels.
 */
internal object FilesOwnerCheck {
    suspend fun confirm(context: Context, reason: String): Boolean {
        if (android.os.Build.VERSION.SDK_INT < android.os.Build.VERSION_CODES.Q) {
            throw IOException("Replacing the Files key on a computer needs Android 10 or later on this phone.")
        }
        return suspendCancellableCoroutine { cont ->
            val cancel = android.os.CancellationSignal()
            cont.invokeOnCancellation { cancel.cancel() }
            val builder = android.hardware.biometrics.BiometricPrompt.Builder(context).setTitle("Confirm it's you").setSubtitle(reason)
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.R) {
                builder.setAllowedAuthenticators(
                    android.hardware.biometrics.BiometricManager.Authenticators.BIOMETRIC_STRONG or android.hardware.biometrics.BiometricManager.Authenticators.DEVICE_CREDENTIAL,
                )
            } else {
                @Suppress("DEPRECATION")
                builder.setDeviceCredentialAllowed(true)
            }
            builder.build().authenticate(cancel, context.mainExecutor, object : android.hardware.biometrics.BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: android.hardware.biometrics.BiometricPrompt.AuthenticationResult?) {
                    if (cont.isActive) cont.resume(true)
                }
                override fun onAuthenticationError(errorCode: Int, errString: CharSequence?) {
                    if (cont.isActive) cont.resume(false)
                }
            })
        }
    }
}

private fun keyMessage(e: Throwable): String = when (e) {
    is FilesKeyRing.HttpException -> if (e.status == 401 || e.status == 403) "Sign in to Allternit again, then try once more." else e.message ?: "Something went wrong."
    else -> e.message ?: "Something went wrong."
}

@Composable
internal fun FilesKeysPage(back: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val ring = remember { FilesKeyNet.ring(context) }
    val c = AllternitTheme.colors
    var info by remember { mutableStateOf<FilesKeyRing.Info?>(null) }
    var state by remember { mutableStateOf<FilesKeyRing.State?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var notice by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf<String?>(null) }
    var confirming by remember { mutableStateOf<FilesKeyRing.Approval?>(null) }
    var removing by remember { mutableStateOf<FilesKeyRing.Device?>(null) }
    var nonce by remember { mutableStateOf(0) }
    var computers by remember { mutableStateOf<List<FilesCloud.Computer>>(emptyList()) }
    LaunchedEffect(Unit) { computers = runCatching { FilesCloud.computers() }.getOrDefault(emptyList()) }

    /**
     * Runs [action] against the computer; when it holds a different key the runtime wants a fresh unlock (the owner
     * check, then a signed token), so ask for it once and try again (iOS deliverUnlocking).
     */
    suspend fun deliverUnlocking(computer: FilesCloud.Computer, action: ((String, String, String?) -> String) -> Unit) {
        try {
            withContext(Dispatchers.IO) { action { m, p, b -> FilesKeyRelay.call(computer.id, ring.deviceId, null, m, p, b) } }
        } catch (e: FilesKeyRelay.Locked) {
            if (!FilesOwnerCheck.confirm(context, "Replace the Files key on ${computer.name}")) throw IOException("Cancelled. ${computer.name} keeps its old key.")
            val token = withContext(Dispatchers.IO) { FilesKeyRelay.unlockToken(computer.id, ring.deviceId) }
            withContext(Dispatchers.IO) { action { m, p, b -> FilesKeyRelay.call(computer.id, ring.deviceId, token, m, p, b) } }
        }
    }

    suspend fun refresh() {
        val result = withContext(Dispatchers.IO) { runCatching { ring.info().let { it to ring.state(it) } } }
        result.onSuccess { (i, s) -> info = i; state = s; error = null }
            .onFailure { if (state == null) state = FilesKeyRing.State.NeedsRequest; error = keyMessage(it) }
    }

    fun run(tag: String, action: () -> Unit) {
        if (busy != null) return
        busy = tag
        notice = null
        scope.launch {
            val r = withContext(Dispatchers.IO) { runCatching(action) }
            val kept = r.exceptionOrNull()?.let(::keyMessage)
            refresh()
            if (kept != null) error = kept
            busy = null
        }
    }

    /** Like [run], for actions that may ask for the owner check; the returned text becomes the notice. */
    fun act(tag: String, action: suspend () -> String?) {
        if (busy != null) return
        busy = tag
        notice = null
        scope.launch {
            val r = runCatching { action() }
            refresh()
            r.onSuccess { notice = it }.onFailure { error = keyMessage(it) }
            busy = null
        }
    }

    LaunchedEffect(nonce) { refresh() }
    val waitingCode = (state as? FilesKeyRing.State.Waiting)?.code
    LaunchedEffect(waitingCode) {
        while (waitingCode != null) {
            delay(5_000)
            refresh()
            if (state is FilesKeyRing.State.Ready) break
        }
    }
    val isReady = state is FilesKeyRing.State.Ready
    val approvals = info?.let { ring.approvals(it) }.orEmpty()

    SPage("Files key", tag = "settingsFilesKeys", onBack = back) {
        SLabel("Files key on this phone")
        SCard {
            Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                when (val s = state) {
                    null -> Text("Checking…", color = c.textSecondary, fontSize = 15.sp)
                    is FilesKeyRing.State.Ready -> {
                        Text("Status: Ready", color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.testTag("filesKeys_ready"))
                        Text("Key: ${s.keyId}", color = c.textSecondary, fontSize = 13.sp, fontFamily = FontFamily.Monospace)
                    }
                    is FilesKeyRing.State.Waiting -> {
                        Text("Approve this phone on another of your devices (Settings > Files > Files key on a phone, or the Devices tab in Files on the web or Desktop). It shows this code:", color = c.textPrimary, fontSize = 15.sp)
                        Text(FilesKeyRing.spaced(s.code), color = c.textPrimary, fontSize = 30.sp, fontWeight = FontWeight.SemiBold, fontFamily = FontFamily.Monospace, modifier = Modifier.testTag("filesKeys_code"))
                        Text("Waiting for approval…", color = c.textSecondary, fontSize = 13.sp)
                        SButton(if (busy == "ask") "Sending…" else "Send the request again", tag = "filesKeys_ask", enabled = busy == null) { run("ask") { ring.requestAccess() } }
                    }
                    FilesKeyRing.State.NeedsRequest -> {
                        Text("Your other devices already have the Files key. Ask one of them to share it with this phone.", color = c.textPrimary, fontSize = 15.sp)
                        SButton(if (busy == "ask") "Asking…" else "Ask for access", tag = "filesKeys_ask", enabled = busy == null) { run("ask") { ring.requestAccess() } }
                    }
                    FilesKeyRing.State.NoKey -> {
                        Text("Your account has no Files key yet. Make it on this phone, then approve your other devices from here.", color = c.textPrimary, fontSize = 15.sp)
                        SButton(if (busy == "create") "Setting up…" else "Set up the Files key", tag = "filesKeys_create", enabled = busy == null) { run("create") { ring.createFirstKey() } }
                    }
                }
                error?.let { Text(it, color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.testTag("filesKeys_error")) }
                notice?.let { Text(it, color = c.textSecondary, fontSize = 13.sp) }
            }
        }
        SNote("Your computers keep sealed copies (replicas) of chosen folders on each other. With the Files key, this phone opens a file from a copy while its computer sleeps, without waking it. The key never leaves your devices; Allternit stores only copies it can't open.")

        if (isReady && approvals.isNotEmpty()) {
            SLabel("Waiting for approval")
            SCard {
                approvals.forEachIndexed { i, a ->
                    if (i > 0) SDivider()
                    Column(Modifier.fillMaxWidth().padding(16.dp).testTag("filesKeys_request_$i"), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(a.request.deviceName, color = c.textPrimary, fontSize = 16.sp)
                        if (a.codeMatches) {
                            Text(FilesKeyRing.spaced(a.code), color = c.textPrimary, fontSize = 22.sp, fontFamily = FontFamily.Monospace)
                        } else {
                            Text("This request's code doesn't match its key. Don't approve it; decline it and ask the device to request again.", color = Danger, fontSize = 13.sp)
                        }
                        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            if (a.codeMatches) SButton("Approve", tag = "filesKeys_approve_$i", enabled = busy == null) { confirming = a }
                            SButton(if (busy == "decline.$i") "Declining…" else "Decline", tag = "filesKeys_decline_$i", enabled = busy == null) { run("decline.$i") { ring.decline(a) } }
                        }
                    }
                }
            }
            SNote("Approve a device only if it shows the same code as here.")
        }

        val i = info
        if (isReady && i != null && i.rotationNeeded) {
            SLabel("New key needed")
            SCard {
                Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text("A device was removed. Make a new Files key so it can't open copies made from now on. Your other devices get the new key automatically.", color = c.textSecondary, fontSize = 14.sp)
                    SButton(if (busy == "rotate") "Making…" else "Make a new key", tag = "filesKeys_rotate", enabled = busy == null) {
                        // New key for every listed device, then the computers among them get their new envelope over
                        // the relay (an asleep one gets it next time; iOS rotate).
                        act("rotate") {
                            withContext(Dispatchers.IO) { ring.rotate(i) }
                            val missed = computers.filter { c -> i.devices.any { it.deviceId == c.id } }.filter { c ->
                                runCatching { deliverUnlocking(c) { relay -> ring.deliver(relay, c.id) } }.isFailure
                            }.map { it.name }
                            if (missed.isEmpty()) null else "New key made. Give it to ${missed.joinToString(", ")} when they're on."
                        }
                    }
                }
            }
        }

        // iOS FilesKeysView "Computers": Give key hands a computer the key over the relay so it keeps sealed copies
        // for your other devices; Send again re-delivers the current envelope.
        if (isReady && i != null && computers.isNotEmpty()) {
            SLabel("Computers")
            SCard {
                computers.forEachIndexed { n, comp ->
                    if (n > 0) SDivider()
                    val has = i.deviceIds.contains(comp.id)
                    val tag = "give.${comp.id}"
                    SRow(
                        comp.name, "filesKeys_computer_$n",
                        detail = if (has) "Has the key" else "No key yet",
                        chevron = false,
                        trailing = ({
                            TextButton(onClick = {
                                act(tag) {
                                    deliverUnlocking(comp) { relay -> if (has) ring.deliver(relay, comp.id) else ring.giveComputer(relay) }
                                    "${comp.name} has the Files key."
                                }
                            }, enabled = busy == null, modifier = Modifier.testTag("filesKeys_give_$n")) {
                                Text(if (busy == tag) "Sending…" else if (has) "Send again" else "Give key")
                            }
                        }),
                    )
                }
            }
            SNote("A computer needs the key to keep sealed copies for your other computers.")
        }

        if (i != null && i.keyId != null && i.devices.isNotEmpty()) {
            SLabel("Devices with the Files key")
            SCard {
                i.devices.forEachIndexed { n, d ->
                    if (n > 0) SDivider()
                    val me = d.deviceId == ring.deviceId
                    SRow(
                        if (me) "${d.deviceName} (this phone)" else d.deviceName, "filesKeys_device_$n",
                        detail = if (d.hasCurrentKey) "Has the current key" else "Waiting for the new key",
                        chevron = false,
                        trailing = if (isReady) ({ TextButton(onClick = { removing = d }, enabled = busy == null) { Text("Remove", color = Danger) } }) else null,
                    )
                }
            }
        }
    }

    confirming?.let { a ->
        AlertDialog(
            onDismissRequest = { confirming = null },
            title = { Text("Give ${a.request.deviceName} the Files key?") },
            text = { Text("It can then open your replicas. Check that ${a.request.deviceName} shows ${FilesKeyRing.spaced(a.code)}.") },
            confirmButton = { TextButton(onClick = { confirming = null; run("approve") { ring.approve(a) } }) { Text("Approve") } },
            dismissButton = { TextButton(onClick = { confirming = null }) { Text("Cancel") } },
        )
    }
    removing?.let { d ->
        AlertDialog(
            onDismissRequest = { removing = null },
            title = { Text("Remove ${d.deviceName}?") },
            text = { Text("${d.deviceName} loses the Files key. Make a new key afterwards so it can't open new copies.") },
            confirmButton = { TextButton(onClick = { removing = null; run("remove") { ring.removeDevice(d.deviceId) } }) { Text("Remove", color = Danger) } },
            dismissButton = { TextButton(onClick = { removing = null }) { Text("Cancel") } },
        )
    }
}
