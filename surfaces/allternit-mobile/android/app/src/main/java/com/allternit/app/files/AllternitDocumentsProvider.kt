package com.allternit.app.files

import android.app.AuthenticationRequiredException
import android.app.PendingIntent
import android.content.Intent
import android.content.res.AssetFileDescriptor
import android.database.Cursor
import android.database.MatrixCursor
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Point
import android.media.MediaMetadataRetriever
import android.net.Uri
import android.os.Bundle
import android.os.CancellationSignal
import android.os.Handler
import android.os.HandlerThread
import android.os.OperationCanceledException
import android.os.ParcelFileDescriptor
import android.os.storage.StorageManager
import android.provider.DocumentsContract
import android.provider.DocumentsContract.Document
import android.provider.DocumentsContract.Root
import android.provider.DocumentsProvider
import android.text.format.DateUtils
import android.webkit.MimeTypeMap
import com.allternit.app.R
import com.allternit.files.contract.ComputerState
import com.allternit.files.contract.DeleteRequest
import com.allternit.files.contract.Freshness
import com.allternit.files.contract.Item
import com.allternit.files.contract.ItemKind
import com.allternit.files.contract.MkdirRequest
import com.allternit.files.contract.MoveRequest
import com.allternit.files.contract.OnConflict
import com.allternit.files.contract.ProvenanceSource
import com.allternit.files.contract.ServedBy
import com.allternit.files.contract.WakeReason
import java.io.File
import java.io.FileNotFoundException
import java.io.FileOutputStream
import java.time.Instant
import java.util.concurrent.Callable
import java.util.concurrent.CancellationException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutionException
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException

/**
 * Every Allternit computer as a location in Android's file picker and Files app
 * (design 7B: one root per computer). Files drive protocol v2 over the relay.
 *
 *  - Listings answer from the phone's cache at once and refresh off the binder thread,
 *    then `notifyChange` reloads the picker (DocumentsUI's EXTRA_LOADING pattern). The
 *    change feed (long poll) keeps an open folder current.
 *  - A sleeping computer is browsed from its snapshot (`Freshness`), never woken by
 *    browsing. Opening or saving a file is what asks: the wake prompt (notification ->
 *    [WakePromptActivity] -> wake consent), or silently when its setting is `auto`.
 *  - Reads: the cached copy when its rev is current, else streamed to the cache (never
 *    held in memory); files over [PROXY_MIN] are served as a seekable proxy fd with
 *    ranged reads. Writes go to a staging file and upload when the writer closes it,
 *    with `base_rev`, `keep_both`, the device name and its UTC offset (SPEC section 4).
 *  - Protected roots (Memory Drive) need a fresh biometric unlock ([UnlockActivity]).
 *  - Signed out: one "Allternit" root whose sign-in button opens [FilesSignInActivity].
 */
class AllternitDocumentsProvider : DocumentsProvider() {
    private val drive by lazy { FilesDrive.get(context!!) }
    private val closeHandler by lazy { Handler(HandlerThread("files-close").apply { start() }.looper) }
    private val rootsChecked = ConcurrentHashMap<String, Long>()

    override fun onCreate(): Boolean = true

    // ------------------------------------------------------------------ roots

    override fun queryRoots(projection: Array<out String>?): Cursor {
        val c = MatrixCursor(projection ?: ROOT_COLS)
        c.setNotificationUri(context!!.contentResolver, DocumentsContract.buildRootsUri(drive.authority))
        val icon = context!!.applicationInfo.icon
        if (!drive.auth.signedIn()) {
            c.newRow().add(Root.COLUMN_ROOT_ID, "signin").add(Root.COLUMN_DOCUMENT_ID, DocIds.SIGNIN)
                .add(Root.COLUMN_TITLE, "Allternit").add(Root.COLUMN_SUMMARY, "Sign in to see your computers")
                .add(Root.COLUMN_FLAGS, 0).add(Root.COLUMN_ICON, icon).add(Root.COLUMN_MIME_TYPES, "*/*")
            return c
        }
        drive.refreshComputersAsync()
        val list = drive.computersInFiles()
        if (list.isEmpty()) {
            val summary = when {
                drive.computers.isNotEmpty() -> "All computers are hidden. Choose them in Allternit Files."
                else -> drive.computersError ?: "No computers yet"
            }
            c.newRow().add(Root.COLUMN_ROOT_ID, "none").add(Root.COLUMN_DOCUMENT_ID, DocIds.SIGNIN)
                .add(Root.COLUMN_TITLE, "Allternit")
                .add(Root.COLUMN_SUMMARY, summary)
                .add(Root.COLUMN_FLAGS, 0).add(Root.COLUMN_ICON, icon).add(Root.COLUMN_MIME_TYPES, "*/*")
            return c
        }
        for (pc in list) {
            c.newRow().add(Root.COLUMN_ROOT_ID, pc.id).add(Root.COLUMN_DOCUMENT_ID, DocIds.computer(pc.id))
                .add(Root.COLUMN_TITLE, pc.name).add(Root.COLUMN_SUMMARY, "Allternit · " + stateLabel(pc))
                .add(Root.COLUMN_FLAGS, Root.FLAG_SUPPORTS_CREATE or Root.FLAG_SUPPORTS_IS_CHILD)
                .add(Root.COLUMN_ICON, icon).add(Root.COLUMN_MIME_TYPES, "*/*")
        }
        return c
    }

