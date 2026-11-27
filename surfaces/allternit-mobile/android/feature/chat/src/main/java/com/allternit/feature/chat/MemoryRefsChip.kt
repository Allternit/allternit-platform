package com.allternit.feature.chat

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import java.time.Instant
import java.time.LocalDate
import java.time.OffsetDateTime
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

/**
 * "Used memory (n)" under an assistant reply (iOS MemoryRefsChip): the memories the reply recalled,
 * saved, updated or deleted. Tapping it lists the saved ones, then "From past chats" with short
 * dates; a past-chat row with a session id opens that chat.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun MemoryRefsChip(refs: List<MemoryRef>, onOpenSession: (String) -> Unit, onManage: () -> Unit = {}) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    var open by remember { mutableStateOf(false) }
    Row(
        Modifier
            .clip(CircleShape)
            .background(c.fillNeutral)
            .testTag("memoryRefsChip")
            .clickable(role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); open = true }
            .semantics { contentDescription = "Used memory, ${refs.size} ${if (refs.size == 1) "entry" else "entries"}. Shows the memories this reply used" }
            .padding(horizontal = 9.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        Icon(painterResource(DesignR.drawable.ph_brain), contentDescription = null, tint = c.accentPrimary, modifier = Modifier.size(12.dp))
        Text("Used memory (${refs.size})", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.Medium)
    }
    if (open) {
        val saved = refs.filter { !it.isPastChat }
        val past = refs.filter { it.isPastChat }
        ModalBottomSheet(onDismissRequest = { open = false }, containerColor = c.bgPrimary) {
            Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp).testTag("memoryRefsSheet"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("Memory used", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                    Text("Done", color = c.accentPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.clickable(role = Role.Button) { open = false }.padding(8.dp))
                }
                saved.forEach { ref ->
                    val (icon, verb) = when (ref.action) {
                        "saved" -> DesignR.drawable.ph_plus_circle to "Saved"
                        "updated" -> DesignR.drawable.ph_pencil_simple to "Updated"
                        "deleted" -> DesignR.drawable.ph_trash to "Deleted"
                        else -> DesignR.drawable.ph_brain to "Recalled"
                    }
                    RefRow(icon, ref.title?.takeIf { it.isNotEmpty() } ?: "Memory", verb, chevron = false, onClick = null)
                }
                if (past.isNotEmpty()) {
                    Text("From past chats", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(top = 6.dp))
                    past.forEach { ref ->
                        val sid = ref.sessionId?.takeIf { it.isNotEmpty() }
                        RefRow(
                            DesignR.drawable.ph_chats_circle, ref.title?.takeIf { it.isNotEmpty() } ?: "Past chat", shortDate(ref.date).ifEmpty { null },
                            chevron = sid != null,
                            onClick = sid?.let { s -> { Haptics.perform(view, Haptics.Kind.Tap); open = false; onOpenSession(s) } },
                        )
                    }
                }
                RefRow(DesignR.drawable.ph_gear, "Manage memory", null, chevron = true, onClick = { Haptics.perform(view, Haptics.Kind.Tap); open = false; onManage() })
            }
        }
    }
}

@Composable
private fun RefRow(icon: Int, title: String, detail: String?, chevron: Boolean, onClick: (() -> Unit)?) {
    val c = AllternitTheme.colors
    Row(
        Modifier.fillMaxWidth().heightIn(min = 44.dp)
            .then(if (onClick != null) Modifier.clickable(role = Role.Button) { onClick() } else Modifier),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(painterResource(icon), contentDescription = null, tint = c.accentPrimary, modifier = Modifier.size(20.dp))
        Column(Modifier.weight(1f)) {
            Text(title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
            if (detail != null) Text(detail, color = c.textSecondary, fontSize = 12.sp)
        }
        if (chevron) Icon(painterResource(DesignR.drawable.ph_caret_right), contentDescription = null, tint = c.textTertiary, modifier = Modifier.size(12.dp))
    }
}

/** "Nov 20" for a past chat's ISO date or date-time; empty when absent or unreadable. */
internal fun shortDate(iso: String?): String {
    if (iso.isNullOrEmpty()) return ""
    val zone = ZoneId.systemDefault()
    val date = runCatching { OffsetDateTime.parse(iso).atZoneSameInstant(zone).toLocalDate() }.getOrNull()
        ?: runCatching { Instant.parse(iso).atZone(zone).toLocalDate() }.getOrNull()
        ?: runCatching { LocalDate.parse(iso.take(10)) }.getOrNull()
        ?: return ""
    return date.format(DateTimeFormatter.ofPattern("MMM d", Locale.getDefault()))
}
