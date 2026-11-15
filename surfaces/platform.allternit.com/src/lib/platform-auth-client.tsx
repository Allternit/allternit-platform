import React, { ReactNode, createContext, useContext, useEffect, useMemo } from "react";
import {
  ClerkProvider,
  OrganizationSwitcher,
  useAuth,
  useClerk as useClerkReact,
  useOrganization,
  useSignIn,
  useUser,
} from "@clerk/clerk-react";
import { api } from "@/lib/api-client";
import { useTheme } from "@/lib/theme";
import {
  CLERK_PUBLISHABLE_KEY,
  CLERK_PROXY_URL,
  CLERK_SIGN_IN_PATH,
  CLERK_SIGN_UP_PATH,
  getAllowedRedirectOrigins,
} from "@/clerkConfig";

export interface PlatformUser {
  id: string;
  firstName?: string | null;
  lastName?: string | null;
  userEmail?: string;
  primaryEmailAddress?: { emailAddress: string } | null;
  emailAddresses?: Array<{ emailAddress: string }>;
  imageUrl?: string | null;
}

export interface PlatformOrganization {
  id: string;
  name: string;
  slug?: string | null;
  imageUrl?: string | null;
}

export interface PlatformOrganizationMembership {
  role?: string | null;
}

interface PlatformAuthShape {
  user: {
    isLoaded: boolean;
    isSignedIn: boolean;
    user: PlatformUser | null;
  };
  organization: {
    isLoaded: boolean;
    organization: PlatformOrganization | null;
    membership: PlatformOrganizationMembership | null;
  };
  auth: {
    isLoaded: boolean;
    isSignedIn: boolean | undefined;
    userId: string | null | undefined;
    sessionId: string | null | undefined;
    orgId: string | null | undefined;
    orgRole: string | null | undefined;
    actor: unknown;
    getToken: () => Promise<string | null>;
  };
  signOut: (_options?: any) => Promise<void>;
  clerk: any;
}

const PlatformAuthContext = createContext<PlatformAuthShape | null>(null);

export function isPlatformAuthDisabled(): boolean {
  return !CLERK_PUBLISHABLE_KEY;
}

function getClerkAppearance(resolved: "light" | "dark") {
  const isLight = resolved === "light";
  const bg = isLight ? "#FFFEFC" : "#141416";
  const bgInput = isLight ? "#FFFFFF" : "#1c1c1f";
  const text = isLight ? "#0D0C0A" : "#e5e5e5";
  const textSecondary = isLight ? "#74716B" : "#a1a1aa";
  const textTertiary = isLight ? "#989590" : "#71717a";
  const border = isLight ? "#E1E0DC" : "rgba(255,255,255,0.10)";
  const borderInput = isLight ? "#D7D5D0" : "rgba(255,255,255,0.12)";
  const label = isLight ? "#403E39" : "#d4d4d8";
  const buttonBg = isLight ? "#1A1916" : "#e5e5e5";
  const buttonText = isLight ? "#FAF9F7" : "#0b0b0c";
  const socialBg = isLight ? "#FFFFFF" : "#1c1c1f";
  const socialText = isLight ? "#1A1916" : "#e5e5e5";

  return {
    variables: {
      colorBackground: bg,
      colorPrimary: isLight ? "#1A1916" : "#9A7658",
      colorText: text,
      colorTextSecondary: textSecondary,
      colorInputBackground: bgInput,
      colorInputText: text,
      colorNeutral: textSecondary,
      colorDanger: "#f87171",
      borderRadius: "16px",
      fontFamily: "inherit",
    },
    elements: {
      cardBox: { background: "transparent", border: "none", boxShadow: "none", width: "100%", maxWidth: "100%" },
      rootBox: { width: "100%", maxWidth: "100%", boxSizing: "border-box" },
      card: { background: "transparent", border: "none", boxShadow: "none", width: "100%", maxWidth: "100%", boxSizing: "border-box", padding: "0" },
      headerTitle: { color: text, fontSize: "28px", fontWeight: 700, letterSpacing: "-0.03em" },
      headerSubtitle: { color: textSecondary },
      socialButtonsBlockButton: { background: socialBg, border: `1px solid ${border}`, color: socialText, boxShadow: "none" },
      socialButtonsBlockButtonText: { color: socialText },
      dividerLine: { background: border },
      dividerText: { color: textTertiary },
      formFieldLabel: { color: label },
      formFieldInput: { background: bgInput, border: `1px solid ${borderInput}`, color: text, boxShadow: "none" },
      formFieldInputShowPasswordButton: { color: textSecondary },
      formFieldInputShowPasswordButtonIcon: { color: textSecondary },
      footerActionText: { color: textSecondary },
      footerActionLink: { color: "#9A7658" },
      footer: { background: "transparent", padding: "20px 0 0", margin: "0" },
      form: { gap: "14px" },
      main: { gap: "18px" },
      formButtonPrimary: { background: buttonBg, color: buttonText, boxShadow: "none", fontWeight: 700 },
      identityPreviewText: { color: text },
      formResendCodeLink: { color: "#9A7658" },
      otpCodeFieldInput: { background: bgInput, border: `1px solid ${borderInput}`, color: text },
      alertText: { color: text },
      alertClerkError: { background: "rgba(248,113,113,0.12)", border: "1px solid rgba(248,113,113,0.24)" },
    },
  };
}