    private fun stateLabel(pc: Computer): String = when (drive.knownState(pc.id)) {
        ComputerState.AWAKE -> "Awake"
        ComputerState.ASLEEP -> "Asleep"
        ComputerState.WAKING -> "Waking"
        ComputerState.OFFLINE -> "Offline"
        null -> when {
            pc.status == "online" -> "Awake"
            pc.wakeable -> "Asleep"
            else -> "Offline"
        }
    }

    // ------------------------------------------------------------------ documents

    override fun queryDocument(documentId: String, projection: Array<out String>?): Cursor = fnf {
        val c = MatrixCursor(projection ?: DOC_COLS)
        when (val p = DocIds.parse(documentId)) {
            DocIds.Parsed.SignIn -> addDir(c, documentId, "Allternit", Document.FLAG_SUPPORTS_SETTINGS, null)
            is DocIds.Parsed.Computer -> addDir(c, documentId, drive.computerName(p.computerId), Document.FLAG_SUPPORTS_SETTINGS, null)
            is DocIds.Parsed.Item -> {
                val rootRow = drive.rootsCached(p.computerId)?.roots?.firstOrNull { it.rootItemId == p.itemId && it.id == p.rootId }
                val item = drive.item(p.computerId, p.itemId)
                    ?: if (rootRow != null) null else blocking { drive.stat(p.computerId, p.itemId) }
                if (item != null) addItem(c, documentId, p.computerId, item)
                else addDir(c, documentId, rootRow!!.name, if (rootRow.policy.write) Document.FLAG_DIR_SUPPORTS_CREATE else 0, null)
            }
            is DocIds.Parsed.Pending -> {
                val rec = drive.cache.record(p.recordId) ?: throw FileNotFoundException("Gone")
                val done = rec.resultItemId?.let { drive.item(rec.computerId, it) }
                if (done != null) addItem(c, documentId, rec.computerId, done) else addPending(c, rec)
            }
        }
        c
    }

