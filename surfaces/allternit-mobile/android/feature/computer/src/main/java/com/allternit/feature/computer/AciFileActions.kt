package com.allternit.feature.computer

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import com.google.mlkit.vision.documentscanner.GmsDocumentScannerOptions
import com.google.mlkit.vision.documentscanner.GmsDocumentScanning
import com.google.mlkit.vision.documentscanner.GmsDocumentScanningResult
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import okhttp3.HttpUrl.Companion.toHttpUrl
import java.io.IOException
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * cloud-api's Files routes for the signed-in user that the computer's Files screens need (iOS `FilesCloudAPI` and
 * AllternitFilesKit `FilesClient.createShare`): the wake setting and "Share with…". Clerk session, never the Files key.
 */
internal object FilesCloudRoutes {
    private suspend fun cloud(method: String, path: String, body: JsonElement?): ApiResult<JsonElement> {
        val url = (LibraryNet.config.cloudBaseUrl.toString().trimEnd('/') + "/" + path.trimStart('/')).toHttpUrl()
        val request = ApiCall.request(method, url, body?.toString() ?: if (method == "GET") null else "{}")
        return when (val r = ApiCall.execute(LibraryNet.okHttp, ApiJson.default, request, JsonElement.serializer())) {
            is ApiResult.Decode -> ApiResult.Ok(JsonNull)
            else -> r
        }
    }

    private fun seg(s: String) = java.net.URLEncoder.encode(s, "UTF-8").replace("+", "%20")

    /** `ask`, `auto` or `never`; `ask` when it can't be read (the safe answer: the sheet asks first). */
    suspend fun wake(cid: String): String =
        ((cloud("GET", "api/v1/computer-files/${seg(cid)}/wake-settings", null) as? ApiResult.Ok)?.value.obj.str("wake")) ?: "ask"

    /**
     * `POST /computer-files/:id/shares` (SPEC section 8). cloud-api signs the link; the runtime re-checks scope on every
     * guest request. Returns the link, or throws with the iOS copy.
     */
    suspend fun createShare(cid: String, rootId: String, itemId: String, role: String, expiresAt: String?, guestWake: Boolean, email: String?): String {
        val body = buildJsonObject {
            put("root_id", JsonPrimitive(rootId))
            put("item_id", JsonPrimitive(itemId))
            put("role", JsonPrimitive(role))
            if (expiresAt != null) put("expires_at", JsonPrimitive(expiresAt))
            put("guest_wake", JsonPrimitive(guestWake))
            if (email != null) put("invite_emails", buildJsonArray { add(JsonPrimitive(email)) })
        }
        return when (val r = cloud("POST", "api/v1/computer-files/${seg(cid)}/shares", body)) {
            is ApiResult.Ok -> r.value.obj.child("share").str("link_url").orEmpty()
            is ApiResult.HttpError ->
                if (r.code == "forbidden" || r.status == 403) throw IOException("This folder can't be shared. Turn on sharing for it in Allternit first.")
                else throw IOException(r.message?.takeIf { it.isNotBlank() } ?: r.friendly("share this"))
            is ApiResult.Network -> throw IOException("You're offline. Check your connection and try again.")
            else -> throw IOException("Allternit sent a reply this app couldn't read.")
        }
    }
}

/**
 * The phone's Files document provider (:app `AllternitDocumentsProvider`, authority `<package>.files`). A feature
 * module can't depend on :app, so open, save, upload and scan go through the provider by its document ids
 * (`i/<computer>/<root>/<item>`, :app DocIds). The provider owns downloads, the upload queue, the wake ticket and
 * the protected-root unlock, so this screen reuses all of it instead of a second client.
 */
internal object FilesDocs {
    private fun authority(context: Context) = context.packageName + ".files"

    fun item(context: Context, cid: String, rid: String, iid: String): Uri =
        DocumentsContract.buildDocumentUri(authority(context), "i/" + Uri.encode(cid) + "/" + Uri.encode(rid) + "/" + Uri.encode(iid))

