package com.allternit.app.files

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.job.JobInfo
import android.app.job.JobScheduler
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
import com.allternit.files.contract.ComputerState
import com.allternit.files.contract.ConflictInfo
import com.allternit.files.contract.EndpointsResponse
import com.allternit.files.contract.ErrorCode
import com.allternit.files.contract.Freshness
import com.allternit.files.contract.Item
import com.allternit.files.contract.ListResponse
import com.allternit.files.contract.OnConflict
import com.allternit.files.contract.Root
import com.allternit.files.contract.RootsResponse
import com.allternit.files.contract.SmallWriteQuery
import com.allternit.files.contract.TransportKind
import com.allternit.files.contract.UploadCreateRequest
import com.allternit.files.contract.UploadState
import com.allternit.files.contract.UploadStatus
import com.allternit.files.contract.WakeConsentRequest
import com.allternit.files.contract.WakeConsentResponse
import com.allternit.files.contract.WakeReason
import com.allternit.files.contract.WakeSetting
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.time.Instant
import java.util.TimeZone
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/** Thrown when content needs a wake the user has not agreed to yet; the prompt is already posted. */
internal class WakeNeeded(val computerName: String) : java.io.IOException("$computerName is asleep. Tap the notification to wake it.")

/**
 * The process-wide Files drive for Android: computers, routes, the offline cache, the
 * change feed, wake tickets, unlocks and the upload queue. The DocumentsProvider, the
 * share screen and the job service all use this one object.
 *
 * Rules it keeps (SPEC):
 *  - Browsing never wakes (section 9): listings and stats go out without a ticket and
 *    are answered from the snapshot while a computer sleeps.
 *  - Only a user's consent wakes: the wake-consent call (from [WakePromptActivity], or
 *    silently when the computer's setting is `auto`) is the only thing that gets a
 *    ticket, and the ticket rides on `X-Allternit-Wake-Ticket` until it expires.
 *  - No silent data loss (section 4): every write carries `base_rev` and `keep_both`.
 */
internal class FilesDrive private constructor(val context: Context) {
    val auth = FilesAuth(context)
    val cache = DriveCache(File(context.noBackupFilesDir, "files-drive"))
    val authority = FilesConfig.authority(context)

    /** Provider network work: never on a binder thread (spike 8 lesson). */
    val io: ExecutorService = Executors.newCachedThreadPool()
    private val uploadsExec = Executors.newSingleThreadExecutor()

    // ------------------------------------------------------------------ computers

    @Volatile var computers: List<Computer> = cache.readComputers()
        private set
    @Volatile private var computersAt = 0L
    @Volatile var computersError: String? = null
        private set

    fun api(): CloudApi = CloudApi(auth.token() ?: throw FilesException(401, ErrorCode.UNAUTHORIZED, null, "Sign in to Allternit Files."))

    fun computer(cid: String): Computer? = computers.firstOrNull { it.id == cid }
    fun computerName(cid: String) = computer(cid)?.name ?: "Your computer"

    fun refreshComputers(): List<Computer> {
        return try {
            val list = api().computers()
            computers = list
            computersAt = System.currentTimeMillis()
            computersError = null
            cache.writeComputers(list)
            list
        } catch (e: FilesException) {
            computersError = e.message
            if (e.code == ErrorCode.UNAUTHORIZED && e.status == 401) auth.signOut()
            throw e
        }
    }

    fun refreshComputersAsync() {
        if (System.currentTimeMillis() - computersAt < 60_000 || !auth.signedIn()) return
        if (!inflight.add("computers")) return
        io.execute {
            try {
                refreshComputers()
            } catch (_: Exception) {
            } finally {
                inflight.remove("computers")
                notifyRoots()
            }
        }
    }

    // ------------------------------------------------------------------ routes

    private class Route(val ep: EndpointsResponse, val at: Long)

    private val routes = ConcurrentHashMap<String, Route>()

    fun knownState(cid: String): ComputerState? = routes[cid]?.ep?.computerState