    override fun queryChildDocuments(parentDocumentId: String, projection: Array<out String>?, sortOrder: String?): Cursor {
        val c = MatrixCursor(projection ?: DOC_COLS)
        val resolver = context!!.contentResolver
        c.setNotificationUri(resolver, DocumentsContract.buildChildDocumentsUri(drive.authority, parentDocumentId))
        val extras = Bundle()
        val p = try {
            DocIds.parse(parentDocumentId)
        } catch (e: FileNotFoundException) {
            return c
        }
        if (!drive.auth.signedIn()) throw signInRequired()
        when (p) {
            DocIds.Parsed.SignIn -> {
                val info = if (drive.computers.isNotEmpty()) {
                    "Your computers are hidden from Files. Open Allternit Files to choose which ones show here."
                } else {
                    "Add a computer at ai.allternit.com, then it shows up here."
                }
                extras.putString(DocumentsContract.EXTRA_INFO, info)
            }
            is DocIds.Parsed.Computer -> {
                val cid = p.computerId
                val roots = drive.rootsCached(cid)
                val checked = rootsChecked[cid] ?: 0L
                if (roots == null || System.currentTimeMillis() - checked > 30_000) {
                    rootsChecked[cid] = System.currentTimeMillis()
                    drive.refreshRootsAsync(cid)
                }
                if (roots == null) {
                    drive.rootsError(cid)?.let { extras.putString(DocumentsContract.EXTRA_ERROR, it) }
                        ?: extras.putBoolean(DocumentsContract.EXTRA_LOADING, true)
                } else {
                    val ids = roots.roots.map { it.id }.toSet()
                    for (r in roots.roots.filter { it.parentRootId == null || it.parentRootId !in ids }) {
                        val locked = r.policy.protected && !drive.isUnlocked(cid)
                        val summary = when {
                            locked -> "Locked · unlock to open"
                            !r.providerReady -> "Not set up yet"
                            else -> null
                        }
                        // FLAG_SUPPORTS_SETTINGS: long-press a folder > Settings opens Allternit Files
                        // (which computers show, sign out).
                        val create = if (r.policy.write && !locked) Document.FLAG_DIR_SUPPORTS_CREATE else 0
                        addDir(c, DocIds.item(cid, r.id, r.rootItemId), r.name, create or Document.FLAG_SUPPORTS_SETTINGS, summary)
                    }
                    info(cid, roots.freshness)?.let { extras.putString(DocumentsContract.EXTRA_INFO, it) }
                }
            }
            is DocIds.Parsed.Item -> {
                val cid = p.computerId
                val root = drive.root(cid, p.rootId)
                if (root != null && root.policy.protected && !drive.isUnlocked(cid)) throw unlockRequired(cid, p.rootId, root.name)
                val listing = drive.listingCached(cid, p.rootId, p.itemId)
                if (!drive.isFresh(cid, p.rootId, p.itemId, listing)) drive.refreshListingAsync(cid, p.rootId, p.itemId, parentDocumentId)
                listing?.items?.forEach { item ->
                    val link = item.rootLink
                    val childId = if (link != null) {
                        DocIds.item(cid, link, drive.root(cid, link)?.rootItemId ?: item.id)
                    } else {
                        DocIds.item(cid, p.rootId, item.id)
                    }
                    addItem(c, childId, cid, item)
                }
                drive.cache.records()
                    .filter { it.computerId == cid && it.parentId == p.itemId && it.state != "done" }
                    .filter { rec -> listing?.items?.none { it.id == rec.resultItemId } ?: true }
                    .forEach { addPending(c, it) }
                val err = drive.lastError(cid, p.rootId, p.itemId)
                when {
                    listing == null && err != null -> extras.putString(DocumentsContract.EXTRA_ERROR, err)
                    listing == null -> extras.putBoolean(DocumentsContract.EXTRA_LOADING, true)
                    else -> info(cid, listing.freshness)?.let { extras.putString(DocumentsContract.EXTRA_INFO, it) }
                }
                drive.watch(cid, p.rootId)
            }
            is DocIds.Parsed.Pending -> Unit
        }
        c.extras = extras
        return c
    }

    private fun info(cid: String, f: Freshness?): String? {
        if (f == null) return null
        val name = drive.computerName(cid)
        val at = f.snapshotAt?.let {
            try {
                DateUtils.getRelativeTimeSpanString(Instant.parse(it).toEpochMilli()).toString()
            } catch (e: Exception) {
                null
            }
        }
        return when {
            f.servedBy == ServedBy.CACHE -> "Offline. Showing the list saved on this phone."
            f.servedBy == ServedBy.REPLICA -> "$name is asleep. Files open from a copy on another of your devices."
            f.servedBy == ServedBy.SNAPSHOT || f.computerState == ComputerState.ASLEEP ->
                "$name is asleep. This is the list from ${at ?: "its last sync"}. Browsing is free; opening a file asks to wake it."
            f.computerState == ComputerState.OFFLINE -> "$name is offline. Showing the last list."
            f.computerState == ComputerState.WAKING -> "$name is waking up."
            else -> null
        }
    }

    // ------------------------------------------------------------------ rows

    private fun addDir(c: MatrixCursor, id: String, name: String, flags: Int, summary: String?) {
        c.newRow().add(Document.COLUMN_DOCUMENT_ID, id).add(Document.COLUMN_DISPLAY_NAME, name)
            .add(Document.COLUMN_MIME_TYPE, Document.MIME_TYPE_DIR).add(Document.COLUMN_FLAGS, flags)
            .add(Document.COLUMN_SIZE, null).add(Document.COLUMN_LAST_MODIFIED, null).add(Document.COLUMN_SUMMARY, summary)
    }

