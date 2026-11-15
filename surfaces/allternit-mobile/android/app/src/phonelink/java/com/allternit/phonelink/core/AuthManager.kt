package com.allternit.phonelink.core

import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/** One paired client. `id` is the token id (first 4 bytes of SHA-256(K), hex). */
data class AuthSession(
    val id: String,
    val keyHex: String,
    val clientName: String,
    val grantedAtMs: Long,
    val lastActiveAtMs: Long,
    val lastAddress: String? = null,
    val lastVia: String? = null,
) {
    val key: ByteArray? get() = Hex.decode(keyHex)?.takeIf { it.size == 32 }

    fun toJson(): JSONObject = JSONObject()
        .put("id", id).put("key", keyHex).put("client_name", clientName)
        .put("granted_at", grantedAtMs).put("last_active_at", lastActiveAtMs)
        .put("last_address", lastAddress ?: JSONObject.NULL)
        .put("last_via", lastVia ?: JSONObject.NULL)

    /** What `config` and the status screen show: never the key. */
    fun publicJson(inactivityMs: Long): JSONObject = JSONObject()
        .put("session_id", id).put("client_name", clientName)
        .put("granted_at", grantedAtMs / 1000).put("last_active_at", lastActiveAtMs / 1000)
        .put("expires", (lastActiveAtMs + inactivityMs) / 1000)
        .put("last_address", lastAddress ?: JSONObject.NULL)
        .put("last_via", lastVia ?: JSONObject.NULL)

    companion object {
        fun fromJson(o: JSONObject): AuthSession? {
            val id = o.optString("id", "")
            val key = o.optString("key", "")
            if (id.isEmpty() || Hex.decode(key)?.size != 32) return null
            return AuthSession(
                id = id, keyHex = key, clientName = o.optString("client_name", "?"),
                grantedAtMs = o.optLong("granted_at", 0), lastActiveAtMs = o.optLong("last_active_at", 0),
                lastAddress = o.optString("last_address", "").ifEmpty { null }.takeUnless { o.isNull("last_address") },
                lastVia = o.optString("last_via", "").ifEmpty { null }.takeUnless { o.isNull("last_via") },
            )
        }

        fun listToJson(list: List<AuthSession>): String = JSONArray().also { a -> list.forEach { a.put(it.toJson()) } }.toString()

        fun listFromJson(text: String): List<AuthSession> {
            val arr = JSONArray(text)
            return (0 until arr.length()).mapNotNull { arr.optJSONObject(it)?.let(::fromJson) }
        }
    }
}

/** Where sessions persist (Keystore-encrypted file on device, memory in tests). */
interface SessionStore {
    fun load(): List<AuthSession>
    fun save(sessions: List<AuthSession>)
}

class MemorySessionStore(initial: List<AuthSession> = emptyList()) : SessionStore {
    @Volatile var saved: List<AuthSession> = initial
    var saveCount = 0
    override fun load() = saved
    override fun save(sessions: List<AuthSession>) { saved = sessions; saveCount++ }
}

/** Asks the device user to approve a pairing (a notification on Android). */
interface PairApprover {
    /** Show the prompt; call [answer] at most once. */
    fun requestApproval(requestId: String, clientName: String, fingerprint: String?, local: Boolean, answer: (Boolean) -> Unit)
    /** The request timed out; withdraw the prompt. */
    fun expire(requestId: String)
}

sealed class PairOutcome {
    data class Approved(val session: AuthSession) : PairOutcome()
    object Denied : PairOutcome()
    data class Busy(val retryAfter: Int) : PairOutcome()
    data class CoolingDown(val retryAfter: Int) : PairOutcome()
}

data class Peer(val address: String, val via: String)

class Opened(val tokId: String, val plaintext: String, val seal: (String) -> String)

/**
 * Pairing + envelope authentication, a port of iCTRLKit/AuthManager.swift.
 * Same limits: 30-day sliding idle expiry, 100 sessions, per-session
 * time-bounded nonce cache, 60 s approval timeout, 15 s cooldown after a
 * refusal, one pending request per slot (loopback vs everyone else).
 *
 * @param consumeArmedGrant one-shot auto-approval (the PROVISION broadcast's
 *   `arm_pairing`), the Android counterpart of ICTRL_ALLOW_AUTH_ONCE. Called
 *   with whether the request came over loopback; returns true at most once.
 */
