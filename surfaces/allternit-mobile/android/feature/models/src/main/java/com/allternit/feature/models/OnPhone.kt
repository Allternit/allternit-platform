package com.allternit.feature.models

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.str
import com.allternit.ondevice.ModelStore
import com.allternit.ondevice.PhoneModels
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonPrimitive
import okhttp3.OkHttpClient
import okhttp3.Request
import java.util.concurrent.TimeUnit
import com.allternit.core.design.R as DesignR
import com.allternit.core.design.EmptyArtKind
import com.allternit.core.design.InlineEmpty

/*
 * Gap run GS-D: "On this phone" (iOS OnThisIPhoneView) and Download on the model page (iOS
 * FreeweightsBrowseView device builds), on the :ondevice llama.cpp engine.
 */

/** One build that runs on a phone (iOS Quant, GGUF builds only: the Android engine is llama.cpp). */
data class PhoneQuant(val name: String, val sizeBytes: Long?, val file: String, val repo: String)

/**
 * One `stream_quants` entry (colibri pack that keeps `ramEst` resident and reads the rest from disk).
 * Shown only as a note for a Mac or PC; the phone never streams or installs one (iOS StreamQuant).
 */
data class StreamQuant(
    val name: String, val engine: String, val sizeBytes: Long, val ramEstBytes: Long, val tier: String, val bestFor: String?,
)

/** Everything `/device` says about a model: builds this phone can run, and packs a computer can stream. */
data class DeviceBuilds(val phone: List<PhoneQuant>, val stream: List<StreamQuant>)

/**
 * Tolerant decode of `stream_quants` (contract section 2): a missing key, a non-array, an unknown key or
 * an entry that lacks a required field (name, size, ram_est, tier) is skipped; its siblings are kept.
 */
internal fun parseStreamQuants(root: kotlinx.serialization.json.JsonElement?): List<StreamQuant> =
    root.listUnder("stream_quants").mapNotNull { q ->
        val name = q.str("name") ?: return@mapNotNull null
        val size = q.num("size") ?: return@mapNotNull null
        val ram = q.num("ram_est") ?: return@mapNotNull null
        val tier = q.str("tier") ?: return@mapNotNull null
        StreamQuant(name, q.str("engine").orEmpty(), size.toLong(), ram.toLong(), tier, q.str("best_for"))
    }

/** Best stream pack to mention: colibri only, chat tier first, then larger size, then name in byte order (fit.md "Sort order"). */
internal fun preferredStream(streams: List<StreamQuant>): StreamQuant? =
    streams.filter { it.engine == "colibri" }.sortedWith(
        compareByDescending<StreamQuant> { it.tier == "chat" }.thenByDescending { it.sizeBytes }
            .thenComparator { a, b -> a.name.toByteArray().let { x -> b.name.toByteArray().let { y -> java.util.Arrays.compareUnsigned(x, y) } } },
    ).firstOrNull()

private fun gbDecimal(bytes: Long): String = String.format(java.util.Locale.US, "%.1f GB", bytes / 1e9)

object DeviceBuildsApi {
    private const val SITE = "https://freeweights.allternit.com"
    private val http: OkHttpClient by lazy { OkHttpClient.Builder().connectTimeout(15, TimeUnit.SECONDS).readTimeout(30, TimeUnit.SECONDS).build() }

