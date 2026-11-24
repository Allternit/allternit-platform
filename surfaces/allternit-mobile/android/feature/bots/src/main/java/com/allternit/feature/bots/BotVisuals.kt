package com.allternit.feature.bots

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.util.LruCache
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.Haptics
import com.allternit.core.design.ThinkingOrb
import com.allternit.core.design.ThinkingOrbSize
import com.allternit.core.design.ThinkingOrbState
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import okhttp3.OkHttpClient
import okhttp3.Request
import com.allternit.core.design.R as DesignR

// The bot faces and the small shared pieces every Bots screen uses (iOS BotAvatarView,
// AgentIdenticonView, BotColor, RosterFace, the list rows and section headers).

internal fun hexColor(hex: String?, fallback: Color = Color(0xFF141413)): Color =
    runCatching { Color(android.graphics.Color.parseColor(hex ?: "")) }.getOrDefault(fallback)

/** A web asset path (`/avatar-packs/...`) on ai.allternit.com (iOS webAssetURL). */
internal fun webAsset(path: String): String? = when {
    path.startsWith("http://") || path.startsWith("https://") -> path
    path.startsWith("/") -> "https://ai.allternit.com$path"
    else -> null
}

/** Remote pictures: a sprite sheet's frame 0 (192×208, iOS SpriteAvatar still) or a whole image. Cached. */
internal object BotPictures {
    private val http by lazy { OkHttpClient() }
    private val cache = LruCache<String, ImageBitmap>(64)

    suspend fun load(url: String, spriteFrame: Boolean): ImageBitmap? {
        val key = (if (spriteFrame) "s:" else "i:") + url
        cache.get(key)?.let { return it }
        return withContext(Dispatchers.IO) {
            runCatching {
                http.newCall(Request.Builder().url(url).build()).execute().use { r ->
                    if (!r.isSuccessful) return@use null
                    val bytes = r.body?.bytes() ?: return@use null
                    val bmp = BitmapFactory.decodeByteArray(bytes, 0, bytes.size) ?: return@use null
                    if (spriteFrame) Bitmap.createBitmap(bmp, 0, 0, minOf(192, bmp.width), minOf(208, bmp.height)).asImageBitmap() else bmp.asImageBitmap()
                }
            }.getOrNull()?.also { cache.put(key, it) }
        }
    }
}

/** Where a bot's face comes from (iOS BotAvatarSource). */
internal sealed interface AvatarSource {
    data class Sprite(val url: String) : AvatarSource
    data class Picture(val url: String) : AvatarSource
    data object Gizzi : AvatarSource
    data class Legacy(val text: String) : AvatarSource

    companion object {
        fun resolve(bot: BotRecord): AvatarSource {
            val candidates = listOfNotNull(
                bot.config["avatar"] as? JsonObject,
                bot.avatar?.takeIf { it.trim().startsWith("{") }?.let { runCatching { com.allternit.core.api.ApiJson.default.parseToJsonElement(it) as? JsonObject }.getOrNull() },
            )
            for (o in candidates) {
                o.child("pet").str("spriteUrl")?.let { webAsset(it) }?.let { return Sprite(it) }
                if (o.str("type") == "image") {
                    (o.str("uri") ?: o.child("data").str("url"))?.let { webAsset(it) }?.let { return Picture(it) }
                }
                return Gizzi
            }
            val a = bot.avatar?.trim().orEmpty()
            if (a.isNotEmpty()) {
                if (a.startsWith("http") || a.startsWith("/avatar-packs/")) webAsset(a)?.let { return Picture(it) }
                if (!a.startsWith("{")) return Legacy(a)
            }
            return Gizzi
        }
    }
}

/** True when the bot has a face of its own (the primary orb, a sprite, a picture or a mark), false when it would fall back to the generic Gizzi tile. */
val BotRecord.hasOwnFace: Boolean get() = isPrimaryTemplateBot || AvatarSource.resolve(this) != AvatarSource.Gizzi