    fun endpoints(cid: String, maxAgeMs: Long = 60_000): EndpointsResponse {
        routes[cid]?.takeIf { System.currentTimeMillis() - it.at < maxAgeMs }?.let { return it.ep }
        val ep = api().endpoints(cid)
        routes[cid] = Route(ep, System.currentTimeMillis())
        return ep
    }

    /**
     * The client for a computer. Android has no mesh in the Play build and is never
     * the owning device, so it always uses the relay candidate (SPEC section 10:
     * "relay: always available"). A pairing-code machine that is mesh-only has no relay
     * candidate and is reported as such.
     */
    fun client(cid: String): FilesClient {
        val ep = endpoints(cid)
        val relay = ep.candidates.filter { it.kind == TransportKind.RELAY }.minByOrNull { it.priority }
            ?: throw FilesException(503, ErrorCode.OFFLINE, null, "${computerName(cid)} can only be reached from your own network.")
        return FilesClient(api(), ep.computerId, relay.baseUrl) { headersFor(cid) }
    }

    // ------------------------------------------------------------------ tickets and unlocks

    private val tickets = ConcurrentHashMap<String, Pair<String, Long>>()
    private val unlocks = ConcurrentHashMap<String, Long>()

    fun hasTicket(cid: String): Boolean = (tickets[cid]?.second ?: 0L) > System.currentTimeMillis() + 2_000

    fun isUnlocked(cid: String): Boolean = (unlocks[cid] ?: 0L) > System.currentTimeMillis()

    /** After a BiometricPrompt success (section 12). Valid [UNLOCK_MS]. */
    fun markUnlocked(cid: String) {
        unlocks[cid] = System.currentTimeMillis() + UNLOCK_MS
    }

    private fun headersFor(cid: String): Map<String, String> {
        val h = HashMap<String, String>()
        h["accept"] = "*/*"
        h["x-allternit-device-id"] = auth.deviceId
        h["x-allternit-device-name"] = auth.deviceName
        h["x-allternit-device-platform"] = "android"
        tickets[cid]?.takeIf { it.second > System.currentTimeMillis() }?.let { h["x-allternit-wake-ticket"] = it.first }
        // The runtime checks the header is present (files_v2 Req::extract); the value says
        // which device unlocked and when, for its audit.
        unlocks[cid]?.takeIf { it > System.currentTimeMillis() }?.let {
            h["x-allternit-unlock"] = "bio1.${auth.deviceId}.${(it - UNLOCK_MS) / 1000}"
        }
        return h
    }

    /** The consent call (section 9.4): the only thing that wakes a computer. */
    fun consentWake(cid: String, reason: WakeReason, itemId: String?): WakeConsentResponse {
        val res = api().wakeConsent(cid, WakeConsentRequest(reason = reason, itemId = itemId, deviceId = auth.deviceId))
        val exp = try {
            Instant.parse(res.expiresAt).toEpochMilli()
        } catch (e: Exception) {
            System.currentTimeMillis() + 110_000
        }
        tickets[cid] = res.ticket to exp
        routes.remove(cid)
        cancelWakeNotice(cid)
        return res
    }

    fun setWakeSetting(cid: String, setting: WakeSetting) {
        api().setWake(cid, setting)
        routes.remove(cid)
    }

    /**
     * Makes sure content requests to `cid` may run now: awake, or a ticket in hand.
     * `auto` gets a ticket silently; `ask` posts the wake prompt and throws [WakeNeeded];
     * `never` throws `wake_denied`.
     */
    fun ensureContent(cid: String, reason: WakeReason, itemId: String?, fileName: String?, hint: WakeSetting? = null) {
        if (hasTicket(cid)) return
        val ep = endpoints(cid, 15_000)
        if (hint == null && ep.computerState == ComputerState.AWAKE) return
        if (ep.computerState == ComputerState.OFFLINE) {
            throw FilesException(503, ErrorCode.OFFLINE, null, "${computerName(cid)} is offline.")
        }
        when (hint ?: ep.wake) {
            WakeSetting.AUTO -> consentWake(cid, reason, itemId)
            WakeSetting.NEVER -> throw FilesException(403, ErrorCode.WAKE_DENIED, null, "Waking is off for ${computerName(cid)}. Turn it on in Allternit to open files while it sleeps.")
            WakeSetting.ASK -> {
                postWakeNotice(cid, reason, itemId, fileName)
                throw WakeNeeded(computerName(cid))
            }
        }
    }

