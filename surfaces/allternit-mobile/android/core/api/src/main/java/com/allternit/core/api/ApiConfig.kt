package com.allternit.core.api

import java.util.concurrent.TimeUnit
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.Interceptor
import okhttp3.OkHttpClient

/**
 * Where the native app talks to, mirroring iOS `Core/AppConfig.swift`.
 *
 * - [apiBaseUrl]: the `/api/v1` router on allternit-api.
 * - [aciBaseUrl] and [factoryBaseUrl]: mounted directly under `/api` on allternit-api
 *   (`.nest("/api", aci_router())`, `.nest("/api/factory", …)`), not under `/api/v1`,
 *   so they are derived by dropping a trailing `v1` segment, exactly like iOS.
 * - `/api/agent-chat` lives on the user's connected instance, not on the gateway, so
 *   [agentChatUrl] takes that instance's base URL.
 */
data class ApiConfig(
    val apiBaseUrl: HttpUrl = DEFAULT_API_BASE_URL.toHttpUrl(),
    /**
     * allternit-cloud-api's origin (iOS `AppConfig.cloudAPIBaseURL`): device tokens and other
     * cloud-only routes, which take absolute `/api/v1/...` paths. Never routed through a
     * user's computer, so the bearer stays with the cloud.
     */
    val cloudBaseUrl: HttpUrl = DEFAULT_CLOUD_BASE_URL.toHttpUrl(),
) {
    /** [apiBaseUrl] without a trailing `v1` (a trailing slash is ignored, like iOS `lastPathComponent`). */
    private val apiRoot: HttpUrl
        get() {
            val segments = apiBaseUrl.pathSegments.filter { it.isNotEmpty() }
            val root = if (segments.lastOrNull() == "v1") segments.dropLast(1) else segments
            return apiBaseUrl.newBuilder()
                .encodedPath("/")
                .apply { root.forEach { addPathSegment(it) } }
                .build()
        }

    /** `…/api/aci` — `POST run`, `GET stream/:id`, `POST stop/:id`, `POST approve/:id`. */
    val aciBaseUrl: HttpUrl get() = apiRoot.newBuilder().addPathSegment("aci").build()

    /** `…/api/factory` — mail threads, send/decide/share, ledger tail, agent streams. */
    val factoryBaseUrl: HttpUrl get() = apiRoot.newBuilder().addPathSegment("factory").build()

    /** `<instance>/api/agent-chat` (SSE). [instanceBaseUrl] is the connected instance's origin. */
    fun agentChatUrl(instanceBaseUrl: HttpUrl): HttpUrl =
        instanceBaseUrl.newBuilder().addPathSegments("api/agent-chat").build()

    companion object {
        const val DEFAULT_API_BASE_URL = "https://api.allternit.com/api/v1"
        const val DEFAULT_CLOUD_BASE_URL = "https://api.allternit.com"
    }
}

/**
 * Supplies the bearer token for each request. Called on OkHttp's thread, so it must
 * not block on the network; return null when signed out (no header is sent).
 */
fun interface BearerProvider {
    fun bearerToken(): String?
}

/** Builds the OkHttp clients the native app shares (one per process is the norm). */
object ApiClients {
    fun okHttp(
        bearer: BearerProvider,
        builder: OkHttpClient.Builder = OkHttpClient.Builder(),
    ): OkHttpClient = builder
        .connectTimeout(15, TimeUnit.SECONDS)
        // SSE and long polls stay open; the stream layer owns its own idle timeout.
        .readTimeout(0, TimeUnit.SECONDS)
        .addInterceptor(bearerInterceptor(bearer))
        .build()

    /** Adds `Authorization: Bearer <token>` unless the request already carries one. */
    fun bearerInterceptor(bearer: BearerProvider): Interceptor = Interceptor { chain ->
        val request = chain.request()
        val token = bearer.bearerToken()
        if (token.isNullOrEmpty() || request.header("Authorization") != null) {
            chain.proceed(request)
        } else {
            chain.proceed(request.newBuilder().header("Authorization", "Bearer $token").build())
        }
    }
}
