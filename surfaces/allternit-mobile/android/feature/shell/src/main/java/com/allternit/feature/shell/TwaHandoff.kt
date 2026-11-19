package com.allternit.feature.shell

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiConfig
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.SecureRandom
import java.util.Base64
import javax.crypto.Cipher
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import okhttp3.OkHttpClient

/**
 * TWA → native sign-in hand-off. People who used the old Play app (m.allternit.com in a
 * Trusted Web Activity) are signed in to Clerk inside Chrome, not in this app. On the first
 * signed-out launch after the update, a Custom Tab (same Chrome cookie jar as the TWA) opens a
 * page that mints a one-time Clerk sign-in token for that session and returns it here, sealed
 * to this phone; the app then calls `signInWithTicket`. Same shape as the Files sign-in
 * (app/files/FilesAuth.kt, `m.allternit.com/files/connect-android`):
 *
 *  1. A fresh RSA-2048 Keystore key (decrypt only, never exportable) and a random `state`.
 *  2. A Custom Tab opens `m.allternit.com/auth/android-handoff?state&pk&alg&app`.
 *  3. Signed in: the page asks cloud-api to mint a sign-in token, seals it to `pk`
 *     (RSA-OAEP SHA-1, the pair WebCrypto and every Keystore since API 23 share) and links to
 *     the App Link `m.allternit.com/auth/android-callback?state&sealed`, or
 *     `allternit-auth://handoff?state&sealed` as an `intent://` URL with `package=<app>` when
 *     the App Link is not verified. Signed out: it links back at once with `error=signed_out`.
 *  4. The app opens `sealed` with its key and signs in with the token.
 *
 * It runs at most once per install, only on an update install (a fresh install never had the
 * TWA), and only when cloud-api says the hand-off exists
 * (`GET /api/v1/auth/android-handoff/config` → `{ "enabled": true }`). Anything else, or the
 * user closing the tab, leaves the normal sign-in screen in place.
 */
internal class TwaHandoff(private val context: Context) {
    private val prefs = context.getSharedPreferences("allternit_shell", Context.MODE_PRIVATE)

    /** Not tried yet, and this install is an update of an older (TWA) app. */
    fun shouldTry(): Boolean {
        if (prefs.getBoolean(KEY_TRIED, false)) return false
        return runCatching {
            val info = if (Build.VERSION.SDK_INT >= 33) {
                context.packageManager.getPackageInfo(context.packageName, PackageManager.PackageInfoFlags.of(0))
            } else {
                @Suppress("DEPRECATION")
                context.packageManager.getPackageInfo(context.packageName, 0)
            }
            info.lastUpdateTime > info.firstInstallTime
        }.getOrDefault(false)
    }

    /** Never ask again on this install (tried, skipped, or signed in some other way). */
    fun markTried() {
        prefs.edit().putBoolean(KEY_TRIED, true).apply()
    }

    /** cloud-api's switch for the hand-off. False on any error, so a missing route skips it. */
    suspend fun isEnabled(client: OkHttpClient): Boolean {
        val url = ApiConfig().apiBaseUrl.newBuilder().addPathSegments("auth/android-handoff/config").build()
        val r = ApiCall.execute(client, ApiJson.default, ApiCall.request("GET", url, null), JsonObject.serializer())
        return r is ApiResult.Ok && (r.value["enabled"] as? JsonPrimitive)?.booleanOrNull == true
    }

