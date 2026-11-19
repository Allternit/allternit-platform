package com.allternit.feature.onboarding

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.withLink
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.auth.StepResult
import com.allternit.core.design.AllternitRadii
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.clerk.api.sso.OAuthProvider
import kotlinx.coroutines.launch

/** Which part of the sign-in rail is open (iOS LoginGateView: buttons, email form, Clerk's code step). */
private enum class Step { Options, Email, Code }

private val SignInPage = Color(0xFF141413)

/**
 * Native sign-in, the Android twin of iOS `LoginGateView` (ios/App/AllternitApp.swift): the
 * offline wordmark/Gizzi/cast loop ([LoginArrival]), headline, Google, GitHub and Apple with
 * their logos (all three through Clerk OAuth in a Custom Tab; Google leads on Android the way
 * Apple leads on iOS), "Continue with email" with the emailed code, "Secured by clerk" and the
 * 18+ / Terms / Privacy line. Like iOS it is always dark; the caller wraps it in
 * `AllternitTheme(ThemeMode.Dark)`.
 *
 * Test ids (maestro/ids.md): `loginHeadline`, `loginContinueEmail`, `loginSkipAuth`.
 * [onSkipAuth] is non-null only in debug builds, as on iOS.
 */
@Composable
fun SignInScreen(
    modifier: Modifier = Modifier,
    onSkipAuth: (() -> Unit)? = null,
) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val view = LocalView.current

    var step by rememberSaveable { mutableStateOf(Step.Options) }
    var email by rememberSaveable { mutableStateOf("") }
    var code by rememberSaveable { mutableStateOf("") }
    var busy by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }

    fun run(label: String, action: suspend () -> StepResult, onOk: () -> Unit = {}) {
        if (busy != null) return
        busy = label
        error = null
        scope.launch {
            val result = action()
            busy = null
            if (result.ok) {
                Haptics.perform(view, Haptics.Kind.Success)
                onOk()
            } else {
                Haptics.perform(view, Haptics.Kind.Warning)
                error = result.error
            }
        }
    }

    Box(
        modifier = modifier
            .fillMaxSize()
            // iOS paints the whole page #141413, the welcome loop's own background, so the loop
            // sits on the page with no visible box (the theme's bgPrimary is lighter).
            .background(SignInPage)
            .systemBarsPadding()
            .imePadding(),
        contentAlignment = Alignment.TopCenter,
    ) {
        Column(
            modifier = Modifier
                .widthIn(max = 480.dp)
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp, vertical = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            LoginArrival(paused = step != Step.Options || busy != null)
            Text(
                text = "Your bots.\nYour workspace.",
                color = c.textPrimary,
                fontSize = 29.sp,
                fontWeight = FontWeight.SemiBold,
                letterSpacing = (-1).sp,
                lineHeight = 34.sp,
                textAlign = TextAlign.Center,
                modifier = Modifier
                    .fillMaxWidth()
                    .semantics { heading() }
                    .testTag("loginHeadline"),
            )
            Text(
                text = "Chat, build and work together.\nAll in one place.",
                color = c.textSecondary,
                fontSize = 13.sp,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth().padding(bottom = 16.dp),
            )

            when (step) {
                Step.Options -> {
                    // Google, GitHub, Apple: Google leads on Android the way Apple leads on iOS.
                    SocialButton("Continue with Google", R.drawable.logo_google, tint = false, busy == "google", enabled = busy == null) {
                        run("google", { AllternitAuth.signInWithOAuth(OAuthProvider.GOOGLE) })
                    }
                    SocialButton("Continue with GitHub", R.drawable.logo_github, tint = true, busy == "github", enabled = busy == null) {
                        run("github", { AllternitAuth.signInWithOAuth(OAuthProvider.GITHUB) })
                    }
                    SocialButton("Continue with Apple", R.drawable.logo_apple, tint = true, busy == "apple", enabled = busy == null) {
                        run("apple", { AllternitAuth.signInWithOAuth(OAuthProvider.APPLE) })
                    }
                    ErrorLine(error)
                    OutlinedButton(
                        onClick = { error = null; step = Step.Email },
                        enabled = busy == null,
                        shape = RoundedCornerShape(AllternitRadii.MD),
                        border = BorderStroke(1.dp, c.borderStrong),
                        colors = ButtonDefaults.outlinedButtonColors(contentColor = c.textPrimary),
                        modifier = Modifier.fillMaxWidth().padding(top = 4.dp).height(44.dp).testTag("loginContinueEmail"),
                    ) { Text("Continue with email", fontSize = 14.sp, fontWeight = FontWeight.SemiBold) }
                }

                Step.Email -> {
                    FieldLabel("Email")
                    AuthField(
                        value = email,
                        onValueChange = { email = it },
                        placeholder = "you@company.com",
                        keyboardType = KeyboardType.Email,
                        onDone = { run("email", { AllternitAuth.sendEmailCode(email) }) { code = ""; step = Step.Code } },
                    )
                    InkButton("Continue", busy == "email", enabled = busy == null && email.isNotBlank()) {
                        run("email", { AllternitAuth.sendEmailCode(email) }) { code = ""; step = Step.Code }
                    }
                    ErrorLine(error)
                    QuietButton("All sign-in options") {
                        AllternitAuth.cancelEmail()
                        error = null
                        step = Step.Options
                    }
                }

                Step.Code -> {
                    Text(
                        text = "We sent a 6-digit code to ${email.trim()}.",
                        color = c.textSecondary,
                        fontSize = 13.sp,
                    )
                    FieldLabel("Code")
                    AuthField(
                        value = code,
                        onValueChange = { code = it.filter(Char::isDigit).take(6) },
                        placeholder = "123456",
                        keyboardType = KeyboardType.NumberPassword,
                        onDone = { run("code", { AllternitAuth.verifyEmailCode(code) }) },
                    )
                    InkButton("Continue", busy == "code", enabled = busy == null && code.length == 6) {
                        run("code", { AllternitAuth.verifyEmailCode(code) })
                    }
                    ErrorLine(error)
                    QuietButton("Send a new code") {
                        run("email", { AllternitAuth.sendEmailCode(email) }) { code = "" }
                    }
                    QuietButton("Use a different email") {
                        AllternitAuth.cancelEmail()
                        error = null
                        step = Step.Email
                    }
                }
            }

            // "Secured by clerk" (Clerk's footer on the web; iOS securedBy).
            Text(
                text = buildAnnotatedString {
                    append("Secured by ")
                    pushStyle(SpanStyle(fontWeight = FontWeight.Bold))
                    append("clerk")
                    pop()
                },
                color = c.textSecondary,
                fontSize = 13.sp,
                textAlign = TextAlign.Center,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(top = 20.dp)
                    .semantics { contentDescription = "Secured by Clerk" },
            )

            if (onSkipAuth != null) {
                QuietButton("Continue without signing in", tag = "loginSkipAuth") {
                    Haptics.perform(view, Haptics.Kind.Tap)
                    onSkipAuth()
                }
            }

            ConsentLine()
        }
    }
}

