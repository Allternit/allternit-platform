package com.allternit.feature.shell

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.auth.Account
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.design.AllternitRadii
import com.allternit.core.design.AllternitTheme
import kotlinx.coroutines.launch

/**
 * What a signed-in person sees until step 7c replaces it with the shell (drawer, mode dial,
 * search, overview). [account] is null for the debug "Continue without signing in" path.
 */
@Composable
internal fun HomePlaceholder(account: Account?, onOpenWeb: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var signingOut by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .systemBarsPadding()
            .padding(horizontal = 24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
    ) {
        Text(
            text = if (account != null) "Hey, ${account.displayName}!" else "Hey there!",
            color = c.textPrimary,
            fontSize = 26.sp,
            fontFamily = FontFamily.Serif,
            fontWeight = FontWeight.Medium,
            textAlign = TextAlign.Center,
        )
        Text(
            text = account?.email ?: "Not signed in",
            color = c.textSecondary,
            fontSize = 14.sp,
            textAlign = TextAlign.Center,
        )
        Text(
            text = "This is the new native Allternit app, in testing. Chats, bots and projects arrive here step by step. Everything already works in the web version.",
            color = c.textTertiary,
            fontSize = 13.sp,
            lineHeight = 18.sp,
            textAlign = TextAlign.Center,
            modifier = Modifier.widthIn(max = 360.dp).padding(bottom = 8.dp),
        )
        HomeButton("Use web version", onClick = onOpenWeb)
        if (account != null) {
            HomeButton(if (signingOut) "Signing out…" else "Sign out", enabled = !signingOut) {
                signingOut = true
                error = null
                scope.launch {
                    error = AllternitAuth.signOut().error
                    signingOut = false
                }
            }
        }
        error?.let { Text(it, color = c.textSecondary, fontSize = 12.5.sp, textAlign = TextAlign.Center) }
    }
}

@Composable
private fun HomeButton(title: String, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    OutlinedButton(
        onClick = onClick,
        enabled = enabled,
        shape = RoundedCornerShape(AllternitRadii.MD),
        border = BorderStroke(1.dp, c.borderStrong),
        colors = ButtonDefaults.outlinedButtonColors(contentColor = c.textPrimary),
        modifier = Modifier.widthIn(max = 360.dp).fillMaxWidth().height(44.dp),
    ) { Text(title, fontSize = 14.sp) }
}
