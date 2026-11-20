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
                        run("rotate") { ring.rotate(i) }
                    }
                }
            }
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
