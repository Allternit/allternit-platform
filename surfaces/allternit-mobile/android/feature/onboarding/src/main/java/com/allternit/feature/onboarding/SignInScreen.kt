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
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.autofill.ContentType
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
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
import androidx.compose.ui.semantics.contentType
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
import com.allternit.core.design.Haptics
import com.clerk.api.sso.OAuthProvider
import kotlinx.coroutines.launch

/** Which part of the sign-in rail is open: the buttons with the email field, or the emailed code. */
private enum class Step { Options, Code }

/**
 * The sign-in palette, the same values as iOS `SignInPalette(.dark)` (SignInStageView.swift) and
 * the web stage: warm cream ink on #141413, not the app theme's neutral greys.
 */
private object SignInColors {
    val Ink = Color(0xFFF0EEE6)
    val Sub = Color(0xFFA1A1AA)
    val Mut = Color(0xFF8A8A93)
    val Line = Color(0xFF2E2E33)
    val Raise = Color(0xFF1C1C1E)
    val Page = Color(0xFF141413)
}

private val SignInPage = SignInColors.Page

/**
 * Native sign-in, laid out like the phone sign-in on m.allternit.com (allternit-ai
 * SignInRail, `.ast-rail-compact`) and iOS `LoginGateView`: the offline wordmark/Gizzi/cast loop
 * ([LoginArrival]), headline, the cream "Continue with Apple" button, GitHub and Google with
 * their logos (all through Clerk OAuth in a Custom Tab), an "or" line, the Email field with a
 * cream Continue that sends a 6-digit code, "Secured by clerk" and the 18+ / Terms / Privacy line.
 * Like iOS and the web it is always dark, with its own palette ([SignInColors]).
 *
 * Test ids (maestro/ids.md): `loginHeadline`, `loginContinueEmail`, `loginSkipAuth`.
 * [onSkipAuth] is non-null only in debug builds, as on iOS.
 */