    private fun addItem(c: MatrixCursor, id: String, cid: String, item: Item) {
        val dir = item.kind == ItemKind.FOLDER
        val mime = if (dir) Document.MIME_TYPE_DIR else (item.mime ?: mimeOf(item.name))
        val perm = item.permissions
        var flags = 0
        if (dir && perm.write) flags = flags or Document.FLAG_DIR_SUPPORTS_CREATE
        if (!dir && perm.write) flags = flags or Document.FLAG_SUPPORTS_WRITE
        if (item.parentId != null) {
            if (perm.delete) flags = flags or Document.FLAG_SUPPORTS_DELETE
            if (perm.write) flags = flags or Document.FLAG_SUPPORTS_RENAME or Document.FLAG_SUPPORTS_MOVE
        }
        if (!dir && (mime.startsWith("image/") || mime.startsWith("video/"))) flags = flags or Document.FLAG_SUPPORTS_THUMBNAIL
        val cached = !dir && drive.cache.cachedRev(cid, item.id) == item.rev
        // Colour badges (design 5C). DocumentsUI has no overlay API, so a badge replaces
        // the type icon, and only for the states worth a glance: a conflict copy and a
        // file an agent made. Thumbnails still draw over it in the grid.
        val conflict = !dir && CONFLICT_NAME.containsMatchIn(item.name)
        val agent = item.provenance?.source == ProvenanceSource.AGENT
        val badge = when {
            conflict -> R.drawable.ic_files_badge_conflict
            agent -> R.drawable.ic_files_badge_agent
            else -> null
        }
        val summary = listOfNotNull(
            if (conflict) "Conflict copy · both kept" else null,
            if (agent) "Made by an agent" else null,
            if (cached) "On this phone" else null,
        ).joinToString(" · ").ifEmpty { null }
        c.newRow().add(Document.COLUMN_DOCUMENT_ID, id).add(Document.COLUMN_DISPLAY_NAME, item.name)
            .add(Document.COLUMN_MIME_TYPE, mime).add(Document.COLUMN_FLAGS, flags)
            .add(Document.COLUMN_SIZE, if (dir) null else item.size)
            .add(Document.COLUMN_LAST_MODIFIED, millis(item.modifiedAt))
            .add(Document.COLUMN_SUMMARY, summary)
            .add(Document.COLUMN_ICON, badge)
    }

    private fun addPending(c: MatrixCursor, rec: UploadRecord) {
        val summary = when (rec.state) {
            "writing" -> "Saving…"
            "queued", "uploading" -> "Uploading to ${drive.computerName(rec.computerId)}"
            "waiting_wake" -> "Waiting for ${drive.computerName(rec.computerId)} to wake"
            "failed" -> "Couldn't upload: ${rec.error ?: "error"}"
            else -> null
        }
        val badge = when (rec.state) {
            "waiting_wake" -> R.drawable.ic_files_badge_asleep
            "failed" -> R.drawable.ic_files_badge_error
            else -> R.drawable.ic_files_badge_upload
        }
        c.newRow().add(Document.COLUMN_DOCUMENT_ID, DocIds.pending(rec.id)).add(Document.COLUMN_DISPLAY_NAME, rec.name)
            .add(Document.COLUMN_MIME_TYPE, rec.mime)
            .add(Document.COLUMN_FLAGS, Document.FLAG_SUPPORTS_WRITE or Document.FLAG_SUPPORTS_DELETE or Document.FLAG_SUPPORTS_RENAME)
            .add(Document.COLUMN_SIZE, rec.file.length()).add(Document.COLUMN_LAST_MODIFIED, rec.updatedAt)
            .add(Document.COLUMN_SUMMARY, summary)
            .add(Document.COLUMN_ICON, badge)
    }

    // ------------------------------------------------------------------ open