    /** `GET /api/models/{org}/{repo}/device` (iOS FreeweightsClient.deviceData), llama.cpp builds only for the phone; `stream_quants` are read for the computer note. */
    suspend fun builds(id: String): FwLoad<DeviceBuilds> = withContext(Dispatchers.IO) {
        val path = id.split('/').joinToString("/") { java.net.URLEncoder.encode(it, "UTF-8").replace("+", "%20") }
        try {
            http.newCall(Request.Builder().url("$SITE/api/models/$path/device").header("Accept", "application/json").build()).execute().use { r ->
                val body = r.body?.string().orEmpty()
                if (!r.isSuccessful) return@withContext FwLoad.Down(if (r.code == 404) "No phone builds for this model" else "Freeweights answered ${r.code}")
                val root = kotlinx.serialization.json.Json.parseToJsonElement(body)
                val phone = root.listUnder("quants").mapNotNull { q ->
                    val engine = q.str("engine").orEmpty().lowercase()
                    val format = q.str("format").orEmpty().lowercase()
                    if (!(engine.contains("llama") || format == "gguf")) return@mapNotNull null
                    val files = (q["files"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content }.orEmpty()
                    val file = files.firstOrNull { it.endsWith(".gguf", ignoreCase = true) } ?: return@mapNotNull null
                    // iOS Quant.size is in GB; a value this large is already bytes.
                    val size = q.num("size")?.let { s -> if (s > 100_000) s.toLong() else (s * 1e9).toLong() }
                    PhoneQuant(q.str("name") ?: file.substringAfterLast('/'), size, file, q.str("repo") ?: id)
                }
                FwLoad.Live(DeviceBuilds(phone, parseStreamQuants(root)))
            }
        } catch (e: Exception) {
            FwLoad.Down("Freeweights isn't reachable. Check your connection.")
        }
    }
}

private fun gbText(bytes: Long): String =
    if (bytes >= 1L shl 30) String.format("%.1f GB", bytes / (1L shl 30).toDouble()) else String.format("%d MB", bytes / (1L shl 20))

@Composable
private fun Bar(fraction: Float, tint: Color) {
    val c = AllternitTheme.colors
    Box(Modifier.fillMaxWidth().height(4.dp).clip(CircleShape).background(c.fillNeutral)) {
        Box(Modifier.fillMaxWidth(fraction.coerceIn(0f, 1f)).height(4.dp).clip(CircleShape).background(tint))
    }
}

@Composable
private fun SmallButton(label: String, tag: String, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Text(label, color = if (enabled) c.textPrimary else c.textTertiary, fontSize = 13.sp, fontWeight = FontWeight.Medium,
        modifier = Modifier.clip(RoundedCornerShape(8.dp)).border(BorderStroke(1.dp, c.borderStrong), RoundedCornerShape(8.dp))
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick).testTag(tag).padding(horizontal = 12.dp, vertical = 8.dp))
}

private fun statusText(d: PhoneModels.Download): String = when (d.status) {
    PhoneModels.Status.Queued -> "Waiting to start"
    PhoneModels.Status.Downloading -> "${gbText(d.done)} of ${gbText(d.total)}"
    PhoneModels.Status.Paused -> d.message ?: "Paused · ${gbText(d.done)} of ${gbText(d.total)}"
    PhoneModels.Status.Verifying -> "Checking against the Freeweights manifest"
    PhoneModels.Status.Failed -> d.message ?: "Download failed."
}

