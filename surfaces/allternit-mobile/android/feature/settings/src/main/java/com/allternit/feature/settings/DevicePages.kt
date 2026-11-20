package com.allternit.feature.settings

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.launch

/* Device pages: Device Access (DeviceAccessSettingsView) and Devices (DevicePairingView). */

/** One part of the phone a bot may use (iOS DeviceAccessKind): off until the person turns it on. */
private data class AccessKind(val id: String, val title: String, val detail: String, val permissions: List<String>)

private val ACCESS_KINDS = listOf(
    AccessKind("camera", "Camera", "Let a bot take a photo when you ask.", listOf(Manifest.permission.CAMERA)),
    AccessKind("photos", "Photos", "Let a bot look at photos you pick.", if (android.os.Build.VERSION.SDK_INT >= 33) listOf(Manifest.permission.READ_MEDIA_IMAGES) else listOf(Manifest.permission.READ_EXTERNAL_STORAGE)),
    AccessKind("location", "Location", "Let a bot know roughly where you are.", listOf(Manifest.permission.ACCESS_COARSE_LOCATION)),
    AccessKind("contacts", "Contacts", "Let a bot look up a contact.", listOf(Manifest.permission.READ_CONTACTS)),
    AccessKind("calendar", "Calendar", "Let a bot read your calendar events.", listOf(Manifest.permission.READ_CALENDAR)),
)

/** Device Access: each kind is a switch; turning one on asks Android for its permission (the iOS "iOS asks when you turn it on"). */
@Composable
internal fun DeviceAccessPage(back: () -> Unit) {
    val context = LocalContext.current
    val enabled = remember { mutableStateMapOf<String, Boolean>().apply { ACCESS_KINDS.forEach { put(it.id, SettingsPrefs.deviceAccess(context, it.id)) } } }
    val granted = remember { mutableStateMapOf<String, Boolean>() }
    fun refresh() = ACCESS_KINDS.forEach { k -> granted[k.id] = k.permissions.all { ContextCompat.checkSelfPermission(context, it) == PackageManager.PERMISSION_GRANTED } }
    var pending by remember { mutableStateOf<AccessKind?>(null) }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { result ->
        pending?.let { k ->
            val ok = result.values.all { it }
            enabled[k.id] = ok
            SettingsPrefs.setDeviceAccess(context, k.id, ok)
        }
        pending = null
        refresh()
    }
    androidx.compose.runtime.LaunchedEffect(Unit) { refresh() }
    SPage("Device Access", tag = "settingsDeviceAccess", onBack = back) {
        SNote("Let your bots use parts of this phone. Each one is off until you turn it on, and you can turn it off at any time.")
        for (kind in ACCESS_KINDS) {
            val on = enabled[kind.id] == true
            val isGranted = granted[kind.id] == true
            SCard(Modifier.padding(top = 8.dp)) {
                SToggleRow(kind.title, "deviceAccess_${kind.id}", on, detail = kind.detail, onChange = { next ->
                    if (!next) {
                        enabled[kind.id] = false
                        SettingsPrefs.setDeviceAccess(context, kind.id, false)
                    } else if (isGranted) {
                        enabled[kind.id] = true
                        SettingsPrefs.setDeviceAccess(context, kind.id, true)
                    } else {
                        pending = kind
                        launcher.launch(kind.permissions.toTypedArray())
                    }
                })
                SDivider()
                SRow(
                    if (on && isGranted) "On. Android access allowed." else if (isGranted) "Off. Android access allowed." else "Not allowed in Android yet. Android asks when you turn it on.",
                    "deviceAccess_${kind.id}_status", chevron = false,
                )
            }
        }
        SNote("Bots reach your phone only while the Allternit app is open on it. If the app is in the background, a bot is told to ask you to open it.")
    }
}

/**
 * iOS DevicePairingView: the paired computers (`GET runtime-devices`), Remove (`DELETE runtime-devices/<id>`) and
 * Approve a device from the code `gizzi pair` shows (`runtime-pairings/code/<code>`, then `/approve` or `/deny`).
 */
