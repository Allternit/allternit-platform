import { useEffect, useState } from 'react';
import {
  ClerkProvider,
  SignIn,
  SignUp,
  TaskChooseOrganization,
  useAuth,
  useClerk,
  useSignIn,
  useUser,
} from '@clerk/clerk-react';
import { Wordmark } from './Wordmark.js';
import { loadClerkConfig } from './clerk-config.js';

function AuthFlow() {
  // Local state only — Clerk rewrites the location hash for its own routing,
  // so deriving the mode from the hash would fight that and snap back.
  const [isSignUp, setIsSignUp] = useState(false);
  const { isSignedIn } = useAuth();
  const { session } = useClerk();
  const appearance = buildAppearance(useIsDark());

  const toggle = () => setIsSignUp((current) => !current);

  if (isSignedIn) {
    return <SignedInView />;
  }

  // Organizations are on: a session with no active organization is pending
  // (no token). TokenBridge picks the user's organization; with none, choose
  // or create one here instead of a blank window.
  if (session?.status === 'pending') {
    return <TaskChooseOrganization redirectUrlComplete={selfRedirectUrl} appearance={appearance} />;
  }

  return (
    <>
      {isSignUp ? (
        <SignUp
          appearance={appearance}
          routing="hash"
        />
      ) : (
        <SignIn
          appearance={appearance}
          routing="hash"
        />
      )}
      <div
        style={{
          marginTop: '18px',
          textAlign: 'center',
          fontSize: '13px',
          color: 'var(--auth-text-2)',
          WebkitAppRegion: 'no-drag',
        }}
      >
        {isSignUp ? 'Already have an account? ' : "Don't have an account? "}
        <button
          type="button"
          onClick={toggle}
          style={{
            background: 'none',
            border: 'none',
            padding: 0,
            color: 'var(--auth-ink)',
            cursor: 'pointer',
            fontSize: 'inherit',
            fontFamily: 'inherit',
            textDecoration: 'underline',
            WebkitAppRegion: 'no-drag',
          }}
        >
          {isSignUp ? 'Sign in' : 'Sign up'}
        </button>
      </div>
    </>
  );
}

function SignedInView() {
  return (
    <div
      style={{
        textAlign: 'center',
        padding: '40px 0',
        color: 'var(--auth-ink)',
      }}
    >
      <div
        style={{
          width: '48px',
          height: '48px',
          borderRadius: '50%',
          background: 'var(--auth-ink)',
          color: 'var(--auth-on-ink)',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          margin: '0 auto 20px',
          fontSize: '24px',
        }}
      >
        ✓
      </div>
      <h2
        style={{
          fontSize: '15px',
          fontWeight: 600,
          margin: '0 0 8px',
        }}
      >
        Signed in
      </h2>
      <p style={{ fontSize: '13px', color: 'var(--auth-text-2)', margin: 0 }}>
        Completing runtime pairing…
      </p>
    </div>
  );
}

/**
 * Stay on the intercepted auth renderer after password success. Without this,
 * Clerk navigates to the Account Portal root (`https://accounts…/`), we block
 * it, and `<SignIn>` remounts as signed-out even though FAPI already created
 * the session. Hash routing on `<SignIn>` is not enough — ClerkProvider still
 * needs an in-app redirect target.
 * URL of the auth page itself (https://accounts.<instance>/__desktop_auth__/). The
 * top-level ClerkProvider uses this as the fallback redirect target so Clerk never
 * navigates the isolated auth window away to the platform website. The embedded
 * <SignIn>/<SignUp> components use hash routing and do NOT set forceRedirectUrl,
 * because that caused a redirect loop: after sign-in Clerk would reload the page, the
 * component would remount, and immediately redirect again.
 */
const selfRedirectUrl =
  typeof window !== 'undefined' ? window.location.origin + window.location.pathname : '/';

/**
 * The instance's application name does not reach the embedded component, which
 * leaves the default title rendering as "Sign in to ". Pin the titles instead.
 */
const clerkLocalization = {
  signIn: {
    start: {
      title: 'Sign in to Allternit',
    },
  },
  signUp: {
    start: {
      title: 'Create your Allternit account',
    },
  },
} as const;

const LIGHT_VARS = {
  colorBackground: '#FFFFFF',
  colorPrimary: '#1F1E1D',
  colorText: '#1F1E1D',
  colorTextSecondary: '#6B6A68',
  colorTextOnPrimaryBackground: '#FFFFFF',
  colorInputBackground: '#FFFFFF',
  colorInputText: '#1F1E1D',
  colorNeutral: '#1F1E1D',
  colorDanger: '#B42318',
  colorShimmer: '#F4F4F3',
};
const DARK_VARS = {
  colorBackground: '#232322',
  colorPrimary: '#ECECEB',
  colorText: '#ECECEB',
  colorTextSecondary: '#A3A29F',
  colorTextOnPrimaryBackground: '#1C1C1B',
  colorInputBackground: '#2C2C2B',
  colorInputText: '#ECECEB',
  colorNeutral: '#ECECEB',
  colorDanger: '#F97066',
  colorShimmer: '#2C2C2B',
};

