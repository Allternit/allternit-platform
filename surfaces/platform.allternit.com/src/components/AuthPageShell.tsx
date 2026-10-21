import React from "react";
import { Moon, Sun } from "@phosphor-icons/react";
import { AllternitWordmark } from "@/components/AllternitWordmark";
import { useTheme } from "@/lib/theme";

interface AuthPageShellProps {
  title: string;
  subtitle: string;
  children: React.ReactNode;
}

const NAV_LINKS = [
  { label: "Docs", href: "https://docs.allternit.com" },
  { label: "API Reference", href: "https://api.allternit.com" },
  { label: "Pricing", href: "https://allternit.com/pricing" },
  { label: "Contact sales", href: "mailto:sales@allternit.com" },
];

export function AuthPageShell({ title, subtitle, children }: AuthPageShellProps) {
  const { resolved, setTheme } = useTheme();
  const isDark = resolved === "dark";

  return (
    <div className="auth-page min-h-screen flex flex-col bg-[var(--bg-primary)] text-[var(--text-primary)]">
      {/* Subtle dot pattern background */}
      <div
        className="pointer-events-none fixed inset-0 -z-10 text-[var(--text-secondary)]"
        style={{
          backgroundImage: "radial-gradient(circle, currentColor 1px, transparent 1px)",
          backgroundSize: "24px 24px",
          opacity: 0.15,
        }}
      />

      <header className="w-full border-b border-[var(--border-subtle)] bg-[var(--bg-primary)]/80 backdrop-blur-sm">
        <div className="mx-auto flex h-16 max-w-7xl items-center justify-between px-6">
          <a
            href="https://allternit.com"
            className="flex items-center gap-2"
            target="_blank"
            rel="noopener noreferrer"
          >
            <AllternitWordmark variant={isDark ? "light" : "dark"} height={28} />
            <span className="text-[15px] font-semibold tracking-tight text-[var(--text-primary)]">
              Console
            </span>
          </a>
          <nav className="hidden items-center gap-8 md:flex">
            {NAV_LINKS.map((link) => (
              <a
                key={link.label}
                href={link.href}
                target="_blank"
                rel="noopener noreferrer"
                className="text-[13px] font-medium text-[var(--text-secondary)] transition-colors hover:text-[var(--text-primary)]"
              >
                {link.label}
              </a>
            ))}
          </nav>
          <button
            type="button"
            onClick={() => setTheme(isDark ? "light" : "dark")}
            className="ml-4 inline-flex items-center justify-center rounded-lg p-2 text-[var(--text-secondary)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)]"
            aria-label={isDark ? "Switch to light mode" : "Switch to dark mode"}
          >
            {isDark ? <Sun size={18} /> : <Moon size={18} />}
          </button>
        </div>
      </header>

      <main className="flex flex-1 flex-col items-center justify-center px-6 py-12">
        <div className="w-full max-w-[420px]">
          <div className="mb-8 text-center">
            <h1 className="text-[32px] font-semibold tracking-tight text-[var(--text-primary)] sm:text-[38px]">
              {title}
            </h1>
            <p className="mx-auto mt-3 max-w-sm text-[15px] leading-relaxed text-[var(--text-secondary)]">
              {subtitle}
            </p>
          </div>

          <div className="rounded-2xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-6 shadow-[0_8px_32px_rgba(0,0,0,0.08)] sm:p-8">
            {children}
          </div>

          <p className="mt-6 text-center text-[12px] leading-relaxed text-[var(--text-tertiary)]">
            By continuing, you agree to Allternit&apos;s{" "}
            <a
              href="https://allternit.com/terms"
              target="_blank"
              rel="noopener noreferrer"
              className="underline underline-offset-2 transition-colors hover:text-[var(--text-primary)]"
            >
              Terms of Service
            </a>{" "}
            and{" "}
            <a
              href="https://allternit.com/privacy"
              target="_blank"
              rel="noopener noreferrer"
              className="underline underline-offset-2 transition-colors hover:text-[var(--text-primary)]"
            >
              Privacy Policy
            </a>
            .
          </p>
        </div>
      </main>

      <footer className="border-t border-[var(--border-subtle)] bg-[var(--bg-primary)] py-6">
        <div className="mx-auto flex max-w-7xl flex-col items-center justify-between gap-3 px-6 text-[12px] text-[var(--text-tertiary)] sm:flex-row">
          <span>© {new Date().getFullYear()} Allternit. All rights reserved.</span>
          <div className="flex items-center gap-6">
            <a
              href="https://allternit.com/terms"
              target="_blank"
              rel="noopener noreferrer"
              className="transition-colors hover:text-[var(--text-primary)]"
            >
              Terms
            </a>
            <a
              href="https://allternit.com/privacy"
              target="_blank"
              rel="noopener noreferrer"
              className="transition-colors hover:text-[var(--text-primary)]"
            >
              Privacy
            </a>
            <a
              href="https://status.allternit.com"
              target="_blank"
              rel="noopener noreferrer"
              className="transition-colors hover:text-[var(--text-primary)]"
            >
              Status
            </a>
          </div>
        </div>
      </footer>
    </div>
  );
}
