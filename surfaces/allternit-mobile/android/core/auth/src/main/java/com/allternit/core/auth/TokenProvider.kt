package com.allternit.core.auth

import com.allternit.core.api.BearerProvider

/**
 * The signed-in user's API token, for every native screen.
 *
 * Step 7b picked Clerk Android (`com.clerk:clerk-android-api`, step 7a spike): the
 * implementation is [ClerkTokenProvider], reached as `AllternitAuth.tokenProvider`.
 * The Files-style Custom Tab token (app `files/FilesAuth.kt`) stays the fallback only.
 *
 * It extends [BearerProvider], so any implementation plugs into
 * `ApiClients.okHttp(bearer = tokenProvider)` unchanged.
 */
interface TokenProvider : BearerProvider {
    /** The cached token, or null when signed out. Never touches the network. */
    fun currentToken(): String?

    /** Gets a fresh token (after a 401 or on expiry); null means the user must sign in again. */
    suspend fun refresh(): String?

    /** Forgets the token on this device. */
    suspend fun signOut()

    override fun bearerToken(): String? = currentToken()
}
