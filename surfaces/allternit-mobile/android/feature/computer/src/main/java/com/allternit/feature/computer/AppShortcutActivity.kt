package com.allternit.feature.computer

import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.Typeface
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import com.allternit.core.design.AllternitTheme
import androidx.core.content.pm.ShortcutInfoCompat
import androidx.core.content.pm.ShortcutManagerCompat
import androidx.core.graphics.drawable.IconCompat

/**
 * An installed Allternit app opened from its home-screen shortcut: the app's page in a web view
 * screen of its own (AppPage: Try again on a failed load). On open it mints the one-time sign-in handoff and loads `open_url`; if the
 * mint fails (or the app is a vendor app) it loads the plain link. Only https pages on our own
 * domains load (AppsShelfLinks); anything else opens in the browser.
 */
class AppShortcutActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val link = intent.getStringExtra(EXTRA_LINK)
        val slug = intent.getStringExtra(EXTRA_SLUG).orEmpty()
        val vendor = intent.getBooleanExtra(EXTRA_VENDOR, false)
        val name = intent.getStringExtra(EXTRA_NAME) ?: "Allternit app"
        title = name
        setContent {
            AllternitTheme {
                var target by remember { mutableStateOf<String?>(null) }
                LaunchedEffect(Unit) {
                    target = AppsShelfModel.openLink(slug, link, vendor)
                    if (target == null) finish()
                }
                // The same page, and the same "Couldn’t load" + Try again state, as the shelf's opened app.
                target?.let { AppPage(it, name, Modifier.fillMaxSize()) }
            }
        }
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