/** iOS SignInSocialButton: the raised outline button with the provider's 18 dp logo. */
@Composable
private fun SocialButton(title: String, logo: Int, tint: Boolean, busy: Boolean, enabled: Boolean, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Button(
        onClick = onClick,
        enabled = enabled,
        shape = RoundedCornerShape(AllternitRadii.MD),
        border = BorderStroke(1.dp, c.borderStrong),
        colors = ButtonDefaults.buttonColors(
            containerColor = c.bgTertiary,
            contentColor = c.textPrimary,
            disabledContainerColor = c.bgTertiary,
            disabledContentColor = c.textTertiary,
        ),
        modifier = Modifier.fillMaxWidth().height(44.dp).semantics { contentDescription = title },
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            if (busy) {
                Spinner()
            } else {
                Image(
                    painter = painterResource(logo),
                    contentDescription = null,
                    colorFilter = if (tint) ColorFilter.tint(c.textPrimary) else null,
                    modifier = Modifier.size(18.dp),
                )
            }
            Text(title, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
        }
    }
}

@Composable
private fun InkButton(title: String, busy: Boolean, enabled: Boolean, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Button(
        onClick = onClick,
        enabled = enabled,
        shape = RoundedCornerShape(AllternitRadii.MD),
        colors = ButtonDefaults.buttonColors(
            containerColor = c.textPrimary,
            contentColor = c.bgPrimary,
            disabledContainerColor = c.textPrimary.copy(alpha = 0.4f),
            disabledContentColor = c.bgPrimary,
        ),
        modifier = Modifier.fillMaxWidth().height(44.dp),
    ) {
        if (busy) Spinner() else Text(title, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
    }
}

@Composable
private fun QuietButton(title: String, tag: String? = null, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    TextButton(
        onClick = onClick,
        modifier = Modifier.fillMaxWidth().let { if (tag != null) it.testTag(tag) else it },
    ) { Text(title, color = c.textSecondary, fontSize = 13.sp) }
}

@Composable
private fun Spinner() {
    CircularProgressIndicator(modifier = Modifier.size(18.dp), strokeWidth = 2.dp, color = AllternitTheme.colors.textPrimary)
}

@Composable
private fun FieldLabel(text: String) {
    Text(text, color = AllternitTheme.colors.textPrimary, fontSize = 12.5.sp, fontWeight = FontWeight.SemiBold)
}

@Composable
private fun AuthField(
    value: String,
    onValueChange: (String) -> Unit,
    placeholder: String,
    keyboardType: KeyboardType,
    onDone: () -> Unit,
) {
    val c = AllternitTheme.colors
    OutlinedTextField(
        value = value,
        onValueChange = onValueChange,
        singleLine = true,
        placeholder = { Text(placeholder, color = c.textTertiary, fontSize = 14.sp) },
        keyboardOptions = KeyboardOptions(keyboardType = keyboardType, imeAction = ImeAction.Done, autoCorrectEnabled = false),
        keyboardActions = KeyboardActions(onDone = { onDone() }),
        shape = RoundedCornerShape(AllternitRadii.MD),
        colors = OutlinedTextFieldDefaults.colors(
            focusedTextColor = c.textPrimary,
            unfocusedTextColor = c.textPrimary,
            focusedContainerColor = c.bgTertiary,
            unfocusedContainerColor = c.bgTertiary,
            focusedBorderColor = c.textPrimary,
            unfocusedBorderColor = c.borderStrong,
            cursorColor = c.textPrimary,
        ),
        modifier = Modifier.fillMaxWidth(),
    )
}

@Composable
private fun ErrorLine(error: String?) {
    if (error == null) return
    Text(
        text = error,
        color = com.allternit.core.design.AllternitStatic.StatusError,
        fontSize = 12.5.sp,
        textAlign = TextAlign.Center,
        modifier = Modifier.fillMaxWidth(),
    )
}

@Composable
private fun ConsentLine() {
    val c = AllternitTheme.colors
    val link = TextLinkStyles(SpanStyle(color = c.textPrimary))
    val text = buildAnnotatedString {
        append("By continuing you confirm you are 18 or older and agree to the ")
        withLink(LinkAnnotation.Url("https://allternit.com/terms", link)) { append("Terms") }
        append(" and ")
        withLink(LinkAnnotation.Url("https://allternit.com/privacy", link)) { append("Privacy Policy") }
        append(".")
    }
    Text(
        text = text,
        color = c.textTertiary,
        fontSize = 11.5.sp,
        lineHeight = 16.sp,
        textAlign = TextAlign.Center,
        modifier = Modifier.fillMaxWidth().padding(top = 18.dp),
    )
}
