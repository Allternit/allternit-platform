package com.allternit.core.auth

import com.clerk.api.Clerk
import com.clerk.api.network.serialization.ClerkResult
import java.util.Base64
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeoutOrNull
import org.json.JSONObject

/**
 * The Clerk session JWT as a [TokenProvider], for `ApiClients.okHttp(bearer = …)`.
 *
 * Clerk session tokens live about 60 s. [currentToken] returns the cached token while it has
 * more than [REFRESH_MARGIN_SECONDS] left; [bearerToken] (called on OkHttp's thread, never
 * the main thread) refreshes through Clerk's own token cache when it is about to expire, the
 * same way iOS `AuthManager.getToken()` asks `Clerk.shared.session?.getToken()` per call.
 */
class ClerkTokenProvider internal constructor() : TokenProvider {
    private data class Cached(val jwt: String, val expiresAtSeconds: Long)

    private val cached = AtomicReference<Cached?>(null)

    override fun currentToken(): String? {
        val c = cached.get() ?: return null
        return if (c.expiresAtSeconds - nowSeconds() > REFRESH_MARGIN_SECONDS) c.jwt else null
    }

    override fun bearerToken(): String? = currentToken() ?: runBlocking {
        withTimeoutOrNull(REFRESH_TIMEOUT_MS) { refresh() }
    }

    override suspend fun refresh(): String? {
        if (Clerk.session == null) {
            clear()
            return null
        }
        val jwt = (Clerk.auth.getToken() as? ClerkResult.Success)?.value?.takeIf { it.isNotEmpty() } ?: return null
        cached.set(Cached(jwt, JwtExpiry.expiresAtSeconds(jwt) ?: (nowSeconds() + FALLBACK_LIFETIME_SECONDS)))
        return jwt
    }

    override suspend fun signOut() {
        AllternitAuth.signOut()
    }

    internal fun clear() = cached.set(null)

    private fun nowSeconds(): Long = System.currentTimeMillis() / 1000

    companion object {
        const val REFRESH_MARGIN_SECONDS = 10L
        private const val FALLBACK_LIFETIME_SECONDS = 30L
        private const val REFRESH_TIMEOUT_MS = 10_000L
    }
}

/** Reads `exp` from a JWT payload without checking the signature (the server checks it). */
object JwtExpiry {
    fun expiresAtSeconds(jwt: String): Long? {
        val parts = jwt.split('.')
        if (parts.size != 3) return null
        return runCatching {
            val payload = String(Base64.getUrlDecoder().decode(parts[1].trimEnd('=')), Charsets.UTF_8)
            JSONObject(payload).optLong("exp", 0L).takeIf { it > 0L }
        }.getOrNull()
    }
}