    override fun openDocument(documentId: String, mode: String, signal: CancellationSignal?): ParcelFileDescriptor = fnf {
        val pfdMode = ParcelFileDescriptor.parseMode(mode)
        when (val p = DocIds.parse(documentId)) {
            is DocIds.Parsed.Item -> openItem(p.computerId, p.rootId, p.itemId, mode, pfdMode, signal)
            is DocIds.Parsed.Pending -> {
                val rec = drive.cache.record(p.recordId) ?: throw FileNotFoundException("This file is gone.")
                val done = rec.resultItemId
                if (rec.state == "done" && done != null) {
                    openItem(rec.computerId, rec.rootId, done, mode, pfdMode, signal)
                } else if (mode.contains('w')) {
                    rec.state = "writing"
                    drive.cache.save(rec)
                    ParcelFileDescriptor.open(rec.file, pfdMode or ParcelFileDescriptor.MODE_CREATE, closeHandler) {
                        drive.queue(drive.cache.record(rec.id) ?: rec, 1_500)
                    }
                } else {
                    ParcelFileDescriptor.open(rec.file, ParcelFileDescriptor.MODE_READ_ONLY)
                }
            }
            else -> throw FileNotFoundException("Not a file")
        }
    }

    private fun openItem(cid: String, rid: String, iid: String, mode: String, pfdMode: Int, signal: CancellationSignal?): ParcelFileDescriptor {
        val item = drive.item(cid, iid) ?: blocking(signal) { drive.stat(cid, iid) }
        if (item.kind == ItemKind.FOLDER) throw FileNotFoundException("${item.name} is a folder")
        val root = drive.root(cid, rid)
        if (root != null && root.policy.protected && !drive.isUnlocked(cid)) {
            throw FileNotFoundException("${root.name} is locked. Open it in Files and unlock it first.")
        }
        val cacheFile = drive.cache.contentFile(cid, iid)
        val cachedRev = drive.cache.cachedRev(cid, iid)
        if (!mode.contains('w')) {
            if (cachedRev == item.rev) {
                drive.cache.touch(cacheFile)
                return ParcelFileDescriptor.open(cacheFile, ParcelFileDescriptor.MODE_READ_ONLY)
            }
            return try {
                if (item.size > PROXY_MIN) {
                    blocking(signal) { drive.ensureContent(cid, WakeReason.OPEN_FILE, iid, item.name) }
                    val thread = HandlerThread("files-range").apply { start() }
                    val reader = RangeReader(drive, cid, iid, item.size, thread)
                    context!!.getSystemService(StorageManager::class.java)
                        .openProxyFileDescriptor(ParcelFileDescriptor.MODE_READ_ONLY, reader, Handler(thread.looper))
                } else {
                    blocking(signal, 300_000) { download(cid, item, signal) }
                    ParcelFileDescriptor.open(cacheFile, ParcelFileDescriptor.MODE_READ_ONLY)
                }
            } catch (e: Exception) {
                // Asleep, offline or refused: an older copy on the phone still opens (served_by cache).
                if (cachedRev != null && e !is OperationCanceledException) {
                    ParcelFileDescriptor.open(cacheFile, ParcelFileDescriptor.MODE_READ_ONLY)
                } else {
                    throw e
                }
            }
        }
        // Write: stage locally, upload on close with base_rev = the rev the writer started from.
        val staging = drive.cache.newStaging()
        if ((pfdMode and ParcelFileDescriptor.MODE_TRUNCATE) == 0) {
            if (drive.cache.cachedRev(cid, iid) != item.rev) blocking(signal, 300_000) { download(cid, item, signal) }
            cacheFile.copyTo(staging, overwrite = true)
        } else {
            staging.createNewFile()
        }
        val rec = drive.newEdit(cid, item.rootId, item, staging)
        return ParcelFileDescriptor.open(staging, pfdMode or ParcelFileDescriptor.MODE_CREATE, closeHandler) {
            drive.queue(drive.cache.record(rec.id) ?: rec, 1_500)
        }
    }

    /** Streams the content into the offline cache, resuming a partial download of the same rev. */
    private fun download(cid: String, item: Item, signal: CancellationSignal?) {
        drive.withWake(cid, WakeReason.OPEN_FILE, item.id, item.name) { client ->
            val part = drive.cache.partFile(cid, item.id, item.rev)
            val from = if (part.exists()) part.length() else 0L
            val conn = client.content(item.id, from.takeIf { it > 0 })
            try {
                val append = conn.responseCode == 206 && from > 0
                FileOutputStream(part, append).use { out ->
                    conn.inputStream.use { input ->
                        FilesClient.copy(input, out) { if (signal?.isCanceled == true) throw OperationCanceledException() }
                    }
                }
                val etag = conn.getHeaderField("ETag")?.removePrefix("W/")?.trim('"', ' ')
                drive.cache.putContent(cid, item.id, etag ?: item.rev, part)
            } finally {
                conn.disconnect()
            }
        }
    }

