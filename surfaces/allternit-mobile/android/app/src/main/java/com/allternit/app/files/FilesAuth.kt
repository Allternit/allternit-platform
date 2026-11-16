package com.allternit.app.files

import android.content.Context
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.io.File
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.SecureRandom
import java.util.Base64
import java.util.UUID
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Fixed addresses. Production only: no localhost or dev fallbacks. */
internal object FilesConfig {
    const val CLOUD_API = "https://api.allternit.com"
    const val WEB = "https://m.allternit.com"
    /** The page that signs the user in (Clerk session in Chrome) and seals a token to this phone. */
    const val CONNECT_PATH = "/files/connect-android"
    /** The https App Link the page returns to (autoVerify filter on FilesSignInActivity). */
    const val CALLBACK_PATH = "/files/android-callback"
    /**
     * The same hand-off for a build whose App Link is not verified (a debug build, or a
     * signing key m.allternit.com's assetlinks.json does not list). The page links to
     * `allternit-files://signin?state&sealed` as an `intent://` URL with `package=<app>`.
     * Safe as a custom scheme: `sealed` opens only with this phone's Keystore key.
     */
    const val CALLBACK_SCHEME = "allternit-files"
    const val CALLBACK_SCHEME_HOST = "signin"
    const val V2 = "/api/v1/files/v2"

    fun authority(context: Context) = context.packageName + ".files"
}

/**
 * The phone's Files credential (SPEC section 2 "Computer ids": Android lists runtime
 * devices from cloud-api and uses those ids as is).
 *
 * Sign-in (spike 8, production form):
 *  1. A fresh RSA-2048 key is made in the Android Keystore (decrypt only, never
 *     exportable) with a random `state`.
 *  2. A Custom Tab opens `m.allternit.com/files/connect-android?state&pk&device`. The
 *     page is signed in through the browser's own session, mints a cloud-api API
 *     token for this phone and encrypts it to `pk` with RSA-OAEP (SHA-1, MGF1 SHA-1:
 *     the pair both WebCrypto and every Keystore since API 23 support).
 *  3. The page links to the https App Link `m.allternit.com/files/android-callback
 *     ?state&sealed`. Only this app is verified for it, and only this phone's
 *     Keystore can open `sealed`, so the token never travels in clear.
 *  4. The token is stored sealed with a Keystore AES-256-GCM key (the same pattern
 *     as Phone Link's KeystoreSealer) in no-backup storage.
 */
internal class FilesAuth(private val context: Context) {
    private val prefs = context.getSharedPreferences("files_drive", Context.MODE_PRIVATE)
    private val dir = File(context.noBackupFilesDir, "files-drive").apply { mkdirs() }
    private val tokenFile = File(dir, "token.bin")
    @Volatile private var cachedToken: String? = null

    /** Stable id for this phone as a Files device (`X-Allternit-Device-Id`, wake consent). */
    val deviceId: String
        get() = prefs.getString("device_id", null) ?: ("and_" + UUID.randomUUID().toString().replace("-", "")).also {
            prefs.edit().putString("device_id", it).apply()
        }

