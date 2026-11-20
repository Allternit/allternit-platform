package com.allternit.core.auth

import android.content.Context
import android.util.Log
import com.clerk.api.Clerk
import com.clerk.api.ClerkConfigurationOptions
import com.clerk.api.auth.types.VerificationType
import com.clerk.api.network.model.error.ClerkErrorResponse
import com.clerk.api.network.serialization.ClerkResult
import com.clerk.api.organizations.Organization
import com.clerk.api.session.Session
import com.clerk.api.session.SessionTaskKey
import com.clerk.api.signin.SignIn
import com.clerk.api.signin.sendCode
import com.clerk.api.signin.verifyCode
import com.clerk.api.signup.SignUp
import com.clerk.api.signup.sendCode
import com.clerk.api.signup.verifyCode
import com.clerk.api.sso.OAuthProvider
import com.clerk.api.user.User
import com.clerk.api.user.delete
import com.clerk.api.user.getOrganizationMemberships
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.launch

/** The signed-in person, as the native screens show them. */
data class Account(
    val userId: String,
    val firstName: String?,
    /** First name, else the email's local part, else "Account" (iOS `displayName`). */
    val displayName: String,
    val email: String?,
    val organizationId: String?,
)

/** Where sign-in stands. The native MainActivity switches on it. */
sealed interface AuthState {
    /** Clerk is still loading its environment and client (about 1 s on a cold start). */
    data object Starting : AuthState

    /** Clerk could not start (no network on first launch, bad key). [message] is plain text. */
    data class Unavailable(val message: String) : AuthState

    data object SignedOut : AuthState

    /** Signed in, but the session still has a task (choose organization) that [AllternitAuth] finishes. */
    data object Finishing : AuthState

    data class SignedIn(val account: Account) : AuthState
}

/** The outcome of one sign-in step: null [error] means it worked. */
data class StepResult(val error: String? = null, val cancelled: Boolean = false) {
    val ok: Boolean get() = error == null && !cancelled

    companion object {
        val OK = StepResult()

        /** The user closed the sign-in tab: not an error, the screen just goes back to idle (as on iOS). */
        val CANCELLED = StepResult(cancelled = true)
    }
}

/**
 * Native sign-in on the Clerk Android SDK (`com.clerk:clerk-android-api`), the twin of iOS
 * `Core/Auth/AuthManager.swift`. One per process: call [start] from the first activity's
 * onCreate, collect [state], and give [tokenProvider] to `ApiClients.okHttp(bearer = …)`.
 *
 * - Email: [sendEmailCode] then [verifyEmailCode]. An unknown email becomes a sign-up
 *   (iOS shows Clerk's sign-in-or-up sheet; Android has no Clerk UI module, so both paths are here).
 * - Google / GitHub: [signInWithOAuth] (Custom Tab, back through Clerk's SSOReceiverActivity,
 *   redirect `clerk://<applicationId>.callback`).
 * - The production instance requires an organization: a new account's session stays PENDING
 *   with the choose-organization task. [finishOrganizationTaskIfNeeded] uses the first
 *   membership or creates "<first name>'s Organization", exactly like iOS.
 */
object AllternitAuth {
    /** Public by design (same key as ios/project.yml), instance clerk.allternit.com. */
    const val PUBLISHABLE_KEY = "pk_live_Y2xlcmsuYWxsdGVybml0LmNvbSQ"

    /** Clerk Frontend API proxy (worker allternit-clerk-proxy), shared with ai.allternit.com. */
    const val PROXY_URL = "https://allternit.com/__clerk"

    private const val TAG = "AllternitAuth"
    private const val CHOOSE_ORG_FALLBACK_NAME = "My Organization"

    private val started = AtomicBoolean(false)
    private val finishingOrg = AtomicBoolean(false)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    private val _state = MutableStateFlow<AuthState>(AuthState.Starting)
    val state: StateFlow<AuthState> = _state.asStateFlow()