export function PlatformAuthProvider({ children }: { children: ReactNode }) {
  const { resolved } = useTheme();

  if (!CLERK_PUBLISHABLE_KEY) {
    const value = buildDisabledAuthValue();
    return <PlatformAuthContext.Provider value={value}>{children}</PlatformAuthContext.Provider>;
  }

  return (
    <ClerkProvider
      publishableKey={CLERK_PUBLISHABLE_KEY}
      appearance={getClerkAppearance(resolved)}
      signInUrl={CLERK_SIGN_IN_PATH}
      signUpUrl={CLERK_SIGN_UP_PATH}
      proxyUrl={CLERK_PROXY_URL}
      allowedRedirectOrigins={getAllowedRedirectOrigins()}
    >
      <ClerkPlatformAuthBridge>{children}</ClerkPlatformAuthBridge>
    </ClerkProvider>
  );
}

export function usePlatformUser(): {
  isLoaded: boolean;
  isSignedIn: boolean;
  user: PlatformUser | null;
} {
  return usePlatformAuthContext().user;
}

export function usePlatformOrganization(): {
  isLoaded: boolean;
  organization: PlatformOrganization | null;
  membership: PlatformOrganizationMembership | null;
} {
  return usePlatformAuthContext().organization;
}

export function PlatformOrganizationSwitcher() {
  const { clerk } = usePlatformAuthContext();
  if (!clerk) return null;
  return (
    <OrganizationSwitcher
      appearance={{
        elements: {
          rootBox: { width: "100%" },
          organizationSwitcherTrigger: {
            width: "100%",
            justifyContent: "space-between",
            border: "1px solid var(--border-subtle)",
            borderRadius: "10px",
            padding: "9px 11px",
            background: "var(--bg-secondary)",
            color: "var(--text-primary)",
            boxShadow: "none",
          },
        },
      }}
    />
  );
}

export function useClerk() {
  return usePlatformAuthContext().clerk;
}

export function usePlatformAuth() {
  return usePlatformAuthContext().auth;
}

export function usePlatformSignOut() {
  return usePlatformAuthContext().signOut;
}

function buildDisabledAuthValue(): PlatformAuthShape {
  if (import.meta.env.DEV && import.meta.env.VITE_DEV_AUTH_BYPASS === "1") {
    const mockUser: PlatformUser = {
      id: "dev-user",
      firstName: "Local",
      lastName: "Developer",
      userEmail: "dev@allternit.local",
      primaryEmailAddress: { emailAddress: "dev@allternit.local" },
      emailAddresses: [{ emailAddress: "dev@allternit.local" }],
      imageUrl: null,
    };
    return {
      user: { isLoaded: true, isSignedIn: true, user: mockUser },
      organization: { isLoaded: true, organization: null, membership: null },
      auth: {
        isLoaded: true,
        isSignedIn: true,
        userId: "dev-user",
        sessionId: "dev-session",
        orgId: null,
        orgRole: null,
        actor: null,
        getToken: async () => "dev-token",
      },
      signOut: async () => {},
      clerk: null,
    };
  }

  return {
    user: { isLoaded: true, isSignedIn: false, user: null },
    organization: { isLoaded: true, organization: null, membership: null },
    auth: {
      isLoaded: true,
      isSignedIn: false,
      userId: null,
      sessionId: null,
      orgId: null,
      orgRole: null,
      actor: null,
      getToken: async () => null,
    },
    signOut: async () => {},
    clerk: null,
  };
}

