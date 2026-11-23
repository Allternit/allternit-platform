package com.allternit.core.api

import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import okhttp3.HttpUrl
import okhttp3.OkHttpClient

/**
 * `GET` and `PUT {cloud}/api/v1/me/flags`: the account flags that follow the user across
 * surfaces (intro seen times, for one). Same call shape as the read-only flag reads in
 * AciData and MeetingsData. The PUT body is `{key: iso}`; a null value clears the key.
 * Until platform #2835 merges the route answers 400 for the new keys, so callers keep a
 * local cache and treat a failed write as "try again later".
 */
class MeFlagsClient(private val okHttp: OkHttpClient, private val cloudBaseUrl: HttpUrl) {
    private fun url() = cloudBaseUrl.newBuilder().addPathSegments("api/v1/me/flags").build()

    /** String-valued flags by key (a null or non-string value is dropped); null when the read failed. */
    suspend fun read(): Map<String, String>? {
        val r = ApiCall.execute(okHttp, ApiJson.default, ApiCall.request("GET", url(), null), JsonElement.serializer())
        val obj = (r as? ApiResult.Ok)?.value as? JsonObject ?: return null
        return obj.mapNotNull { (k, v) -> (v as? JsonPrimitive)?.takeIf { v !is JsonNull }?.contentOrNull?.let { k to it } }.toMap()
    }

    /** Sets [key] to [iso] (or clears it when null). True when the server accepted it. */
    suspend fun write(key: String, iso: String?): Boolean {
        val body = JsonObject(mapOf(key to (if (iso == null) JsonNull else JsonPrimitive(iso)))).toString()
        return ApiCall.executeUnit(okHttp, ApiJson.default, ApiCall.request("PUT", url(), body)) is ApiResult.Ok
    }
}
