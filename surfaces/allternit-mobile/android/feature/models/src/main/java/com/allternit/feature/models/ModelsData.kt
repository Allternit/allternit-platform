package com.allternit.feature.models

import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.OkHttpClient
import okhttp3.Request
import java.net.URLEncoder
import java.util.concurrent.TimeUnit

/*
 * Models data (iOS FreeweightsClient + ComputerModelPullClient + the runtime model list).
 * Freeweights is a public site, so it is read with a plain client that never carries the account token.
 */

/** One catalog row (iOS FreeweightsCatalogRow, the fields the screens show). */
data class FwRow(
    val id: String,
    val displayName: String?,
    val section: String?,
    val license: String?,
    val params: Long?,
    val totalSize: Long?,
    val formats: List<String>,
    val note: String?,
) {
    val name: String get() = displayName?.takeIf { it.isNotBlank() } ?: id.substringAfterLast('/')
    val publisher: String get() = id.substringBefore('/', "")
    val mono: String get() = name.filter { it.isLetterOrDigit() }.take(2).uppercase().ifEmpty { "AI" }
}

sealed interface FwLoad<out T> {
    data class Live<T>(val value: T) : FwLoad<T>
    data class Down(val reason: String) : FwLoad<Nothing>
}

object FreeweightsApi {
    private const val SITE = "https://freeweights.allternit.com"
    private val http: OkHttpClient by lazy { OkHttpClient.Builder().connectTimeout(15, TimeUnit.SECONDS).readTimeout(30, TimeUnit.SECONDS).build() }

    fun pageUrl(id: String) = "$SITE/$id"

    private suspend fun get(url: String): FwLoad<JsonElement> = withContext(Dispatchers.IO) {
        try {
            http.newCall(Request.Builder().url(url).header("Accept", "application/json").build()).execute().use { r ->
                val body = r.body?.string().orEmpty()
                if (r.isSuccessful) FwLoad.Live(Json.parseToJsonElement(body))
                else FwLoad.Down(if (r.code == 404) "Not in the catalog" else "Freeweights answered ${r.code}")
            }
        } catch (e: Exception) {
            FwLoad.Down("Freeweights isn't reachable. Check your connection.")
        }
    }

    /** `GET /api/catalog`: every row in the catalog's own order. */
    suspend fun catalog(): FwLoad<List<FwRow>> = when (val r = get("$SITE/api/catalog")) {
        is FwLoad.Down -> r
        is FwLoad.Live -> FwLoad.Live(r.value.listUnder("models").mapNotNull { o ->
            val id = o.str("id") ?: return@mapNotNull null
            FwRow(
                id = id, displayName = o.str("display_name"), section = o.str("section"), license = o.str("license_id"),
                params = o.num("params")?.toLong(), totalSize = o.num("total_size")?.toLong(),
                formats = (o["formats"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content }.orEmpty(), note = o.str("note"),
            )
        })
    }

    /** `GET /api/models?sort=rank&search=`: the ranked search (Everything). Returns ids in order. */
    suspend fun search(query: String, limit: Int = 60): FwLoad<List<String>> {
        val q = if (query.isBlank()) "" else "&search=" + URLEncoder.encode(query.trim(), "UTF-8")
        return when (val r = get("$SITE/api/models?sort=rank&limit=$limit$q")) {
            is FwLoad.Down -> r
            is FwLoad.Live -> FwLoad.Live(r.value.listUnder("models", "items").mapNotNull { it.str("id") })
        }
    }
}

/** A pull on the user's computer (iOS ComputerModelStatus). */
data class PullStatus(val id: String, val status: String)

/** iOS ComputerModelPullClient: `local-engine/models/download` and `local-engine/models` on the API origin. */
object ComputerPullApi {
    private fun url(path: String): String = LibraryNet.config.apiBaseUrl.toString().trimEnd('/').removeSuffix("/v1").trimEnd('/') + "/local-engine/" + path

    suspend fun start(repoId: String): Pair<PullStatus?, String?> {
        val body = buildJsonObject { put("repo_id", repoId); put("revision", "main") }
        val req = ApiCall.request("POST", url("models/download").toHttpUrl(), body.toString())
        return when (val r = ApiCall.execute(LibraryNet.okHttp, ApiJson.default, req, JsonElement.serializer())) {
            is ApiResult.Ok -> {
                val o = r.value as? JsonObject
                (o.str("id") ?: o.str("model_id"))?.let { PullStatus(it, o.str("status") ?: "queued") to null } ?: (null to "Your computer didn't start the download.")
            }
            is ApiResult.HttpError -> null to (if (r.status == 428 || r.code == "compute_required") "Connect a computer first: the download goes to it." else r.friendly("send the download"))
            else -> null to r.friendly("send the download")
        }
    }

    suspend fun status(id: String): PullStatus? {
        val req = ApiCall.request("GET", url("models").toHttpUrl(), null)
        val r = ApiCall.execute(LibraryNet.okHttp, ApiJson.default, req, JsonElement.serializer())
        return (r as? ApiResult.Ok)?.value.listUnder("models").firstOrNull { it.str("id") == id }?.let { PullStatus(id, it.str("status").orEmpty()) }
    }

    fun label(status: String): String = when (status) {
        "ready", "completed", "done" -> "Ready on your computer"
        "failed", "error" -> "The download failed"
        "queued" -> "Waiting to start"
        else -> "Downloading"
    }
}

/** A model the runtime lists (`GET /api/v1/models`, the composer's source). */
data class RuntimeModelRow(val id: String, val name: String)

suspend fun runtimeModels(): List<RuntimeModelRow> =
    (LibraryNet.get("models") as? ApiResult.Ok)?.value.listUnder("models", "items").mapNotNull { o ->
        val id = o.str("id") ?: return@mapNotNull null
        RuntimeModelRow(id, o.str("name") ?: o.str("displayName") ?: id)
    }
