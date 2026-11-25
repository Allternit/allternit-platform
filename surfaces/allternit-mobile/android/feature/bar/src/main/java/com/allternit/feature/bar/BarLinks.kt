package com.allternit.feature.bar

import android.content.Context
import android.content.Intent
import android.net.Uri
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * What `allternit://bar` asks the bar to show (iOS BarLinkRequest). Every field may be empty: a bare link opens the bar.
 * `allternit://bar?app=<kind>:<ref>&tool=<name>&bot=<id>&v.<key>=<value>`.
 */
data class BarLinkRequest(
    val app: String? = null,
    val tool: String? = null,
    val bot: String? = null,
    val values: Map<String, String> = emptyMap(),
)

/** Builds and reads `allternit://bar` links. Lane 37 (BarWidget, LiveRunNotification) uses [intent] / [uri]. */
object BarDeepLink {
    const val SCHEME = "allternit"
    const val HOST = "bar"

    fun uri(request: BarLinkRequest = BarLinkRequest()): Uri = Uri.Builder().scheme(SCHEME).authority(HOST).apply {
        request.app?.takeIf { it.isNotEmpty() }?.let { appendQueryParameter("app", it) }
        request.tool?.takeIf { it.isNotEmpty() }?.let { appendQueryParameter("tool", it) }
        request.bot?.takeIf { it.isNotEmpty() }?.let { appendQueryParameter("bot", it) }
        request.values.keys.sorted().forEach { k -> appendQueryParameter("v.$k", request.values.getValue(k)) }
    }.build()

    fun isBar(uri: Uri?): Boolean = uri?.scheme == SCHEME && uri.host == HOST

    fun parse(uri: Uri?): BarLinkRequest? {
        if (!isBar(uri)) return null
        val u = uri!!
        val values = u.queryParameterNames.filter { it.startsWith("v.") && it.length > 2 }
            .associate { it.removePrefix("v.") to u.getQueryParameter(it).orEmpty() }
        return BarLinkRequest(u.getQueryParameter("app"), u.getQueryParameter("tool"), u.getQueryParameter("bot"), values)
    }

    /** An explicit intent to [BarActivity] (works from a widget, a notification or the app; no chooser). */
    fun intent(context: Context, request: BarLinkRequest = BarLinkRequest()): Intent =
        Intent(context, BarActivity::class.java)
            .setAction(Intent.ACTION_VIEW)
            .setData(uri(request))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

    fun open(context: Context, request: BarLinkRequest = BarLinkRequest()) = context.startActivity(intent(context, request))
}

/** The bar job a notification or widget follows (lane 37 reads [BarJobs.current]). */
data class BarJobState(
    val jobId: String,
    val state: String,
    val title: String,
    val appName: String?,
    val botName: String?,
    val approverName: String?,
    val progressLine: String?,
    val receipt: String?,
    val error: String?,
) {
    val isTerminal: Boolean get() = state in TERMINAL
    /** "Waiting on Noah", "Working", "Refunded €320" (same line as the bar). */
    val stateLine: String get() = BarWording.stateLine(state, approverName, receipt, error)
    /** 1-4: Asked, Approval, Running, Done. */
    val stepsReached: Int get() = BarWording.stepsReached(state)

    companion object {
        val TERMINAL = setOf("completed", "failed", "cancelled")
    }
}

/** Process-wide: the bar job being followed now, null when none. The bar writes it; lane 37 collects it. */
object BarJobs {
    private val _current = MutableStateFlow<BarJobState?>(null)
    val current: StateFlow<BarJobState?> = _current.asStateFlow()

    internal fun publish(state: BarJobState?) {
        _current.value = state
    }
}