    /** Runs a content call; a `wake_consent_required` answer goes through [ensureContent] once. */
    fun <T> withWake(cid: String, reason: WakeReason, itemId: String?, fileName: String?, block: (FilesClient) -> T): T {
        ensureContent(cid, reason, itemId, fileName)
        return try {
            block(client(cid))
        } catch (e: FilesException) {
            if (!e.needsWake) throw e
            routes.remove(cid)
            tickets.remove(cid)
            ensureContent(cid, reason, itemId, fileName, hint = e.body?.wake?.setting ?: WakeSetting.ASK)
            block(client(cid))
        }
    }

    // ------------------------------------------------------------------ items, roots, listings

    private val items = ConcurrentHashMap<String, Item>()

    fun remember(cid: String, item: Item) {
        items["$cid/${item.id}"] = item
    }

    fun item(cid: String, iid: String): Item? = items["$cid/$iid"]

    fun stat(cid: String, iid: String): Item = client(cid).stat(iid).item.also { remember(cid, it) }

    private val rootsMem = ConcurrentHashMap<String, RootsResponse>()

    fun rootsCached(cid: String): RootsResponse? = rootsMem[cid] ?: cache.readRoots(cid)?.let {
        try {
            WireJson.decode(it, RootsResponse::class.java).also { r -> rootsMem[cid] = r }
        } catch (e: Exception) {
            null
        }
    }

    fun refreshRoots(cid: String): RootsResponse {
        val text = client(cid).rootsText()
        val r = WireJson.decode(text, RootsResponse::class.java)
        rootsMem[cid] = r
        cache.writeText(cache.rootsFile(cid), text)
        return r
    }

    fun root(cid: String, rid: String): Root? = rootsCached(cid)?.roots?.firstOrNull { it.id == rid }

    class Listing(val items: List<Item>, val freshness: Freshness?, val at: Long)

    private val stale = ConcurrentHashMap.newKeySet<String>()
    private val inflight = ConcurrentHashMap.newKeySet<String>()
    private val lastErrors = ConcurrentHashMap<String, Pair<String, Long>>()
    private val knownParents = ConcurrentHashMap<String, MutableSet<String>>()

    private fun listKey(cid: String, rid: String, parent: String) = "$cid\u0000$rid\u0000$parent"

    fun listingCached(cid: String, rid: String, parent: String): Listing? {
        val o = cache.readList(cid, rid, parent) ?: return null
        return try {
            val arr = o.getJSONArray("items")
            val list = List(arr.length()) { WireJson.decodeObject(arr.getJSONObject(it), Item::class.java) }
            list.forEach { remember(cid, it) }
            val fresh = o.optJSONObject("freshness")?.let { WireJson.decodeObject(it, Freshness::class.java) }
            Listing(list, fresh, o.optLong("at"))
        } catch (e: Exception) {
            null
        }
    }

    fun isFresh(cid: String, rid: String, parent: String, l: Listing?): Boolean =
        l != null && !stale.contains(listKey(cid, rid, parent)) && System.currentTimeMillis() - l.at < LIST_FRESH_MS

    fun lastError(cid: String, rid: String, parent: String): String? =
        lastErrors[listKey(cid, rid, parent)]?.takeIf { System.currentTimeMillis() - it.second < 60_000 }?.first