    override fun openDocumentThumbnail(documentId: String, sizeHint: Point, signal: CancellationSignal?): AssetFileDescriptor = fnf {
        val p = DocIds.parse(documentId) as? DocIds.Parsed.Item ?: throw FileNotFoundException("No thumbnail")
        val cid = p.computerId
        val item = drive.item(cid, p.itemId) ?: throw FileNotFoundException("No thumbnail")
        val mime = item.mime ?: mimeOf(item.name)
        val thumb = File(drive.cache.thumbs, drive.cache.key(cid, item.id, item.rev) + ".jpg")
        if (!thumb.exists()) {
            var src = drive.cache.contentFile(cid, item.id).takeIf { drive.cache.cachedRev(cid, item.id) != null }
            if (src == null && mime.startsWith("image/") && item.size <= THUMB_FETCH_MAX && drive.knownState(cid) == ComputerState.AWAKE) {
                // Only while awake and without a ticket: a thumbnail never wakes a computer.
                blocking(signal) {
                    val conn = drive.client(cid).content(item.id)
                    try {
                        val part = drive.cache.partFile(cid, item.id, item.rev)
                        FileOutputStream(part).use { out -> conn.inputStream.use { FilesClient.copy(it, out) } }
                        drive.cache.putContent(cid, item.id, item.rev, part)
                    } finally {
                        conn.disconnect()
                    }
                }
                src = drive.cache.contentFile(cid, item.id)
            }
            if (src == null || !src.exists()) throw FileNotFoundException("No thumbnail yet")
            val bmp = if (mime.startsWith("video/")) videoFrame(src) else decodeSampled(src, maxOf(sizeHint.x, sizeHint.y, 96))
                ?: throw FileNotFoundException("No thumbnail")
            FileOutputStream(thumb).use { bmp.compress(Bitmap.CompressFormat.JPEG, 85, it) }
            bmp.recycle()
        }
        AssetFileDescriptor(ParcelFileDescriptor.open(thumb, ParcelFileDescriptor.MODE_READ_ONLY), 0, AssetFileDescriptor.UNKNOWN_LENGTH)
    }

    private fun decodeSampled(f: File, target: Int): Bitmap? {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeFile(f.path, bounds)
        var sample = 1
        while (bounds.outWidth / (sample * 2) >= target && bounds.outHeight / (sample * 2) >= target) sample *= 2
        return BitmapFactory.decodeFile(f.path, BitmapFactory.Options().apply { inSampleSize = sample })
    }

    private fun videoFrame(f: File): Bitmap? {
        val r = MediaMetadataRetriever()
        return try {
            r.setDataSource(f.path)
            r.frameAtTime
        } catch (e: Exception) {
            null
        } finally {
            r.release()
        }
    }

    // ------------------------------------------------------------------ create, rename, move, delete

    override fun createDocument(parentDocumentId: String, mimeType: String, displayName: String): String = fnf {
        val p = DocIds.parse(parentDocumentId) as? DocIds.Parsed.Item
            ?: throw FileNotFoundException("Choose a folder on the computer first")
        val cid = p.computerId
        if (mimeType == Document.MIME_TYPE_DIR) {
            val res = blocking {
                drive.withWake(cid, WakeReason.SYNC, null, displayName) { it.mkdir(MkdirRequest(p.itemId, displayName, OnConflict.KEEP_BOTH)) }
            }
            drive.putInListing(cid, p.rootId, res.item)
            drive.notifyChildren(parentDocumentId)
            return@fnf DocIds.item(cid, p.rootId, res.item.id)
        }
        val name = withExtension(displayName, mimeType)
        // Created on the phone first, so a save works even while the computer sleeps;
        // it uploads when the app closes it (or after a few seconds if it never writes).
        val rec = drive.newPending(cid, p.rootId, p.itemId, name, mimeType, "queued")
        drive.kickUploads(5_000)
        drive.notifyChildren(parentDocumentId)
        DocIds.pending(rec.id)
    }