function useIsDark(): boolean {
  const [dark, setDark] = useState(
    () => typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: dark)').matches,
  );
  useEffect(() => {
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const on = (e: MediaQueryListEvent) => setDark(e.matches);
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  }, []);
  return dark;
}

// Colours come from the CSS tokens in auth.css, so light and dark both follow
// prefers-color-scheme. Clerk's own variables are swapped per scheme above.
function buildAppearance(dark: boolean) {
  return {
    variables: {
      ...(dark ? DARK_VARS : LIGHT_VARS),
      borderRadius: '12px',
      fontFamily: 'inherit',
      fontSize: '13px',
    },
    elements: {
      cardBox: { background: 'transparent', border: 'none', boxShadow: 'none', width: '100%', maxWidth: '100%' },
      rootBox: { width: '100%', maxWidth: '100%', boxSizing: 'border-box' },
      card: {
        background: 'transparent',
        border: 'none',
        boxShadow: 'none',
        width: '100%',
        maxWidth: '100%',
        boxSizing: 'border-box',
        padding: '0',
      },
      // The instance logo (a tan Gizzi mascot) is set in the Clerk dashboard; the wordmark above replaces it.
      logoBox: { display: 'none' },
      logoImage: { display: 'none' },
      headerTitle: { color: 'var(--auth-ink)', fontSize: '15px', fontWeight: '600', letterSpacing: '0' },
      headerSubtitle: { color: 'var(--auth-text-2)', fontSize: '13px' },
      socialButtonsBlockButton: {
        background: 'var(--auth-input-bg)',
        border: '1px solid var(--auth-border)',
        color: 'var(--auth-ink)',
        boxShadow: 'none',
      },
      socialButtonsBlockButtonText: { color: 'var(--auth-ink)', fontWeight: '500' },
      dividerLine: { background: 'var(--auth-border)' },
      dividerText: { color: 'var(--auth-text-3)' },
      formFieldLabel: { color: 'var(--auth-ink)', fontSize: '13px' },
      formFieldInput: {
        background: 'var(--auth-input-bg)',
        border: '1px solid var(--auth-border-strong)',
        color: 'var(--auth-ink)',
        boxShadow: 'none',
      },
      formFieldErrorText: { color: 'var(--auth-danger)' },
      footerActionText: { display: 'none' },
      footerActionLink: { display: 'none' },
      footer: { background: 'transparent', padding: '20px 0 0', margin: '0' },
      footerAction: { display: 'none' },
      form: { gap: '14px' },
      main: { gap: '18px' },
      formButtonPrimary: {
        background: 'var(--auth-ink)',
        color: 'var(--auth-on-ink)',
        boxShadow: 'none',
        fontWeight: '500',
        fontSize: '13px',
        borderRadius: '9999px',
      },
      identityPreviewText: { color: 'var(--auth-ink)' },
      identityPreviewEditButton: { color: 'var(--auth-ink)' },
      formResendCodeLink: { color: 'var(--auth-ink)' },
      otpCodeFieldInput: {
        background: 'var(--auth-input-bg)',
        border: '1px solid var(--auth-border-strong)',
        color: 'var(--auth-ink)',
      },
      alertText: { color: 'var(--auth-ink)' },
      alertClerkError: {
        background: 'var(--auth-danger-bg)',
        border: '1px solid var(--auth-danger-border)',
      },
    },
  } as const;
}

/**
 * DEBUG-only seeded auto-login: if VITE_CLERK_SEED_EMAIL/PASSWORD are set,
 * sign in automatically so the auth window doesn't block dev/test runs.
 */
function SeedAuth() {
  const clerk = useClerk();
  const { isLoaded, isSignedIn } = clerk;
  const { user } = useUser();
  const { signIn, setActive } = useSignIn();

  useEffect(() => {
    if (!import.meta.env.DEV) return;
    if (!isLoaded || isSignedIn) return;
    if (!signIn || !setActive) return;
    const email = import.meta.env.VITE_CLERK_SEED_EMAIL as string | undefined;
    const password = import.meta.env.VITE_CLERK_SEED_PASSWORD as string | undefined;
    if (!email || !password) return;

    let active = true;
    const run = async () => {
      try {
        const result = await signIn.create({ identifier: email, password });
        if (!active) return;
        if (result.status === 'complete' && result.createdSessionId) {
          // Organizations are enabled on this Clerk instance. A seeded account
          // with no memberships ends up in a pending session; pick an existing
          // membership or create a personal seed org so the session is active.
          let orgId: string | undefined = user?.organizationMemberships?.[0]?.organization.id;
          if (!orgId) {
            try {
              const org = await clerk.createOrganization({ name: 'Allternit Seed' });
              orgId = org.id;
            } catch (orgErr) {
              console.warn('[SeedAuth] Failed to create seed organization:', orgErr);
            }
          }
          await setActive({ session: result.createdSessionId, organization: orgId });
        } else {
          console.warn('[SeedAuth] Clerk sign-in requires extra steps:', result.status);
        }
      } catch (err) {
        console.error('[SeedAuth] Auto sign-in failed:', err);
      }
    };
    void run();
    return () => {
      active = false;
    };
  }, [clerk, isLoaded, isSignedIn, signIn, setActive, user]);

  return null;
}

