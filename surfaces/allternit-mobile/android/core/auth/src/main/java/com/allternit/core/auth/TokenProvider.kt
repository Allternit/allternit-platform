package com.allternit.core.auth

import com.allternit.core.api.BearerProvider

/**
 * The signed-in user's API token, for every native screen.
 *
 * Only the interface exists until step 7 of the native app plan
 * (handoffs/android-native-2026-11-18/PLAN.md), which picks the implementation after
 * a device spike:
 *  - Clerk Android (`com.clerk:clerk-android`, still pre-1.0) with org auto-create, or
 *  - the Files-style sign-in that already ships: a Custom Tab to m.allternit.com mints a
 *    cloud-api token sealed to a Keystore RSA key (app `files/FilesAuth.kt`). This one
 *    also carries the TWA → native hand-off, since the user is signed in there already.
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
