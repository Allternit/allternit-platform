package com.allternit.core.api

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import okhttp3.HttpUrl
import okhttp3.OkHttpClient

/** The owner's presence. Same four rows, order and words as iOS `UserStatus`. */
enum class UserStatus(val wire: String, val title: String, val subline: String) {
    Online("online", "Online", ""),
    Autopilot("autopilot", "Autopilot", "Non-urgent asks wait. Gizzi keeps working."),
    Dnd("dnd", "Do not disturb", "Non-urgent asks wait"),
    Away("away", "Away", "A summary waits for you");

    companion object {
        fun fromWire(s: String?): UserStatus = entries.firstOrNull { it.wire == s } ?: Online
    }
}

/** `GET` and `PUT {cloud}/api/v1/me/status`. Nothing saved reads as online. */
class UserStatusClient(private val okHttp: OkHttpClient, private val cloudBaseUrl: HttpUrl) {
    private fun url() = cloudBaseUrl.newBuilder().addPathSegments("api/v1/me/status").build()

    /** The saved status; null when the read failed. */
    suspend fun get(): UserStatus? {
        val r = ApiCall.execute(okHttp, ApiJson.default, ApiCall.request("GET", url(), null), JsonObject.serializer())
        val obj = (r as? ApiResult.Ok)?.value ?: return null
        return UserStatus.fromWire((obj["status"] as? JsonPrimitive)?.contentOrNull)
    }

    /** True when the server accepted it. */
    suspend fun set(status: UserStatus): Boolean {
        val body = JsonObject(mapOf("status" to JsonPrimitive(status.wire))).toString()
        return ApiCall.executeUnit(okHttp, ApiJson.default, ApiCall.request("PUT", url(), body)) is ApiResult.Ok
    }
}