    fun displayName(context: Context, uri: Uri): String? = runCatching {
        context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
            if (c.moveToFirst()) c.getString(0) else null
        }
    }.getOrNull()

    /** Reads the file once through the provider (download into its cache), so a failure shows here, not in another app. */
    fun prefetch(context: Context, uri: Uri) {
        context.contentResolver.openFileDescriptor(uri, "r")?.close() ?: throw IOException("Couldn't open this file.")
    }

    /** A new file in a computer folder: created on the phone first, then the provider uploads it (`keep both` on a clash). */
    fun copyInto(context: Context, parent: Uri, source: Uri, name: String, mime: String) {
        val resolver = context.contentResolver
        val created = DocumentsContract.createDocument(resolver, parent, mime, name) ?: throw IOException("Couldn't save $name.")
        resolver.openInputStream(source).use { input ->
            input ?: throw IOException("Couldn't read $name.")
            resolver.openOutputStream(created).use { out -> input.copyTo(out ?: throw IOException("Couldn't save $name.")) }
        }
    }
}

private tailrec fun Context.findActivity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.findActivity()
    else -> null
}

/** What "Share with…" is about (a file from its sheet, or the folder on screen). */
internal data class FilesShareTarget(val rootId: String, val itemId: String, val name: String, val isFolder: Boolean)

/**
 * The file actions of one computer's Files screens, the port of iOS `FilesComputerModel` (needingWake, wake,
 * localCopy, upload, saveScan) and the sheets in `FilesSheets.swift` (FilesItemSheet, FilesWakeSheet) plus the File
 * Provider UI's "Share with…" sheet (Android has no Files-app action extension, so it lives on the file's sheet).
 *
 * Browsing never wakes the computer. Opening, saving, uploading and scanning need it awake: with the wake setting
 * `auto` the provider wakes it silently; with `ask` the wake sheet (:app `WakePromptActivity`, reused, started for a
 * result) asks first and the action runs again after a successful wake; with `never` it says so.
 */
internal class AciFileActions(private val context: Context, val device: AciDevice, private val scope: CoroutineScope) {
    var busy by mutableStateOf<String?>(null)
    var itemSheet by mutableStateOf<Pair<FilesRoot, FilesItem>?>(null)
    var addSheet by mutableStateOf<Pair<FilesRoot, String>?>(null)
    var shareSheet by mutableStateOf<FilesShareTarget?>(null)
    /** Bumped after an upload, so open folder screens list again. */
    var revision by mutableIntStateOf(0)
    /** The computer answers from its snapshot (set by the screen on screen from its freshness). */
    var asleep = false
    /** The roots, once the roots screen read them (a scan goes to Scans). */
    var roots: List<FilesRoot> = emptyList()

    internal var launchWake: ((Intent) -> Unit)? = null
    internal var launchPick: (() -> Unit)? = null
    internal var launchSave: ((String) -> Unit)? = null
    internal var launchScan: ((Activity) -> Unit)? = null

    private var pendingRetry: (suspend () -> Unit)? = null
    private var uploadTarget: Pair<FilesRoot, String>? = null
    private var scanTarget: Pair<FilesRoot, String>? = null
    private var saveSource: Uri? = null

    private val name get() = device.displayName

    fun say(text: String) = Toast.makeText(context, text, Toast.LENGTH_LONG).show()

    private fun message(e: Throwable) = e.message?.takeIf { it.isNotBlank() } ?: "Couldn't do that. Try again."

    /** iOS needingWake: runs [action]; a missing wake consent turns into the wake sheet (or a notice when `never`). */
    fun needingWake(reason: String, itemId: String?, fileName: String?, action: suspend () -> Unit) {
        scope.launch {
            if (asleep) {
                when (FilesCloudRoutes.wake(device.id)) {
                    "never" -> { say("$name is set to never wake for Files. Change it in Settings > Files."); return@launch }
                    "auto" -> Unit
                    else -> { askToWake(reason, itemId, fileName, action); return@launch }
                }
            }
            try {
                action()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                // The provider's WakeNeeded ("… is asleep …") when the computer fell asleep since the list loaded.
                if (e.message.orEmpty().contains("asleep")) askToWake(reason, itemId, fileName, action) else say(message(e))
            }
        }
    }

