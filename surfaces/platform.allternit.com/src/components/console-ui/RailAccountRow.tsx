import React, { useEffect, useRef, useState } from "react";
import { useLocation } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import { UnfoldMoreIcon, Logout01Icon, UserCircleIcon } from "@hugeicons/core-free-icons";
import { OrganizationSwitcher, UserButton } from "@clerk/clerk-react";
import { cn } from "@/lib/utils";
import {
  usePlatformAuth,
  usePlatformOrganization,
  usePlatformSignOut,
  usePlatformUser,
  useClerk,
} from "@/lib/platform-auth-client";

/** Renders nothing when Clerk organization features fail (e.g. orgs disabled). */
class ClerkSilentBoundary extends React.Component<
  { children: React.ReactNode },
  { hasError: boolean }
> {
  state = { hasError: false };

  static getDerivedStateFromError() {
    return { hasError: true };
  }

  componentDidCatch() {
    // Clerk orgs unavailable — stay silent.
  }

  render() {
    return this.state.hasError ? null : this.props.children;
  }
}

function initialsOf(name: string): string {
  const words = name.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return "A";
  if (words.length === 1) return words[0].charAt(0).toUpperCase();
  return (words[0].charAt(0) + words[1].charAt(0)).toUpperCase();
}

function useAccountSummary() {
  const clerk = useClerk();
  const { user } = usePlatformUser();
  const { organization, membership } = usePlatformOrganization();

  const email =
    user?.primaryEmailAddress?.emailAddress ||
    user?.userEmail ||
    user?.emailAddresses?.[0]?.emailAddress ||
    "";
  const userName = [user?.firstName, user?.lastName].filter(Boolean).join(" ") || email || "Account";
  const rawRole = membership?.role ? membership.role.replace(/^org:/, "") : organization ? "Member" : "Personal";
  const role = rawRole.charAt(0).toUpperCase() + rawRole.slice(1);
  const title = organization?.name || "Personal account";

  // Clerk serves a generated placeholder as imageUrl when no logo was uploaded;
  // it renders as a squeezed tile, so only a real upload (hasImage) is shown.
  // hasImage is read from the live Clerk org resource because the platform
  // auth bridge does not forward it.
  const clerkOrg = (clerk as { organization?: { id?: string; hasImage?: boolean } } | null)?.organization;
  const logoUrl =
    organization?.imageUrl && clerkOrg?.id === organization.id && clerkOrg?.hasImage === true
      ? organization.imageUrl
      : null;

  return {
    clerk,
    email,
    userName,
    role,
    title,
    logoUrl,
    initials: initialsOf(organization?.name || userName),
  };
}

/** 28 px org tile: the uploaded logo, or a neutral initials tile. */
export function OrgTile({ logoUrl, initials }: { logoUrl: string | null; initials: string }) {
  if (logoUrl) {
    return (
      <img
        src={logoUrl}
        alt=""
        className="size-7 shrink-0 rounded-lg object-cover"
      />
    );
  }
  return (
    <span
      aria-hidden="true"
      className="flex size-7 shrink-0 items-center justify-center rounded-lg bg-[var(--surface-hover)] text-[11px] font-semibold text-[var(--text-primary)] ring-1 ring-inset ring-[var(--border-subtle)]"
    >
      {initials}
    </span>
  );
}

/** Avatar-only account control for the collapsed rail and the phone header. */
export function RailAccountAvatar() {
  const { clerk, userName } = useAccountSummary();
  return (
    <div className="flex justify-center px-2 py-2">
      {clerk ? (
        <UserButton
          afterSignOutUrl="/sign-in"
          appearance={{ elements: { userButtonAvatarBox: "size-8 rounded-lg" } }}
        />
      ) : (
        <span
          title={userName}
          className="flex size-8 items-center justify-center rounded-lg bg-[var(--surface-hover)] text-[11px] font-semibold text-[var(--text-primary)]"
        >
          {initialsOf(userName)}
        </span>
      )}
    </div>
  );
}

/**
 * The one account row at the bottom of the expanded rail: organization on line
 * one, role · email on line two. One click opens a panel with the Clerk
 * organization switcher and the account controls (profile, sign out).
 */