    /** Bearer for :core:api clients: the Clerk session JWT, cached until shortly before expiry. */
    val tokenProvider: ClerkTokenProvider = ClerkTokenProvider()

    private var pendingSignIn: SignIn? = null
    private var pendingSignUp: SignUp? = null

    /** Starts Clerk once per process. Safe to call from every activity's onCreate. */
    fun start(context: Context, publishableKey: String = PUBLISHABLE_KEY) {
        if (!started.compareAndSet(false, true)) return
        // Through the same proxy the web app uses (infrastructure/clerk-proxy). Google's OAuth
        // client only allows https://allternit.com/__clerk/v1/oauth_callback; talking to
        // clerk.allternit.com directly sends that host's callback and Google answers
        // redirect_uri_mismatch (seen on the Moto 2026-11-18).
        Clerk.initialize(
            context.applicationContext,
            publishableKey,
            ClerkConfigurationOptions(enableDebugMode = false, proxyUrl = PROXY_URL),
        )
        scope.launch {
            combine(Clerk.isInitialized, Clerk.initializationError, Clerk.sessionFlow, Clerk.userFlow) { ready, error, session, user ->
                derive(ready, error, session, user)
            }.collect { next ->
                val previous = _state.value
                _state.value = next
                if (next !is AuthState.SignedIn) tokenProvider.clear()
                if (next is AuthState.Finishing && previous !is AuthState.Finishing) {
                    scope.launch { finishOrganizationTaskIfNeeded() }
                }
            }
        }
    }

    /** "Try again" after [AuthState.Unavailable]: Clerk loads its environment again. */
    fun retry() {
        if (started.get()) Clerk.reinitialize()
    }

    private fun derive(ready: Boolean, error: Throwable?, session: Session?, user: User?): AuthState = when {
        !ready && error != null -> AuthState.Unavailable("Sign-in can't load right now. Check your connection and try again.")
        !ready -> AuthState.Starting
        session == null -> AuthState.SignedOut
        session.status == Session.SessionStatus.PENDING -> AuthState.Finishing
        session.status == Session.SessionStatus.ACTIVE -> AuthState.SignedIn(account(user ?: session.user, session))
        else -> AuthState.SignedOut
    }

    private fun account(user: User?, session: Session): Account {
        val email = user?.primaryEmailAddress?.emailAddress ?: user?.emailAddresses.orEmpty().firstOrNull()?.emailAddress
        val first = user?.firstName?.trim()?.takeIf { it.isNotEmpty() }
        val display = first ?: email?.substringBefore('@')?.takeIf { it.isNotEmpty() } ?: "Account"
        return Account(
            userId = user?.id.orEmpty(),
            firstName = first,
            displayName = display,
            email = email,
            organizationId = session.lastActiveOrganizationId,
        )
    }

    // region Email code

    /** Sends a one-time code to [email]. Signs in a known address; signs up a new one. */
    suspend fun sendEmailCode(email: String): StepResult {
        val address = email.trim()
        if (!address.contains('@')) return StepResult("Enter your email address.")
        pendingSignIn = null
        pendingSignUp = null
        when (val created = Clerk.auth.signIn { this.email = address }) {
            is ClerkResult.Success -> return when (val sent = created.value.sendCode { this.email = address }) {
                is ClerkResult.Success -> { pendingSignIn = sent.value; StepResult.OK }
                is ClerkResult.Failure -> StepResult(sent.message())
            }
            is ClerkResult.Failure -> if (created.errorCode() != "form_identifier_not_found") return StepResult(created.message())
        }
        // New address: sign up. The sign-in screen shows the 18+ / Terms / Privacy consent
        // line under the buttons (same copy as iOS), so legal acceptance is recorded here.
        return when (val up = Clerk.auth.signUp { this.email = address; legalAccepted = true }) {
            is ClerkResult.Success -> when (val sent = up.value.sendCode { this.email = address }) {
                is ClerkResult.Success -> { pendingSignUp = sent.value; StepResult.OK }
                is ClerkResult.Failure -> StepResult(sent.message())
            }
            is ClerkResult.Failure -> StepResult(up.message())
        }
    }