function ClerkPlatformAuthBridge({ children }: { children: ReactNode }) {
  const clerkUser = useUser();
  const clerkAuth = useAuth();
  const clerkOrganization = useOrganization();
  const clerk = useClerkReact();
  const { signIn, setActive } = useSignIn();

  // DEV-only seeded auto-login. If VITE_CLERK_SEED_EMAIL/PASSWORD are set in
  // .env.local, sign in automatically so agent/UI work can start authenticated.
  // The seed account should be a real verified test user; the effect also
  // ensures an active organization so the JWT is org-scoped.
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    if (!clerkAuth.isLoaded || clerkAuth.isSignedIn) return;
    if (!signIn || !setActive || !clerk) return;
    const email = import.meta.env.VITE_CLERK_SEED_EMAIL as string | undefined;
    const password = import.meta.env.VITE_CLERK_SEED_PASSWORD as string | undefined;
    if (!email || !password) return;
    let active = true;
    const run = async () => {
      try {
        const result = await signIn.create({ identifier: email, password });
        if (!active) return;
        if (result.status === "complete" && result.createdSessionId) {
          const memberships = clerk.user?.organizationMemberships;
          let orgId: string | undefined = memberships?.[0]?.organization.id;
          if (!orgId) {
            try {
              const org = await clerk.createOrganization({ name: "Allternit Seed" });
              orgId = org.id;
            } catch (orgErr) {
              console.warn("[PlatformSeedAuth] Failed to create seed organization:", orgErr);
            }
          }
          await clerk.setActive({ session: result.createdSessionId, organization: orgId });
        } else {
          console.warn("[PlatformSeedAuth] Clerk sign-in requires extra steps:", result.status);
        }
      } catch (err) {
        console.error("[PlatformSeedAuth] Auto sign-in failed:", err);
      }
    };
    void run();
    return () => {
      active = false;
    };
  }, [clerkAuth.isLoaded, clerkAuth.isSignedIn, signIn, setActive, clerk]);

  useEffect(() => {
    if (!clerkAuth.isSignedIn || !clerk.session) {
      api.clearToken();
      api.clearTokenProvider();
      return;
    }

    // Register the Clerk token provider. The api client calls it on every
    // request; Clerk's getToken() caches the JWT and refreshes it before it
    // expires, so no copy is held here. (A timer-synced copy went stale:
    // Clerk tokens live 60 s and background tabs throttle timers, so idle
    // tabs sent expired JWTs.) On a 401 the client asks again with
    // skipCache for a newly minted token.
    api.setTokenProvider(async (options) => {
      try {
        const token = options?.skipCache
          ? await clerk.session?.getToken({ skipCache: true })
          : await clerk.session?.getToken();
        return token ?? null;
      } catch {
        return null;
      }
    });

    return () => {
      api.clearTokenProvider();
    };
  }, [clerkAuth.isSignedIn, clerk.session]);

  const value = useMemo(
    () => ({
      user: {
        isLoaded: clerkUser.isLoaded,
        isSignedIn: clerkUser.isSignedIn ?? false,
        user: (clerkUser.user as PlatformUser | null | undefined) ?? null,
      },
      organization: {
        isLoaded: clerkOrganization.isLoaded,
        organization: clerkOrganization.organization
          ? {
              id: clerkOrganization.organization.id,
              name: clerkOrganization.organization.name,
              slug: clerkOrganization.organization.slug,
              imageUrl: clerkOrganization.organization.imageUrl,
            }
          : null,
        membership: clerkOrganization.membership
          ? { role: clerkOrganization.membership.role }
          : null,
      },
      auth: clerkAuth,
      signOut: clerk.signOut,
      clerk,
    }),
    [clerk, clerkAuth, clerkOrganization]
  );

  return <PlatformAuthContext.Provider value={value}>{children}</PlatformAuthContext.Provider>;
}

function usePlatformAuthContext() {
  const context = useContext(PlatformAuthContext);
  if (!context) {
    throw new Error("PlatformAuthProvider is missing");
  }
  return context;
}
