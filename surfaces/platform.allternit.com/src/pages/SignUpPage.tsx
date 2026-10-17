import React from "react";
import { SignUp } from "@clerk/clerk-react";
import { Link, useSearchParams } from "react-router-dom";
import { CLERK_SIGN_IN_PATH, CLERK_SIGN_UP_PATH } from "@/clerkConfig";
import { AuthPageShell } from "@/components/AuthPageShell";

const cleanCardAppearance = {
  elements: {
    headerTitle: { display: "none" },
    headerSubtitle: { display: "none" },
    logoBox: { display: "none" },
    footer: { display: "none" },
  },
} as const;

function safeRedirect(value: string | null): string {
  if (!value) return "/";
  if (value.startsWith("/") && !value.startsWith("//")) return value;
  return "/";
}

export function SignUpPage() {
  const [searchParams] = useSearchParams();
  const redirectUrl = safeRedirect(searchParams.get("redirect_url"));
  const signInUrl = `${CLERK_SIGN_IN_PATH}?redirect_url=${encodeURIComponent(redirectUrl)}`;

  return (
    <AuthPageShell
      title="Get started with Allternit"
      subtitle="Create your account to start building with managed compute, billing, and API access."
    >
      <SignUp
        routing="path"
        path={CLERK_SIGN_UP_PATH}
        forceRedirectUrl={redirectUrl}
        signInUrl={signInUrl}
        signInForceRedirectUrl={redirectUrl}
        appearance={cleanCardAppearance}
      />
      <p className="ob-alt">
        Already have an account?{" "}
        <Link to={signInUrl}>
          Sign in
        </Link>
      </p>
    </AuthPageShell>
  );
}
