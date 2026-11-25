package com.allternit.feature.artifacts

import android.graphics.BitmapFactory
import android.provider.Settings
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.DotField
import com.allternit.core.design.DotFieldMode
import com.allternit.core.design.DotPattern
import kotlin.math.exp
import kotlin.math.max
import kotlin.math.min
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * Real byte progress for a download (stored clips and pictures, saving to a file). Nothing here guesses:
 * [totalBytes] is the response's content length and is null when there is none (or the body is content-encoded,
 * because then the length counts compressed bytes while we count decoded ones); [etaSeconds] comes from a smoothed
 * rate and is null until it means something. Mirrors iOS `ByteProgress` and the web `byte-progress.ts`.
 */
data class ByteProgress(val bytes: Long, val totalBytes: Long? = null, val etaSeconds: Int? = null) {
    /** 0..1 when the total is known; null otherwise. */
    val fraction: Double? get() = totalBytes?.takeIf { it > 0 }?.let { min(1.0, bytes.toDouble() / it) }
}

/** The size and ETA rules, with time passed in (seconds, any monotonic clock) so they can be tested. */
class ByteProgressTracker(val total: Long?, private val startedAt: Double) {
    private var lastAt = startedAt
    private var bytes = 0L
    private var pending = 0L
    private var rate: Double? = null // bytes per second

    /** Records a chunk and returns the progress so far. */
    fun add(count: Int, now: Double): ByteProgress {
        bytes += count
        pending += count
        val dt = now - lastAt
        if (dt > 0) {
            val instant = pending / dt
            val alpha = 1 - exp(-dt / RATE_WINDOW)
            rate = rate?.let { it + alpha * (instant - it) } ?: instant
            pending = 0
            lastAt = now
        }
        val t = total ?: return ByteProgress(bytes)
        val r = rate
        val eta = if (r != null && r > 0 && now - startedAt >= ETA_MIN_ELAPSED && bytes < t) Math.ceil((t - bytes) / r).toInt() else null
        return ByteProgress(bytes, max(t, bytes), eta)
    }

    /** The last report, once the body is complete. */
    fun finished() = ByteProgress(bytes, total?.let { max(it, bytes) } ?: bytes)

    companion object {
        /** Rate smoothing window: an exponentially weighted average over about this long. */
        const val RATE_WINDOW = 2.0
        /** No ETA until the download has run this long. */
        const val ETA_MIN_ELAPSED = 1.0

        /** The whole size from a response: null for an unknown length (-1 or 0) and for a content-encoded body. */
        fun totalBytes(contentLength: Long, contentEncoding: String?): Long? {
            val enc = contentEncoding?.trim()?.lowercase()
            if (!enc.isNullOrEmpty() && enc != "identity") return null
            return if (contentLength > 0) contentLength else null
        }
    }
}

object ByteProgressText {
    /** "3.0 MB", "412 KB". */
    fun formatBytes(n: Long): String = when {
        n >= 1_048_576 -> String.format(java.util.Locale.US, "%.1f MB", n / 1_048_576.0)
        n >= 1024 -> "${Math.round(n / 1024.0)} KB"
        else -> "$n B"
    }

    /** "Loading 41% · 3.0 of 7.4 MB · ETA 3 s", or with no size "Loading · 3.0 MB · — ETA". */
    fun status(verb: String, p: ByteProgress): String {
        val eta = p.etaSeconds?.let { "ETA $it s" } ?: "— ETA"
        val total = p.totalBytes ?: return "$verb · ${formatBytes(p.bytes)} · $eta"
        val pct = min(100, Math.floor(p.bytes.toDouble() / total * 100).toInt())
        val sameUnit = (total >= 1_048_576) == (p.bytes >= 1_048_576)
        var done = formatBytes(p.bytes)
        if (sameUnit && done.contains(' ')) done = done.substringBeforeLast(' ')
        return "$verb $pct% · $done of ${formatBytes(total)} · $eta"
    }

    /** Poster blur in dp: 28 at 0 bytes down to 0 at the end; held at 16 when the size is unknown. */
    fun blur(p: ByteProgress?): Int {
        if (p == null) return 28
        val f = p.fraction ?: return 16
        return Math.round(28 * (1 - f)).toInt()
    }
}

/** The poster a finished `media_generate` carried: `meta.media.poster`, only as an inline image or an https link (same rule as the web card). */
object ByteLoading {
    fun poster(meta: JsonObject?): String? {
        val poster = ((meta?.get("media") as? JsonObject)?.get("poster") as? JsonPrimitive)?.takeIf { it.isString }?.content ?: return null
        val lowered = poster.lowercase()
        return poster.takeIf { lowered.startsWith("data:image/") || lowered.startsWith("https:") }
    }

