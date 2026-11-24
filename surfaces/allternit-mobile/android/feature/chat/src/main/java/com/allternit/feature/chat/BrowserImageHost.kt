package com.allternit.feature.chat

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.graphics.asImageBitmap
import kotlinx.coroutines.launch

/** Shell-level host: hiding this view only detaches it; the job owns the WebView. */
@Composable
fun BrowserImageHost(viewModel: ChatViewModel) {
    val owner = viewModel.browserImages ?: return
    val request = owner.request ?: return
    if (!owner.confirmed && viewModel.imageChatVisible && viewModel.currentSessionId == request.session) {
        AlertDialog(
            onDismissRequest = { owner.cancel() },
            title = { Text("Make this image in ChatGPT?") },
            text = { Column(Modifier.verticalScroll(rememberScrollState())) {
                Text("This phone uses a separate ChatGPT website sign-in. Confirm the exact prompt:")
                Spacer(Modifier.height(12.dp)); Text(request.prompt)
            } },
            confirmButton = { TextButton(onClick = { viewModel.confirmBrowserImage() }) { Text("Confirm prompt") } },
            dismissButton = { TextButton(onClick = { owner.cancel() }) { Text("Cancel") } },
        )
    }
    if (owner.visible && owner.confirmed) {
        Dialog(onDismissRequest = { owner.visible = false }, properties = DialogProperties(usePlatformDefaultWidth = false)) {
            Surface(Modifier.fillMaxSize()) {
                Column {
                    Text(owner.label, Modifier.padding(12.dp))
                    Row {
                        TextButton(onClick = { owner.visible = false }) { Text("Return to chat") }
                        TextButton(onClick = { owner.resume() }) { Text("Resume") }
                        TextButton(onClick = { owner.cancel() }) { Text("Stop") }
                    }
                    owner.webView?.let { web ->
                        AndroidView(factory = {
                            (web.parent as? android.view.ViewGroup)?.removeView(web); web
                        }, modifier = Modifier.fillMaxWidth().weight(1f), onRelease = {
                            (it.parent as? android.view.ViewGroup)?.removeView(it)
                        })
                    }
                }
            }
        }
    }
}

@Composable
internal fun BrowserImageControls(viewModel: ChatViewModel) {
    val owner = viewModel.browserImages ?: return
    val request = owner.request ?: return
    if (!owner.confirmed || viewModel.currentSessionId != request.session) return
    Row {
        TextButton(onClick = { owner.visible = true }) { Text("Open image browser") }
        TextButton(onClick = { owner.cancel() }) { Text("Stop image") }
    }
}

/** Uses the actual PNG result; final browser output is never a WebView screenshot. */
@Composable
internal fun BrowserImageResult(call: ToolCallRecord) {
    val payload = call.browserImageResult ?: call.output
    val result = remember(payload) { runCatching { org.json.JSONObject(payload.orEmpty()) }.getOrNull() } ?: return
    val context = androidx.compose.ui.platform.LocalContext.current
    if (!result.optBoolean("ok")) return
    val source = result.optString("localImage").ifBlank { result.optString("image") }
    if (source.length > 12 * 1024 * 1024) return
    val bytes = remember(source) { runCatching {
        if (source.startsWith("data:image/png;base64,")) android.util.Base64.decode(source.substringAfter(','), android.util.Base64.DEFAULT)
        else {
            LocalImageStore.readImage(source)
        }
    }.getOrNull() } ?: return
    if (bytes.size > 8 * 1024 * 1024) return
    val bitmap by produceState<androidx.compose.ui.graphics.ImageBitmap?>(null, source) {
        value = kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.Default) {
            val bounds = android.graphics.BitmapFactory.Options().apply { inJustDecodeBounds = true }
            android.graphics.BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
            if (bounds.outWidth in 1..4096 && bounds.outHeight in 1..4096 && bounds.outWidth.toLong() * bounds.outHeight <= 16000000)
                android.graphics.BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap()
            else null
        }
    }
    val scope = rememberCoroutineScope()
    var error by remember { mutableStateOf<String?>(null) }
    val save = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.CreateDocument("image/png")) { uri ->
        if (uri != null) scope.launch {
            val saved = kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) {
                runCatching { checkNotNull(context.contentResolver.openOutputStream(uri)).use { it.write(bytes) } }.isSuccess
            }
            if (!saved) error = "Could not save the image. Choose another location."
        }
    }
    var opened by remember { mutableStateOf(false) }
    if (opened) Dialog(onDismissRequest = { opened = false }, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize()) { Column(Modifier.verticalScroll(rememberScrollState())) {
            TextButton(onClick = { opened = false }) { Text("Close image") }
            bitmap?.let { androidx.compose.foundation.Image(it, "Generated image", Modifier.fillMaxWidth().aspectRatio(it.width.toFloat() / it.height)) }
        } }
    }
    Column {
        bitmap?.let { androidx.compose.foundation.Image(it, "Generated image", Modifier.fillMaxWidth().aspectRatio(it.width.toFloat() / it.height)) }
        Row {
            TextButton(onClick = { save.launch("Generated image.png") }) { Text("Download") }
            TextButton(onClick = { opened = true }) { Text("Open") }
            TextButton(onClick = {
                val clipboard = context.getSystemService(android.content.Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager
                clipboard.setPrimaryClip(android.content.ClipData.newPlainText("Image prompt", call.input?.get("prompt")?.let { (it as? kotlinx.serialization.json.JsonPrimitive)?.content }.orEmpty()))
            }) { Text("Copy prompt") }
        }
        error?.let { Text(it) }
    }
}

/** Available from every chat, including a fresh chat after restart; no runtime fetch required. */
@Composable
fun BrowserImageLibrary(viewModel: ChatViewModel) {
    var open by remember { mutableStateOf(false) }
    TextButton(onClick = { open = true }) { Text("Outputs") }
    if (open) Dialog(onDismissRequest = { open = false }, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize()) {
            androidx.compose.foundation.lazy.LazyColumn(Modifier.padding(16.dp)) {
                item { TextButton(onClick = { open = false }) { Text("Close Outputs") } }
                if (viewModel.savedImageTurns.isEmpty()) item { Text("Images saved on this phone appear here.") }
                items(viewModel.savedImageTurns.size) { i ->
                    val row = viewModel.savedImageTurns[viewModel.savedImageTurns.lastIndex - i]
                    Column(Modifier.padding(vertical = 12.dp)) {
                        Text(row.getString("prompt"))
                        TextButton(onClick = { viewModel.loadSession(row.getString("session")); open = false }) { Text("Open original chat") }
                        val result = row.optJSONObject("result")
                        if (result?.optBoolean("ok") == true) BrowserImageResult(ToolCallRecord(row.getString("call"), "media_generate", ToolState.Done,
                            browserImageResult = result.toString(), input = kotlinx.serialization.json.buildJsonObject { put("prompt", kotlinx.serialization.json.JsonPrimitive(row.getString("prompt"))) }))
                        else Text(if (result == null) "Unfinished request — open the original chat. Requests never resend automatically." else result.optString("error"))
                    }
                }
            }
        }
    }
}