    private fun askToWake(reason: String, itemId: String?, fileName: String?, action: suspend () -> Unit) {
        val launch = launchWake ?: return say("$name is asleep. Wake it from the Allternit notification, then try again.")
        pendingRetry = action
        val intent = Intent().setClassName(context.packageName, WAKE_PROMPT)
            .putExtra("computer_id", device.id).putExtra("reason", reason).putExtra("retry", true)
        itemId?.let { intent.putExtra("item_id", it) }
        fileName?.let { intent.putExtra("file_name", it) }
        try {
            launch(intent)
        } catch (e: ActivityNotFoundException) {
            pendingRetry = null
            say("$name is asleep. Wake it from the Allternit notification, then try again.")
        }
    }

    /** The wake sheet answered. RESULT_OK = woken (and "automatically from now on" saved when ticked): run it again. */
    fun onWakeResult(ok: Boolean) {
        val retry = pendingRetry
        pendingRetry = null
        if (!ok || retry == null) return
        asleep = false
        scope.launch {
            try { retry() } catch (e: CancellationException) { throw e } catch (e: Exception) { say(message(e)) }
        }
    }

    private fun start(intent: Intent) {
        val activity = context.findActivity()
        try {
            if (activity != null) activity.startActivity(intent) else context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        } catch (e: ActivityNotFoundException) {
            say("No app on this phone opens this kind of file. Use Save to this phone or Share… instead.")
        }
    }

    private suspend fun fetched(root: FilesRoot, item: FilesItem): Pair<Uri, String> {
        busy = item.name
        try {
            val uri = FilesDocs.item(context, device.id, root.id, item.id)
            val type = withContext(Dispatchers.IO) {
                FilesDocs.prefetch(context, uri)
                context.contentResolver.getType(uri)
            } ?: "application/octet-stream"
            return uri to type
        } finally {
            busy = null
        }
    }

