package com.allternit.feature.settings

import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.foundation.background
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.spring
import androidx.compose.foundation.border
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearOutSlowInEasing
import androidx.compose.animation.core.tween
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.foundation.LocalIndication
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.offset
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.Dp
import com.allternit.core.design.AnimatedGlyph
import com.allternit.core.design.systemAnimationsOff
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitRadii
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.LocalDockOffset
import com.allternit.core.design.ProvideDockScroll
import com.allternit.feature.artifacts.DockTitleLayer
import androidx.compose.ui.zIndex
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.friendly
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import okhttp3.HttpUrl.Companion.toHttpUrl
import com.allternit.core.design.R as DesignR

/*
 * The Settings kit (iOS SettingsPageKit: PhoneSettingsLabel, PhoneSettingsCard, PhoneSettingsRowContent,
 * PhoneToggleRow, PhoneSettingsPage). Flat groups on the page (web 390 px phone frame `S-after-phone-root`): grey
 * section labels, label-left / control-right 16sp rows, hairlines between rows, no card fill.
 */

internal val Danger = Color(0xFFD64545)

/** What a page's request is doing (iOS Load enums). [NoComputer] is the 428 / compute_required state, never a raw error. */
internal sealed interface Load<out T> {
    data object Loading : Load<Nothing>
    data class Ready<T>(val value: T) : Load<T>
    data object NoComputer : Load<Nothing>
    data class Failed(val message: String) : Load<Nothing>
}

internal fun ApiResult<JsonElement>.toLoad(doing: String): Load<JsonElement> = when (this) {
    is ApiResult.Ok -> Load.Ready(value)
    is ApiResult.HttpError -> if (status == 428 || code == "compute_required") Load.NoComputer else Load.Failed(friendly(doing))
    else -> Load.Failed(friendly(doing))
}

/** The Settings network paths. `/api/v1` through the account token ([LibraryNet]) and cloud-api. */
internal object Net {
    suspend fun get(path: String): ApiResult<JsonElement> = LibraryNet.get(path)

    suspend fun send(method: String, path: String, body: JsonElement? = null): ApiResult<JsonElement> = LibraryNet.send(method, path, body)

    /** cloud-api (`api/v1/notifications/...`, `api/v1/alerts/...`). */
    suspend fun cloud(method: String, path: String, body: JsonElement? = null): ApiResult<JsonElement> {
        val url = (LibraryNet.config.cloudBaseUrl.toString().trimEnd('/') + "/" + path.trimStart('/')).toHttpUrl()
        val request = ApiCall.request(method, url, body?.toString() ?: if (method == "GET") null else "{}")
        return when (val r = ApiCall.execute(LibraryNet.okHttp, ApiJson.default, request, JsonElement.serializer())) {
            is ApiResult.Decode -> ApiResult.Ok(JsonNull)
            else -> r
        }
    }
}

/** Runs [fetch] on enter and again each time the returned reload is called. */
@Composable
internal fun <T> rememberLoad(key: Any?, fetch: suspend () -> Load<T>): Pair<Load<T>, () -> Unit> {
    var state by remember(key) { mutableStateOf<Load<T>>(Load.Loading) }
    var nonce by remember(key) { mutableIntStateOf(0) }
    LaunchedEffect(key, nonce) {
        if (state !is Load.Ready) state = Load.Loading
        state = fetch()
    }
    return state to { nonce += 1 }
}

/** Loading spinner, no-computer, error with Try again, or [content]. */
@Composable
internal fun <T> LoadView(load: Load<T>, retry: () -> Unit, doing: String = "load this", content: @Composable (T) -> Unit) {
    when (load) {
        Load.Loading -> Box(Modifier.fillMaxWidth().padding(vertical = 32.dp).testTag("settingsLoading"), contentAlignment = Alignment.Center) {
            CircularProgressIndicator(Modifier.size(24.dp), color = AllternitTheme.colors.textSecondary, strokeWidth = 2.dp)
        }
        Load.NoComputer -> StateBox("Connect a computer", "This part of Settings lives on your computer. Connect one and it shows up here.", null, null)
        is Load.Failed -> StateBox("Couldn't $doing", load.message, "Try again", retry)
        is Load.Ready -> content(load.value)
    }
}