    /** Checks the emailed code. On success the session appears and [state] moves on by itself. */
    suspend fun verifyEmailCode(code: String): StepResult {
        val digits = code.filter { it.isDigit() }
        if (digits.length < 6) return StepResult("Enter the 6-digit code from the email.")
        pendingSignIn?.let { signIn ->
            return when (val r = signIn.verifyCode(digits)) {
                is ClerkResult.Success -> if (r.value.status == SignIn.Status.COMPLETE) {
                    pendingSignIn = null
                    StepResult.OK
                } else {
                    StepResult("This account needs another sign-in step. Use Google or GitHub, or sign in on the web.")
                }
                is ClerkResult.Failure -> StepResult(r.message())
            }
        }
        pendingSignUp?.let { signUp ->
            return when (val r = signUp.verifyCode(digits, VerificationType.EMAIL)) {
                is ClerkResult.Success -> if (r.value.status == SignUp.Status.COMPLETE) {
                    pendingSignUp = null
                    StepResult.OK
                } else {
                    StepResult("Your account needs a few more details. Finish creating it on allternit.com, then sign in here.")
                }
                is ClerkResult.Failure -> StepResult(r.message())
            }
        }
        return StepResult("Send a new code first.")
    }

    /** Back from the code step to the email field. */
    fun cancelEmail() {
        pendingSignIn = null
        pendingSignUp = null
    }

    // endregion

    /**
     * Google, GitHub or Apple through Clerk OAuth in a Custom Tab (signs up a new account too).
     * Closing the tab comes back as [StepResult.CANCELLED], never as a connection error.
     */
    suspend fun signInWithOAuth(provider: OAuthProvider): StepResult =
        when (val r = Clerk.auth.signInWithOAuth(provider)) {
            is ClerkResult.Success -> StepResult.OK
            is ClerkResult.Failure -> if (r.isCancelled()) StepResult.CANCELLED else StepResult(r.message())
        }

    /**
     * Sign-in token (`sign_in_tokens` from the Clerk backend). Only the native app's debug
     * build calls this, for Maestro and emulator checks (intent extra `ticket`).
     */
    suspend fun signInWithTicket(ticket: String): StepResult =
        when (val r = Clerk.auth.signInWithTicket(ticket)) {
            is ClerkResult.Success -> StepResult.OK
            is ClerkResult.Failure -> StepResult(r.message())
        }

    /**
     * iOS `finishOrganizationTaskIfNeeded`: a PENDING session with the choose-organization
     * task gets the first membership, or a new "<first name>'s Organization", set active.
     */
    suspend fun finishOrganizationTaskIfNeeded() {
        val session = Clerk.session ?: return
        if (session.status != Session.SessionStatus.PENDING) return
        val choose = session.tasks.orEmpty().any { SessionTaskKey.fromRaw(it.key) == SessionTaskKey.CHOOSE_ORGANIZATION }
        if (!choose || !finishingOrg.compareAndSet(false, true)) return
        try {
            val user = Clerk.user ?: session.user
            var orgId: String? = user?.organizationMemberships.orEmpty().firstOrNull()?.organization?.id
            if (orgId == null && user != null) {
                val memberships = user.getOrganizationMemberships()
                if (memberships is ClerkResult.Success) orgId = memberships.value.data.firstOrNull()?.organization?.id
            }
            if (orgId == null) {
                val first = user?.firstName?.trim().orEmpty()
                val name = if (first.isEmpty()) CHOOSE_ORG_FALLBACK_NAME else "$first's Organization"
                when (val created = Organization.create(name, null)) {
                    is ClerkResult.Success -> orgId = created.value.id
                    is ClerkResult.Failure -> {
                        Log.w(TAG, "Creating the organization failed: ${created.message()}")
                        return
                    }
                }
            }
            val active = Clerk.auth.setActive(session.id, orgId)
            if (active is ClerkResult.Failure) Log.w(TAG, "Activating the organization failed: ${active.message()}")
        } finally {
            finishingOrg.set(false)
        }
    }