    override fun renameDocument(documentId: String, displayName: String): String? = fnf {
        when (val p = DocIds.parse(documentId)) {
            is DocIds.Parsed.Pending -> {
                val rec = drive.cache.record(p.recordId) ?: throw FileNotFoundException("Gone")
                if (rec.state == "done") throw FileNotFoundException("Already uploaded; rename it from its folder")
                rec.name = displayName
                drive.cache.save(rec)
                drive.notifyChildren(DocIds.item(rec.computerId, rec.rootId, rec.parentId))
                null
            }
            is DocIds.Parsed.Item -> {
                val item = drive.item(p.computerId, p.itemId) ?: blocking { drive.stat(p.computerId, p.itemId) }
                val res = blocking {
                    drive.withWake(p.computerId, WakeReason.SYNC, item.id, item.name) {
                        it.move(MoveRequest(itemId = item.id, baseRev = item.rev, newName = displayName, onConflict = OnConflict.FAIL))
                    }
                }
                drive.putInListing(p.computerId, p.rootId, res.item)
                item.parentId?.let { drive.notifyChildren(DocIds.item(p.computerId, p.rootId, it)) }
                null // the item id survives a rename (SPEC section 4)
            }
            else -> throw FileNotFoundException("Can't rename this")
        }
    }

    override fun deleteDocument(documentId: String) {
        fnf<Unit> {
            when (val p = DocIds.parse(documentId)) {
                is DocIds.Parsed.Pending -> {
                    val rec = drive.cache.record(p.recordId) ?: return@fnf
                    drive.cache.deleteRecord(rec)
                    drive.notifyChildren(DocIds.item(rec.computerId, rec.rootId, rec.parentId))
                }
                is DocIds.Parsed.Item -> {
                    val item = drive.item(p.computerId, p.itemId) ?: blocking { drive.stat(p.computerId, p.itemId) }
                    blocking {
                        drive.withWake(p.computerId, WakeReason.SYNC, item.id, item.name) {
                            it.delete(DeleteRequest(itemId = item.id, baseRev = item.rev, permanent = false))
                        }
                    }
                    item.parentId?.let {
                        drive.dropFromListing(p.computerId, p.rootId, it, item.id)
                        drive.notifyChildren(DocIds.item(p.computerId, p.rootId, it))
                    }
                }
                else -> throw FileNotFoundException("Can't delete this")
            }
        }
    }

    override fun moveDocument(sourceDocumentId: String, sourceParentDocumentId: String, targetParentDocumentId: String): String = fnf {
        val src = DocIds.parse(sourceDocumentId) as? DocIds.Parsed.Item ?: throw FileNotFoundException("Can't move this")
        val dst = DocIds.parse(targetParentDocumentId) as? DocIds.Parsed.Item ?: throw FileNotFoundException("Choose a folder")
        if (src.computerId != dst.computerId) throw FileNotFoundException("Files move within one computer. Copy it to the other computer instead.")
        val cid = src.computerId
        val item = drive.item(cid, src.itemId) ?: blocking { drive.stat(cid, src.itemId) }
        val res = blocking {
            drive.withWake(cid, WakeReason.SYNC, item.id, item.name) {
                it.move(MoveRequest(itemId = item.id, baseRev = item.rev, newParentId = dst.itemId, onConflict = OnConflict.KEEP_BOTH))
            }
        }
        item.parentId?.let { drive.dropFromListing(cid, src.rootId, it, item.id) }
        drive.putInListing(cid, res.item.rootId, res.item)
        drive.notifyChildren(sourceParentDocumentId)
        drive.notifyChildren(targetParentDocumentId)
        DocIds.item(cid, res.item.rootId, res.item.id) // the id survives the move, the root may change
    }

    override fun isChildDocument(parentDocumentId: String, documentId: String): Boolean = try {
        val parent = DocIds.parse(parentDocumentId)
        val (cid, start) = when (val d = DocIds.parse(documentId)) {
            is DocIds.Parsed.Item -> d.computerId to d.itemId
            is DocIds.Parsed.Pending -> drive.cache.record(d.recordId)?.let { it.computerId to it.parentId } ?: (null to null)
            else -> null to null
        }
        when (parent) {
            is DocIds.Parsed.Computer -> cid != null && cid == parent.computerId
            is DocIds.Parsed.Item -> cid != null && start != null && cid == parent.computerId && descends(cid, parent.itemId, start)
            else -> false
        }
    } catch (e: Exception) {
        false
    }

