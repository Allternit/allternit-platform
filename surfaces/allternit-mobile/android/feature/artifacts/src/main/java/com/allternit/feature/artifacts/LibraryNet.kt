package com.allternit.feature.artifacts

import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiClients
import com.allternit.core.api.ApiConfig
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.design.FriendlyErrorMessage
import java.net.URLEncoder
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.longOrNull
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.OkHttpClient

/**
 * The hand-written routes wave D calls (iOS APIClient.shared get/post/patch/put/delete with a
 * path relative to `/api/v1`). Bodies are read as [JsonElement] and picked apart with the
 * helpers below, the same tolerant way the iOS Codable structs decode optional fields, so a new
 * server field never breaks a screen. Shared by :feature:artifacts and :feature:projects.
 */
object LibraryNet {
    val config = ApiConfig()
    val okHttp: OkHttpClient by lazy { ApiClients.okHttp(bearer = AllternitAuth.tokenProvider) }

    /** `/api/v1/<path>`; [path] may carry a query (`artifacts?type=design`). */
    fun url(path: String): HttpUrl {
        val trimmed = path.trimStart('/')
        return (config.apiBaseUrl.toString().trimEnd('/') + "/" + trimmed).toHttpUrlOrNull()
            ?: config.apiBaseUrl
    }

    /** A full URL for an API-relative path: `/api/v1/...` on the API host, `/api/v2/...` on the cloud host. */
    fun absolute(path: String): String {
        if (path.startsWith("http://") || path.startsWith("https://")) return path
        val base = if (path.startsWith("/api/v1/")) config.apiBaseUrl else config.cloudBaseUrl
        return base.scheme + "://" + base.host + (if (base.port != HttpUrl.defaultPort(base.scheme)) ":${base.port}" else "") + path
    }

    fun escape(segment: String): String = URLEncoder.encode(segment, "UTF-8").replace("+", "%20")

    suspend fun get(path: String): ApiResult<JsonElement> =
        ApiCall.execute(okHttp, ApiJson.default, ApiCall.request("GET", url(path), null), JsonElement.serializer())

    /** POST / PUT / PATCH / DELETE. An empty 2xx body decodes as [JsonNull]. */
    suspend fun send(method: String, path: String, body: JsonElement? = null): ApiResult<JsonElement> {
        val request = ApiCall.request(method, url(path), body?.toString() ?: if (method == "DELETE") null else "{}")
        return when (val r = ApiCall.execute(okHttp, ApiJson.default, request, JsonElement.serializer())) {
            is ApiResult.Decode -> ApiResult.Ok(JsonNull)
            else -> r
        }
    }
}

/** Product copy for a failed call (iOS FriendlyErrorMessage). */
fun ApiResult<*>.friendly(doing: String): String = when (this) {
    is ApiResult.Ok -> ""
    is ApiResult.HttpError -> when (status) {
        401, 403 -> "Sign in again to $doing."
        404 -> "That item is gone. It may have been deleted on another device."
        else -> FriendlyErrorMessage.from(message ?: code ?: "HTTP $status", doing)
    }
    is ApiResult.Network -> "You're offline. Check your connection and try again."
    is ApiResult.Decode -> "This app needs an update to $doing."
}

// MARK: - JSON helpers (tolerant like the iOS decoders)

val JsonElement?.obj: JsonObject? get() = this as? JsonObject
val JsonElement?.arr: JsonArray? get() = this as? JsonArray
fun JsonObject?.str(key: String): String? = (this?.get(key) as? JsonPrimitive)?.takeIf { it !is JsonNull }?.contentOrNull
fun JsonObject?.num(key: String): Double? = (this?.get(key) as? JsonPrimitive)?.let { it.doubleOrNull ?: it.contentOrNull?.toDoubleOrNull() }
fun JsonObject?.long(key: String): Long? = (this?.get(key) as? JsonPrimitive)?.longOrNull
fun JsonObject?.bool(key: String): Boolean? = (this?.get(key) as? JsonPrimitive)?.booleanOrNull
fun JsonObject?.child(key: String): JsonObject? = this?.get(key) as? JsonObject
fun JsonObject?.list(key: String): List<JsonObject> = (this?.get(key) as? JsonArray)?.mapNotNull { it as? JsonObject }.orEmpty()

/** The first array found under any of [keys] (servers wrap lists as `{items}`, `{artifacts}` or a bare array). */
fun JsonElement?.listUnder(vararg keys: String): List<JsonObject> {
    (this as? JsonArray)?.let { a -> return a.mapNotNull { it as? JsonObject } }
    val o = this as? JsonObject ?: return emptyList()
    for (k in keys) (o[k] as? JsonArray)?.let { a -> return a.mapNotNull { it as? JsonObject } }
    o.values.firstOrNull { it is JsonArray }?.let { a -> return (a as JsonArray).mapNotNull { it as? JsonObject } }
    return emptyList()
}

/** ISO-8601 (with or without fraction / zone) or epoch seconds/millis → epoch millis, 0 if unknown. */
fun parseTime(raw: String?): Long {
    if (raw.isNullOrBlank()) return 0
    raw.toLongOrNull()?.let { return if (it < 10_000_000_000L) it * 1000 else it }
    return runCatching { java.time.OffsetDateTime.parse(raw).toInstant().toEpochMilli() }.getOrNull()
        ?: runCatching { java.time.Instant.parse(raw).toEpochMilli() }.getOrNull()
        ?: runCatching { java.time.LocalDateTime.parse(raw).atZone(java.time.ZoneOffset.UTC).toInstant().toEpochMilli() }.getOrNull()
        ?: 0
}

/** "Just now", "5m ago", "3h ago", "2d ago", else a short date (iOS RelativeDateTimeFormatter, short). */
fun relativeTime(millis: Long, now: Long = System.currentTimeMillis()): String {
    if (millis <= 0) return ""
    val s = (now - millis) / 1000
    return when {
        s < 60 -> "Just now"
        s < 3600 -> "${s / 60}m ago"
        s < 86_400 -> "${s / 3600}h ago"
        s < 7 * 86_400 -> "${s / 86_400}d ago"
        else -> java.text.SimpleDateFormat("MMM d", java.util.Locale.getDefault()).format(java.util.Date(millis))
    }
}