    /** Lists every page (up to 20 x 500) and stores it. Metadata only: never wakes. */
    fun refreshListing(cid: String, rid: String, parent: String): Listing {
        val c = client(cid)
        val all = JSONArray()
        var freshness: JSONObject? = null
        var token: String? = null
        var cursor: String? = null
        var pages = 0
        do {
            val text = c.listText(rid, parent, token)
            val page = JSONObject(text)
            val decoded = WireJson.decode(text, ListResponse::class.java) // validates the shape
            val arr = page.getJSONArray("items")
            for (i in 0 until arr.length()) all.put(arr.get(i))
            decoded.items.forEach { remember(cid, it) }
            remember(cid, decoded.parent)
            freshness = page.optJSONObject("freshness")
            cursor = decoded.cursor
            token = decoded.nextPageToken
            pages += 1
        } while (token != null && pages < 20)
        val now = System.currentTimeMillis()
        cache.writeText(cache.listFile(cid, rid, parent), JSONObject().put("items", all).put("freshness", freshness).put("at", now).toString())
        stale.remove(listKey(cid, rid, parent))
        lastErrors.remove(listKey(cid, rid, parent))
        knownParents.getOrPut("$cid\u0000$rid") { ConcurrentHashMap.newKeySet() }.add(parent)
        if (cursor != null) cursors.putIfAbsent("$cid\u0000$rid", cursor)
        freshness?.let {
            val f = WireJson.decodeObject(it, Freshness::class.java)
            routes[cid]?.let { r -> if (r.ep.computerState != f.computerState) routes.remove(cid) }
        }
        return listingCached(cid, rid, parent) ?: Listing(emptyList(), null, now)
    }

    /** Refreshes off-thread, then tells observers of `notifyDocId`'s children to reload. */
    fun refreshListingAsync(cid: String, rid: String, parent: String, notifyDocId: String) {
        val k = listKey(cid, rid, parent)
        val last = lastErrors[k]
        if (last != null && System.currentTimeMillis() - last.second < 15_000) return
        if (!inflight.add(k)) return
        io.execute {
            try {
                refreshListing(cid, rid, parent)
            } catch (e: Exception) {
                lastErrors[k] = (e.message ?: "Couldn't reach ${computerName(cid)}") to System.currentTimeMillis()
            } finally {
                inflight.remove(k)
                notifyChildren(notifyDocId)
            }
        }
    }

    fun refreshRootsAsync(cid: String) {
        val k = "roots\u0000$cid"
        val last = lastErrors[k]
        if (last != null && System.currentTimeMillis() - last.second < 15_000) return
        if (!inflight.add(k)) return
        io.execute {
            try {
                refreshRoots(cid)
                lastErrors.remove(k)
            } catch (e: Exception) {
                lastErrors[k] = (e.message ?: "Couldn't reach ${computerName(cid)}") to System.currentTimeMillis()
            } finally {
                inflight.remove(k)
                notifyChildren(DocIds.computer(cid))
                notifyRoots()
            }
        }
    }

    fun rootsError(cid: String): String? = lastErrors["roots\u0000$cid"]?.takeIf { System.currentTimeMillis() - it.second < 60_000 }?.first

    /** Writes `item` into its parent's cached listing (after an upload, rename or mkdir). */
    fun putInListing(cid: String, rid: String, item: Item, removeId: String? = null) {
        val parent = item.parentId ?: return
        val o = cache.readList(cid, rid, parent) ?: return
        val arr = o.getJSONArray("items")
        val out = JSONArray()
        for (i in 0 until arr.length()) {
            val id = arr.getJSONObject(i).optString("id")
            if (id != item.id && id != removeId) out.put(arr.get(i))
        }
        out.put(WireJson.encode(item))
        cache.writeText(cache.listFile(cid, rid, parent), o.put("items", out).toString())
        remember(cid, item)
    }

    fun dropFromListing(cid: String, rid: String, parent: String, itemId: String) {
        val o = cache.readList(cid, rid, parent) ?: return
        val arr = o.getJSONArray("items")
        val out = JSONArray()
        for (i in 0 until arr.length()) if (arr.getJSONObject(i).optString("id") != itemId) out.put(arr.get(i))
        cache.writeText(cache.listFile(cid, rid, parent), o.put("items", out).toString())
    }

    // ------------------------------------------------------------------ change feed (SPEC section 6)