/** A bot's face (iOS BotAvatarView): the primary bot is the Allternit orb; otherwise its sprite, picture, Gizzi or mark. */
@Composable
fun BotAvatar(bot: BotRecord, size: Dp = 40.dp, live: Boolean = false, working: Boolean = false) {
    val c = AllternitTheme.colors
    Box(Modifier.size(size).semantics { contentDescription = "${bot.displayName} avatar" }, contentAlignment = Alignment.Center) {
        if (bot.isPrimaryTemplateBot) {
            ThinkingOrb(modifier = Modifier.size(size * 0.62f), state = if (working) ThinkingOrbState.Working else ThinkingOrbState.Breathing, size = if (size >= 48.dp) ThinkingOrbSize.Avatar else ThinkingOrbSize.Inline, paused = !working && !live)
            return@Box
        }
        when (val src = AvatarSource.resolve(bot)) {
            is AvatarSource.Sprite, is AvatarSource.Picture -> {
                val url = if (src is AvatarSource.Sprite) src.url else (src as AvatarSource.Picture).url
                val bmp by produceState<ImageBitmap?>(null, url) { value = BotPictures.load(url, src is AvatarSource.Sprite) }
                val b = bmp
                if (b != null) Image(b, null, contentScale = if (src is AvatarSource.Sprite) ContentScale.Fit else ContentScale.Crop, modifier = Modifier.size(size).clip(if (src is AvatarSource.Picture) CircleShape else RoundedCornerShape(0.dp)))
                else Box(Modifier.size(size).clip(CircleShape).background(hexColor(bot.accentHex, c.fillNeutral).copy(alpha = 0.25f)))
            }
            AvatarSource.Gizzi -> Box(Modifier.size(size).clip(CircleShape).background(hexColor(bot.accentHex, c.accentPrimary).copy(alpha = 0.18f)), contentAlignment = Alignment.Center) {
                ThinkingOrb(modifier = Modifier.size(size * 0.6f), state = ThinkingOrbState.Breathing, size = if (size >= 48.dp) ThinkingOrbSize.Avatar else ThinkingOrbSize.Inline, paused = !live)
            }
            is AvatarSource.Legacy -> AgentMark(bot, size)
        }
    }
}

/** The legacy agent mark: an emoji/character, else the identicon (iOS AgentAvatarView). */
@Composable
internal fun AgentMark(bot: BotRecord, size: Dp) {
    val c = AllternitTheme.colors
    val a = bot.avatar?.trim().orEmpty()
    if (a.isNotEmpty() && a.length <= 4) {
        Box(Modifier.size(size).clip(CircleShape).background(c.bgSecondary), contentAlignment = Alignment.Center) {
            Text(a, fontSize = (size.value * 0.5f).sp)
        }
    } else AgentIdenticon(seed = bot.id.ifEmpty { bot.name }, size = size)
}

/** A face with a working ring or waiting dot (iOS BotStatusAvatar). */
@Composable
fun BotStatusAvatar(bot: BotRecord, size: Dp = 40.dp, working: Boolean = false, waiting: Boolean = false) {
    Box {
        BotAvatar(bot, size, working = working)
        if (working || waiting) Box(
            Modifier.align(Alignment.BottomEnd).size(size * 0.26f).clip(CircleShape)
                .background(if (waiting) AllternitStatic.StatusWarning else AllternitStatic.StatusSuccess)
                .border(1.5.dp, AllternitTheme.colors.bgPrimary, CircleShape),
        )
    }
}

/**
 * The 5×5 mirrored identicon (iOS AgentIdenticonView): FNV-1a of the seed picks the cells, a
 * second hash of "seed~tint" picks the accent cells; the centre cell is always on.
 */
@Composable
fun AgentIdenticon(seed: String, size: Dp = 40.dp) {
    val accent = AllternitTheme.colors.accentPrimary
    val warm = Color(0.78f, 0.70f, 0.58f)
    val h = fnv1a(seed)
    val tint = fnv1a("$seed~tint")
    Canvas(Modifier.size(size).semantics { contentDescription = "Agent mark" }) {
        val unit = this.size.width / 5
        for (row in 0 until 5) for (col in 0 until 3) {
            val bit = row * 3 + col
            val filled = ((h shr bit) and 1uL) == 1uL || bit == 12
            if (!filled) continue
            val color = if (((tint shr bit) and 3uL) == 0uL) accent else warm
            for (x in listOf(col * unit, (4 - col) * unit)) {
                drawRoundRect(color, Offset(x, row * unit), Size(unit * 0.84f, unit * 0.84f), CornerRadius(unit * 0.22f))
            }
        }
    }
}

internal fun fnv1a(s: String): ULong {
    var h = 0xcbf29ce484222325uL
    for (b in s.toByteArray(Charsets.UTF_8)) {
        h = h xor (b.toInt() and 0xff).toULong()
        h *= 0x100000001b3uL
    }
    return h
}

/** iOS RosterStatus. */
enum class RosterStatus(val word: String) {
    Waiting("Needs you"), Talking("Talking"), Working("Working"), Idle("Idle");

    @Composable
    fun tone(): Color = when (this) {
        Waiting -> AllternitStatic.StatusWarning
        Talking -> AllternitStatic.StatusSuccess
        Working -> AllternitTheme.colors.textPrimary
        Idle -> AllternitTheme.colors.textTertiary
    }
}

// MARK: - Shared chrome

/** Section header in a list (iOS List Section header). */
@Composable
internal fun SectionHeader(title: String, modifier: Modifier = Modifier) {
    Text(
        title.uppercase(), color = AllternitTheme.colors.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.Medium, letterSpacing = 0.4.sp,
        modifier = modifier.padding(start = 20.dp, end = 20.dp, top = 18.dp, bottom = 6.dp).semantics { heading() },
    )
}

