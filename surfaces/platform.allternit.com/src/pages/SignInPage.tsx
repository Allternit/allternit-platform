import React from "react";
import { SignIn } from "@clerk/clerk-react";
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

export function SignInPage() {
  const [searchParams] = useSearchParams();
  const redirectUrl = safeRedirect(searchParams.get("redirect_url"));
  const signUpUrl = `${CLERK_SIGN_UP_PATH}?redirect_url=${encodeURIComponent(redirectUrl)}`;

  return (
    <AuthPageShell
      title="Build on the Allternit Platform"
      subtitle="Create agents and applications with managed compute, billing, and API access."
    >
      <SignIn
        routing="path"
        path={CLERK_SIGN_IN_PATH}
        forceRedirectUrl={redirectUrl}
        signUpUrl={signUpUrl}
        signUpForceRedirectUrl={redirectUrl}
        appearance={cleanCardAppearance}
      />
      <p className="ob-alt">
        Don&apos;t have an account?{" "}
        <Link to={signUpUrl}>
          Sign up
        </Link>
      </p>
    </AuthPageShell>
  );
}