    /** Open: the provider downloads it, then the phone's viewer for its type opens it (iOS Quick Look). */
    fun open(root: FilesRoot, item: FilesItem) = needingWake("open_file", item.id, item.name) {
        val (uri, type) = fetched(root, item)
        start(Intent(Intent.ACTION_VIEW).setDataAndType(uri, type).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))
    }

    /** Share…: the system share sheet with the file (iOS ArtifactShareSheet on the local copy). */
    fun shareOut(root: FilesRoot, item: FilesItem) = needingWake("open_file", item.id, item.name) {
        val (uri, type) = fetched(root, item)
        val send = Intent(Intent.ACTION_SEND).setType(type).putExtra(Intent.EXTRA_STREAM, uri).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        send.clipData = ClipData.newRawUri(item.name, uri)
        start(Intent.createChooser(send, item.name))
    }

    /** Save to this phone: the system "save as" picker, then a copy through the provider. */
    fun saveToPhone(root: FilesRoot, item: FilesItem) = needingWake("open_file", item.id, item.name) {
        val launch = launchSave ?: return@needingWake
        saveSource = fetched(root, item).first
        launch(item.name)
    }

    fun onSaveTarget(dest: Uri?) {
        val src = saveSource ?: return
        saveSource = null
        if (dest == null) return
        scope.launch {
            busy = "Saving"
            try {
                withContext(Dispatchers.IO) {
                    context.contentResolver.openInputStream(src).use { i ->
                        i ?: throw IOException("Couldn't read this file.")
                        context.contentResolver.openOutputStream(dest).use { o -> i.copyTo(o ?: throw IOException("Couldn't save the copy.")) }
                    }
                }
                say("Saved to this phone.")
            } catch (e: Exception) {
                say(message(e))
            } finally {
                busy = null
            }
        }
    }

    /** Upload: pick files on the phone; each lands under its own name in this folder (a taken name keeps both). */
    fun upload(root: FilesRoot, parentId: String) {
        uploadTarget = root to parentId
        launchPick?.invoke()
    }

    fun onPicked(uris: List<Uri>) {
        val (root, parentId) = uploadTarget ?: return
        uploadTarget = null
        if (uris.isEmpty()) return
        val first = FilesDocs.displayName(context, uris[0])
        needingWake("upload", null, if (uris.size == 1) first else null) {
            busy = "Uploading"
            try {
                withContext(Dispatchers.IO) {
                    val parent = FilesDocs.item(context, device.id, root.id, parentId)
                    for (u in uris) {
                        val n = FilesDocs.displayName(context, u) ?: "file"
                        FilesDocs.copyInto(context, parent, u, n, context.contentResolver.getType(u) ?: "application/octet-stream")
                    }
                }
            } finally {
                busy = null
            }
            revision++
            say(if (uris.size == 1) "Saving ${first ?: "the file"} to ${root.name}." else "Saving ${uris.size} files to ${root.name}.")
        }
    }

    /**
     * Scan to PDF (iOS FilesDocumentScanner + saveScan): the scan goes to the Scans root, or to the given folder when
     * that folder is inside Scans; with no Scans root, to the given folder, else Inbox.
     */
    fun scan(near: FilesRoot?, parentId: String?) {
        val scans = roots.firstOrNull { it.kind == "scans" }
        val target = when {
            near != null && parentId != null && near.kind == "scans" -> near to parentId
            scans != null -> scans to scans.rootItemId
            near != null && parentId != null -> near to parentId
            else -> roots.firstOrNull { it.kind == "inbox" }?.let { it to it.rootItemId }
        } ?: return say("$name has no Scans folder yet.")
        val activity = context.findActivity() ?: return say("Couldn't start the scanner.")
        scanTarget = target
        launchScan?.invoke(activity)
    }

    fun onScanned(pdf: Uri?) {
        val (root, parentId) = scanTarget ?: return
        scanTarget = null
        if (pdf == null) return
        val fileName = "Scan " + SimpleDateFormat("yyyy-MM-dd HHmm", Locale.US).format(Date()) + ".pdf"
        needingWake("upload", null, fileName) {
            busy = "Saving scan"
            try {
                withContext(Dispatchers.IO) {
                    FilesDocs.copyInto(context, FilesDocs.item(context, device.id, root.id, parentId), pdf, fileName, "application/pdf")
                }
            } finally {
                busy = null
            }
            revision++
            say("Scan saved to ${root.name}.")
        }
    }

    companion object {
        /** :app's wake prompt (com.allternit.app.files), reused by class name: a feature module can't depend on :app. */
        const val WAKE_PROMPT = "com.allternit.app.files.WakePromptActivity"
    }
}

/** The launchers (wake sheet, file picker, save-as, document scanner) bound to one computer's [AciFileActions]. */
@Composable
internal fun rememberAciFileActions(device: AciDevice): AciFileActions {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val actions = remember(device.id, context) { AciFileActions(context, device, scope) }
    val wake = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { actions.onWakeResult(it.resultCode == Activity.RESULT_OK) }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { actions.onPicked(it) }
    val save = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("*/*")) { actions.onSaveTarget(it) }
    val scan = rememberLauncherForActivityResult(ActivityResultContracts.StartIntentSenderForResult()) { r ->
        actions.onScanned(if (r.resultCode == Activity.RESULT_OK) GmsDocumentScanningResult.fromActivityResultIntent(r.data)?.pdf?.uri else null)
    }
    actions.launchWake = { wake.launch(it) }
    actions.launchPick = { pick.launch(arrayOf("*/*")) }
    actions.launchSave = { save.launch(it) }
    actions.launchScan = { activity ->
        val options = GmsDocumentScannerOptions.Builder()
            .setGalleryImportAllowed(false)
            .setResultFormats(GmsDocumentScannerOptions.RESULT_FORMAT_PDF)
            .setScannerMode(GmsDocumentScannerOptions.SCANNER_MODE_FULL)
            .build()
        GmsDocumentScanning.getClient(options).getStartScanIntent(activity)
            .addOnSuccessListener { sender -> scan.launch(IntentSenderRequest.Builder(sender).build()) }
            .addOnFailureListener { actions.say("The scanner isn't available on this phone. Update Google Play services and try again.") }
    }
    return actions
}

