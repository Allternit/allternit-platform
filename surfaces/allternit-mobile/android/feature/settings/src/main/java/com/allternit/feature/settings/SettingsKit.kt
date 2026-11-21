package com.allternit.feature.settings

import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
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
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.friendly
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import okhttp3.HttpUrl.Companion.toHttpUrl
import com.allternit.core.design.R as DesignR

/*
 * The Settings kit (iOS SettingsPageKit: PhoneSettingsLabel, PhoneSettingsCard, PhoneSettingsRowContent,
 * PhoneToggleRow, PhoneSettingsPage). Rounded neutral cards, hairline dividers, 16sp rows.
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
internal fun StateBox(title: String, message: String, actionTitle: String?, onAction: (() -> Unit)?) {
    val c = AllternitTheme.colors
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(AllternitRadii.LG)).background(c.fillNeutral).padding(20.dp).testTag("settingsState"),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(title, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center)
        Text(message, color = c.textSecondary, fontSize = 14.sp, lineHeight = 20.sp, textAlign = TextAlign.Center)
        if (actionTitle != null && onAction != null) SButton(actionTitle, onClick = onAction)
    }
}

/** A pushed Settings page: a round ‹ (or [leading]), a centred title, and a scrolling column. */
@Composable
internal fun SPage(
    title: String,
    tag: String,
    onBack: (() -> Unit)? = null,
    leading: (@Composable () -> Unit)? = null,
    trailing: @Composable () -> Unit = {},
    content: @Composable ColumnScope.() -> Unit,
) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().navigationBarsPadding().testTag(tag)) {
        Box(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 12.dp)) {
            Box(Modifier.align(Alignment.CenterStart)) {
                when {
                    leading != null -> leading()
                    onBack != null -> SRoundButton(DesignR.drawable.ph_caret_left, "Back", "settingsBack", onBack)
                }
            }
            Text(
                title, color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis,
                modifier = Modifier.align(Alignment.Center).padding(horizontal = 52.dp).semantics { contentDescription = title },
            )
            Box(Modifier.align(Alignment.CenterEnd)) { trailing() }
        }
        Column(
            Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(start = 16.dp, end = 16.dp, top = 8.dp, bottom = 32.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
            content = content,
        )
    }
}

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

/** `.phone-settings-label`. */
@Composable
internal fun SLabel(text: String) {
    Text(
        text, color = AllternitTheme.colors.textSecondary, fontSize = 14.sp,
        modifier = Modifier.fillMaxWidth().padding(start = 6.dp, end = 6.dp, top = 18.dp, bottom = 8.dp).semantics { contentDescription = "$text section" },
    )
}

/** `.phone-settings-card`: a rounded neutral card; put [SDivider] between rows. */
@Composable
internal fun SCard(modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    Column(modifier.fillMaxWidth().clip(RoundedCornerShape(AllternitRadii.LG)).background(AllternitTheme.colors.fillNeutral), content = content)
}

@Composable
internal fun SDivider() {
    Box(Modifier.fillMaxWidth().height(0.5.dp).background(AllternitTheme.colors.borderSubtle))
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
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val tint = if (destructive) Danger else c.textPrimary
    Row(
        Modifier.fillMaxWidth().heightIn(min = 50.dp).testTag(tag)
            .then(if (onClick != null) Modifier.clickable(enabled = enabled, role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onClick() } else Modifier)
            .padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        if (icon != null) Icon(painterResource(icon), contentDescription = null, tint = tint, modifier = Modifier.size(22.dp))
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

/** `PhoneToggleRow`. */
@Composable
internal fun SToggleRow(title: String, tag: String, checked: Boolean, onChange: (Boolean) -> Unit, icon: Int? = null, detail: String? = null, enabled: Boolean = true) {
    val c = AllternitTheme.colors
    SRow(title, tag, icon = icon, detail = detail, chevron = false, enabled = enabled, onClick = { onChange(!checked) }, trailing = {
        Switch(
            checked = checked, onCheckedChange = null, enabled = enabled,
            colors = SwitchDefaults.colors(checkedTrackColor = c.brandAccent, checkedThumbColor = c.onAccent),
        )
    })
}

/** `SettingsDescription`: a paragraph of quiet text. */
@Composable
internal fun SNote(text: String, modifier: Modifier = Modifier) {
    Text(text, color = AllternitTheme.colors.textSecondary, fontSize = 13.sp, lineHeight = 18.sp, modifier = modifier.fillMaxWidth().padding(horizontal = 6.dp, vertical = 6.dp))
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