export function RailAccountRow() {
  const auth = usePlatformAuth();
  const signOut = usePlatformSignOut();
  const location = useLocation();
  const { clerk, email, userName, role, title, logoUrl, initials } = useAccountSummary();
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setOpen(false);
  }, [location.pathname]);

  useEffect(() => {
    if (!open) return;
    const onPointer = (event: PointerEvent) => {
      const target = event.target as Element | null;
      if (!target || rootRef.current?.contains(target)) return;
      // Clerk renders its own popovers and modals in portals; clicks there
      // belong to the controls inside this panel.
      if (target.closest?.('[class*="cl-"]')) return;
      setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", onPointer);
    window.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onPointer);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const subline = [role, email].filter(Boolean).join(" · ");

  return (
    <div ref={rootRef} className="relative">
      {open && (
        <div
          role="dialog"
          aria-label="Organization and account"
          className="absolute inset-x-0 bottom-full z-50 mb-1.5 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-2 shadow-xl"
        >
          {clerk && auth.isSignedIn && (
            <div className="mb-2">
              <div className="px-1.5 pb-1.5 text-[11px] font-medium text-[var(--text-tertiary)]">
                Organization
              </div>
              <ClerkSilentBoundary>
                <OrganizationSwitcher
                  hidePersonal={false}
                  appearance={{
                    elements: {
                      rootBox: { width: "100%" },
                      organizationSwitcherTrigger: {
                        width: "100%",
                        justifyContent: "space-between",
                        border: "1px solid var(--border-subtle)",
                        borderRadius: "10px",
                        padding: "7px 10px",
                        background: "var(--bg-primary)",
                        color: "var(--text-primary)",
                        boxShadow: "none",
                        fontSize: "13px",
                      },
                    },
                  }}
                />
              </ClerkSilentBoundary>
            </div>
          )}

          <div className="px-1.5 pb-1.5 text-[11px] font-medium text-[var(--text-tertiary)]">Account</div>
          <div className="flex items-center gap-2.5 rounded-lg px-1.5 py-1.5">
            {clerk ? (
              <UserButton
                afterSignOutUrl="/sign-in"
                appearance={{ elements: { userButtonAvatarBox: "size-7 rounded-lg" } }}
              />
            ) : (
              <span className="flex size-7 items-center justify-center rounded-lg bg-[var(--surface-hover)] text-[11px] font-semibold text-[var(--text-primary)]">
                {initialsOf(userName)}
              </span>
            )}
            <div className="min-w-0 flex-1">
              <div className="truncate text-[13px] font-medium text-[var(--text-primary)]">{userName}</div>
              {email && email !== userName && (
                <div className="truncate text-[11px] text-[var(--text-tertiary)]">{email}</div>
              )}
            </div>
          </div>
          {clerk && (
            <button
              type="button"
              onClick={() => {
                setOpen(false);
                clerk.openUserProfile?.();
              }}
              className="flex w-full items-center gap-2.5 rounded-lg px-2 py-1.5 text-left text-[13px] text-[var(--text-secondary)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)]"
            >
              <HugeiconsIcon icon={UserCircleIcon} size={15} />
              Manage account
            </button>
          )}
          <button
            type="button"
            onClick={() => {
              setOpen(false);
              void signOut({ redirectUrl: "/sign-in" });
            }}
            className="flex w-full items-center gap-2.5 rounded-lg px-2 py-1.5 text-left text-[13px] text-[var(--text-secondary)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)]"
          >
            <HugeiconsIcon icon={Logout01Icon} size={15} />
            Sign out
          </button>
        </div>
      )}

      <button
        type="button"
        onClick={() => setOpen((prev) => !prev)}
        aria-expanded={open}
        aria-haspopup="dialog"
        className={cn(
          "flex w-full items-center gap-2.5 rounded-xl px-2 py-1.5 text-left transition-colors hover:bg-[var(--surface-hover)]",
          open && "bg-[var(--surface-hover)]"
        )}
      >
        <OrgTile logoUrl={logoUrl} initials={initials} />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-[13px] font-medium text-[var(--text-primary)]">{title}</span>
          {subline && (
            <span className="block truncate text-[11px] text-[var(--text-tertiary)]">{subline}</span>
          )}
        </span>
        <HugeiconsIcon icon={UnfoldMoreIcon} size={14} className="shrink-0 text-[var(--text-tertiary)]" />
      </button>
    </div>
  );
}
