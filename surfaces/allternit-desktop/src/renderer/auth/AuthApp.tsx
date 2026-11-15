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
import { loadClerkConfig } from './clerk-config.js';

function AuthFlow() {
  // Local state only — Clerk rewrites the location hash for its own routing,
  // so deriving the mode from the hash would fight that and snap back.
  const [isSignUp, setIsSignUp] = useState(false);
  const { isSignedIn } = useAuth();
  const { session } = useClerk();

  const toggle = () => setIsSignUp((current) => !current);

  if (isSignedIn) {
    return <SignedInView />;
  }

  // Organizations are on: a session with no active organization is pending
  // (no token). TokenBridge picks the user's organization; with none, choose
  // or create one here instead of a blank window.
  if (session?.status === 'pending') {
    return <TaskChooseOrganization redirectUrlComplete={selfRedirectUrl} appearance={clerkAppearance} />;
  }

  return (
    <>
      {isSignUp ? (
        <SignUp
          appearance={clerkAppearance}
          routing="hash"
        />
      ) : (
        <SignIn
          appearance={clerkAppearance}
          routing="hash"
        />
      )}
      <div
        style={{
          marginTop: '18px',
          textAlign: 'center',
          fontSize: '14px',
          color: '#74716B',
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
            color: '#9A7658',
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
        color: '#0D0C0A',
      }}
    >
      <div
        style={{
          width: '48px',
          height: '48px',
          borderRadius: '50%',
          background: '#1A1916',
          color: '#FAF9F7',
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
          fontSize: '22px',
          fontWeight: 700,
          margin: '0 0 8px',
        }}
      >
        Signed in
      </h2>
      <p style={{ fontSize: '14px', color: '#74716B', margin: 0 }}>
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

const clerkAppearance = {
  variables: {
    colorBackground: '#FFFEFC',
    colorPrimary: '#1A1916',
    colorText: '#0D0C0A',
    colorTextSecondary: '#74716B',
    colorInputBackground: '#FFFFFF',
    colorInputText: '#0D0C0A',
    colorNeutral: '#74716B',
    colorDanger: '#f87171',
    borderRadius: '16px',
    fontFamily: 'inherit',
  },
  elements: {
    cardBox: {
      background: 'transparent',
      border: 'none',
      boxShadow: 'none',
      width: '100%',
      maxWidth: '100%',
    },
    rootBox: {
      width: '100%',
      maxWidth: '100%',
      boxSizing: 'border-box',
    },
    card: {
      background: 'transparent',
      border: 'none',
      boxShadow: 'none',
      width: '100%',
      maxWidth: '100%',
      boxSizing: 'border-box',
      padding: '0',
    },
    headerTitle: {
      color: '#0D0C0A',
      fontSize: '28px',
      fontWeight: '700',
      letterSpacing: '-0.03em',
    },
    headerSubtitle: {
      color: '#74716B',
    },
    socialButtonsBlockButton: {
      background: '#FFFFFF',
      border: '1px solid #E1E0DC',
      color: '#1A1916',
      boxShadow: 'none',
    },
    socialButtonsBlockButtonText: {
      color: '#1A1916',
    },
    dividerLine: {
      background: '#E1E0DC',
    },
    dividerText: {
      color: '#989590',
    },
    formFieldLabel: {
      color: '#403E39',
    },
    formFieldInput: {
      background: '#FFFFFF',
      border: '1px solid #D7D5D0',
      color: '#0D0C0A',
      boxShadow: 'none',
    },
    footerActionText: {
      display: 'none',
    },
    footerActionLink: {
      display: 'none',
    },
    footer: {
      background: 'transparent',
      padding: '20px 0 0',
      margin: '0',
    },
    footerAction: {
      display: 'none',
    },
    form: {
      gap: '14px',
    },
    main: {
      gap: '18px',
    },
    formButtonPrimary: {
      background: '#1A1916',
      color: '#FAF9F7',
      boxShadow: 'none',
      fontWeight: '700',
    },
    identityPreviewText: {
      color: '#0D0C0A',
    },
    formResendCodeLink: {
      color: '#9A7658',
    },
    otpCodeFieldInput: {
      background: '#FFFFFF',
      border: '1px solid #D7D5D0',
      color: '#0D0C0A',
    },
    alertText: {
      color: '#0D0C0A',
    },
    alertClerkError: {
      background: 'rgba(248,113,113,0.12)',
      border: '1px solid rgba(248,113,113,0.24)',
    },
  },
} as const;

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
        background: 'rgba(248,113,113,0.12)',
        border: '1px solid rgba(248,113,113,0.24)',
        color: '#9c2a25',
        fontSize: '13px',
        textAlign: 'center',
      }}
    >
      {error}
    </div>
  );
}

export default function AuthApp() {
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
          color: '#9c2a25',
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
            border: '2px solid #e1e5eb',
            borderTopColor: '#B08D6E',
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
      appearance={clerkAppearance}
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
          <div
            style={{
              textAlign: 'center',
              marginBottom: '28px',
              fontFamily: "'Allternit Serif', Georgia, ui-serif, Cambria, 'Times New Roman', Times, serif",
              fontSize: '28px',
              fontWeight: 700,
              color: '#0d0c0a',
            }}
          >
            Allternit
          </div>
          <AuthFlow />
          <TokenBridge />
        </div>
      </div>
      <SeedAuth />
    </ClerkProvider>
  );
}