/** Big full-width button from the design's sheets (iOS FilesBigButton). */
@Composable
private fun FilesBigButton(title: String, tag: String, primary: Boolean = true, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        Modifier.fillMaxWidth().heightIn(min = 50.dp).clip(RoundedCornerShape(14.dp))
            .background(if (primary) c.textPrimary else c.fillNeutral)
            .clickable(enabled = enabled, onClick = onClick).testTag(tag),
        contentAlignment = Alignment.Center,
    ) { Text(title, color = if (primary) c.bgPrimary else c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold) }
}

/** The sheets of a computer's Files screens: one file, add (upload or scan), and "Share with…". */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun AciFileSheets(actions: AciFileActions) {
    val c = AllternitTheme.colors
    actions.busy?.let { b ->
        Box(Modifier.fillMaxWidth().padding(16.dp), contentAlignment = Alignment.BottomCenter) {
            Text("$b…", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).padding(horizontal = 14.dp, vertical = 8.dp).testTag("filesTransfer"))
        }
    }
    actions.itemSheet?.let { (root, item) ->
        ModalBottomSheet(onDismissRequest = { actions.itemSheet = null }, containerColor = c.bgPrimary) {
            Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp).padding(bottom = 28.dp).testTag("filesItemSheet"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(item.name, color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                Text(listOf(formatBytes(item.size), formatWhen(item.modifiedAt)).filter { it.isNotEmpty() }.joinToString(" · "), color = c.textSecondary, fontSize = 14.sp)
                FilesBigButton("Open", "filesItemOpen") { actions.itemSheet = null; actions.open(root, item) }
                FilesBigButton("Share…", "filesItemShare", primary = false) { actions.itemSheet = null; actions.shareOut(root, item) }
                FilesBigButton("Save to this phone", "filesItemSave", primary = false) { actions.itemSheet = null; actions.saveToPhone(root, item) }
                FilesBigButton("Share with…", "filesItemShareWith", primary = false) {
                    actions.itemSheet = null
                    actions.shareSheet = FilesShareTarget(root.id, item.id, item.name, isFolder = false)
                }
            }
        }
    }
    actions.addSheet?.let { (root, parentId) ->
        ModalBottomSheet(onDismissRequest = { actions.addSheet = null }, containerColor = c.bgPrimary) {
            Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp).padding(bottom = 28.dp).testTag("filesAddSheet"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("Add to ${root.name}", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                FilesBigButton("Upload a file", "filesAddUpload") { actions.addSheet = null; actions.upload(root, parentId) }
                FilesBigButton("Scan a document", "filesAddScan", primary = false) { actions.addSheet = null; actions.scan(root, parentId) }
            }
        }
    }
    actions.shareSheet?.let { target -> AciShareSheet(actions, target) }
}

private enum class ShareExpiry(val label: String, val seconds: Long?, val phrase: String) {
    Day("24h", 86_400, "24 hours"), Week("7d", 604_800, "7 days"), Month("30d", 2_592_000, "30 days"), Open("No end", null, "as long as the link is on")
}

