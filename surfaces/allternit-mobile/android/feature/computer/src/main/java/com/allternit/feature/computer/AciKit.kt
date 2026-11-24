package com.allternit.feature.computer

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.wrapContentSize
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import com.allternit.core.design.DockSpec
import com.allternit.core.design.DockingBlockTitle
import com.allternit.core.design.LocalDockOffset
import kotlin.math.roundToInt
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiClients
import com.allternit.core.api.ApiConfig
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.chat.RuntimeCall
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** One line of text for a failed runtime call (iOS FriendlyErrorMessage); null when the call worked. */
internal fun RuntimeCall.failureText(): String? = when (this) {
    is RuntimeCall.Ok -> null
    RuntimeCall.NoComputer -> "No computer is connected."
    is RuntimeCall.Failed -> message
    is RuntimeCall.Http -> message?.takeIf { it.isNotBlank() } ?: when (status) {
        404 -> "This computer doesn’t have that yet. Update Allternit Desktop, then try again."
        in 502..504 -> "This computer isn't responding."
        else -> "Couldn’t load this. Try again."
    }
}

/**
 * A call through the relay to one named computer (not the picked one): `POST /api/v1/runtime-devices/:id/proxy`
 * with the `{method, path, headers, body, bodyEncoding}` envelope (the same one `ChatBackend.runtimeJson` sends).
 * The Files drive screens use it for `/api/v1/files/v2/…`; :app's Files client is not a dependency of a feature module.
 */
internal object AciRelay {
    private val config = ApiConfig()
    private val http by lazy { ApiClients.okHttp(bearer = AllternitAuth.tokenProvider) }
    private val deviceId = "android-aci-" + java.util.UUID.randomUUID().toString().take(12)

    suspend fun call(runtimeId: String, method: String, path: String, jsonBody: String? = null): RuntimeCall {
        val url = config.cloudBaseUrl.newBuilder().addPathSegments("api/v1/runtime-devices").addPathSegment(runtimeId).addPathSegment("proxy").build()
        val envelope = buildJsonObject {
            put("method", method)
            put("path", path)
            put("headers", buildJsonObject {
                put("accept", "application/json")
                put("x-allternit-device-id", deviceId)
                put("x-allternit-device-platform", "android")
                if (jsonBody != null) put("content-type", "application/json")
            })
            put("body", jsonBody ?: "")
            put("bodyEncoding", "utf8")
        }.toString()
        return when (val r = ApiCall.execute(http, ApiJson.default, ApiCall.request("POST", url, envelope), JsonElement.serializer())) {
            is ApiResult.Ok -> RuntimeCall.Ok(r.value)
            is ApiResult.Decode -> RuntimeCall.Ok(JsonNull)
            is ApiResult.HttpError ->
                if (r.status == 428 || r.code == "compute_required") RuntimeCall.NoComputer
                else if (r.status == 503) RuntimeCall.Failed("This computer isn't reachable right now. Wake it, then try again.")
                else RuntimeCall.Http(r.status, r.code, r.message)
            is ApiResult.Network -> RuntimeCall.Failed("You're offline. Check your connection and try again.")
        }
    }
}

/**
 * Draws [title] as the docking block title from inside a header: a screen-sized, touch-free overlay that is
 * pinned to the screen origin whatever the header's own position or the page scroll ([counterScroll] for a header
 * that scrolls with the page). [titleTop] and [barCenter] get the header's root y (at scroll 0) in dp.
 */
@Composable
internal fun AciDockOverlay(
    title: String,
    offset: () -> Float,
    counterScroll: Boolean,
    titleTop: (anchorY: Float) -> Dp,
    barCenter: (anchorY: Float) -> Dp,
) {
    val d = LocalDensity.current.density
    val conf = LocalConfiguration.current
    var ox by remember { mutableFloatStateOf(0f) }
    var oy by remember { mutableFloatStateOf(0f) }
    Box(
        Modifier.size(0.dp)
            .onGloballyPositioned {
                val p = it.positionInRoot()
                ox = p.x
                oy = p.y + if (counterScroll) offset() * d else 0f
            }
            .wrapContentSize(Alignment.TopStart, unbounded = true)
            .offset { IntOffset(-ox.roundToInt(), -(oy - (if (counterScroll) offset() * d else 0f)).roundToInt()) },
    ) {
        DockingBlockTitle(
            text = title, scrollOffset = offset, barCenter = barCenter(oy / d), top = titleTop(oy / d),
            modifier = Modifier.requiredSize(conf.screenWidthDp.dp, conf.screenHeightDp.dp),
        )
    }
}