@Composable
internal fun DevicePairingPage(host: SettingsHost, back: () -> Unit) {
    val scope = rememberCoroutineScope()
    val (load, reload) = rememberLoad("devices") { Net.get("runtime-devices").toLoad("load your devices") }
    var code by remember { mutableStateOf("") }
    var found by remember { mutableStateOf<kotlinx.serialization.json.JsonObject?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    var removeId by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    val normalized = code.trim().uppercase().replace(" ", "")
    SPage("Devices", tag = "settingsDevices", onBack = back) {
        SLabel("Approve a device")
        SNote("Run `gizzi pair` on the machine and type the code it shows.")
        SField("Pairing code", code, "devices_code", { code = it; found = null }, enabled = !busy)
        Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (found == null) {
                SButton(if (busy) "Looking..." else "Look up", "devices_lookup", enabled = normalized.length >= 4 && !busy) {
                    busy = true
                    message = null
                    scope.launch {
                        val r = Net.get("runtime-pairings/code/${LibraryNet.escape(normalized)}")
                        if (r is ApiResult.Ok) found = r.value.obj else message = r.friendly("look up that code")
                        busy = false
                    }
                }
            } else {
                SButton("Approve", "devices_approve", enabled = !busy) {
                    busy = true
                    scope.launch {
                        val r = Net.send("POST", "runtime-pairings/code/${LibraryNet.escape(normalized)}/approve")
                        if (r is ApiResult.Ok) { found = null; code = ""; message = "Device approved."; reload() } else message = r.friendly("approve the device")
                        busy = false
                    }
                }
                SButton("Deny", "devices_deny", destructive = true, enabled = !busy) {
                    busy = true
                    scope.launch {
                        val r = Net.send("POST", "runtime-pairings/code/${LibraryNet.escape(normalized)}/deny")
                        if (r is ApiResult.Ok) { found = null; code = ""; message = "Request denied." } else message = r.friendly("deny the request")
                        busy = false
                    }
                }
            }
        }
        found?.let { f -> SNote("${f.str("name") ?: f.str("device_name") ?: "A device"} wants to pair${f.str("platform")?.let { " ($it)" } ?: ""}.") }
        SError(message?.takeIf { it != "Device approved." && it != "Request denied." })
        if (message == "Device approved." || message == "Request denied.") SNote(message ?: "")
        SLabel("Paired devices")
        LoadView(load, reload, "load your devices") { body ->
            val devices = body.listUnder("devices", "items")
            if (devices.isEmpty()) {
                StateBox("No paired devices", "Install Allternit Desktop or run `gizzi pair` on a computer to connect it.", null, null)
            } else SCard {
                devices.forEachIndexed { i, d ->
                    val id = d.str("id") ?: return@forEachIndexed
                    if (i > 0) SDivider()
                    SRow(d.str("name") ?: d.str("device_name") ?: "Device", "device_$id", detail = d.str("public_key_fingerprint")?.let { "Key $it" } ?: d.str("platform"), chevron = false, onClick = { removeId = id },
                        trailing = { androidx.compose.material3.Text("Remove", color = Danger, fontSize = 14.sp) })
                }
            }
        }
        removeId?.let { id ->
            androidx.compose.material3.AlertDialog(
                onDismissRequest = { removeId = null },
                title = { androidx.compose.material3.Text("Remove this device?") },
                text = { androidx.compose.material3.Text("It stops reaching your Allternit account until you pair it again.") },
                confirmButton = {
                    androidx.compose.material3.TextButton(onClick = {
                        removeId = null
                        scope.launch {
                            val r = Net.send("DELETE", "runtime-devices/${LibraryNet.escape(id)}")
                            if (r is ApiResult.Ok) reload() else message = r.friendly("remove the device")
                        }
                    }) { androidx.compose.material3.Text("Remove", color = Danger) }
                },
                dismissButton = { androidx.compose.material3.TextButton(onClick = { removeId = null }) { androidx.compose.material3.Text("Cancel") } },
            )
        }
    }
}