    private val watched = ConcurrentHashMap<String, Long>()
    private val polling = ConcurrentHashMap.newKeySet<String>()
    private val cursors = ConcurrentHashMap<String, String>()

    /**
     * Keeps a long poll on `rid` while someone is looking at it (3 minutes after the last
     * query). A long poll never counts as activity and never keeps a computer awake.
     */
    fun watch(cid: String, rid: String) {
        val k = "$cid\u0000$rid"
        watched[k] = System.currentTimeMillis() + WATCH_MS
        if (polling.add(k)) io.execute { pollLoop(cid, rid, k) }
    }

    private fun pollLoop(cid: String, rid: String, k: String) {
        try {
            while ((watched[k] ?: 0L) > System.currentTimeMillis() && auth.signedIn()) {
                val cursor = cursors[k] ?: root(cid, rid)?.cursor ?: try {
                    refreshRoots(cid).roots.firstOrNull { it.id == rid }?.cursor
                } catch (e: Exception) {
                    null
                }
                if (cursor == null) {
                    Thread.sleep(30_000)
                    continue
                }
                val started = System.currentTimeMillis()
                try {
                    val res = client(cid).changes(rid, cursor, 25)
                    val touched = HashSet<String>()
                    if (res.resetRequired) {
                        knownParents["$cid\u0000$rid"]?.forEach { p ->
                            stale.add(listKey(cid, rid, p))
                            touched.add(DocIds.item(cid, rid, p))
                        }
                        touched.add(DocIds.computer(cid))
                    }
                    for (ch in res.changes) {
                        val old = item(cid, ch.itemId)
                        ch.item?.let { remember(cid, it) }
                        listOfNotNull(ch.item?.parentId, ch.oldParentId, old?.parentId).forEach { p ->
                            val r = ch.item?.rootId ?: rid
                            stale.add(listKey(cid, r, p))
                            touched.add(DocIds.item(cid, r, p))
                        }
                    }
                    cursors[k] = res.cursor
                    touched.forEach { notifyChildren(it) }
                    if (res.freshness.computerState != ComputerState.AWAKE) {
                        Thread.sleep(60_000) // a sleeping computer's snapshot changes at most every 5 minutes
                    } else if (!res.hasMore && System.currentTimeMillis() - started < 5_000) {
                        Thread.sleep(10_000) // the answer did not hold: don't spin
                    }
                } catch (e: FilesException) {
                    if (e.code == ErrorCode.CURSOR_EXPIRED) {
                        cursors.remove(k)
                        rootsMem.remove(cid)
                        knownParents["$cid\u0000$rid"]?.forEach { p ->
                            stale.add(listKey(cid, rid, p))
                            notifyChildren(DocIds.item(cid, rid, p))
                        }
                    } else if (e.status == 401) {
                        return
                    } else {
                        Thread.sleep(30_000)
                    }
                } catch (e: InterruptedException) {
                    throw e
                } catch (e: Exception) {
                    Thread.sleep(30_000) // a malformed answer: never let the poll thread crash the app
                }
            }
        } catch (_: InterruptedException) {
        } finally {
            polling.remove(k)
        }
    }

    // ------------------------------------------------------------------ uploads

    /** A new file in `parent` (a save from another app or a share). Uploads once the writer closes it. */
    fun newPending(cid: String, rid: String, parent: String, name: String, mime: String, state: String): UploadRecord {
        val rec = UploadRecord(
            id = java.util.UUID.randomUUID().toString(),
            computerId = cid, rootId = rid, parentId = parent, name = name, mime = mime,
            itemId = null, baseRev = null, staging = cache.newStaging().path, state = state,
        )
        rec.file.createNewFile()
        cache.save(rec)
        return rec
    }

    /** An edit of an existing file: replaces it at `baseRev`, or lands as a conflict copy. */
    fun newEdit(cid: String, rid: String, item: Item, staging: File): UploadRecord {
        val rec = UploadRecord(
            id = java.util.UUID.randomUUID().toString(),
            computerId = cid, rootId = rid, parentId = item.parentId ?: throw IllegalStateException("A root folder can't be replaced"),
            name = item.name, mime = item.mime ?: "application/octet-stream",
            itemId = item.id, baseRev = item.rev, staging = staging.path, state = "writing",
        )
        cache.save(rec)
        return rec
    }

