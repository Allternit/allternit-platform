package com.allternit.feature.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.dp
import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.RuntimeCall
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/* Gap run C settings leftovers: the execution-mode switch (iOS RuntimeOperationsView) and the node join token (iOS NodesManagerView). */

private val MODE_COPY = mapOf(
    "plan" to ("Plan rail" to "Generate and inspect a plan before runtime changes are allowed."),
    "safe" to ("Safe rail" to "Require runtime rails and policy checks before direct execution."),
    "auto" to ("Auto rail" to "Execute directly when the runtime and rails permit the request."),
)

/** `GET/POST /runtime/execution-mode` on the person's computer (iOS RuntimeExecutionModeStatus: mode, supported_modes). */
@Composable
internal fun ExecutionModeCard(host: SettingsHost) {
    val scope = rememberCoroutineScope()
    var mode by remember { mutableStateOf<String?>(null) }
    var supported by remember { mutableStateOf(listOf("plan", "safe", "auto")) }
    var state by remember { mutableStateOf("loading") }
    var error by remember { mutableStateOf<String?>(null) }
    fun apply(r: RuntimeCall) {
        when (r) {
            is RuntimeCall.Ok -> {
                val o: JsonObject = r.body.obj
                mode = o.str("mode") ?: mode
                (o["supported_modes"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content }?.takeIf { it.isNotEmpty() }?.let { supported = it }
                state = "ready"
                error = null
            }
            RuntimeCall.NoComputer -> state = "nocomputer"
            is RuntimeCall.Http -> { state = "ready"; error = r.message ?: "Your computer answered ${r.status}." }
            is RuntimeCall.Failed -> { state = "ready"; error = r.message }
        }
    }
    androidx.compose.runtime.LaunchedEffect(Unit) { apply(host.backend.runtimeJson("GET", "/runtime/execution-mode", null)) }
    SLabel("Execution mode")
    when (state) {
        "loading" -> SNote("Waiting for the runtime to report the shared execution default.")
        "nocomputer" -> SNote("Start a computer to choose how your bots run.")
        else -> {
            SCard {
                supported.forEachIndexed { i, m ->
                    if (i > 0) SDivider()
                    val (title, detail) = MODE_COPY[m] ?: (m.replaceFirstChar { it.uppercase() } to "")
                    SRow(title, "runtime_mode_$m", detail = detail.ifBlank { null }, chevron = false, trailing = { if (mode == m) CheckMark() }, onClick = {
                        scope.launch { apply(host.backend.runtimeJson("POST", "/runtime/execution-mode", buildJsonObject { put("mode", m) }.toString())) }
                    })
                }
            }
            SError(error)
        }
    }
}

/** `POST nodes/token` (iOS NodeTokenResponse: node_id, token, install_command): the command to run on the machine that should join. */
@Composable
internal fun NodeJoinToken() {
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboardManager.current
    var command by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    SLabel("Join token")
    SNote("Make a token, then run the command on the machine you want to connect. The token works once.")
    Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        SButton(if (busy) "Making..." else "Make a join token", "nodes_token", enabled = !busy) {
            busy = true
            error = null
            scope.launch {
                val r = Net.send("POST", "nodes/token", buildJsonObject { })
                if (r is ApiResult.Ok) command = r.value.obj.str("install_command") ?: r.value.obj.str("token") else error = r.friendly("make a join token")
                busy = false
            }
        }
        command?.let { c -> SButton("Copy command", "nodes_token_copy") { clipboard.setText(AnnotatedString(c)) } }
    }
    command?.let { SNote("Run this on the node you want to connect:\n\n$it") }
    SError(error)
}
