package com.allternit.core.design

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.util.LruCache
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * iOS AppLogoCatalog + AppLogoView: an app's bundled logo (`assets/logos/<key>.webp`, made by
 * `scripts/convert-logos.mjs`), found by the app's id, then its name, then its site; else the site's favicon;
 * else the app's initials. The brand mark sits on a white tile with 24 % corners in both themes.
 */
object AppLogoCatalog {
    private val fold = Regex("[^a-z0-9]+")
    private val platformLabels = setOf("googleapis", "githubcopilot", "amazonaws", "azure", "cloudfront")

    fun key(value: String): String = value.lowercase().replace(fold, "_").trim('_')

    /** Model-provider ids whose logo file is named differently. */
    private val providerAlias = mapOf(
        "google" to "google", "gemini" to "googlegemini", "moonshot" to "moonshot", "kimi" to "kimi",
        "local-brain" to "ollama", "allternit-sidecar" to "allternit", "omlx" to "ollama",
        "copilot" to "githubcopilot", "z-ai" to "zai", "z.ai" to "zai",
        // Runtime and brain ids (gizzi cli-catalog, subscription and plan ids).
        "cursor-agent" to "cursoragent", "kimi-cli" to "kimi", "plan-kimi" to "kimi", "qwen-cli" to "qwencli",
        "claude-code" to "claude", "claude-cli" to "claude", "claude-org" to "claude", "plan-claude" to "claude",
        "codex-cli" to "codex", "plan-chatgpt" to "codex", "chatgpt-phone" to "codex", "plan-grok" to "grok",
        "gemini-cli" to "googlegemini", "kiro-cli" to "kirocli",
    )

    /** The logo key for a provider or brain id (anthropic, openai, deepseek, zai, copilot and so on). */
    fun providerKey(pid: String): String = providerAlias[pid.lowercase()] ?: key(pid)

    /** `mcp.airtable.com` becomes `airtable`. */
    fun siteHost(url: String?): String? {
        val host = url?.let { runCatching { java.net.URI(it).host }.getOrNull() }?.lowercase() ?: return null
        val parts = host.split('.')
        if (parts.size < 2) return null
        val second = parts[parts.size - 2]
        return if (second in listOf("co", "com", "net", "org") && parts.size >= 3) parts[parts.size - 3] else second
    }

    fun siteLabel(url: String?): String? = siteHost(url)?.takeIf { it !in platformLabels }

    fun candidates(id: String?, name: String, url: String?): List<String> {
        val out = mutableListOf<String>()
        fun push(raw: String?) { if (raw == null) return; val k = key(raw); if (k.isNotEmpty() && k !in out) out.add(k) }
        push(id?.replace(Regex("^(connector:|mcp:|url:)"), ""))
        push(name)
        push(siteLabel(url))
        return out
    }

    fun initials(name: String): String =
        name.split(Regex("[\\s._-]+")).filter { it.isNotEmpty() }.take(2).joinToString("") { it.first().uppercase() }.ifEmpty { "?" }

    private val cache = object : LruCache<String, Bitmap>(96) {}

    fun bitmap(context: android.content.Context, id: String?, name: String, url: String?): Bitmap? {
        for (k in candidates(id, name, url)) {
            cache.get(k)?.let { return it }
            val bmp = runCatching { context.assets.open("logos/$k.webp").use { BitmapFactory.decodeStream(it) } }.getOrNull()
            if (bmp != null) { cache.put(k, bmp); return bmp }
        }
        return null
    }

    fun faviconUrl(url: String?, size: Int): String? {
        val host = url?.let { runCatching { java.net.URI(it).host }.getOrNull() } ?: return null
        return "https://www.google.com/s2/favicons?domain=${host.removePrefix("www.")}&sz=${maxOf(64, size * 2)}"
    }
}

@Composable
fun AppLogo(name: String, modifier: Modifier = Modifier, id: String? = null, url: String? = null, size: Dp = 40.dp) {
    val context = LocalContext.current
    val c = AllternitTheme.colors
    val bundled = remember(id, name, url) { AppLogoCatalog.bitmap(context, id, name, url) }
    var favicon by remember(url) { mutableStateOf<Bitmap?>(null) }
    LaunchedEffect(url, bundled) {
        if (bundled != null || favicon != null) return@LaunchedEffect
        val source = AppLogoCatalog.faviconUrl(url, size.value.toInt()) ?: return@LaunchedEffect
        val bmp = withContext(Dispatchers.IO) { runCatching { java.net.URL(source).openStream().use { BitmapFactory.decodeStream(it) } }.getOrNull() }
        // The favicon service answers unknown sites with a 16 px globe.
        if (bmp != null && bmp.width > 16) favicon = bmp
    }
    val shape = RoundedCornerShape((size.value * 0.24f).dp)
    Box(modifier.size(size).clip(shape).background(Color.White).border(1.dp, c.borderSubtle, shape), contentAlignment = Alignment.Center) {
        val image = bundled ?: favicon
        if (image != null) Image(image.asImageBitmap(), null, Modifier.size((size.value * 0.58f).dp), contentScale = ContentScale.Fit)
        else Text(AppLogoCatalog.initials(name), color = Color(0xFF6B6B6B), fontSize = (size.value * 0.34f).sp, fontWeight = FontWeight.SemiBold)
    }
}

/**
 * A runtime or brain mark by id (cursor-agent, kimi-cli, plan-claude, codebuddy ...): the bundled brand logo on the white tile,
 * else a neutral terminal glyph on the neutral tile. Never a text initial.
 */
@Composable
fun ProviderLogoMark(key: String, size: Dp) {
    val context = LocalContext.current
    val c = AllternitTheme.colors
    val bitmap = remember(key) {
        val id = key.lowercase().removePrefix("subs-")
        listOf(id, id.removeSuffix("-cli")).distinct().firstNotNullOfOrNull { AppLogoCatalog.bitmap(context, AppLogoCatalog.providerKey(it), it, null) }
    }
    val shape = RoundedCornerShape((size.value * 0.24f).dp)
    if (bitmap != null) {
        Box(Modifier.size(size).clip(shape).background(Color.White).border(1.dp, c.borderSubtle, shape), contentAlignment = Alignment.Center) {
            Image(bitmap.asImageBitmap(), null, Modifier.size((size.value * 0.58f).dp), contentScale = ContentScale.Fit)
        }
    } else {
        Box(Modifier.size(size).clip(shape).background(c.fillNeutral).border(1.dp, c.borderSubtle, shape), contentAlignment = Alignment.Center) {
            Icon(painterResource(R.drawable.ph_terminal_window), null, tint = c.textSecondary, modifier = Modifier.size((size.value * 0.5f).dp))
        }
    }
}