    fun queue(rec: UploadRecord, delayMs: Long = 0) {
        rec.state = "queued"
        rec.error = null
        cache.save(rec)
        kickUploads(delayMs)
    }

    fun kickUploads(delayMs: Long = 0) {
        uploadsExec.execute {
            try {
                if (delayMs > 0) Thread.sleep(delayMs)
                if (runUploads()) scheduleUploadJob()
            } catch (_: Exception) {
                scheduleUploadJob()
            }
        }
    }

    /** Runs every queued upload once. True when some still wait for the network. */
    @Synchronized
    fun runUploads(): Boolean {
        if (!auth.signedIn()) return false
        cache.pruneRecords()
        var retry = false
        val blocked = HashSet<String>() // computers waiting for a wake this round
        for (rec in cache.records()) {
            val ready = rec.state == "queued" || rec.state == "uploading" || (rec.state == "waiting_wake" && hasTicket(rec.computerId))
            if (!ready || rec.computerId in blocked) continue
            try {
                rec.state = "uploading"
                rec.attempts += 1
                cache.save(rec)
                upload(rec)
            } catch (e: WakeNeeded) {
                rec.state = "waiting_wake"
                rec.error = e.message
                cache.save(rec)
                blocked.add(rec.computerId)
            } catch (e: FilesException) {
                val transient = e.status == 0 || e.status >= 500 || e.code == ErrorCode.RATE_LIMITED
                rec.state = if (transient) "queued" else "failed"
                rec.error = e.message
                cache.save(rec)
                if (transient) retry = true else postUploadFailed(rec)
            } catch (e: Exception) {
                rec.state = "queued"
                rec.error = e.message
                cache.save(rec)
                retry = true
            } finally {
                notifyChildren(DocIds.item(rec.computerId, rec.rootId, rec.parentId))
            }
        }
        return retry
    }

    private fun upload(rec: UploadRecord) {
        val f = rec.file
        if (!f.exists()) throw FilesException(400, ErrorCode.INVALID_REQUEST, null, "The file to upload is gone.")
        val offset = (TimeZone.getDefault().getOffset(System.currentTimeMillis()) / 60_000).toLong()
        val reason = WakeReason.UPLOAD
        if (f.length() <= Chunker.MAX) {
            val res = withWake(rec.computerId, reason, rec.itemId, rec.name) { c ->
                c.smallWrite(
                    rec.rootId,
                    SmallWriteQuery(
                        parentId = rec.parentId, name = rec.name, baseRev = rec.baseRev,
                        onConflict = OnConflict.KEEP_BOTH, deviceName = auth.deviceName, utcOffsetMinutes = offset,
                    ),
                    f,
                )
            }
            finish(rec, res.item, res.conflict)
            return
        }
        // Chunked, resumable (SPEC section 5): the pinned chunker, have/need via the session.
        val plan = f.inputStream().buffered(256 * 1024).use { Chunker.chunk(it) }
        withWake(rec.computerId, reason, rec.itemId, rec.name) { c ->
            var status: UploadStatus? = rec.sessionId?.let { sid ->
                try {
                    c.uploadStatus(sid).takeIf { it.state == UploadState.OPEN }
                } catch (e: FilesException) {
                    if (e.code in GONE_SESSION) null else throw e
                }
            }
            if (status == null) {
                val first = plan.chunks.take(REF_BATCH)
                status = c.uploadCreate(
                    UploadCreateRequest(
                        rootId = rec.rootId, parentId = rec.parentId, name = rec.name,
                        itemId = rec.itemId, baseRev = rec.baseRev,
                        size = plan.size, contentHash = plan.contentHash, chunker = Chunker.PARAMS,
                        chunks = first, totalChunks = plan.chunks.size.toLong(),
                        onConflict = OnConflict.KEEP_BOTH,
                        modifiedAt = Instant.ofEpochMilli(f.lastModified()).toString(),
                        deviceName = auth.deviceName, utcOffsetMinutes = offset,
                    ),
                )
                rec.sessionId = status.sessionId
                cache.save(rec)
                var i = first.size
                val need = LinkedHashSet(status.need)
                while (i < plan.chunks.size) {
                    val batch = plan.chunks.subList(i, minOf(i + REF_BATCH, plan.chunks.size))
                    need.addAll(c.uploadAppend(status!!.sessionId, batch).need)
                    i += batch.size
                }
                status = status!!.copy(need = need.toList())
            }
            val sid = status!!.sessionId
            val byHash = plan.chunks.associateBy { it.hash }
            for (h in status!!.need) byHash[h]?.let { c.putChunk(sid, it, f) }
            val done = try {
                c.uploadCommit(sid)
            } catch (e: FilesException) {
                if (e.code == ErrorCode.CHUNK_MISSING && !e.body?.missingChunks.isNullOrEmpty()) {
                    e.body!!.missingChunks!!.forEach { h -> byHash[h]?.let { c.putChunk(sid, it, f) } }
                    c.uploadCommit(sid)
                } else {
                    if (e.code == ErrorCode.MANIFEST_INVALID || e.code in GONE_SESSION) {
                        rec.sessionId = null
                        cache.save(rec)
                    }
                    throw e
                }
            }
            finish(rec, done.item, done.conflict)
        }
    }