/** A full-width top bar: Back chevron text, centred title (iOS inline navigation title). */
@Composable
internal fun AciTopBar(title: String, backTitle: String, onBack: () -> Unit, trailing: @Composable () -> Unit = {}) {
    val c = AllternitTheme.colors
    val dock = LocalDockOffset.current
    Box(Modifier.fillMaxWidth().heightIn(min = 52.dp).background(c.bgPrimary)) {
        Row(
            Modifier.heightIn(min = 52.dp).clickable(onClickLabel = "Back to $backTitle") { onBack() }.padding(horizontal = 14.dp).testTag("aciBack"),
            verticalAlignment = Alignment.CenterVertically,
        ) { Text("‹ $backTitle", color = c.textPrimary, fontSize = 15.sp) }
        // With a dock scroll the title is the block title: big in the page, spelled into this bar's centre.
        Text(title, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, maxLines = 1,
            modifier = Modifier.align(Alignment.Center).padding(horizontal = 96.dp)
                .then(if (dock != null) Modifier.alpha(0f).clearAndSetSemantics { } else Modifier))
        Box(Modifier.align(Alignment.CenterEnd).padding(end = 12.dp)) { trailing() }
        if (dock != null) {
            AciDockOverlay(title, dock, counterScroll = false, titleTop = { DockSpec.TOP.dp }, barCenter = { y -> (y + 26f).dp })
        }
    }
    Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))
}

/** Loading, empty and error states (iOS FriendlyStateView). */
@Composable
internal fun AciStateCard(title: String, message: String, actionTitle: String? = null, onAction: (() -> Unit)? = null, tag: String = "aciState") {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 40.dp).testTag(tag), horizontalAlignment = Alignment.CenterHorizontally) {
        Text(title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center)
        Spacer(Modifier.size(6.dp))
        Text(message, color = c.textSecondary, fontSize = 13.sp, textAlign = TextAlign.Center)
        if (actionTitle != null && onAction != null) {
            Spacer(Modifier.size(14.dp))
            AciPillButton(actionTitle, "$tag-action") { onAction() }
        }
    }
}

@Composable
internal fun AciSearchField(value: String, placeholder: String, onChange: (String) -> Unit, tag: String = "aciSearch") {
    val c = AllternitTheme.colors
    Box(
        Modifier.fillMaxWidth().heightIn(min = 44.dp).clip(RoundedCornerShape(12.dp)).background(c.bgPanel)
            .border(1.dp, c.borderSubtle, RoundedCornerShape(12.dp)).padding(horizontal = 12.dp),
        contentAlignment = Alignment.CenterStart,
    ) {
        if (value.isEmpty()) Text(placeholder, color = c.textTertiary, fontSize = 15.sp)
        BasicTextField(
            value = value, onValueChange = onChange, singleLine = true,
            textStyle = TextStyle(color = c.textPrimary, fontSize = 15.sp), cursorBrush = SolidColor(c.textPrimary),
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Text),
            modifier = Modifier.fillMaxWidth().semantics { contentDescription = placeholder }.testTag(tag),
        )
    }
}

/** A selectable pill (iOS ACIBarChip / category pill). */
@Composable
internal fun AciChip(title: String, on: Boolean, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        Modifier.heightIn(min = 44.dp).clip(CircleShape).background(if (on) c.accentPrimary else c.fillNeutral)
            .border(BorderStroke(1.dp, if (on) c.accentPrimary else c.borderSubtle), CircleShape)
            .clickable(role = Role.Tab, onClickLabel = title) { onClick() }.padding(horizontal = 14.dp).testTag(tag),
        contentAlignment = Alignment.Center,
    ) { Text(title, color = if (on) c.onAccent else c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold) }
}

@Composable
internal fun AciChipRow(content: @Composable () -> Unit) {
    Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) { content() }
}

/** A grouped card row: title, optional subtitle, optional trailing text. Tap acts. */
@Composable
internal fun AciListRow(title: String, subtitle: String? = null, trailing: String? = null, tag: String, onClick: (() -> Unit)? = null) {
    val c = AllternitTheme.colors
    Row(
        Modifier.fillMaxWidth().heightIn(min = 52.dp).let { if (onClick != null) it.clickable { onClick() } else it }.padding(vertical = 10.dp).testTag(tag),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1)
            if (!subtitle.isNullOrBlank()) Text(subtitle, color = c.textSecondary, fontSize = 12.sp, maxLines = 2)
        }
        if (trailing != null) Text(trailing, color = c.textTertiary, fontSize = 12.sp)
    }
    Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))
}

/** A bordered card (iOS BgPanel + border). */
@Composable
internal fun AciCard(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    val c = AllternitTheme.colors
    Box(modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(c.bgPanel).border(1.dp, c.borderSubtle, RoundedCornerShape(16.dp)).padding(14.dp)) { content() }
}

@Composable
internal fun AciFullPage(content: @Composable () -> Unit) {
    Box(Modifier.fillMaxSize().background(AllternitTheme.colors.bgPrimary)) { content() }
}

internal fun formatWhen(raw: String?): String {
    if (raw.isNullOrBlank()) return ""
    return runCatching {
        java.text.DateFormat.getDateTimeInstance(java.text.DateFormat.MEDIUM, java.text.DateFormat.SHORT).format(java.util.Date.from(java.time.Instant.parse(raw)))
    }.getOrDefault("")
}

internal fun formatBytes(n: Long): String = when {
    n < 1024 -> "$n B"
    n < 1024 * 1024 -> "${n / 1024} KB"
    n < 1024L * 1024 * 1024 -> String.format(java.util.Locale.US, "%.1f MB", n / 1048576.0)
    else -> String.format(java.util.Locale.US, "%.1f GB", n / 1073741824.0)
}
