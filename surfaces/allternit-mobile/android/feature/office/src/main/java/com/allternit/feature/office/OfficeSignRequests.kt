package com.allternit.feature.office

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.allternit.core.api.ApiResult
import com.allternit.core.api.gen.SignRequest
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import com.allternit.feature.office.sign.SignRepository
import kotlinx.coroutines.launch

private val SignWarn = Color(0xFFE0612F)

/**
 * "Awaiting your signature" on the Office floor (iOS floor card, same layout as [OfficeApprovalCard]): the oldest pending
 * sign request as a card with the requester's name, "+N more" for the rest, and a "Review & sign" button. Tapping the
 * card opens [PdfSignScreen] with the request id; the card's menu has Decline. Loads once when the floor appears for a
 * signed-in user and again after the signing screen closes. Shows nothing when nothing is pending or the load fails.
 */
@Composable
fun OfficeSignRequestCard(userId: String?, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val view = LocalView.current
    var pending by remember(userId) { mutableStateOf<List<SignRequest>>(emptyList()) }
    var reload by remember { mutableStateOf(0) }
    var opened by remember { mutableStateOf<SignRequest?>(null) }
    var menu by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(userId, reload) {
        if (userId == null) { pending = emptyList(); return@LaunchedEffect }
        val r = SignRepository.list(status = "pending")
        if (r is ApiResult.Ok) pending = r.value.items.filter { it.status == "pending" }.sortedBy { it.createdAt }
    }

    opened?.let { req ->
        Dialog(
            onDismissRequest = { opened = null; reload += 1 },
            properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false),
        ) {
            Box(Modifier.fillMaxSize()) {
                PdfSignScreen(title = req.title, source = null, onClose = { opened = null; reload += 1 }, requestId = req.id)
            }
        }
    }

    val first = pending.firstOrNull() ?: return
    val more = pending.size - 1
    fun decline() {
        busy = true; error = null
        scope.launch {
            val r = SignRepository.decline(first.id)
            busy = false
            if (r is ApiResult.Ok) { Haptics.perform(view, Haptics.Kind.Tap); pending = pending.filterNot { it.id == first.id } }
            else error = "Couldn't decline. ${SignRepository.errorMessage(r)}"
        }
    }

    Column(
        modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(c.bgPanel)
            .border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(16.dp))
            .clickable(enabled = !busy, onClickLabel = "Review and sign ${first.title}", role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); opened = first }
            .padding(14.dp).testTag("officeSignCard"),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Icon(painterResource(DesignR.drawable.ph_signature), contentDescription = null, tint = SignWarn, modifier = Modifier.size(14.dp))
            Text("Awaiting your signature", color = SignWarn, fontSize = 12.5.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f).semantics { heading() })
            if (more > 0) Text("+$more more", color = c.textTertiary, fontSize = 12.sp)
            Box {
                Icon(
                    painterResource(DesignR.drawable.ph_dots_three), contentDescription = null, tint = c.textSecondary,
                    modifier = Modifier.size(24.dp).clickable(enabled = !busy, role = Role.Button, onClickLabel = "More") { menu = true }
                        .semantics { this.contentDescription = "More for ${first.title}" }.testTag("officeSignMenu"),
                )
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    DropdownMenuItem(text = { Text("Decline") }, onClick = { menu = false; decline() }, modifier = Modifier.testTag("officeSignDecline"))
                }
            }
        }
        Text(first.title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 3, overflow = TextOverflow.Ellipsis)
        first.requesterName?.takeIf { it.isNotBlank() }?.let {
            Text("From $it", color = c.textSecondary, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
            CardButton("Review & sign", "officeSignReview", primary = true, label = "Review and sign: ${first.title}", enabled = !busy, busy = busy) { opened = first }
        }
        error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.5.sp, modifier = Modifier.testTag("officeSignError")) }
    }
}