    private fun finish(rec: UploadRecord, item: Item, conflict: ConflictInfo?) {
        val onDisk = cache.record(rec.id)
        if (onDisk != null && onDisk.state == "writing") {
            // An app opened the file again while it uploaded: its next close replaces
            // what just landed (base_rev = this rev) instead of creating a second file.
            onDisk.itemId = item.id
            onDisk.baseRev = item.rev
            onDisk.resultItemId = item.id
            onDisk.sessionId = null
            cache.save(onDisk)
            remember(rec.computerId, item)
            putInListing(rec.computerId, item.rootId, item)
            notifyChildren(DocIds.item(rec.computerId, item.rootId, rec.parentId))
            return
        }
        rec.state = "done"
        rec.resultItemId = item.id
        rec.error = null
        rec.sessionId = null
        // What was just saved is on the phone already: keep it as the offline copy.
        if (rec.file.exists()) cache.putContent(rec.computerId, item.id, item.rev, rec.file)
        cache.save(rec)
        remember(rec.computerId, item)
        putInListing(rec.computerId, item.rootId, item)
        notifyChildren(DocIds.item(rec.computerId, item.rootId, rec.parentId))
        context.contentResolver.notifyChange(DocumentsContract.buildDocumentUri(authority, DocIds.pending(rec.id)), null, false)
        if (conflict != null) postConflict(rec, conflict) else postSaved(rec)
    }

    fun scheduleUploadJob() {
        val js = context.getSystemService(JobScheduler::class.java) ?: return
        js.schedule(
            JobInfo.Builder(UPLOAD_JOB_ID, ComponentName(context, FilesUploadJobService::class.java))
                .setRequiredNetworkType(JobInfo.NETWORK_TYPE_ANY)
                .setBackoffCriteria(30_000, JobInfo.BACKOFF_POLICY_EXPONENTIAL)
                .build(),
        )
    }

    // ------------------------------------------------------------------ notify

    fun notifyRoots() {
        context.contentResolver.notifyChange(DocumentsContract.buildRootsUri(authority), null, false)
    }

    fun notifyChildren(docId: String) {
        context.contentResolver.notifyChange(DocumentsContract.buildChildDocumentsUri(authority, docId), null, false)
    }

    fun signOut() {
        auth.signOut()
        routes.clear()
        tickets.clear()
        unlocks.clear()
        rootsMem.clear()
        items.clear()
        watched.clear()
        cache.clearAll()
        computers = emptyList()
        notifyRoots()
    }

    // ------------------------------------------------------------------ notifications

    private val nm get() = context.getSystemService(NotificationManager::class.java)