@Composable
internal fun StateBox(title: String, message: String, actionTitle: String?, onAction: (() -> Unit)?, art: com.allternit.core.design.EmptyArtKind? = null) {
    val c = AllternitTheme.colors
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(AllternitRadii.LG)).background(c.fillNeutral).padding(20.dp).testTag("settingsState"),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (art != null) com.allternit.core.design.EmptyArt(art, Modifier.clearAndSetSemantics {}, size = 64.dp)
        Text(title, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center)
        Text(message, color = c.textSecondary, fontSize = 14.sp, lineHeight = 20.sp, textAlign = TextAlign.Center)
        if (actionTitle != null && onAction != null) SButton(actionTitle, onClick = onAction)
    }
}

/** The icon of the page being shown, for the back crumb tile (set by SettingsRoot per pushed page). */
internal val LocalCrumbIcon = compositionLocalOf<Int?> { null }

/** A pushed Settings page: a round ‹ (or [leading]), a centred title, and a scrolling column. */
@Composable
internal fun SPage(
    title: String,
    tag: String,
    onBack: (() -> Unit)? = null,
    leading: (@Composable () -> Unit)? = null,
    trailing: @Composable () -> Unit = {},
    animateIn: Boolean = true,
    content: @Composable ColumnScope.() -> Unit,
) {
    val c = AllternitTheme.colors
    val scroll = rememberScrollState()
    val crumb = LocalCrumbIcon.current
    val reduced = rememberReduceMotion()
    // Contents fade in and rise 6dp as the page finishes sliding in.
    val intro = remember { Animatable(if (reduced || !animateIn) 1f else 0f) }
    LaunchedEffect(Unit) { if (intro.value < 1f) intro.animateTo(1f, tween(220, delayMillis = 100, easing = LinearOutSlowInEasing)) }
    // Every settings page docks its title: the big title sits under the bar and spells into the bar's centre.
    ProvideDockScroll(scroll) {
        val offset = LocalDockOffset.current
        Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().navigationBarsPadding().testTag(tag)) {
            Box(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 12.dp).zIndex(1f)) {
                if (offset != null && title.isNotBlank()) DockTitleLayer(title, offset)
                Box(Modifier.align(Alignment.CenterStart)) {
                    when {
                        leading != null -> leading()
                        onBack != null -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            SRoundButton(DesignR.drawable.ph_caret_left, "Back", "settingsBack", onBack)
                            if (crumb != null) STile(crumb, c.textPrimary, playOnAppear = true)
                        }
                    }
                }
                Box(Modifier.align(Alignment.CenterEnd)) { trailing() }
            }
            Column(
                Modifier.weight(1f).fillMaxWidth().graphicsLayer { alpha = intro.value; translationY = 6.dp.toPx() * (1f - intro.value) }.verticalScroll(scroll).padding(start = 16.dp, end = 16.dp, top = 8.dp, bottom = 32.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                // Room for the big title (it flies up into the bar as this column scrolls).
                Spacer(Modifier.height(SETTINGS_DOCK_TITLE_ROOM))
                content()
            }
        }
    }
}

private val SETTINGS_DOCK_TITLE_ROOM = 48.dp

/** `roundButton`: a 40dp filled circle. */
@Composable
internal fun SRoundButton(icon: Int, label: String, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Box(
        Modifier.size(40.dp).clip(CircleShape).background(c.fillNeutral).testTag(tag)
            .clickable(role = Role.Button, onClickLabel = label) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) { Icon(painterResource(icon), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(20.dp)) }
}

/** `.phone-settings-label`: the grey section label above a flat group. */
@Composable
internal fun SLabel(text: String) {
    Text(
        text, color = AllternitTheme.colors.textSecondary, fontSize = 14.sp,
        modifier = Modifier.fillMaxWidth().padding(start = 6.dp, end = 6.dp, top = 18.dp, bottom = 8.dp).semantics { contentDescription = "$text section" },
    )
}

/** `.phone-settings-card`: a flat group of rows on the page (no fill, no corners); put [SDivider] between rows. */
@Composable
internal fun SCard(modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    Column(modifier.fillMaxWidth(), content = content)
}

/** The hairline between two rows of a group. */
@Composable
internal fun SDivider() {
    Box(Modifier.fillMaxWidth().height(0.5.dp).background(AllternitTheme.colors.borderStrong))
}

