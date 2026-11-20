package com.allternit.feature.push

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import com.allternit.core.api.ApiConfig
import com.allternit.core.files.KeystoreBox
import com.allternit.core.files.SealedSecret
import okhttp3.OkHttpClient
import okhttp3.Request
import org.json.JSONObject
import java.util.concurrent.TimeUnit

/**
 * Private notification bodies, the port of the iOS notification service extension (`ios/AllternitNotifications/
 * NotificationService.swift`, plan step I2). For a push whose `private` is true the FCM body is generic; this fetches
 * the real text by event id (`GET /api/v1/notifications/content/:eventId`, cloud-api device_read.rs) with the device
 * read token, plus the bot's avatar, and says whether the cloud marks it a communication (a bot's direct reply).
 * Any failure or the time budget leaves the generic content: a notification is never dropped.
 *
 * The read token (`adr_<row>_<secret>`, handed back by every device registration) is kept Keystore-sealed
 * ([SealedSecret], StrongBox when present) in no-backup storage, never in prefs or logs, and removed on sign-out.
 */
internal object PrivateBodies {
    class Content(val title: String?, val body: String?, val senderName: String?, val avatar: Bitmap?, val communication: Boolean)

    /** Own budget, under the time FCM allows `onMessageReceived` (iOS uses 20 s of its 30). */
    private const val BUDGET_MS = 9_000L
    private val http by lazy { OkHttpClient.Builder().callTimeout(BUDGET_MS, TimeUnit.MILLISECONDS).build() }

    private fun secret(context: Context) = SealedSecret(context.applicationContext, "device-read-token", KeystoreBox("push_read_token_v1", "allternit-device-read-token-v1"))

    fun storeReadToken(context: Context, token: String?) = runCatching { secret(context).write(token) }

    fun readToken(context: Context): String? = runCatching { secret(context).read() }.getOrNull()

    /** The event id of a private push (`data.private` true, `eventId` inside the `data` JSON), else null. */
    fun privateEventId(data: Map<String, String>): String? {
        if (data["private"] != "true") return null
        val inner = runCatching { JSONObject(data["data"].orEmpty()) }.getOrNull()
        return (inner?.optString("eventId")?.takeIf { it.isNotEmpty() } ?: data["eventId"])?.takeIf { it.isNotBlank() }
    }

    /** Blocking (FCM runs `onMessageReceived` off the main thread). Null on any failure. */
    fun fetch(context: Context, eventId: String): Content? {
        val started = System.currentTimeMillis()
        val token = readToken(context) ?: return null
        val seg = java.net.URLEncoder.encode(eventId, "UTF-8").replace("+", "%20")
        val url = ApiConfig().cloudBaseUrl.newBuilder().addPathSegments("api/v1/notifications/content").addEncodedPathSegment(seg).build()
        val req = Request.Builder().url(url).get().header("Authorization", "Bearer $token").header("Accept", "application/json").build()
        val json = runCatching {
            http.newCall(req).execute().use { r -> if (r.code == 200) r.body?.string() else null }
        }.getOrNull()?.let { runCatching { JSONObject(it) }.getOrNull() } ?: return null
        val sender = json.optJSONObject("sender")
        val avatarUrl = sender?.optString("avatarUrl")?.takeIf { it.startsWith("https://") }
        val left = BUDGET_MS - (System.currentTimeMillis() - started)
        val avatar = if (avatarUrl != null && left > 1_000) download(avatarUrl, left) else null
        return Content(
            title = json.optString("title").takeIf { it.isNotBlank() && !json.isNull("title") },
            body = json.optString("body").takeIf { it.isNotBlank() && !json.isNull("body") },
            senderName = sender?.optString("name")?.trim()?.takeIf { it.isNotEmpty() && !sender.isNull("name") },
            avatar = avatar,
            communication = json.optBoolean("communication"),
        )
    }

    private fun download(url: String, budgetMs: Long): Bitmap? = runCatching {
        val client = http.newBuilder().callTimeout(minOf(budgetMs, 8_000L), TimeUnit.MILLISECONDS).build()
        client.newCall(Request.Builder().url(url).build()).execute().use { r ->
            val bytes = if (r.code == 200) r.body?.bytes() else null
            if (bytes == null || bytes.isEmpty() || bytes.size > 2_000_000) null else BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
        }
    }.getOrNull()
}