class AuthManager(
    private val store: SessionStore,
    private val clock: () -> Long = System::currentTimeMillis,
    private val randomBytes: (Int) -> ByteArray = SecureEnvelope::randomBytes,
    private val approvalTimeoutMs: Long = APPROVAL_TIMEOUT_MS,
    private val consumeArmedGrant: (local: Boolean) -> Boolean = { false },
) {
    companion object {
        const val INACTIVITY_MS = 30L * 24 * 60 * 60 * 1000
        const val MAX_SESSIONS = 100
        const val NONCE_CAPACITY_PER_SESSION = 20_000
        const val APPROVAL_TIMEOUT_MS = 60_000L
        const val PAIR_COOLDOWN_MS = 15_000L
        private const val PERSIST_USE_INTERVAL_MS = 3_600_000L
        const val AUTH_FAILED_MESSAGE = "Authentication failed — this key is not usable. Re-pair to continue: " +
            "POST /pair (the device user must tap Allow). Note a key is dropped after 30 days with no " +
            "successful call, so a long-idle client must re-pair. If instead the clock is off you would " +
            "get ts_skew, not this."
    }

    @Volatile var approver: PairApprover? = null

    private val lock = Any()
    private val sessions = LinkedHashMap<String, AuthSession>()
    private val seenNonces = HashMap<String, HashMap<String, Long>>()
    private val lastPersistedUse = HashMap<String, Long>()

    private class PairSlot(var pending: Boolean = false, var cooldownUntil: Long = 0)
    private val pairSlots = HashMap<Boolean, PairSlot>()

    /** Called whenever the session list changes (status screen refresh). */
    @Volatile var onSessionsChanged: (() -> Unit)? = null

    init {
        synchronized(lock) { store.load().forEach { sessions[it.id] = it } }
        pruneExpired()
    }

    val activeSessions: List<AuthSession>
        get() {
            pruneExpired()
            return synchronized(lock) { sessions.values.sortedByDescending { it.grantedAtMs } }
        }

    // ── Pairing ─────────────────────────────────────────────────────────────

    /** Blocks until approved, denied or timed out. */
    fun requestAuth(clientName: String, local: Boolean, fingerprint: String?): PairOutcome {
        val now = clock()
        synchronized(lock) {
            val slot = pairSlots.getOrPut(local) { PairSlot() }
            if (slot.pending) return PairOutcome.Busy((approvalTimeoutMs / 1000).toInt())
            if (now < slot.cooldownUntil) {
                val wait = ((slot.cooldownUntil - now + 999) / 1000).toInt()
                return PairOutcome.CoolingDown(maxOf(wait, 1))
            }
            slot.pending = true
        }
        if (consumeArmedGrant(local)) return finishPairing(clientName, true, local)

        val approver = approver ?: return finishPairing(clientName, false, local)
        val requestId = "auth-" + UUID.randomUUID().toString()
        val latch = CountDownLatch(1)
        val claimed = AtomicBoolean(false)
        val approved = AtomicBoolean(false)
        approver.requestApproval(requestId, clientName, fingerprint, local) { yes ->
            if (claimed.compareAndSet(false, true)) {
                approved.set(yes)
                latch.countDown()
            }
        }
        val answered = latch.await(approvalTimeoutMs, TimeUnit.MILLISECONDS)
        if (!answered && claimed.compareAndSet(false, true)) {
            approver.expire(requestId)
            return finishPairing(clientName, false, local)
        }
        if (!answered) latch.await() // the answer won the race with the timeout; it has counted down
        return finishPairing(clientName, approved.get(), local)
    }

    private fun finishPairing(clientName: String, approved: Boolean, local: Boolean): PairOutcome {
        val session = if (approved) createSession(clientName) else null
        synchronized(lock) {
            val slot = pairSlots.getOrPut(local) { PairSlot() }
            slot.pending = false
            if (session == null) slot.cooldownUntil = clock() + PAIR_COOLDOWN_MS
        }
        return session?.let { PairOutcome.Approved(it) } ?: PairOutcome.Denied
    }

    private fun createSession(clientName: String): AuthSession {
        val key = randomBytes(32)
        val now = clock()
        val s = AuthSession(SecureEnvelope.tokenId(key), key.toHex(), clientName, now, now)
        synchronized(lock) { sessions[s.id] = s }
        persist()
        return s
    }

    // ── Envelope verification ───────────────────────────────────────────────

    /**
     * Verifies and decrypts a request envelope. Order mirrors the Swift
     * server: parse → session lookup → idle expiry → timestamp window → MAC →
     * replay cache. The MAC is checked before the replay cache is touched.
     */
    fun openEnvelope(obj: JSONObject, peer: Peer? = null): Opened {
        val p = SecureEnvelope.parse(obj)
        var session = synchronized(lock) { sessions[p.tokId] }
            ?: throw SecureEnvelope.Failure("auth_failed", AUTH_FAILED_MESSAGE)
        val now = clock()
        if (now - session.lastActiveAtMs > INACTIVITY_MS) {
            synchronized(lock) { sessions.remove(p.tokId); seenNonces.remove(p.tokId) }
            persist()
            throw SecureEnvelope.Failure("auth_failed", AUTH_FAILED_MESSAGE)
        }
        val nowS = now / 1000.0
        if (Math.abs(nowS - p.ts) > SecureEnvelope.TIMESTAMP_WINDOW_S) {
            throw SecureEnvelope.Failure("ts_skew", "Timestamp outside ±120s window")
        }
        val key = session.key ?: throw SecureEnvelope.Failure("auth_failed", AUTH_FAILED_MESSAGE)

        val plaintext = SecureEnvelope.open(p, key)

        var needsPersist = false
        synchronized(lock) {
            val seen = seenNonces.getOrPut(p.tokId) { HashMap() }
            val cutoff = (now / 1000) - SecureEnvelope.TIMESTAMP_WINDOW_S
            if (seen.size > 64) seen.entries.removeIf { it.value < cutoff }
            if (seen.containsKey(p.nonceHex)) throw SecureEnvelope.Failure("nonce_replayed", "Duplicate nonce")
            if (seen.size >= NONCE_CAPACITY_PER_SESSION) {
                throw SecureEnvelope.Failure("nonce_flood", "Too many in-flight nonces; slow down")
            }
            seen[p.nonceHex] = p.ts
            session = sessions[p.tokId] ?: session
            val peerChanged = peer != null && (peer.address != session.lastAddress || peer.via != session.lastVia)
            session = session.copy(
                lastActiveAtMs = now,
                lastAddress = peer?.address ?: session.lastAddress,
                lastVia = peer?.via ?: session.lastVia,
            )
            sessions[p.tokId] = session
            val last = lastPersistedUse[p.tokId]
            needsPersist = peerChanged || last == null || now - last > PERSIST_USE_INTERVAL_MS
            if (needsPersist) lastPersistedUse[p.tokId] = now
        }
        if (needsPersist) persist()

        val nonce = p.nonce
        val nonceHex = p.nonceHex
        return Opened(p.tokId, plaintext) { json -> SecureEnvelope.seal(json, key, nonce, nonceHex) }
    }

    // ── Management ──────────────────────────────────────────────────────────

    fun recordPeer(sessionId: String, peer: Peer?) {
        if (peer == null) return
        synchronized(lock) {
            val s = sessions[sessionId] ?: return
            sessions[sessionId] = s.copy(lastAddress = peer.address, lastVia = peer.via)
        }
        persist()
    }

    fun revokeSession(id: String): Boolean {
        val removed = synchronized(lock) {
            seenNonces.remove(id)
            sessions.remove(id) != null
        }
        persist()
        return removed
    }

    fun revokeAll() {
        synchronized(lock) { sessions.clear(); seenNonces.clear() }
        persist()
    }

    fun pruneExpired(): Int {
        var removed = 0
        synchronized(lock) {
            val before = sessions.size
            val now = clock()
            sessions.entries.removeIf { now - it.value.lastActiveAtMs >= INACTIVITY_MS }
            if (sessions.size > MAX_SESSIONS) {
                val keep = sessions.values.sortedByDescending { it.lastActiveAtMs }.take(MAX_SESSIONS)
                sessions.clear()
                keep.forEach { sessions[it.id] = it }
            }
            seenNonces.keys.retainAll(sessions.keys)
            removed = before - sessions.size
        }
        if (removed > 0) persist()
        return removed
    }

    private fun persist() {
        val list = synchronized(lock) { sessions.values.toList() }
        store.save(list)
        onSessionsChanged?.invoke()
    }
}