    /** The phone's name, used in conflict copies ("Lease (conflict Pixel 9 2026-11-15 1042).pdf"). */
    val deviceName: String
        get() = Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)
            ?.takeIf { it.isNotBlank() } ?: Build.MODEL

    fun token(): String? {
        cachedToken?.let { return it }
        if (!tokenFile.exists()) return null
        return try {
            String(open(tokenFile.readBytes()), Charsets.UTF_8).also { cachedToken = it }
        } catch (e: Exception) {
            // Key gone (device restore, Keystore wipe): sign in again.
            tokenFile.delete()
            null
        }
    }

    fun signedIn(): Boolean = token() != null

    fun signOut() {
        cachedToken = null
        tokenFile.delete()
        prefs.edit().remove("signin_state").remove("signin_at").apply()
    }

    /** Step 1 + 2: a new hand-off key and the URL the Custom Tab opens. */
    fun beginSignIn(): Uri {
        val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        if (ks.containsAlias(HANDOFF_ALIAS)) ks.deleteEntry(HANDOFF_ALIAS)
        val gen = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_RSA, "AndroidKeyStore")
        gen.initialize(
            KeyGenParameterSpec.Builder(HANDOFF_ALIAS, KeyProperties.PURPOSE_DECRYPT)
                .setKeySize(2048)
                .setDigests(KeyProperties.DIGEST_SHA1, KeyProperties.DIGEST_SHA256)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_RSA_OAEP)
                .build(),
        )
        val pair = gen.generateKeyPair()
        val state = b64url(ByteArray(24).also { SecureRandom().nextBytes(it) })
        prefs.edit().putString("signin_state", state).putLong("signin_at", System.currentTimeMillis()).apply()
        return Uri.parse(FilesConfig.WEB + FilesConfig.CONNECT_PATH).buildUpon()
            .appendQueryParameter("state", state)
            .appendQueryParameter("pk", b64url(pair.public.encoded)) // X.509 SubjectPublicKeyInfo (WebCrypto "spki")
            .appendQueryParameter("alg", "RSA-OAEP-SHA1")
            .appendQueryParameter("device", deviceName)
            .appendQueryParameter("device_id", deviceId)
            // The package to return to for the custom-scheme fallback (debug installs as
            // com.allternit.app.ondevice).
            .appendQueryParameter("app", context.packageName)
            .build()
    }

    /** True when `uri` is our App Link callback (or its custom-scheme fallback). */
    fun isCallback(uri: Uri?): Boolean {
        if (uri == null) return false
        if (uri.scheme == FilesConfig.CALLBACK_SCHEME) return uri.host == FilesConfig.CALLBACK_SCHEME_HOST
        return uri.scheme == "https" && uri.host == Uri.parse(FilesConfig.WEB).host && uri.path == FilesConfig.CALLBACK_PATH
    }

    /** Step 3 + 4. Throws with a user-readable message when the hand-off is not ours or is stale. */
    fun completeSignIn(uri: Uri) {
        uri.getQueryParameter("error")?.let { throw IllegalStateException(it) }
        val state = uri.getQueryParameter("state") ?: throw IllegalStateException("The sign-in link is missing its state.")
        val sealed = uri.getQueryParameter("sealed") ?: throw IllegalStateException("The sign-in link is missing its token.")
        val expected = prefs.getString("signin_state", null)
        val at = prefs.getLong("signin_at", 0)
        if (expected == null || expected != state) throw IllegalStateException("This sign-in link was not started on this phone. Start again.")
        if (System.currentTimeMillis() - at > SIGNIN_TTL_MS) throw IllegalStateException("The sign-in took too long. Start again.")
        val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        val key = ks.getKey(HANDOFF_ALIAS, null) as? PrivateKey ?: throw IllegalStateException("Start sign-in again.")
        val cipher = Cipher.getInstance("RSA/ECB/OAEPWithSHA-1AndMGF1Padding")
        cipher.init(Cipher.DECRYPT_MODE, key)
        val token = String(cipher.doFinal(Base64.getUrlDecoder().decode(sealed)), Charsets.UTF_8).trim()
        require(token.isNotEmpty()) { "Empty token" }
        tokenFile.writeBytes(seal(token.toByteArray(Charsets.UTF_8)))
        cachedToken = token
        ks.deleteEntry(HANDOFF_ALIAS)
        prefs.edit().remove("signin_state").remove("signin_at").apply()
    }

    // AES-256-GCM, non-exportable Keystore key (KeystoreSealer pattern, src/phonelink).
    // Blob: version(1) | ivLen(1) | iv | ciphertext+tag.
    @Synchronized
    private fun aesKey(): SecretKey {
        val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (ks.getKey(TOKEN_ALIAS, null) as? SecretKey)?.let { return it }
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        gen.init(
            KeyGenParameterSpec.Builder(TOKEN_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return gen.generateKey()
    }

    private fun seal(plain: ByteArray): ByteArray {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, aesKey())
        cipher.updateAAD(AAD)
        val iv = cipher.iv
        return byteArrayOf(1, iv.size.toByte()) + iv + cipher.doFinal(plain)
    }

    private fun open(blob: ByteArray): ByteArray {
        require(blob.size > 2 && blob[0].toInt() == 1) { "unknown blob version" }
        val ivLen = blob[1].toInt() and 0xff
        require(blob.size > 2 + ivLen) { "truncated blob" }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, aesKey(), GCMParameterSpec(128, blob.copyOfRange(2, 2 + ivLen)))
        cipher.updateAAD(AAD)
        return cipher.doFinal(blob, 2 + ivLen, blob.size - 2 - ivLen)
    }

    companion object {
        private const val HANDOFF_ALIAS = "files_signin_handoff_v1"
        private const val TOKEN_ALIAS = "files_token_v1"
        private val AAD = "allternit-files-token-v1".toByteArray()
        private const val SIGNIN_TTL_MS = 15 * 60_000L

        fun b64url(b: ByteArray): String = Base64.getUrlEncoder().withoutPadding().encodeToString(b)
    }
}
