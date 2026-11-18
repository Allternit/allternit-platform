package com.allternit.core.api

import java.io.IOException
import kotlin.coroutines.resume
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.serialization.DeserializationStrategy
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import okhttp3.Call
import okhttp3.Callback
import okhttp3.HttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response

/**
 * What every generated call returns. Screens switch on it; no exception reaches the UI
 * (coroutine cancellation still propagates, as it must).
 */
sealed interface ApiResult<out T> {
    data class Ok<T>(val value: T) : ApiResult<T>

    /**
     * The server answered non-2xx. [code] is the wire error code (`{"error":"no_brain"}`,
     * or `error.code` / `error.type` when `error` is an object); [message] the human text.
     */
    data class HttpError(val status: Int, val code: String?, val message: String?) : ApiResult<Nothing>

    /** No answer: offline, DNS, TLS, timeout, connection reset. */
    data class Network(val cause: IOException) : ApiResult<Nothing>

    /** A 2xx whose body did not match the contract (a server/app version skew). */
    data class Decode(val status: Int, val cause: SerializationException) : ApiResult<Nothing>
}

/** The JSON settings the generated models expect. */
object ApiJson {
    val default: Json = Json {
        // New server fields must not break older apps.
        ignoreUnknownKeys = true
        // Optional request fields are left out when null, like the iOS encoders.
        explicitNulls = false
        // An unknown enum value or a null in a defaulted field falls back to the default.
        coerceInputValues = true
        isLenient = false
    }
}

/** Plumbing the generated [com.allternit.core.api.gen.AllternitApi] uses. */
object ApiCall {
    private val JSON = "application/json; charset=utf-8".toMediaType()

    fun request(method: String, url: HttpUrl, jsonBody: String?, accept: String = "application/json"): Request =
        Request.Builder()
            .url(url)
            .header("Accept", accept)
            .method(method, jsonBody?.toRequestBody(JSON))
            .build()

    suspend fun <T> execute(
        client: OkHttpClient,
        json: Json,
        request: Request,
        deserializer: DeserializationStrategy<T>,
    ): ApiResult<T> = run(client, request) { status, body ->
        try {
            ApiResult.Ok(json.decodeFromString(deserializer, body))
        } catch (e: SerializationException) {
            ApiResult.Decode(status, e)
        } catch (e: IllegalArgumentException) {
            ApiResult.Decode(status, SerializationException(e.message, e))
        }
    }

    suspend fun executeUnit(client: OkHttpClient, json: Json, request: Request): ApiResult<Unit> =
        run(client, request) { _, _ -> ApiResult.Ok(Unit) }

    /** Parses an error body: `{"error":"code","message"?}` or `{"error":{"code"|"type","message"}}`. */
    fun httpError(status: Int, body: String, json: Json = ApiJson.default): ApiResult.HttpError {
        val obj = runCatching { json.parseToJsonElement(body) as? JsonObject }.getOrNull()
        val error = obj?.get("error")
        val code: String?
        var message = (obj?.get("message") as? JsonPrimitive)?.contentOrNull
        when (error) {
            is JsonPrimitive -> code = error.contentOrNull
            is JsonObject -> {
                code = (error["code"] as? JsonPrimitive)?.contentOrNull
                    ?: (error["type"] as? JsonPrimitive)?.contentOrNull
                message = message ?: (error["message"] as? JsonPrimitive)?.contentOrNull
            }
            else -> code = null
        }
        return ApiResult.HttpError(status, code, message)
    }

    private suspend fun <T> run(
        client: OkHttpClient,
        request: Request,
        onSuccess: (status: Int, body: String) -> ApiResult<T>,
    ): ApiResult<T> = suspendCancellableCoroutine { cont ->
        val call = client.newCall(request)
        cont.invokeOnCancellation { call.cancel() }
        call.enqueue(object : Callback {
            override fun onFailure(call: Call, e: IOException) {
                if (cont.isActive) cont.resume(ApiResult.Network(e))
            }

            override fun onResponse(call: Call, response: Response) {
                val result = try {
                    response.use {
                        val body = it.body?.string().orEmpty()
                        if (it.isSuccessful) onSuccess(it.code, body) else httpError(it.code, body)
                    }
                } catch (e: IOException) {
                    ApiResult.Network(e)
                }
                if (cont.isActive) cont.resume(result)
            }
        })
    }
}