    /** Passes byte reports on about 8 a second; a complete one always goes through. */
    fun shouldSend(now: Double, last: Double, minGap: Double, complete: Boolean) = complete || now - last >= minGap
}

/**
 * Reads a file body (https with no bearer, `/api/...` paths with the bearer, or a data: URL) and reports real bytes as
 * chunks arrive, about 8 times a second; the last report always goes out. Null when it could not be read or is over
 * 512 MB. Call off the main thread.
 */
internal fun fetchBytesWithProgress(ref: String, onProgress: ((ByteProgress) -> Unit)?): ByteArray? = runCatching {
    when {
        ref.startsWith("data:") -> android.util.Base64.decode(ref.substringAfter(','), android.util.Base64.DEFAULT).also { onProgress?.invoke(ByteProgress(it.size.toLong(), it.size.toLong())) }
        ref.startsWith("https://") -> download(okhttp3.OkHttpClient(), ref, onProgress)
        ref.startsWith("/api/") -> download(LibraryNet.okHttp, LibraryNet.absolute(ref), onProgress)
        else -> null
    }
}.getOrNull()?.takeIf { it.isNotEmpty() && it.size <= 512 * 1024 * 1024 }

private fun download(client: okhttp3.OkHttpClient, url: String, onProgress: ((ByteProgress) -> Unit)?): ByteArray? =
    client.newCall(okhttp3.Request.Builder().url(url).build()).execute().use { r ->
        if (!r.isSuccessful) return null
        val body = r.body ?: return null
        val clock = { System.nanoTime() / 1e9 }
        val tracker = ByteProgressTracker(ByteProgressTracker.totalBytes(body.contentLength(), r.header("Content-Encoding")), clock())
        onProgress?.invoke(ByteProgress(0, tracker.total))
        val out = java.io.ByteArrayOutputStream()
        val buf = ByteArray(16 * 1024)
        var last = Double.NEGATIVE_INFINITY
        body.byteStream().use { input ->
            while (true) {
                val n = input.read(buf)
                if (n < 0) break
                out.write(buf, 0, n)
                val now = clock()
                val p = tracker.add(n, now)
                val complete = p.totalBytes?.let { p.bytes >= it } ?: false
                if (onProgress != null && ByteLoading.shouldSend(now, last, 0.12, complete)) { last = now; onProgress(p) }
            }
        }
        onProgress?.invoke(tracker.finished())
        out.toByteArray()
    }

/**
 * Download "sharpens as it loads": the poster is a dot halftone that gets finer by the real fraction of bytes in;
 * an unknown size holds the coarse pitch and shows a small spinner. The status line gives bytes, percent and ETA ("— ETA"
 * when there is none). Reduced motion: the halftone is a fixed medium pitch and nothing animates or spins.
 */
@Composable
internal fun ByteLoadingView(posterUrl: String?, progress: ByteProgress?, modifier: Modifier = Modifier, verb: String = "Loading", onPoster: (android.graphics.Bitmap?) -> Unit = {}) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val reduce = remember(context) { runCatching { Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }.getOrDefault(false) }
    val line = ByteProgressText.status(verb, progress ?: ByteProgress(0))
    val poster by produceState<android.graphics.Bitmap?>(null, posterUrl) {
        value = posterUrl?.let { u -> withContext(Dispatchers.IO) { fetchBytesWithProgress(u, null)?.let { b -> BitmapFactory.decodeByteArray(b, 0, b.size) } } }
    }
    androidx.compose.runtime.LaunchedEffect(poster) { onPoster(poster) }
    Box(modifier.fillMaxSize().background(c.fillNeutral).semantics { contentDescription = line }, contentAlignment = Alignment.BottomCenter) {
        // Dots, not blur: the poster as halftone, finer with the real byte fraction (unknown total holds coarse); no poster yet = the band field.
        val shown = poster
        DotField(
            if (shown != null) DotFieldMode.Halftone(shown, progress?.fraction?.toFloat()?.coerceIn(0f, 1f)) else DotFieldMode.Field(DotPattern.Band),
            Modifier.fillMaxSize(),
        )
        Row(
            Modifier.padding(bottom = 14.dp).background(c.bgPanel, CircleShape).border(1.dp, c.borderSubtle, CircleShape).padding(horizontal = 12.dp, vertical = 7.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically,
        ) {
            if (progress?.fraction == null && !reduce) CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 1.5.dp, color = c.textSecondary)
            Text(line, color = c.textPrimary, fontSize = 12.5.sp, fontWeight = FontWeight.Medium, maxLines = 1, style = TextStyle(fontFeatureSettings = "tnum"))
        }
    }
}