/** The File Provider UI's "Share with…" sheet (iOS ActionSheets.swift ShareSheet): guest email, role, link length, guest wake. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AciShareSheet(actions: AciFileActions, target: FilesShareTarget) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val roles = if (target.isFolder) listOf("view_download" to "View", "upload_only" to "Upload", "comment" to "Comment", "edit" to "Edit")
    else listOf("view_download" to "View", "comment" to "Comment", "edit" to "Edit")
    var email by remember { mutableStateOf("") }
    var role by remember { mutableStateOf("view_download") }
    var expiry by remember { mutableStateOf(ShareExpiry.Week) }
    var guestWake by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var failure by remember { mutableStateOf<String?>(null) }
    var link by remember { mutableStateOf<String?>(null) }
    val trimmed = email.trim()
    val emailValid = trimmed.isEmpty() || Regex("^[^@\\s]+@[^@\\s]+\\.[^@\\s]+$").matches(trimmed)
    val who = if (trimmed.isEmpty()) "Anyone with the link" else trimmed
    val verb = when (role) { "upload_only" -> "drop files into"; "comment" -> "view and comment on"; "edit" -> "edit"; else -> "see and download" }

    ModalBottomSheet(onDismissRequest = { actions.shareSheet = null }, containerColor = c.bgPrimary) {
        Column(
            Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).padding(bottom = 28.dp).testTag("filesShareSheet"),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Text("Share “${target.name}”", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
            Text(actions.device.displayName, color = c.textSecondary, fontSize = 13.sp)
            val made = link
            if (made != null) {
                Text("Link ready. $who can $verb it for ${expiry.phrase}.", color = c.textPrimary, fontSize = 15.sp)
                Text(made, color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.testTag("filesShareLink"))
                FilesBigButton("Copy link", "filesShareCopy") {
                    (context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).setPrimaryClip(ClipData.newPlainText("Share link", made))
                    actions.say("Link copied")
                }
                FilesBigButton("Done", "filesShareDone", primary = false) { actions.shareSheet = null }
                return@Column
            }
            OutlinedTextField(
                value = email, onValueChange = { email = it }, singleLine = true, isError = !emailValid,
                label = { Text("Guest email (optional)") }, placeholder = { Text("name@example.com") },
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Email),
                modifier = Modifier.fillMaxWidth().testTag("filesShareEmail"),
            )
            Text("Role", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.Medium)
            ShareSegment(roles, role, "filesShareRole") { role = it }
            Text("Link works for", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.Medium)
            ShareSegment(ShareExpiry.entries.map { it to it.label }, expiry, "filesShareExpiry") { expiry = it }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text("Guests can wake ${actions.device.displayName}", color = c.textPrimary, fontSize = 15.sp)
                    Text("Waking bills your credits. Off: guests see the saved list while it sleeps.", color = c.textSecondary, fontSize = 13.sp)
                }
                Switch(checked = guestWake, onCheckedChange = { guestWake = it }, modifier = Modifier.testTag("filesShareGuestWake"))
            }
            Text("$who can $verb “${target.name}” for ${expiry.phrase}. Sharing never lets an agent act on these files.", color = c.textSecondary, fontSize = 13.sp)
            failure?.let { Text(it, color = androidx.compose.ui.graphics.Color(0xFFD64545), fontSize = 13.sp) }
            FilesBigButton(if (busy) "Making the link…" else "Make link", "filesShareCreate", enabled = !busy && emailValid) {
                busy = true
                failure = null
                scope.launch {
                    try {
                        val expiresAt = expiry.seconds?.let { java.time.Instant.now().plusSeconds(it).truncatedTo(java.time.temporal.ChronoUnit.SECONDS).toString() }
                        link = FilesCloudRoutes.createShare(actions.device.id, target.rootId, target.itemId, role, expiresAt, guestWake, trimmed.ifEmpty { null })
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        failure = e.message ?: "Couldn't make the link. Try again."
                    } finally {
                        busy = false
                    }
                }
            }
        }
    }
}

@Composable
private fun <T> ShareSegment(options: List<Pair<T, String>>, value: T, tag: String, onPick: (T) -> Unit) {
    val c = AllternitTheme.colors
    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).padding(2.dp), horizontalArrangement = Arrangement.spacedBy(2.dp)) {
        options.forEachIndexed { i, (v, label) ->
            val on = v == value
            Box(
                Modifier.weight(1f).heightIn(min = 36.dp).clip(RoundedCornerShape(8.dp)).background(if (on) c.bgPrimary else c.fillNeutral)
                    .clickable { onPick(v) }.testTag("${tag}_$i"),
                contentAlignment = Alignment.Center,
            ) { Text(label, color = c.textPrimary, fontSize = 14.sp, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal) }
        }
    }
}
