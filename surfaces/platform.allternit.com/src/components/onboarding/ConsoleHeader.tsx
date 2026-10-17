import React from "react";
import { Moon, Sun } from "@phosphor-icons/react";
import { AllternitWordmark } from "@/components/AllternitWordmark";
import { useTheme } from "@/lib/theme";

export const CONSOLE_NAV_LINKS = [
  { label: "Docs", href: "https://docs.allternit.com" },
  { label: "API Reference", href: "https://api.allternit.com" },
  { label: "Pricing", href: "https://allternit.com/pricing" },
  { label: "Contact sales", href: "mailto:sales@allternit.com" },
];

/** The A://TERNIT Console header used on sign-in, sign-up and setup (as live). */
export function ConsoleHeader({ right }: { right?: React.ReactNode }) {
  const { resolved, setTheme } = useTheme();
  const isDark = resolved === "dark";
  return (
    <header className="w-full border-b border-[var(--border-subtle)] bg-[var(--bg-primary)]/80 backdrop-blur-sm">
      <div className="mx-auto flex h-16 max-w-7xl items-center justify-between gap-3 px-4 sm:px-6">
        <a href="https://allternit.com" className="flex min-w-0 items-center gap-2" target="_blank" rel="noopener noreferrer">
          <AllternitWordmark variant={isDark ? "light" : "dark"} height={28} className="ob-hdr-wm" />
          <span className="whitespace-nowrap text-[15px] font-semibold tracking-tight text-[var(--text-primary)]">Console</span>
        </a>
        <nav className="hidden items-center gap-8 md:flex" aria-label="Console">
          {CONSOLE_NAV_LINKS.map((link) => (
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
        <div className="flex items-center gap-2">
          {right}
          <button
            type="button"
            onClick={() => setTheme(isDark ? "light" : "dark")}
            className="inline-flex items-center justify-center rounded-lg p-2 text-[var(--text-secondary)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)]"
            aria-label={isDark ? "Switch to light mode" : "Switch to dark mode"}
          >
            {isDark ? <Sun size={18} /> : <Moon size={18} />}
          </button>
        </div>
      </div>
    </header>
  );
}
