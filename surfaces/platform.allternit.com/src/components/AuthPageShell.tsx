import React from "react";
import { ConsoleHeader } from "@/components/onboarding/ConsoleHeader";
import { ConsoleShowcase } from "@/components/onboarding/ConsoleShowcase";
import "@/styles/onboarding.css";

interface AuthPageShellProps {
  title: string;
  subtitle: string;
  children: React.ReactNode;
}

/** What first-run setup walks a new account through after sign-in. */
const SETUP_PREVIEW = ["1 Organization", "2 Where it runs", "3 API key", "4 First call", "5 Plan"];


/**
 * Sign-in / sign-up frame: the A://TERNIT Console header (as live), the
 * looping developer showcase on the left, and the real Clerk card on the
 * right. Stacks to showcase-then-card below 1024 px.
 */
export function AuthPageShell({ title, subtitle, children }: AuthPageShellProps) {
  return (
    <div className="auth-page ob min-h-screen flex flex-col bg-[var(--bg-primary)] text-[var(--text-primary)]">
      <ConsoleHeader />

      <main className="ob-auth">
        <section className="ob-studio" aria-label="Platform preview">
          <div className="ob-studio-in">
            <ConsoleShowcase />
          </div>
        </section>

        <section className="ob-rail" aria-label="Sign in">
          <h1 className="ob-h1">{title}</h1>
          <p className="ob-lead">{subtitle}</p>
          <div className="ob-card ob-auth-card">{children}</div>
          <div className="ob-next5" aria-label="After you sign in, setup covers">
            {SETUP_PREVIEW.map((s) => (
              <span key={s}>{s}</span>
            ))}
          </div>
          <p className="ob-legal">
            By continuing, you agree to Allternit&apos;s{" "}
            <a href="https://allternit.com/terms" target="_blank" rel="noopener noreferrer">
              Terms of Service
            </a>{" "}
            and{" "}
            <a href="https://allternit.com/privacy" target="_blank" rel="noopener noreferrer">
              Privacy Policy
            </a>
            .
          </p>
        </section>
      </main>

      <footer className="border-t border-[var(--border-subtle)] bg-[var(--bg-primary)] py-6">
        <div className="mx-auto flex max-w-7xl flex-col items-center justify-between gap-3 px-6 text-[12px] text-[var(--text-tertiary)] sm:flex-row">
          <span>© {new Date().getFullYear()} Allternit. All rights reserved.</span>
          <div className="flex items-center gap-6">
            <a href="https://allternit.com/terms" target="_blank" rel="noopener noreferrer" className="transition-colors hover:text-[var(--text-primary)]">
              Terms
            </a>
            <a href="https://allternit.com/privacy" target="_blank" rel="noopener noreferrer" className="transition-colors hover:text-[var(--text-primary)]">
              Privacy
            </a>
            <a href="https://status.allternit.com" target="_blank" rel="noopener noreferrer" className="transition-colors hover:text-[var(--text-primary)]">
              Status
            </a>
          </div>
        </div>
      </footer>
    </div>
  );
}
