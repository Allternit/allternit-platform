package com.allternit.feature.computer

import android.annotation.SuppressLint
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.Typeface
import android.os.Bundle
import android.view.ViewGroup
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.FrameLayout
import androidx.core.content.pm.ShortcutInfoCompat
import androidx.core.content.pm.ShortcutManagerCompat
import androidx.core.graphics.drawable.IconCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

/**
 * An installed Allternit app opened from its home-screen shortcut: the app's page in a web view
 * screen of its own. On open it mints the one-time sign-in handoff and loads `open_url`; if the
 * mint fails (or the app is a vendor app) it loads the plain link. Only https pages on our own
 * domains load (AppsShelfLinks); anything else opens in the browser.
 */
class AppShortcutActivity : Activity() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var web: WebView? = null

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val link = intent.getStringExtra(EXTRA_LINK)
        val slug = intent.getStringExtra(EXTRA_SLUG).orEmpty()
        val vendor = intent.getBooleanExtra(EXTRA_VENDOR, false)
        title = intent.getStringExtra(EXTRA_NAME) ?: "Allternit app"
        val view = WebView(this).apply {
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            webViewClient = object : WebViewClient() {
                override fun shouldOverrideUrlLoading(v: WebView, request: WebResourceRequest): Boolean {
                    if (AppsShelfLinks.openUrl(request.url.toString()) != null) return false
                    openExternal(this@AppShortcutActivity, request.url.toString())
                    return true
                }
            }
        }
        web = view
        setContentView(FrameLayout(this).apply {
            setBackgroundColor(Color.WHITE)
            addView(view, ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        })
        scope.launch {
            val target = AppsShelfModel.openLink(slug, link, vendor)
            if (target != null) view.loadUrl(target) else finish()
        }
    }

    @Deprecated("Back goes through the page history first")
    override fun onBackPressed() {
        val w = web
        if (w != null && w.canGoBack()) w.goBack() else super.onBackPressed()
    }

    override fun onDestroy() {
        scope.cancel()
        web?.destroy()
        super.onDestroy()
    }

    companion object {
        const val EXTRA_LINK = "allternit.app.link"
        const val EXTRA_SLUG = "allternit.app.slug"
        const val EXTRA_NAME = "allternit.app.name"
        const val EXTRA_VENDOR = "allternit.app.vendor"
    }
}

/** "Add to home screen" for an installed app (ShortcutManagerCompat.requestPinShortcut). */
internal object AppShortcuts {
    /** True when the launcher accepted the request (it then asks the person to confirm). */
    fun pin(context: Context, row: ShelfRow): Boolean {
        if (!ShortcutManagerCompat.isRequestPinShortcutSupported(context)) return false
        val intent = Intent(context, AppShortcutActivity::class.java)
            .setAction(Intent.ACTION_VIEW)
            .putExtra(AppShortcutActivity.EXTRA_LINK, row.link)
            .putExtra(AppShortcutActivity.EXTRA_SLUG, row.slug)
            .putExtra(AppShortcutActivity.EXTRA_NAME, row.name)
            .putExtra(AppShortcutActivity.EXTRA_VENDOR, row.isVendor)
        val info = ShortcutInfoCompat.Builder(context, "app-${row.kind}-${row.slug}")
            .setShortLabel(row.name.take(24))
            .setLongLabel(row.name)
            .setIcon(IconCompat.createWithBitmap(iconBitmap(row)))
            .setIntent(intent)
            .build()
        return ShortcutManagerCompat.requestPinShortcut(context, info, null)
    }

    /** The app record's icon (an emoji or letter) on a light tile; the first letter of the name when it has none. */
    private fun iconBitmap(row: ShelfRow): Bitmap {
        val size = 192
        val bmp = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bmp)
        canvas.drawRoundRect(RectF(0f, 0f, size.toFloat(), size.toFloat()), 44f, 44f, Paint(Paint.ANTI_ALIAS_FLAG).apply { color = Color.rgb(240, 240, 240) })
        val glyph = row.glyph
        val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = Color.rgb(20, 20, 20)
            textSize = 96f
            textAlign = Paint.Align.CENTER
            typeface = Typeface.DEFAULT_BOLD
        }
        canvas.drawText(glyph, size / 2f, size / 2f - (paint.ascent() + paint.descent()) / 2f, paint)
        return bmp
    }
}