@Composable
fun SignInScreen(
    modifier: Modifier = Modifier,
    onSkipAuth: (() -> Unit)? = null,
) {
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
            if (result.cancelled) return@launch
            if (result.ok) {
                Haptics.perform(view, Haptics.Kind.Success)
                onOk()
            } else {
                Haptics.perform(view, Haptics.Kind.Warning)
                error = result.error
            }
        }
    }

    fun sendCode() {
        if (email.isBlank()) {
            Haptics.perform(view, Haptics.Kind.Warning)
            error = "Enter your email address."
            return
        }
        run("email", { AllternitAuth.sendEmailCode(email) }) { code = ""; step = Step.Code }
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
                color = SignInColors.Ink,
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
                color = SignInColors.Sub,
                fontSize = 13.sp,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth().padding(bottom = 16.dp),
            )

            when (step) {
                Step.Options -> {
                    // Same order as m.allternit.com: Apple (cream), GitHub, Google.
                    SocialButton("Continue with Apple", R.drawable.logo_apple, tint = true, busy == "apple", enabled = busy == null, filled = true) {
                        run("apple", { AllternitAuth.signInWithOAuth(OAuthProvider.APPLE) })
                    }
                    SocialButton("Continue with GitHub", R.drawable.logo_github, tint = true, busy == "github", enabled = busy == null) {
                        run("github", { AllternitAuth.signInWithOAuth(OAuthProvider.GITHUB) })
                    }
                    SocialButton("Continue with Google", R.drawable.logo_google, tint = false, busy == "google", enabled = busy == null) {
                        run("google", { AllternitAuth.signInWithOAuth(OAuthProvider.GOOGLE) })
                    }
                    OrDivider()
                    FieldLabel("Email")
                    AuthField(
                        value = email,
                        onValueChange = { email = it },
                        placeholder = "you@company.com",
                        keyboardType = KeyboardType.Email,
                        contentType = ContentType.EmailAddress,
                        // Flows tap this id to start email sign-in (maestro/ids.md).
                        tag = "loginContinueEmail",
                        onDone = { sendCode() },
                    )
                    // Always cream like the web's Continue; an empty field says so instead of greying out.
                    InkButton("Continue", busy == "email", enabled = busy == null) { sendCode() }
                    ErrorLine(error)
                }

                Step.Code -> {
                    Text(
                        text = "We sent a 6-digit code to ${email.trim()}.",
                        color = SignInColors.Sub,
                        fontSize = 13.sp,
                    )
                    FieldLabel("Code")
                    AuthField(
                        value = code,
                        onValueChange = { code = it.filter(Char::isDigit).take(6) },
                        placeholder = "123456",
                        // Number, not NumberPassword: a password field makes Google Password
                        // Manager offer to save the code as the account's password (Moto run).
                        keyboardType = KeyboardType.Number,
                        contentType = ContentType.SmsOtpCode,
                        autoFocus = true,
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
                        step = Step.Options
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
                color = SignInColors.Sub,
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
private fun SocialButton(
    title: String,
    logo: Int,
    tint: Boolean,
    busy: Boolean,
    enabled: Boolean,
    filled: Boolean = false,
    onClick: () -> Unit,
) {
    // `filled` is the web's cream Apple button: ink background, page-colored label and logo.
    val container = if (filled) SignInColors.Ink else SignInColors.Raise
    val content = if (filled) SignInPage else SignInColors.Ink
    Button(
        onClick = onClick,
        enabled = enabled,
        shape = RoundedCornerShape(AllternitRadii.MD),
        border = BorderStroke(1.dp, if (filled) SignInColors.Ink else SignInColors.Line),
        colors = ButtonDefaults.buttonColors(
            containerColor = container,
            contentColor = content,
            disabledContainerColor = container,
            disabledContentColor = content.copy(alpha = 0.6f),
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
                    colorFilter = if (tint) ColorFilter.tint(content) else null,
                    modifier = Modifier.size(18.dp),
                )
            }
            Text(title, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
        }
    }
}

@Composable
private fun InkButton(title: String, busy: Boolean, enabled: Boolean, onClick: () -> Unit) {
    Button(
        onClick = onClick,
        enabled = enabled,
        shape = RoundedCornerShape(AllternitRadii.MD),
        colors = ButtonDefaults.buttonColors(
            containerColor = SignInColors.Ink,
            contentColor = SignInColors.Page,
            disabledContainerColor = SignInColors.Ink.copy(alpha = 0.4f),
            disabledContentColor = SignInColors.Page,
        ),
        modifier = Modifier.fillMaxWidth().height(44.dp),
    ) {
        if (busy) Spinner() else Text(title, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
    }
}

/** "—— or ——" between the provider buttons and the Email field (web `.ast-or`). */
@Composable
private fun OrDivider() {
    Row(
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
    ) {
        Box(Modifier.weight(1f).height(1.dp).background(SignInColors.Line))
        Text("or", color = SignInColors.Mut, fontSize = 11.5.sp)
        Box(Modifier.weight(1f).height(1.dp).background(SignInColors.Line))
    }
}

@Composable
private fun QuietButton(title: String, tag: String? = null, onClick: () -> Unit) {
    TextButton(
        onClick = onClick,
        modifier = Modifier.fillMaxWidth().let { if (tag != null) it.testTag(tag) else it },
    ) { Text(title, color = SignInColors.Sub, fontSize = 13.sp) }
}

@Composable
private fun Spinner() {
    CircularProgressIndicator(modifier = Modifier.size(18.dp), strokeWidth = 2.dp, color = SignInColors.Ink)
}

@Composable
private fun FieldLabel(text: String) {
    Text(text, color = SignInColors.Ink, fontSize = 12.5.sp, fontWeight = FontWeight.SemiBold)
}

@Composable
private fun AuthField(
    value: String,
    onValueChange: (String) -> Unit,
    placeholder: String,
    keyboardType: KeyboardType,
    contentType: ContentType,
    tag: String? = null,
    autoFocus: Boolean = false,
    onDone: () -> Unit,
) {
    // The code field takes focus when its step opens, so the keyboard is up at once.
    val focus = remember { FocusRequester() }
    if (autoFocus) LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
    OutlinedTextField(
        value = value,
        onValueChange = onValueChange,
        singleLine = true,
        placeholder = { Text(placeholder, color = SignInColors.Mut, fontSize = 14.sp) },
        keyboardOptions = KeyboardOptions(keyboardType = keyboardType, imeAction = ImeAction.Done, autoCorrectEnabled = false),
        keyboardActions = KeyboardActions(onDone = { onDone() }),
        shape = RoundedCornerShape(AllternitRadii.MD),
        colors = OutlinedTextFieldDefaults.colors(
            focusedTextColor = SignInColors.Ink,
            unfocusedTextColor = SignInColors.Ink,
            focusedContainerColor = SignInColors.Raise,
            unfocusedContainerColor = SignInColors.Raise,
            focusedBorderColor = SignInColors.Ink,
            unfocusedBorderColor = SignInColors.Line,
            cursorColor = SignInColors.Ink,
        ),
        modifier = Modifier
            .fillMaxWidth()
            .focusRequester(focus)
            .semantics { this.contentType = contentType }
            .let { if (tag != null) it.testTag(tag) else it },
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
    val link = TextLinkStyles(SpanStyle(color = SignInColors.Ink))
    val text = buildAnnotatedString {
        append("By continuing you confirm you are 18 or older and agree to the ")
        withLink(LinkAnnotation.Url("https://allternit.com/terms", link)) { append("Terms") }
        append(" and ")
        withLink(LinkAnnotation.Url("https://allternit.com/privacy", link)) { append("Privacy Policy") }
        append(".")
    }
    Text(
        text = text,
        color = SignInColors.Mut,
        fontSize = 11.5.sp,
        lineHeight = 16.sp,
        textAlign = TextAlign.Center,
        modifier = Modifier.fillMaxWidth().padding(top = 18.dp),
    )
}
