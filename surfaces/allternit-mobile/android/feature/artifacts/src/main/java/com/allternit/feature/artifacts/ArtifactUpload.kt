package com.allternit.feature.artifacts

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Base64
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.design.R as DesignR
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject
import okhttp3.MediaType.Companion.toMediaTypeOrNull
import okhttp3.RequestBody.Companion.toRequestBody

/*
 * Upload in the artifacts library (iOS ArtifactsLibraryView "Upload": PDF, Image, Video).
 * PDF: base64 inline (15 MB). Image: Allternit Cloud storage from 1 MiB when signed in, else a
 * data: URL (15 MB). Video: always Allternit Cloud (500 MB, sign-in needed), as a FilmCraft
 * project with `meta.media`. Then `POST api/v2/artifacts` and open it.
 */

private const val MAX_UPLOAD = 15L * 1024 * 1024
private const val MAX_VIDEO = 500L * 1024 * 1024
private const val MIN_CLOUD = 1024L * 1024

/** iOS CloudFilesClient: `POST api/v1/files/uploads` → PUT bytes to `putUrl` → complete → a 7-day link. */
internal object CloudFiles {
    private fun url(path: String) = LibraryNet.config.cloudBaseUrl.newBuilder().addPathSegments(path).build()

    suspend fun upload(bytes: ByteArray, name: String, mime: String): String? = withContext(Dispatchers.IO) {
        val meta = buildJsonObject { put("name", name); put("contentType", mime); put("bytes", bytes.size) }.toString()
        val start = ApiCall.execute(LibraryNet.okHttp, ApiJson.default, ApiCall.request("POST", url("api/v1/files/uploads"), meta), JsonElement.serializer())
        val o = (start as? ApiResult.Ok)?.value.obj ?: return@withContext null
        val id = o.str("fileId") ?: return@withContext null
        val put = o.str("putUrl") ?: return@withContext null
        val ok = runCatching {
            okhttp3.OkHttpClient().newCall(okhttp3.Request.Builder().url(put).put(bytes.toRequestBody(mime.toMediaTypeOrNull())).build()).execute().use { it.isSuccessful }
        }.getOrDefault(false)
        if (!ok) return@withContext null
        ApiCall.execute(LibraryNet.okHttp, ApiJson.default, ApiCall.request("POST", url("api/v1/files/${LibraryNet.escape(id)}/complete"), meta), JsonElement.serializer())
        val link = ApiCall.execute(
            LibraryNet.okHttp, ApiJson.default,
            ApiCall.request("GET", url("api/v1/files/${LibraryNet.escape(id)}").newBuilder().addQueryParameter("expires", "604800").build(), null),
            JsonElement.serializer(),
        )
        (link as? ApiResult.Ok)?.value.obj.let { it.str("linkUrl") ?: it.str("url") }
    }
}

/** web buildInitialVideoProject: the `.fcproj` envelope with an empty tree (schema 11). */
internal fun initialVideoProject(title: String): String = buildJsonObject {
    put("generator", "Allternit")
    putJsonObject("project") {
        put("items", JsonObject(emptyMap()))
        put("name", title)
        put("next_id", 1)
        putJsonObject("root") { put("children", buildJsonArray { }); put("id", 0); put("name", title) }
        putJsonObject("settings") {
            put("audio_display_samples", true)
            put("default_audio_transition_duration", 254016000000L)
            put("default_still_duration", 1270080000000L)
            put("default_transition_duration_frames", 24)
            put("renderer", "FilmCraft GPU Acceleration (wgpu)")
            put("video_display", "Timecode")
        }
    }
    put("schema_version", 11)
}.toString()

private fun displayName(context: Context, uri: Uri): String? =
    context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { if (it.moveToFirst()) it.getString(0) else null }

private fun readCapped(context: Context, uri: Uri, cap: Long): ByteArray? {
    val size = context.contentResolver.query(uri, arrayOf(OpenableColumns.SIZE), null, null, null)?.use { if (it.moveToFirst()) it.getLong(0) else -1L } ?: -1L
    if (size > cap) throw IllegalArgumentException("tooLarge")
    return context.contentResolver.openInputStream(uri)?.use { it.readBytes() }?.also { if (it.size > cap) throw IllegalArgumentException("tooLarge") }
}

private fun libraryOrigin() = buildJsonObject { put("surface", "library") }

/** The three upload flows; each returns the created artifact's id or a notice. */
internal object ArtifactUploads {
    suspend fun pdf(context: Context, uri: Uri): Result<String> = withContext(Dispatchers.IO) {
        val name = displayName(context, uri) ?: "Untitled PDF.pdf"
        val bytes = try { readCapped(context, uri, MAX_UPLOAD) } catch (_: IllegalArgumentException) {
            return@withContext Result.failure(IllegalStateException("That PDF is over 15 MB. Split or shrink it and try again."))
        } ?: return@withContext Result.failure(IllegalStateException("Couldn't read that PDF."))
        create(buildJsonObject {
            put("kind", "pdf"); put("title", name.substringBeforeLast('.').ifBlank { "Untitled PDF" })
            put("body", Base64.encodeToString(bytes, Base64.NO_WRAP)); put("body_format", "application/pdf")
            putJsonObject("meta") { put("filename", name) }; put("origin", libraryOrigin())
        }, "Couldn't upload that PDF.")
    }