/** A rounded group of rows (iOS inset grouped List section). */
@Composable
internal fun RowGroup(modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    val c = AllternitTheme.colors
    Column(
        modifier.padding(horizontal = 16.dp).fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.bgPanel)
            .border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(14.dp)),
        content = content,
    )
}

/** One tappable row: icon, label, value and a chevron (iOS BotPageView pageRow). */
@Composable
internal fun NavRow(label: String, icon: Int?, value: String? = null, testTag: String? = null, last: Boolean = true, destructive: Boolean = false, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Column {
        Row(
            Modifier.fillMaxWidth().then(if (testTag != null) Modifier.testTag(testTag) else Modifier)
                .clickable(role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
                .heightIn(min = 50.dp).padding(horizontal = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            if (icon != null) Icon(painterResource(icon), null, tint = if (destructive) AllternitStatic.StatusError else c.textSecondary, modifier = Modifier.size(20.dp))
            Text(label, color = if (destructive) AllternitStatic.StatusError else c.textPrimary, fontSize = 16.sp, modifier = Modifier.weight(1f, fill = false))
            Spacer(Modifier.weight(1f))
            if (value != null) Text(value, color = c.textSecondary, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
            if (!destructive) Icon(painterResource(com.allternit.core.design.R.drawable.ph_caret_right), null, tint = c.textTertiary, modifier = Modifier.size(14.dp))
        }
        if (!last) Box(Modifier.padding(start = 48.dp).fillMaxWidth().height(1.dp).background(c.borderSubtle))
    }
}

/** A toggle row (iOS Toggle in a List). */
@Composable
internal fun ToggleRow(title: String, subtitle: String?, checked: Boolean, testTag: String, enabled: Boolean = true, onChange: (Boolean) -> Unit) {
    val c = AllternitTheme.colors
    Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, color = c.textPrimary, fontSize = 15.sp)
            if (!subtitle.isNullOrEmpty()) Text(subtitle, color = c.textSecondary, fontSize = 12.5.sp)
        }
        Switch(
            checked = checked, onCheckedChange = onChange, enabled = enabled, modifier = Modifier.testTag(testTag),
            colors = SwitchDefaults.colors(checkedTrackColor = c.textPrimary, checkedThumbColor = c.bgPrimary),
        )
    }
}

/** A pill button (iOS capsule buttons on cards). */
@Composable
internal fun Pill(title: String, testTag: String, primary: Boolean = false, enabled: Boolean = true, busy: Boolean = false, icon: Int? = null, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        Modifier.height(36.dp).clip(CircleShape)
            .background(if (primary) c.textPrimary else c.fillNeutral)
            .testTag(testTag)
            .clickable(enabled = enabled && !busy, role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        val ink = if (primary) c.bgPrimary else c.textPrimary
        if (busy) CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 1.5.dp, color = ink)
        else if (icon != null) Icon(painterResource(icon), null, tint = if (enabled) ink else c.textTertiary, modifier = Modifier.size(15.dp))
        Text(title, color = if (enabled) ink else c.textTertiary, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1)
    }
}

/** The standard loading / error / empty block for a list (iOS ProgressView + FriendlyStateView). */
@Composable
internal fun ListState(loading: Boolean, error: String?, empty: Boolean, errorTitle: String, emptyText: String, onRetry: () -> Unit): Boolean {
    val c = AllternitTheme.colors
    when {
        loading && empty -> Box(Modifier.fillMaxWidth().padding(vertical = 40.dp), contentAlignment = Alignment.Center) {
            CircularProgressIndicator(Modifier.size(24.dp).semantics { contentDescription = "Loading" }, strokeWidth = 2.dp, color = c.textSecondary)
        }
        error != null && empty -> FriendlyState(FriendlyStyle.Error, errorTitle, error, actionTitle = "Retry", onAction = onRetry) {
            Icon(painterResource(DesignR.drawable.ph_warning), null, tint = AllternitStatic.StatusError, modifier = Modifier.size(28.dp))
        }
        empty -> Text(emptyText, color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 28.dp))
        else -> return false
    }
    return true
}

/** The red line under a form (iOS errorBanner / saveError rows). */
@Composable
internal fun ErrorLine(message: String, modifier: Modifier = Modifier) {
    Row(modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Icon(painterResource(DesignR.drawable.ph_warning), null, tint = AllternitStatic.StatusError, modifier = Modifier.size(14.dp).offset(y = 2.dp))
        Text(message, color = AllternitStatic.StatusError, fontSize = 13.sp)
    }
}

@Composable
internal fun Dot(color: Color, size: Dp = 8.dp) = Box(Modifier.size(size).clip(CircleShape).background(color))

@Composable
internal fun Hairline() = Box(Modifier.fillMaxWidth().height(1.dp).background(AllternitTheme.colors.borderSubtle))

@Composable
internal fun Gap(h: Dp) = Spacer(Modifier.height(h).width(1.dp))
