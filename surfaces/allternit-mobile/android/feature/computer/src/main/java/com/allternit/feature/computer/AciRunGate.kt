package com.allternit.feature.computer

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.ExperimentalMaterial3Api
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.auth.DeviceAuth
import com.allternit.core.design.AllternitTheme

/** What a run is waiting on. One at a time; the model holds the latest (iOS `ACIRunGate`). */
sealed interface AciRunGate {
    val key: String

    data class Approval(val request: AciComputerEvent.ConfirmationRequired) : AciRunGate {
        override val key get() = "approval-${request.approvalId}"
    }

    data class NeedsYou(val runId: String?, val reason: String, val secret: Boolean) : AciRunGate {
        override val key get() = "human-${runId ?: "run"}-$reason"
    }

    data class Blocked(val block: AciComputerEvent.BlockDetected) : AciRunGate {
        override val key get() = "block-${block.runId ?: "run"}-${block.domain}-${block.kind}"
    }
}

private val DenyRed = Color(0xFFC0392B)

private tailrec fun Context.findActivity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.findActivity()
    else -> null
}

/**
 * The sheet over the run view when the bot needs the owner: credential approval, needs-you, or blocked.
 * Same copy, order and states as iOS `ACIRunGateSheet`. Approve and Allow for this run ask for fingerprint, face
 * or the screen lock first; a phone with no screen lock cannot approve.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AciRunGateSheet(
    gate: AciRunGate,
    botName: String,
    onApprove: (scope: String?) -> Unit,
    onDeny: () -> Unit,
    onContinue: () -> Unit,
    onStop: () -> Unit,
    onRerun: (AciRerunTarget) -> Unit,
    onDismiss: () -> Unit,
) {
    val c = AllternitTheme.colors
    val activity = LocalContext.current.findActivity()
    var secret by remember(gate.key) { mutableStateOf("") }
    var authFailed by remember(gate.key) { mutableStateOf(false) }

    fun approveWithAuth(scope: String?, reason: String) {
        if (activity == null) { authFailed = true; return }
        DeviceAuth.confirm(activity, reason, null) { ok ->
            authFailed = !ok
            if (ok) onApprove(scope)
        }
    }

    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = c.bgPrimary) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 20.dp).padding(bottom = 20.dp).testTag("aciRunGate"),
            verticalArrangement = Arrangement.spacedBy(16.dp)) {
            when (gate) {
                is AciRunGate.Approval -> {
                    val r = gate.request
                    if (r.isCredential) {
                        val name = r.credentialName.orEmpty()
                        val domain = r.credentialDomain ?: "this site"
                        Title("Fill your $name login on $domain?")
                        Caption("$botName will fill it in. The password never shows on screen or in the log.")
                        Primary("Approve", "aciGateApprove") { approveWithAuth(null, "Fill your $name login on $domain") }
                        Secondary("Allow for this run", "aciGateAllowRun") { approveWithAuth("run", "Allow $name on $domain for this run") }
                        Destructive("Deny", "aciGateDeny", onDeny)
                    } else {
                        Title("$botName wants to do something")
                        Caption(r.message ?: "Review it before it goes ahead.")
                        Primary("Approve", "aciGateApprove") { approveWithAuth(null, "Approve this action") }
                        Destructive("Deny", "aciGateDeny", onDeny)
                    }
                    if (authFailed) Caption("Your fingerprint, face or screen lock didn't confirm, so nothing was approved.")
                }
                is AciRunGate.NeedsYou -> {
                    Title("$botName needs you")
                    Caption(gate.reason)
                    if (gate.secret) {
                        Box(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(12.dp)) {
                            if (secret.isEmpty()) Text("Secret", color = c.textTertiary, fontSize = 16.sp)
                            BasicTextField(secret, { secret = it }, singleLine = true, visualTransformation = PasswordVisualTransformation(),
                                textStyle = TextStyle(color = c.textPrimary, fontSize = 16.sp), cursorBrush = SolidColor(c.textPrimary),
                                keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password),
                                modifier = Modifier.fillMaxWidth().testTag("aciGateSecret"))
                        }
                    }
                    // Like iOS, the box is for the person to type into the page's own field; it is cleared and never sent.
                    Primary("Continue", "aciGateContinue") { secret = ""; onContinue() }
                    Destructive("Stop", "aciGateStop", onStop)
                }
                is AciRunGate.Blocked -> {
                    val b = gate.block
                    Title("${b.domain} blocked $botName")
                    Caption(blockReason(b.kind))
                    Primary("Continue", "aciGateContinue", onContinue)
                    if (b.rerunTargets.isEmpty()) {
                        Caption("No other computer is ready to re-run this. Add one in Computers.")
                    } else {
                        b.rerunTargets.forEach { t -> Secondary("Re-run on ${t.name}", "aciGateRerun") { onRerun(t) } }
                        Caption("A re-run starts fresh. No logins carry over.")
                    }
                    Destructive("Stop", "aciGateStop", onStop)
                }
            }
        }
    }
}

private fun blockReason(kind: String) = when (kind) {
    "login" -> "The site wants you to sign in."
    "two_factor" -> "The site wants a verification code."
    "captcha" -> "The site is showing a puzzle."
    "rate_limited" -> "The site says there were too many requests."
    else -> "The site is not letting the bot in."
}

@Composable
private fun Title(text: String) = Text(text, color = AllternitTheme.colors.textPrimary, fontSize = 20.sp, fontWeight = FontWeight.SemiBold)

@Composable
private fun Caption(text: String) = Text(text, color = AllternitTheme.colors.textSecondary, fontSize = 14.sp)

@Composable
private fun Primary(text: String, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.textPrimary).clickable(onClick = onClick).padding(vertical = 14.dp).testTag(tag),
        contentAlignment = Alignment.Center) { Text(text, color = c.bgPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold) }
}

@Composable
private fun Secondary(text: String, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.fillNeutral).clickable(onClick = onClick).padding(vertical = 14.dp).testTag(tag),
        contentAlignment = Alignment.Center) { Text(text, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold) }
}

@Composable
private fun Destructive(text: String, tag: String, onClick: () -> Unit) {
    Box(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(vertical = 12.dp).testTag(tag), contentAlignment = Alignment.Center) {
        Text(text, color = DenyRed, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
    }
}