    suspend fun image(context: Context, uri: Uri, signedIn: Boolean): Result<String> = withContext(Dispatchers.IO) {
        val mime = context.contentResolver.getType(uri) ?: "image/jpeg"
        val name = "Image." + (mime.substringAfter('/').substringBefore('+').ifBlank { "jpg" })
        val bytes = try { readCapped(context, uri, MAX_UPLOAD) } catch (_: IllegalArgumentException) {
            return@withContext Result.failure(IllegalStateException("That image is over 15 MB. Shrink it and try again."))
        } ?: return@withContext Result.failure(IllegalStateException("Couldn't read that image."))
        val body = if (signedIn && bytes.size >= MIN_CLOUD) {
            CloudFiles.upload(bytes, name, mime) ?: return@withContext Result.failure(IllegalStateException("Couldn't upload that image."))
        } else "data:$mime;base64," + Base64.encodeToString(bytes, Base64.NO_WRAP)
        create(buildJsonObject {
            put("kind", "image"); put("title", "Untitled image"); put("body", body); put("body_format", "text/uri-list")
            putJsonObject("meta") { put("filename", name) }; put("origin", libraryOrigin())
            putJsonObject("capabilities") { put("storage", true) }
        }, "Couldn't upload that image.")
    }

    suspend fun video(context: Context, uri: Uri, signedIn: Boolean): Result<String> = withContext(Dispatchers.IO) {
        val mime = context.contentResolver.getType(uri) ?: "video/mp4"
        if (!mime.startsWith("video/")) return@withContext Result.failure(IllegalStateException("Only video files (MP4, MOV, WebM) can be uploaded as video artifacts."))
        if (!signedIn) return@withContext Result.failure(IllegalStateException("Sign in to upload videos — the footage is stored in Allternit Cloud."))
        val name = displayName(context, uri) ?: "Untitled video.mp4"
        val bytes = try { readCapped(context, uri, MAX_VIDEO) } catch (_: IllegalArgumentException) {
            return@withContext Result.failure(IllegalStateException("That video is over 500 MB. Export a smaller version and try again."))
        } ?: return@withContext Result.failure(IllegalStateException("Couldn't upload that video."))
        val link = CloudFiles.upload(bytes, name, mime) ?: return@withContext Result.failure(IllegalStateException("Couldn't upload that video."))
        val title = name.substringBeforeLast('.').ifBlank { "Untitled video" }
        create(buildJsonObject {
            put("kind", "video"); put("title", title); put("body", initialVideoProject(title)); put("body_format", "application/vnd.allternit.video+json")
            putJsonObject("meta") { put("filename", name); putJsonObject("media") { put("url", link); put("mime", mime) } }
            put("origin", libraryOrigin()); putJsonObject("capabilities") { put("storage", true) }
        }, "Couldn't upload that video.")
    }

    private suspend fun create(input: JsonObject, failure: String): Result<String> =
        when (val r = ArtifactsV2Client.create(input)) {
            is ApiResult.Ok -> r.value?.let { Result.success(it.id) } ?: Result.failure(IllegalStateException(failure))
            else -> Result.failure(IllegalStateException("$failure ${artifactError(r)}"))
        }
}

/**
 * iOS "New artifact" (the floating button's menu, as a sheet so it is reached by tap): Make
 * something new (the studio tiles), Upload (PDF, Image, Video) and Start from your connected tools.
 */
@Composable
internal fun NewArtifactSheet(
    tiles: List<StudioKind>,
    signedIn: Boolean,
    onTile: (StudioKind) -> Unit,
    onConnectors: () -> Unit,
    onUploading: (String) -> Unit,
    onUploaded: (Result<String>) -> Unit,
    onDismiss: () -> Unit,
    /** The library's scope: an upload keeps going after the sheet closes (its own scope would cancel it). */
    scope: kotlinx.coroutines.CoroutineScope? = null,
) {
    val context = LocalContext.current
    val ownScope = rememberCoroutineScope()
    val scope = scope ?: ownScope
    fun run(name: String, block: suspend () -> Result<String>) { onUploading("Uploading $name…"); scope.launch { onUploaded(block()) } }
    val pdf = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { u -> if (u != null) run(displayName(context, u) ?: "PDF") { ArtifactUploads.pdf(context, u) } }
    val image = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { u -> if (u != null) run("image") { ArtifactUploads.image(context, u, signedIn) } }
    val video = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { u -> if (u != null) run(displayName(context, u) ?: "video") { ArtifactUploads.video(context, u, signedIn) } }
    LibrarySheet("New artifact", "newArtifactSheet", onDismiss = onDismiss, leading = LibrarySheetAction("Cancel", onClick = onDismiss)) {
        Column(Modifier.padding(horizontal = 16.dp).padding(bottom = 24.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterLabel("Make something new")
            tiles.forEach { k -> LibraryRowButton(k.label, k.deck, "newArtifact${k.raw}") { onDismiss(); onTile(k) } }
            FilterLabel("Upload")
            LibraryRowButton("PDF", DesignR.drawable.ph_upload_simple, "artifactsUploadPdf") { pdf.launch(arrayOf("application/pdf")) }
            LibraryRowButton("Image", DesignR.drawable.ph_upload_simple, "artifactsUploadImage") { image.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) }
            LibraryRowButton("Video", DesignR.drawable.ph_upload_simple, "artifactsUploadVideo") { video.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.VideoOnly)) }
            FilterLabel("Start from your connected tools")
            LibraryRowButton("Connect your tools", DesignR.drawable.ph_plugs_connected, "newArtifactConnectors") { onDismiss(); onConnectors() }
        }
    }
}
