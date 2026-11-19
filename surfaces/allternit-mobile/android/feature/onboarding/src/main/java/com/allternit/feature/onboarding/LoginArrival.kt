package com.allternit.feature.onboarding

import android.annotation.SuppressLint
import android.content.Context
import android.provider.Settings
import android.view.View
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner

private const val ARRIVAL_ASSET = "login-arrival.html"
private val ArrivalBackground = Color(0xFF141413)

/**
 * The approved offline wordmark/Gizzi/cast loop above the sign-in buttons, the twin of iOS
 * `LoginArrivalView` (ios/Features/Onboarding/SignInStage/LoginArrivalView.swift). It plays the
 * same bundled page (shared/login-arrival/login-arrival.html, copied into assets by
 * :feature:onboarding's build): inline data images and a hash-locked inline script whose CSP
 * forbids every network request. Network loads are also blocked here, every navigation is
 * refused, and the view takes no touches. No credentials ever enter it.
 *
 * Until the page has painted (or if the WebView is missing), the A://TERNIT wordmark and Gizzi
 * show instead, so sign-in is usable at once. [paused] stops the loop while the user types.
 */
@Composable
fun LoginArrival(paused: Boolean, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    var loaded by remember { mutableStateOf(false) }
    var resumed by remember { mutableStateOf(lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)) }
    val html = remember { readArrivalHtml(context) }
    val reducedMotion = remember { animationsOff(context) }
    val state by rememberUpdatedState(paused || !resumed)

    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_RESUME -> resumed = true
                Lifecycle.Event.ON_PAUSE -> resumed = false
                else -> Unit
            }
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }

    Box(
        modifier = modifier
            .fillMaxWidth()
            .aspectRatio(720f / 500f)
            .background(ArrivalBackground)
            .semantics {
                contentDescription = "Allternit welcomes you. Gizzi and the bot team assemble, then the scene repeats."
            },
        contentAlignment = Alignment.Center,
    ) {
        if (html != null) {
            AndroidView(
                factory = { ctx -> arrivalWebView(ctx, html) { loaded = true } },
                update = { view ->
                    if (state) view.onPause() else view.onResume()
                    // Reading `loaded` reruns this once the page finishes, which sends the first state.
                    if (loaded) view.sendState(paused = state, reducedMotion = reducedMotion)
                },
                onRelease = { view ->
                    view.sendState(paused = true, reducedMotion = true)
                    view.stopLoading()
                    view.destroy()
                },
                modifier = Modifier.fillMaxSize().alpha(if (loaded) 1f else 0f),
            )
        }
        if (!loaded) {
            Column(
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(24.dp),
            ) {
                Image(
                    painter = painterResource(R.drawable.allternit_wordmark),
                    contentDescription = null,
                    modifier = Modifier.height(18.dp),
                )
                Image(
                    painter = painterResource(R.drawable.gizzi_mascot),
                    contentDescription = null,
                    modifier = Modifier.size(120.dp),
                )
            }
        }
    }
}

private fun readArrivalHtml(context: Context): String? =
    runCatching { context.assets.open(ARRIVAL_ASSET).bufferedReader().use { it.readText() } }.getOrNull()

/** Android's "Remove animations" sets the animator scale to 0; the page then shows one settled frame. */
private fun animationsOff(context: Context): Boolean =
    Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f

private fun WebView.sendState(paused: Boolean, reducedMotion: Boolean) {
    if (tag != true) return // page not finished yet; onPageFinished sends the first state
    evaluateJavascript("window.allternitArrivalState&&window.allternitArrivalState({paused:$paused,reducedMotion:$reducedMotion})", null)
}

@SuppressLint("SetJavaScriptEnabled", "ClickableViewAccessibility")
private fun arrivalWebView(context: Context, html: String, onLoaded: () -> Unit): WebView =
    WebView(context).apply {
        setBackgroundColor(ArrivalBackground.toArgbInt())
        isVerticalScrollBarEnabled = false
        isHorizontalScrollBarEnabled = false
        overScrollMode = View.OVER_SCROLL_NEVER
        isFocusable = false
        isClickable = false
        setOnTouchListener { _, _ -> true }
        importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS
        settings.apply {
            javaScriptEnabled = true // the page's one hash-locked inline script
            blockNetworkLoads = true
            allowFileAccess = false
            allowContentAccess = false
            domStorageEnabled = false
            setSupportZoom(false)
            builtInZoomControls = false
        }
        webViewClient = object : WebViewClient() {
            override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean = true

            override fun onPageFinished(view: WebView, url: String?) {
                view.tag = true
                onLoaded()
            }
        }
        loadDataWithBaseURL(null, html, "text/html", "utf-8", null)
    }

private fun Color.toArgbInt(): Int =
    android.graphics.Color.argb((alpha * 255).toInt(), (red * 255).toInt(), (green * 255).toInt(), (blue * 255).toInt())