/**
 * A two-option segmented control for a row's right side (label left, control right): Sans | Serif and the like.
 * The chosen side gets the panel fill and a hairline; the other stays quiet.
 */
@Composable
internal fun SSegmented(options: List<String>, selected: Int, tag: String, onSelect: (Int) -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(Modifier.clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).padding(2.dp).testTag(tag)) {
        options.forEachIndexed { index, label ->
            val on = index == selected
            Box(
                Modifier.clip(RoundedCornerShape(8.dp))
                    .then(if (on) Modifier.background(c.bgPanel).border(0.5.dp, c.borderStrong, RoundedCornerShape(8.dp)) else Modifier)
                    .clickable(role = Role.RadioButton) { if (!on) { Haptics.perform(view, Haptics.Kind.Tap); onSelect(index) } }
                    .semantics { stateDescription = if (on) "Selected" else "Not selected" }
                    .testTag("${tag}_$index")
                    .heightIn(min = 32.dp).padding(horizontal = 12.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text(label, color = if (on) c.textPrimary else c.textSecondary, fontSize = 14.sp, fontWeight = if (on) FontWeight.Medium else FontWeight.Normal)
            }
        }
    }
}

/** `.phone-row`: icon (optional), title, optional detail and value, optional ›. */
@Composable
internal fun SRow(
    title: String,
    tag: String,
    icon: Int? = null,
    detail: String? = null,
    value: String? = null,
    chevron: Boolean = true,
    destructive: Boolean = false,
    enabled: Boolean = true,
    onClick: (() -> Unit)? = null,
    trailing: (@Composable () -> Unit)? = null,
    logoName: String? = null,
    logoId: String? = null,
    interactionSource: MutableInteractionSource? = null,
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val tint = if (destructive) Danger else c.textPrimary
    var playKey by remember { mutableIntStateOf(0) }
    val source = interactionSource ?: remember { MutableInteractionSource() }
    Row(
        Modifier.fillMaxWidth().heightIn(min = 50.dp).testTag(tag)
            .then(if (onClick != null) Modifier.clickable(interactionSource = source, indication = LocalIndication.current, enabled = enabled, role = Role.Button) { playKey++; Haptics.perform(view, Haptics.Kind.Tap); onClick() } else Modifier)
            .padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        if (icon != null) STile(icon, tint, playKey = playKey)
        if (logoName != null) com.allternit.core.design.AppLogo(logoName, id = logoId, size = 32.dp)
        Column(Modifier.weight(1f)) {
            Text(title, color = if (enabled) tint else c.textTertiary, fontSize = 16.sp)
            if (detail != null) Text(detail, color = c.textSecondary, fontSize = 13.sp, lineHeight = 18.sp)
        }
        if (value != null) Text(value, color = c.textSecondary, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(start = 4.dp))
        if (trailing != null) trailing()
        else if (chevron && onClick != null) Icon(painterResource(DesignR.drawable.ph_caret_right), contentDescription = null, tint = c.textTertiary, modifier = Modifier.size(14.dp))
    }
}

/** True when Settings → Reduce motion is on or the system animator scale is 0. */
@Composable
internal fun rememberReduceMotion(): Boolean {
    val context = LocalContext.current
    return SettingsPrefs.reduceMotion(context) || systemAnimationsOff(context)
}

/** White icon tile (web `Tile`): panel fill, 1dp neutral border, neutral ink, animated glyph. */
@Composable
internal fun STile(icon: Int, tint: Color, playKey: Int = 0, playOnAppear: Boolean = false, size: Dp = 28.dp) {
    val c = AllternitTheme.colors
    val shape = RoundedCornerShape(size * 0.27f)
    Box(Modifier.size(size).clip(shape).background(c.bgPanel).border(1.dp, c.borderStrong, shape), contentAlignment = Alignment.Center) {
        AnimatedGlyph(icon, tint, size * 0.66f, playKey = playKey, playOnAppear = playOnAppear, reduced = rememberReduceMotion())
    }
}