/**
 * Sends the Clerk session token to the Electron main process once the user is
 * signed in. The main process then completes the runtime pairing exchange.
 */
function TokenBridge() {
  const { isSignedIn, getToken } = useAuth();
  const clerk = useClerk();
  const { user } = useUser();
  const [reported, setReported] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (isSignedIn || !clerk.loaded) return;
    const sessionId =
      clerk.session?.id ??
      clerk.client?.activeSessions?.[0]?.id ??
      clerk.client?.sessions?.[0]?.id;
    if (!sessionId) return;
    // Re-activating without an organization leaves the session pending, so
    // the hidden refresh timed out every 15 minutes. Use the user's own.
    const session = clerk.client?.sessions?.find((s) => s.id === sessionId);
    const organization =
      session?.lastActiveOrganizationId ?? session?.user?.organizationMemberships?.[0]?.organization.id;
    void clerk.setActive({ session: sessionId, ...(organization ? { organization } : {}) }).catch((err) => {
      console.warn('[TokenBridge] Failed to activate existing Clerk session:', err);
    });
  }, [clerk, isSignedIn]);

  useEffect(() => {
    if (!isSignedIn || !getToken || reported) return;

    let active = true;
    void (async () => {
      try {
        const token = await getToken();
        if (!active || !token) return;
        const email =
          user?.primaryEmailAddress?.emailAddress ??
          user?.emailAddresses?.[0]?.emailAddress ??
          '';
        setReported(true);
        await window.allternitAuth?.onClerkToken?.({ token, userId: user?.id ?? '', email });
      } catch (err) {
        const message = err instanceof Error ? err.message : 'Clerk session error';
        setError(message);
        await window.allternitAuth?.onClerkError?.(message);
      }
    })();

    return () => {
      active = false;
    };
  }, [isSignedIn, getToken, user, reported]);

  if (!error) return null;

  return (
    <div
      style={{
        marginTop: '16px',
        padding: '12px 14px',
        borderRadius: '12px',
        background: 'var(--auth-danger-bg)',
        border: '1px solid var(--auth-danger-border)',
        color: 'var(--auth-danger)',
        fontSize: '13px',
        textAlign: 'center',
      }}
    >
      {error}
    </div>
  );
}

export default function AuthApp() {
  const appearance = buildAppearance(useIsDark());
  const [config, setConfig] = useState<Awaited<ReturnType<typeof loadClerkConfig>> | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    loadClerkConfig()
      .then(setConfig)
      .catch((err) => setError(err instanceof Error ? err.message : String(err)));
  }, []);

  if (error) {
    return (
      <div
        style={{
          width: '100%',
          height: '100%',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          padding: '40px',
          textAlign: 'center',
          color: 'var(--auth-danger)',
        }}
      >
        {error}
      </div>
    );
  }

  if (!config) {
    return (
      <div
        style={{
          width: '100%',
          height: '100%',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
        }}
      >
        <div
          style={{
            width: '28px',
            height: '28px',
            border: '2px solid var(--auth-border)',
            borderTopColor: 'var(--auth-ink)',
            borderRadius: '50%',
            animation: 'spin 0.8s linear infinite',
          }}
        />
        <style>{`@keyframes spin { to { transform: rotate(360deg); } }`}</style>
      </div>
    );
  }

  return (
    <ClerkProvider
      publishableKey={config.publishableKey}
      proxyUrl={config.proxyUrl}
      appearance={appearance}
      localization={clerkLocalization}
      signInFallbackRedirectUrl={selfRedirectUrl}
      signUpFallbackRedirectUrl={selfRedirectUrl}
    >
      <div
        style={{
          position: 'fixed',
          top: 0,
          left: 0,
          right: 0,
          height: 52,
          WebkitAppRegion: 'drag',
        }}
      />
      <div
        style={{
          width: '100%',
          height: '100%',
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          justifyContent: 'flex-start',
          padding: '52px 32px 24px',
          WebkitAppRegion: 'no-drag',
          overflow: 'auto',
        }}
      >
        <div style={{ width: '100%', maxWidth: '380px', WebkitAppRegion: 'no-drag' }}>
          <div style={{ display: 'flex', justifyContent: 'center', marginBottom: '28px' }}>
            <Wordmark width={168} />
          </div>
          <AuthFlow />
          <TokenBridge />
        </div>
      </div>
      <SeedAuth />
    </ClerkProvider>
  );
}
