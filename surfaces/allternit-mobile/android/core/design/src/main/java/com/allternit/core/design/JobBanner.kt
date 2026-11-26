package com.allternit.core.design

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay

/** What a banner is about; picks the icon. */
enum class JobKind { Job, Compaction }

data class JobBannerItem(val id: String, val kind: JobKind, val title: String, val done: Boolean = false)

/** Process-wide state for the one capsule above the composer (item 1: job banners). Main thread only. */
object JobBanners {
    private val items = mutableStateListOf<JobBannerItem>()
    val current: JobBannerItem? get() = items.lastOrNull()

    fun show(id: String, kind: JobKind, title: String) {
        items.removeAll { it.id == id }
        items.add(JobBannerItem(id, kind, title))
    }

    fun update(id: String, title: String, done: Boolean) {
        val i = items.indexOfFirst { it.id == id }
        if (i < 0) items.add(JobBannerItem(id, JobKind.Job, title, done)) else items[i] = items[i].copy(title = title, done = done)
    }

    fun dismiss(id: String) {
        items.removeAll { it.id == id }
    }
}

/** One capsule: icon per kind, the text cross-fades when it changes, announced politely to screen readers. */
@Composable
fun JobBannerHost(modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    val item = JobBanners.current ?: return
    // A finished banner clears itself after a moment.
    LaunchedEffect(item.id, item.done) {
        if (item.done) { delay(3_000); JobBanners.dismiss(item.id) }
    }
    Row(
        modifier = modifier
            .clip(CircleShape)
            .background(c.bgPrimary)
            .border(1.dp, c.borderStrong, CircleShape)
            .padding(horizontal = 14.dp, vertical = 8.dp)
            .semantics { liveRegion = LiveRegionMode.Polite },
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        val icon = when {
            item.done -> R.drawable.ph_check
            item.kind == JobKind.Compaction -> R.drawable.ph_sparkle
            else -> R.drawable.ph_gear
        }
        Icon(painterResource(icon), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
        AnimatedContent(item.title, transitionSpec = { fadeIn() togetherWith fadeOut() }, label = "jobBannerText") { text ->
            Text(text, color = c.textPrimary, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}