/** Switch with a light overshoot on the thumb and a press stretch (approved Settings look). */
@Composable
internal fun SSwitch(checked: Boolean, enabled: Boolean, pressed: Boolean) {
    val c = AllternitTheme.colors
    val reduced = rememberReduceMotion()
    val thumbW by animateDpAsState(if (pressed && enabled) 33.dp else 27.dp, if (reduced) snap() else spring(stiffness = Spring.StiffnessMedium), label = "thumbW")
    val x by animateDpAsState(if (checked) 47.dp - thumbW else 0.dp, if (reduced) snap() else spring(dampingRatio = 0.62f, stiffness = Spring.StiffnessMedium), label = "thumbX")
    val track by animateColorAsState(if (checked) c.brandAccent else c.borderStrong, label = "track")
    Box(
        Modifier.size(51.dp, 31.dp).clip(CircleShape).background(track.copy(alpha = if (enabled) 1f else 0.5f)).padding(2.dp)
            .semantics { stateDescription = if (checked) "On" else "Off" },
    ) {
        Box(Modifier.offset(x = x).size(thumbW, 27.dp).clip(CircleShape).background(Color.White))
    }
}

/** `PhoneToggleRow`. */
@Composable
internal fun SToggleRow(title: String, tag: String, checked: Boolean, onChange: (Boolean) -> Unit, icon: Int? = null, detail: String? = null, enabled: Boolean = true) {
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    SRow(title, tag, icon = icon, detail = detail, chevron = false, enabled = enabled, onClick = { onChange(!checked) }, interactionSource = source, trailing = {
        SSwitch(checked, enabled, pressed)
    })
}

/** `SettingsDescription`: a paragraph of quiet text. */
@Composable
internal fun SNote(text: String, modifier: Modifier = Modifier, art: com.allternit.core.design.EmptyArtKind? = null) {
    if (art != null) Row(modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        com.allternit.core.design.EmptyArt(art, Modifier.clearAndSetSemantics {}, size = 36.dp)
        Text(text, color = AllternitTheme.colors.textSecondary, fontSize = 13.sp, lineHeight = 18.sp, modifier = Modifier.weight(1f).padding(horizontal = 6.dp, vertical = 6.dp))
    } else Text(text, color = AllternitTheme.colors.textSecondary, fontSize = 13.sp, lineHeight = 18.sp, modifier = modifier.fillMaxWidth().padding(horizontal = 6.dp, vertical = 6.dp))
}

@Composable
internal fun SError(text: String?) {
    if (text != null) Text(text, color = Danger, fontSize = 13.sp, lineHeight = 18.sp, modifier = Modifier.fillMaxWidth().padding(horizontal = 6.dp, vertical = 6.dp).testTag("settingsError"))
}

/** A pill button (iOS SettingsQuietButtonStyle). */
@Composable
internal fun SButton(title: String, tag: String? = null, enabled: Boolean = true, destructive: Boolean = false, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Box(
        Modifier.clip(RoundedCornerShape(22.dp)).background(c.fillNeutral)
            .then(if (tag != null) Modifier.testTag(tag) else Modifier)
            .clickable(enabled = enabled, role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .heightIn(min = 44.dp).padding(horizontal = 18.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(title, color = if (!enabled) c.textTertiary else if (destructive) Danger else c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
    }
}

@Composable
internal fun SField(label: String, value: String, tag: String, onChange: (String) -> Unit, singleLine: Boolean = true, enabled: Boolean = true) {
    OutlinedTextField(
        value = value, onValueChange = onChange, label = { Text(label) }, singleLine = singleLine, enabled = enabled,
        modifier = Modifier.fillMaxWidth().testTag(tag),
    )
}

/** Tells a part-2 page apart from a built one (iOS has every screen; part 2 of this wave replaces these). */
@Composable
internal fun SPlaceholder(label: String, onBack: () -> Unit) {
    SPage(label, tag = "settingsPlaceholder", onBack = onBack) {
        StateBox(label, "$label is part of the next Settings update on Android. Until then, open it in Allternit on the web or on iPhone.", null, null)
    }
}

/** iOS SafariView: a link opens in an in-app Custom Tab, or in the browser when "Open links in" is Safari. */
internal fun openLink(context: Context, url: String) {
    val uri = Uri.parse(url)
    runCatching {
        if (SettingsPrefs.openLinksInApp(context)) CustomTabsIntent.Builder().setShowTitle(true).build().launchUrl(context, uri)
        else context.startActivity(Intent(Intent.ACTION_VIEW, uri).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }
}

@Composable
internal fun rememberLinkOpener(): (String) -> Unit {
    val context = LocalContext.current
    return remember(context) { { url: String -> openLink(context, url) } }
}