    /** Steps 1 + 2: a new key and state, then the Custom Tab. False when nothing could open it. */
    fun launch(from: Context): Boolean {
        val url = try {
            begin()
        } catch (e: Exception) {
            return false
        }
        // A Custom Tab without androidx.browser: ACTION_VIEW plus the session extra (null binder).
        val tab = Intent(Intent.ACTION_VIEW, url).apply {
            putExtras(Bundle().apply { putBinder(EXTRA_SESSION, null) })
            putExtra(EXTRA_TITLE_VISIBILITY, 1)
            // From the activity, so the callback link comes back to the same task (singleTop).
            if (from !is android.app.Activity) addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        return try {
            from.startActivity(tab)
            true
        } catch (e: ActivityNotFoundException) {
            false
        }
    }

    private fun begin(): Uri {
        val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        if (ks.containsAlias(ALIAS)) ks.deleteEntry(ALIAS)
        val gen = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_RSA, "AndroidKeyStore")
        gen.initialize(
            KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_DECRYPT)
                .setKeySize(2048)
                .setDigests(KeyProperties.DIGEST_SHA1, KeyProperties.DIGEST_SHA256)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_RSA_OAEP)
                .build(),
        )
        val pair = gen.generateKeyPair()
        val state = b64url(ByteArray(24).also { SecureRandom().nextBytes(it) })
        prefs.edit().putString(KEY_STATE, state).putLong(KEY_AT, System.currentTimeMillis()).apply()
        return Uri.parse(WEB + CONNECT_PATH).buildUpon()
            .appendQueryParameter("state", state)
            .appendQueryParameter("pk", b64url(pair.public.encoded))
            .appendQueryParameter("alg", "RSA-OAEP-SHA1")
            .appendQueryParameter("app", context.packageName)
            .build()
    }

    /** True when [uri] is our App Link callback or its custom-scheme fallback. */
    fun isCallback(uri: Uri?): Boolean {
        if (uri == null) return false
        if (uri.scheme == CALLBACK_SCHEME) return uri.host == CALLBACK_HOST
        return uri.scheme == "https" && uri.host == Uri.parse(WEB).host && uri.path == CALLBACK_PATH
    }

    /**
     * Steps 3 + 4: the sign-in token, or null when the page said the browser was signed out.
     * Throws with a readable message when the link is not ours or is stale.
     */
    fun complete(uri: Uri): String? {
        try {
            uri.getQueryParameter("error")?.let { if (it == "signed_out") return null else throw IllegalStateException(it) }
            val state = uri.getQueryParameter("state") ?: throw IllegalStateException("The sign-in link is missing its state.")
            val sealed = uri.getQueryParameter("sealed") ?: throw IllegalStateException("The sign-in link is missing its token.")
            val expected = prefs.getString(KEY_STATE, null)
            if (expected == null || expected != state) throw IllegalStateException("This sign-in link was not started on this phone.")
            if (System.currentTimeMillis() - prefs.getLong(KEY_AT, 0) > TTL_MS) throw IllegalStateException("The sign-in took too long.")
            val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            val key = ks.getKey(ALIAS, null) as? PrivateKey ?: throw IllegalStateException("Start sign-in again.")
            val cipher = Cipher.getInstance("RSA/ECB/OAEPWithSHA-1AndMGF1Padding")
            cipher.init(Cipher.DECRYPT_MODE, key)
            return String(cipher.doFinal(Base64.getUrlDecoder().decode(sealed)), Charsets.UTF_8).trim().takeIf { it.isNotEmpty() }
        } finally {
            runCatching { KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.deleteEntry(ALIAS) }
            prefs.edit().remove(KEY_STATE).remove(KEY_AT).apply()
        }
    }

    companion object {
        const val WEB = "https://m.allternit.com"
        const val CONNECT_PATH = "/auth/android-handoff"
        const val CALLBACK_PATH = "/auth/android-callback"
        const val CALLBACK_SCHEME = "allternit-auth"
        const val CALLBACK_HOST = "handoff"
        private const val ALIAS = "twa_signin_handoff_v1"
        private const val KEY_TRIED = "twa-handoff-tried"
        private const val KEY_STATE = "twa-handoff-state"
        private const val KEY_AT = "twa-handoff-at"
        private const val TTL_MS = 10 * 60_000L
        private const val EXTRA_SESSION = "android.support.customtabs.extra.SESSION"
        private const val EXTRA_TITLE_VISIBILITY = "android.support.customtabs.extra.TITLE_VISIBILITY"

        private fun b64url(b: ByteArray): String = Base64.getUrlEncoder().withoutPadding().encodeToString(b)
    }
}