/** iOS OnThisIPhoneView: memory, the engine, downloads in progress, downloaded models with Use in chat and Remove. */
@Composable
internal fun OnThisPhoneScreen(host: ModelsHost, onBrowse: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    remember { PhoneModels.init(context); true }
    var ready by remember { mutableStateOf<Boolean?>(null) }
    var backend by remember { mutableStateOf("cpu") }
    var installed by remember { mutableStateOf<List<ModelStore.Installed>>(emptyList()) }
    var downloads by remember { mutableStateOf<List<PhoneModels.Download>>(emptyList()) }
    var budget by remember { mutableStateOf(0L) }
    var used by remember { mutableStateOf(0L) }
    var free by remember { mutableStateOf(0L) }
    var runningKey by remember { mutableStateOf<String?>(null) }
    var runningDetail by remember { mutableStateOf<String?>(null) }
    var confirmRemove by remember { mutableStateOf<ModelStore.Installed?>(null) }

    LaunchedEffect(Unit) {
        while (true) {
            withContext(Dispatchers.IO) {
                if (ready == null) { ready = PhoneModels.engineReady(); backend = PhoneModels.backend() }
                installed = PhoneModels.installed()
                downloads = PhoneModels.downloads()
                budget = PhoneModels.memoryBudget()
                used = PhoneModels.storageUsed()
                free = PhoneModels.freeBytes()
                val r = PhoneModels.running()
                runningKey = r?.optString("key")
                runningDetail = r?.let { j ->
                    listOfNotNull("llama.cpp", if (j.optString("backend") == "opencl") "GPU" else "CPU",
                        j.optDouble("tokPerSec", 0.0).takeIf { it > 0 }?.let { "%.0f tok/s".format(it) }).joinToString(" · ")
                }
            }
            delay(700)
        }
    }

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 24.dp).testTag("modelsDevice"),
        verticalArrangement = Arrangement.spacedBy(12.dp)) {
        // Running now / Memory.
        Section(if (runningKey == null) "Memory" else "Running now") {
            installed.firstOrNull { it.key == runningKey }?.let { m ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text("${m.name} ${m.quant}", color = c.textPrimary, fontSize = 15.sp)
                        runningDetail?.let { Text(it, color = c.textSecondary, fontSize = 12.sp) }
                    }
                    SmallButton("Stop", "modelsDeviceStop") { PhoneModels.unload() }
                }
            }
            val held = installed.firstOrNull { it.key == runningKey }?.size ?: 0L
            Bar(if (budget > 0) held.toFloat() / budget else 0f, c.textPrimary)
            Text(if (runningKey == null) "Nothing is loaded. Allternit can give a model up to ${gbText(budget)} of memory on this phone."
                else "${gbText(held)} of ${gbText(budget)} this app can use for models", color = c.textSecondary, fontSize = 12.sp)
            Text("Models unload when you tap Stop, on a low-memory warning, and after 5 idle minutes.", color = c.textTertiary, fontSize = 11.sp)
        }
        // Engines.
        Section("Engines") {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.testTag("modelsDeviceEngine")) {
                Box(Modifier.size(7.dp).clip(CircleShape).background(when (ready) { true -> AllternitStatic.StatusSuccess; false -> c.textTertiary; null -> c.textTertiary }))
                Column {
                    Text("llama.cpp", color = c.textPrimary, fontSize = 15.sp)
                    Text(when (ready) {
                        null -> "Checking…"
                        true -> if (backend == "opencl") "Ready · GPU (OpenCL)" else "Ready · CPU"
                        false -> "Not available on this build"
                    }, color = c.textSecondary, fontSize = 12.sp)
                }
            }
        }
        // Downloads in progress.
        if (downloads.isNotEmpty()) Section("Downloading") {
            downloads.forEach { d ->
                Column(Modifier.fillMaxWidth().testTag("modelsDeviceDownload${d.key}"), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text("${d.name} ${d.quant}", color = c.textPrimary, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    if (d.status != PhoneModels.Status.Failed) Bar(d.fraction, c.textPrimary)
                    Text(statusText(d), color = if (d.status == PhoneModels.Status.Failed) AllternitStatic.StatusError else c.textSecondary, fontSize = 12.sp)
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        when (d.status) {
                            PhoneModels.Status.Downloading, PhoneModels.Status.Queued -> SmallButton("Pause", "modelsDevicePause") { PhoneModels.pause(d.key) }
                            PhoneModels.Status.Paused -> SmallButton("Resume", "modelsDeviceResume") { PhoneModels.resume(d.key) }
                            PhoneModels.Status.Failed -> SmallButton("Dismiss", "modelsDeviceDismiss") { PhoneModels.dismiss(d.key) }
                            PhoneModels.Status.Verifying -> Unit
                        }
                        if (d.status != PhoneModels.Status.Verifying && d.status != PhoneModels.Status.Failed) {
                            SmallButton("Cancel", "modelsDeviceCancel") { PhoneModels.cancel(d.key) }
                        }
                    }
                }
            }
        }
        // Downloaded.
        if (installed.isEmpty() && downloads.isEmpty()) {
            FriendlyState(
                FriendlyStyle.Empty, "No models on this phone",
                if (ready == false) "Models on this phone are not available on this build. You can still send models to your computer from the catalog."
                else "Download a model from the catalog to chat on this phone, even with no connection.",
                Modifier.padding(top = 8.dp).testTag("modelsDeviceEmpty"), "Browse the catalog", onBrowse,
            ) { Icon(painterResource(DesignR.drawable.ph_cpu), null, tint = c.textSecondary) }
        } else if (installed.isNotEmpty()) Section("Downloaded") {
            installed.forEach { m ->
                Row(Modifier.fillMaxWidth().testTag("modelsDeviceInstalled${m.key}"), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Column(Modifier.weight(1f)) {
                        Text(m.name, color = c.textPrimary, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text("${m.quant} · ${gbText(m.size)} · verified", color = c.textSecondary, fontSize = 12.sp)
                    }
                    SmallButton("Use in chat", "modelsDeviceUse", enabled = ready == true) { host.onRun(PhoneModels.chatId(m.key)) }
                    SmallButton("Remove", "modelsDeviceRemove") { confirmRemove = m }
                }
            }
            Text("Storage: ${gbText(used)} used by models · ${gbText(free)} free on this phone", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.testTag("modelsDeviceStorage"))
        }
    }

    confirmRemove?.let { m ->
        AlertDialog(
            onDismissRequest = { confirmRemove = null },
            title = { Text("Remove this model from your phone?") },
            text = { Text("You can download it again from the catalog.") },
            confirmButton = { TextButton(onClick = { PhoneModels.delete(m.key); confirmRemove = null }) { Text("Remove ${gbText(m.size)}", color = AllternitStatic.StatusError) } },
            dismissButton = { TextButton(onClick = { confirmRemove = null }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun Section(title: String, content: @Composable () -> Unit) {
    val c = AllternitTheme.colors
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(title.uppercase(), color = c.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.8.sp, modifier = Modifier.padding(start = 4.dp))
        Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(14.dp)).padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp)) { content() }
    }
}

