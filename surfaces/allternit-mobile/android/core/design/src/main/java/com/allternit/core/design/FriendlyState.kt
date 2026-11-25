package com.allternit.core.design

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** Port of iOS FriendlyStateView.Style. */
enum class FriendlyStyle { Empty, Error, Offline }

private fun FriendlyStyle.tint(c: AllternitColorScheme): Color = when (this) {
    FriendlyStyle.Empty -> c.accentPrimary
    FriendlyStyle.Error -> AllternitStatic.StatusError
    FriendlyStyle.Offline -> AllternitStatic.StatusWarning
}

/**
 * A calm empty / error / offline state (iOS `FriendlyStateView`). [icon] is a slot so the
 * caller supplies a `ph_*` vector. Pass friendly copy only; run raw errors through
 * [FriendlyErrorMessage] first so a code never reaches the screen.
 */
@Composable
fun FriendlyState(
    style: FriendlyStyle,
    title: String,
    message: String,
    modifier: Modifier = Modifier,
    actionTitle: String? = null,
    onAction: (() -> Unit)? = null,
    art: EmptyArtKind? = null,
    gizzi: Boolean = false,
    panel: Boolean = false,
    paper: Color = Color.Unspecified,
    icon: @Composable () -> Unit,
) {
    val c = AllternitTheme.colors
    val tint = style.tint(c)
    val view = LocalView.current
    Column(
        modifier = modifier.fillMaxWidth().padding(top = 48.dp, bottom = 24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        if (gizzi) GizziWalk() else if (art != null) EmptyArt(art, size = if (panel) 64.dp else 96.dp, paper = paper) else Box(
            modifier = Modifier.size(64.dp)
                .clip(RoundedCornerShape(AllternitRadii.LG))
                .background(tint.copy(alpha = 0.12f))
                .border(1.dp, tint.copy(alpha = 0.25f), RoundedCornerShape(AllternitRadii.LG)),
            contentAlignment = Alignment.Center,
        ) { icon() }
        Text(
            title, color = c.textPrimary, fontSize = 20.sp, fontWeight = FontWeight.Medium,
            fontFamily = AllternitSerif, textAlign = TextAlign.Center,
        )
        Text(
            message, color = c.textSecondary, fontSize = 15.sp, textAlign = TextAlign.Center,
            modifier = Modifier.padding(horizontal = 40.dp),
        )
        if (actionTitle != null && onAction != null) {
            Box(
                modifier = Modifier.padding(top = 4.dp).height(40.dp)
                    .clip(CircleShape).background(c.bgPanel)
                    .border(BorderStroke(1.dp, c.borderSubtle), CircleShape)
                    .clickable { Haptics.perform(view, Haptics.Kind.Tap); onAction() }
                    .padding(horizontal = 18.dp),
                contentAlignment = Alignment.Center,
            ) { Text(actionTitle, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold) }
        }
    }
}

/** Turns raw transport errors into product copy (iOS `FriendlyErrorMessage`). */
object FriendlyErrorMessage {
    const val FALLBACK = "Something went wrong. Check your connection and try again."

    private val rawMarkers = listOf(
        "http", "status code", "nsurl", "error domain", "decoding", "json", "sqlx", "errno", "code=",
        "decode", "couldn't be read", "correct format", "server response", "header", "bearer", "token",
        "authorization", "unauthorized",
    )

    fun from(error: String): String {
        val lowered = error.lowercase()
        return when {
            "data-plane node" in lowered -> "Connect a computer or start Allternit Cloud to use this."
            "could not connect" in lowered || "failed to connect" in lowered ->
                "Could not connect to the server. Check your connection and try again."
            "cancelled" in lowered || "canceled" in lowered -> "The request was cancelled."
            "not found" in lowered || "404" in lowered -> "That resource couldn't be found."
            else -> plain(error)
        }
    }

    /** Like [from] but names what failed: `doing` = "load engines". */
    fun from(error: String, doing: String): String {
        val message = from(error)
        return if (message == FALLBACK) "Couldn't $doing. Check your connection and try again." else message
    }

    private fun plain(error: String): String {
        val trimmed = error.trim()
        val lowered = trimmed.lowercase()
        val words = trimmed.split(" ").filter { it.isNotEmpty() }.size
        val raw = words < 4 || rawMarkers.any { it in lowered } || (trimmed.any { it.isDigit() } && words < 6)
        return if (raw) FALLBACK else trimmed
    }
}
