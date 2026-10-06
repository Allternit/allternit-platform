import React from "react";
import { Link } from "react-router-dom";
import { PublicHeader } from "@/components/PublicHeader";

const FOOTER_LINKS = [
  {
    title: "Product",
    links: [
      { label: "Models", to: "/models" },
      { label: "Plans", to: "/billing" },
      { label: "Console", to: "/" },
    ],
  },
  {
    title: "Resources",
    links: [
      { label: "Docs", to: "/docs" },
      { label: "Model router", href: "https://github.com/Allternit/allternit-platform/blob/session/d89ae6f0-3d9f-418e-8d5a-e2f91a39256b/cmd/allternit-cloud-api/docs/model-router.md" },
      { label: "API status", href: "https://status.allternit.com" },
      { label: "Changelog", href: "https://allternit.com/changelog" },
    ],
  },
  {
    title: "Company",
    links: [
      { label: "Support", href: "mailto:support@allternit.com" },
      { label: "Contact", href: "mailto:hello@allternit.com" },
    ],
  },
  {
    title: "Legal",
    links: [
      { label: "Terms", href: "https://allternit.com/terms" },
      { label: "Privacy", href: "https://allternit.com/privacy" },
    ],
  },
];

export function PublicPageShell({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex min-h-screen flex-col bg-[var(--bg-primary)] text-[var(--text-primary)]">
      <PublicHeader />
      <main className="flex-1">{children}</main>
      <footer className="border-t border-[var(--border-subtle)] bg-[var(--bg-primary)] px-6 py-12">
        <div className="mx-auto grid max-w-7xl grid-cols-2 gap-8 md:grid-cols-5">
          <div className="col-span-2 md:col-span-1">
            <span className="text-[14px] font-bold tracking-tight text-[var(--text-primary)]">
              Allternit
            </span>
            <p className="mt-2 max-w-xs text-[13px] leading-relaxed text-[var(--text-tertiary)]">
              One account for local and cloud models, hosted tools, and managed compute.
            </p>
          </div>
          {FOOTER_LINKS.map((group) => (
            <div key={group.title}>
              <h3 className="text-[12px] font-semibold uppercase tracking-wider text-[var(--text-secondary)]">
                {group.title}
              </h3>
              <ul className="mt-3 space-y-2">
                {group.links.map((link) => {
                  const isExternal = "href" in link;
                  const className =
                    "text-[13px] text-[var(--text-tertiary)] transition-colors hover:text-[var(--text-primary)]";
                  return (
                    <li key={link.label}>
                      {isExternal ? (
                        <a
                          href={link.href}
                          target="_blank"
                          rel="noopener noreferrer"
                          className={className}
                        >
                          {link.label}
                        </a>
                      ) : (
                        <Link to={link.to} className={className}>
                          {link.label}
                        </Link>
                      )}
                    </li>
                  );
                })}
              </ul>
            </div>
          ))}
        </div>
        <div className="mx-auto mt-10 flex max-w-7xl flex-col items-center justify-between gap-3 border-t border-[var(--border-subtle)] pt-6 text-[12px] text-[var(--text-tertiary)] sm:flex-row">
          <span>© {new Date().getFullYear()} Allternit. All rights reserved.</span>
          <a
            href="https://status.allternit.com"
            target="_blank"
            rel="noopener noreferrer"
            className="transition-colors hover:text-[var(--text-primary)]"
          >
            Status
          </a>
        </div>
      </footer>
    </div>
  );
}