/** iOS streamsFromDiskNote: the phone never streams; this tells the owner a Mac or PC can run the model. */
@Composable
private fun StreamsFromDiskNote(streams: List<StreamQuant>) {
    val s = preferredStream(streams) ?: return
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(14.dp)).padding(14.dp)
        .testTag("modelStreamsFromDisk").semantics(mergeDescendants = true) {}, verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text("Streams from disk", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.Medium)
        Text("On your Mac or PC: ${gbDecimal(s.sizeBytes)} on disk. " + (s.bestFor ?: "Not on this phone."), color = c.textSecondary, fontSize = 13.sp)
    }
}

/** The model page's "Run on this phone" builds with Download (iOS device builds + OnDeviceRuntime.install). */
@Composable
internal fun PhoneBuildsSection(m: FwRow) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    remember { PhoneModels.init(context); true }
    var load by remember { mutableStateOf<FwLoad<DeviceBuilds>?>(null) }
    var ready by remember { mutableStateOf<Boolean?>(null) }
    var installedKeys by remember { mutableStateOf<Set<String>>(emptySet()) }
    var active by remember { mutableStateOf<Map<String, PhoneModels.Download>>(emptyMap()) }
    var budget by remember { mutableStateOf(0L) }
    LaunchedEffect(m.id) { load = DeviceBuildsApi.builds(m.id) }
    LaunchedEffect(Unit) {
        while (true) {
            withContext(Dispatchers.IO) {
                if (ready == null) ready = PhoneModels.engineReady()
                installedKeys = PhoneModels.installed().map { it.key }.toSet()
                active = PhoneModels.downloads().associateBy { it.key }
                budget = PhoneModels.memoryBudget()
            }
            delay(700)
        }
    }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.testTag("modelPhoneBuilds")) {
        Text("Run on this phone", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
        when (val l = load) {
            null -> CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = c.textSecondary)
            is FwLoad.Down -> Text(l.reason, color = c.textSecondary, fontSize = 13.sp)
            is FwLoad.Live -> when {
                ready == false -> Text("Not available on this build.", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.testTag("modelPhoneUnavailable"))
                l.value.phone.isEmpty() -> {
                    InlineEmpty("No phone builds for this model yet.", art = EmptyArtKind.Phone)
                    StreamsFromDiskNote(l.value.stream)
                }
                else -> {
                    l.value.phone.forEach { q ->
                    val key = ModelStore.keyOf(m.id, q.name)
                    val d = active[key]
                    val fits = q.sizeBytes == null || budget <= 0 || q.sizeBytes * 12 / 10 + 400_000_000L <= budget
                    Row(Modifier.fillMaxWidth().heightIn(min = 44.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Column(Modifier.weight(1f)) {
                            Text(q.name, color = c.textPrimary, fontSize = 14.sp)
                            Text(listOfNotNull(q.sizeBytes?.let { gbText(it) }, if (fits) "Fits this phone" else "Too big for this phone").joinToString(" · "),
                                color = if (fits) c.textSecondary else AllternitStatic.StatusWarning, fontSize = 12.sp)
                        }
                        when {
                            key in installedKeys -> Text("On this phone", color = AllternitStatic.StatusSuccess, fontSize = 13.sp)
                            d != null && d.status != PhoneModels.Status.Failed -> Text(statusText(d), color = c.textSecondary, fontSize = 12.sp)
                            else -> SmallButton("Download", "modelPhoneDownload${q.name}", enabled = fits && ready == true) {
                                PhoneModels.download(m.id, q.name, q.repo, q.file, m.name)
                            }
                        }
                    }
                    if (d?.status == PhoneModels.Status.Failed) Text(statusText(d), color = AllternitStatic.StatusError, fontSize = 12.sp)
                    }
                    // iOS: the note shows only when the phone's own fit is not ok/slow. Android has no fit port,
                    // so the same condition is "no phone build fits this phone's memory budget".
                    val anyFits = l.value.phone.any { q -> q.sizeBytes == null || budget <= 0 || q.sizeBytes * 12 / 10 + 400_000_000L <= budget }
                    if (!anyFits) StreamsFromDiskNote(l.value.stream)
                }
            }
        }
        Text("Downloads use any network, Wi-Fi or mobile data, and are checked against the signed Freeweights manifest.", color = c.textTertiary, fontSize = 11.sp)
    }
}