    /** Walks `parent_id` up from `from` (at most 64 levels). Unknown links are fetched (stat; never wakes). */
    private fun descends(cid: String, ancestor: String, from: String): Boolean {
        var cur: String = from
        repeat(64) {
            if (cur == ancestor) return true
            val item = drive.item(cid, cur) ?: blocking { drive.stat(cid, cur) }
            cur = item.parentId ?: return false
        }
        return false
    }

    // ------------------------------------------------------------------ helpers

    private fun signInRequired(): AuthenticationRequiredException {
        val i = Intent(context, FilesSignInActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        val pi = PendingIntent.getActivity(context, 1, i, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        return AuthenticationRequiredException(SecurityException("Sign in to Allternit Files"), pi)
    }

    private fun unlockRequired(cid: String, rid: String, rootName: String): AuthenticationRequiredException {
        val i = Intent(context, UnlockActivity::class.java)
            .setData(Uri.parse("allternit-files://unlock/" + Uri.encode(cid) + "/" + Uri.encode(rid)))
            .putExtra(UnlockActivity.EXTRA_COMPUTER, cid)
            .putExtra(UnlockActivity.EXTRA_ROOT, rid)
            .putExtra(UnlockActivity.EXTRA_NAME, rootName)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        val pi = PendingIntent.getActivity(context, (cid + rid).hashCode(), i, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        return AuthenticationRequiredException(SecurityException("$rootName is locked"), pi)
    }

    /** Runs network work off the binder thread (the caller's StrictMode follows binder calls). */
    private fun <T> blocking(signal: CancellationSignal? = null, timeoutMs: Long = 45_000, f: () -> T): T {
        val fut = drive.io.submit(Callable { f() })
        signal?.setOnCancelListener { fut.cancel(true) }
        try {
            return fut.get(timeoutMs, TimeUnit.MILLISECONDS)
        } catch (e: ExecutionException) {
            throw e.cause ?: e
        } catch (e: TimeoutException) {
            fut.cancel(true)
            throw FileNotFoundException("The computer took too long to answer. Try again.")
        } catch (e: CancellationException) {
            throw OperationCanceledException()
        }
    }

    /**
     * Provider calls may only throw what the binder carries: FileNotFoundException,
     * AuthenticationRequiredException (DocumentsUI shows its button), cancellation.
     */
    private inline fun <T> fnf(f: () -> T): T = try {
        f()
    } catch (e: FileNotFoundException) {
        throw e
    } catch (e: AuthenticationRequiredException) {
        throw e
    } catch (e: OperationCanceledException) {
        throw e
    } catch (e: Exception) {
        throw FileNotFoundException(e.message ?: "Couldn't reach the computer")
    }

    companion object {
        private const val PROXY_MIN = 16L * 1024 * 1024
        private const val THUMB_FETCH_MAX = 8L * 1024 * 1024

        private val ROOT_COLS = arrayOf(
            Root.COLUMN_ROOT_ID, Root.COLUMN_TITLE, Root.COLUMN_SUMMARY, Root.COLUMN_FLAGS,
            Root.COLUMN_ICON, Root.COLUMN_DOCUMENT_ID, Root.COLUMN_MIME_TYPES,
        )
        private val DOC_COLS = arrayOf(
            Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME, Document.COLUMN_MIME_TYPE,
            Document.COLUMN_SIZE, Document.COLUMN_LAST_MODIFIED, Document.COLUMN_FLAGS, Document.COLUMN_SUMMARY,
            Document.COLUMN_ICON,
        )

        /** SPEC section 4 conflict copy: `<stem> (conflict <device> <YYYY-MM-DD HHmm>[ n]).<ext>`. */
        private val CONFLICT_NAME = Regex(""" \(conflict .+ \d{4}-\d{2}-\d{2} \d{4}( \d+)?\)""")

        fun mimeOf(name: String): String =
            MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substringAfterLast('.', "").lowercase()) ?: "application/octet-stream"

        fun withExtension(name: String, mime: String): String {
            if (name.contains('.') || mime == "application/octet-stream") return name
            val ext = MimeTypeMap.getSingleton().getExtensionFromMimeType(mime) ?: return name
            return "$name.$ext"
        }

        private fun millis(ts: String): Long? = try {
            Instant.parse(ts).toEpochMilli()
        } catch (e: Exception) {
            null
        }
    }
}