    /**
     * Deletes the signed-in person's Clerk account (iOS AuthManager.deleteAccount: `user.delete()`).
     * Only the Delete account screen calls this, after the person typed DELETE. The caller signs out
     * afterwards (iOS finishAccountDeletion).
     */
    suspend fun deleteAccount(): StepResult {
        val user = Clerk.user ?: return StepResult("You're not signed in.")
        return when (val r = user.delete()) {
            is ClerkResult.Success -> {
                tokenProvider.clear()
                StepResult.OK
            }
            is ClerkResult.Failure -> StepResult(r.message())
        }
    }

    /** Work that must run before the session is dropped (wave I1: push removes this phone's token). */
    private val signOutHooks = java.util.concurrent.CopyOnWriteArrayList<suspend () -> Unit>()

    /**
     * Registers [hook] to run at the start of [signOut], while the leaving account's token still works
     * (iOS PushNotificationManager.willSignOut runs before the session goes). A failing hook never blocks sign-out.
     */
    fun addSignOutHook(hook: suspend () -> Unit) {
        signOutHooks += hook
    }

    /** One organization the person belongs to (iOS SettingsView memberships). */
    data class OrgMembership(val id: String, val name: String)

    /** The person's organizations: the cached list first, then a refresh (a failed refresh keeps the cached list). */
    suspend fun organizations(): List<OrgMembership> {
        val user = Clerk.user ?: return emptyList()
        var list = user.organizationMemberships.orEmpty().mapNotNull { m -> m.organization?.let { OrgMembership(it.id, it.name) } }
        val fresh = user.getOrganizationMemberships()
        if (fresh is ClerkResult.Success) {
            list = fresh.value.data.mapNotNull { m -> m.organization?.let { OrgMembership(it.id, it.name) } }
        }
        return list
    }

    /** iOS AccountSwitch.activate: `setActive` on the current session with the picked organization. */
    suspend fun switchOrganization(orgId: String): StepResult {
        val session = Clerk.session ?: return StepResult("You're signed out.")
        tokenProvider.clear()
        return when (val r = Clerk.auth.setActive(session.id, orgId)) {
            is ClerkResult.Success -> StepResult.OK
            is ClerkResult.Failure -> StepResult(r.message())
        }
    }

    /** Signs out of Clerk on this device and forgets the cached token. */
    suspend fun signOut(): StepResult {
        for (hook in signOutHooks) runCatching { hook() }
        tokenProvider.clear()
        cancelEmail()
        return when (val r = Clerk.auth.signOut()) {
            is ClerkResult.Success -> StepResult.OK
            is ClerkResult.Failure -> StepResult(r.message())
        }
    }
}

private fun ClerkResult.Failure<ClerkErrorResponse>.errorCode(): String? = error?.errors?.firstOrNull()?.code

/**
 * Clerk ends the OAuth flow with `com.clerk.api.sso.SSOCancellationException` ("Authentication
 * cancelled") when the user closes the Custom Tab or presses back. Matched by name, because the
 * class isn't part of Clerk's documented API; a cancelled coroutine counts too.
 */
private fun ClerkResult.Failure<ClerkErrorResponse>.isCancelled(): Boolean =
    generateSequence(throwable) { it.cause }.take(8).any {
        it.javaClass.simpleName == "SSOCancellationException" || it is kotlin.coroutines.cancellation.CancellationException
    }

/** Clerk's own sentence when it has one (it is written for people), else a plain fallback. */
private fun ClerkResult.Failure<ClerkErrorResponse>.message(): String {
    val first = error?.errors?.firstOrNull()
    return first?.longMessage?.takeIf { it.isNotBlank() }
        ?: first?.message?.takeIf { it.isNotBlank() }
        ?: if (throwable != null) "Couldn't reach sign-in. Check your connection and try again." else "Sign-in didn't work. Try again."
}