    private fun channel(): String {
        nm.createNotificationChannel(NotificationChannel(CHANNEL, "Files on your computers", NotificationManager.IMPORTANCE_DEFAULT).apply {
            description = "Wake prompts and saves to your Allternit computers"
        })
        return CHANNEL
    }

    private fun post(id: Int, title: String, text: String, intent: PendingIntent?) {
        val n = android.app.Notification.Builder(context, channel())
            .setSmallIcon(android.R.drawable.stat_sys_upload_done)
            .setContentTitle(title)
            .setContentText(text)
            .setStyle(android.app.Notification.BigTextStyle().bigText(text))
            .setAutoCancel(true)
            .apply { if (intent != null) setContentIntent(intent) }
            .build()
        try {
            nm.notify(id, n)
        } catch (_: SecurityException) {
            // POST_NOTIFICATIONS not granted (Android 13+): the provider's error text still says what to do.
        }
    }

    fun wakeIntent(cid: String, reason: WakeReason, itemId: String?, fileName: String?): PendingIntent {
        val i = Intent(context, WakePromptActivity::class.java)
            .setData(Uri.parse("allternit-files://wake/" + Uri.encode(cid))) // one PendingIntent per computer
            .putExtra(WakePromptActivity.EXTRA_COMPUTER, cid)
            .putExtra(WakePromptActivity.EXTRA_REASON, reason.wire)
            .putExtra(WakePromptActivity.EXTRA_ITEM, itemId)
            .putExtra(WakePromptActivity.EXTRA_FILE, fileName)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        return PendingIntent.getActivity(context, cid.hashCode(), i, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    private fun postWakeNotice(cid: String, reason: WakeReason, itemId: String?, fileName: String?) {
        val name = computerName(cid)
        val what = when (reason) {
            WakeReason.UPLOAD -> if (fileName != null) "Saving $fileName" else "Saving files"
            else -> if (fileName != null) "Opening $fileName" else "Opening this file"
        }
        post(NOTE_WAKE + (cid.hashCode() and 0xffff), "Wake $name?", "$what needs $name awake. Tap to wake it.", wakeIntent(cid, reason, itemId, fileName))
    }

    private fun cancelWakeNotice(cid: String) {
        nm.cancel(NOTE_WAKE + (cid.hashCode() and 0xffff))
    }

    private fun postSaved(rec: UploadRecord) {
        post(NOTE_UPLOAD + (rec.id.hashCode() and 0xffff), "Saved to ${computerName(rec.computerId)}", rec.name, null)
    }

    private fun postConflict(rec: UploadRecord, c: ConflictInfo) {
        post(
            NOTE_UPLOAD + (rec.id.hashCode() and 0xffff),
            "Saved as a copy",
            "${rec.name} changed on ${computerName(rec.computerId)} while you edited it. Both are kept: yours is \"${c.conflictName}\".",
            null,
        )
    }

    private fun postUploadFailed(rec: UploadRecord) {
        post(NOTE_UPLOAD + (rec.id.hashCode() and 0xffff), "Couldn't save ${rec.name}", rec.error ?: "The computer refused the file.", null)
    }

    companion object {
        private const val CHANNEL = "files_drive"
        private const val NOTE_WAKE = 0x2200000
        private const val NOTE_UPLOAD = 0x2300000
        const val UPLOAD_JOB_ID = 22_022
        private const val UNLOCK_MS = 5 * 60_000L
        private const val LIST_FRESH_MS = 20_000L
        private const val WATCH_MS = 3 * 60_000L
        private const val REF_BATCH = 1000
        private val GONE_SESSION = setOf(ErrorCode.UPLOAD_SESSION_NOT_FOUND, ErrorCode.UPLOAD_SESSION_EXPIRED, ErrorCode.NOT_FOUND)

        @Volatile private var instance: FilesDrive? = null

        fun get(context: Context): FilesDrive = instance ?: synchronized(this) {
            instance ?: FilesDrive(context.applicationContext).also { instance = it }
        }
    }
}
