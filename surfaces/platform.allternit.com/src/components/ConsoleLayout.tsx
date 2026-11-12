import React, { useEffect, useState } from "react";
import { NavLink, useLocation } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Wallet01Icon,
  Search01Icon,
  Notification01Icon,
  Rocket01Icon,
  ListIcon,
  Cancel01Icon,
  LifebuoyIcon,
  CircleIcon,
  ScrollIcon,
  ChevronRightIcon,
  ChevronDownIcon,
  PanelLeftIcon,
  BookOpen02Icon,
} from "@hugeicons/core-free-icons";
import { OrganizationSwitcher, UserButton } from "@clerk/clerk-react";
import { cn } from "@/lib/utils";
import { api } from "@/lib/api-client";
import {
  usePlatformOrganization,
  usePlatformUser,
  usePlatformAuth,
  useClerk,
} from "@/lib/platform-auth-client";
import {
  consoleNav,
  consoleNavLabelForPath,
  type ConsoleNavGroup,
} from "@/components/console-ui/navConfig";
import { CommandPalette } from "@/components/console-ui/CommandPalette";
import { AnnouncementModal } from "@/components/console-ui/AnnouncementModal";
import { AllternitWordmark } from "@/components/AllternitWordmark";

function groupContainsPath(group: ConsoleNavGroup, pathname: string): boolean {
  return group.items.some(
    (item) => pathname === item.to || pathname.startsWith(`${item.to}/`)
  );
}

/** Error boundary that renders nothing when Clerk organization features fail. */
class ClerkSilentBoundary extends React.Component<
  { children: React.ReactNode },
  { hasError: boolean }
> {
  state = { hasError: false };

  static getDerivedStateFromError() {
    return { hasError: true };
  }

  componentDidCatch() {
    // Clerk orgs unavailable (e.g. personal workspaces disabled) — stay silent.
  }

  render() {
    return this.state.hasError ? null : this.props.children;
  }
}

function SidebarOrgSwitcher() {
  const auth = usePlatformAuth();
  if (!auth.isSignedIn) return null;
  return (
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
  );
}

/**
 * Credits balance chip. Fetched via the platform api client on mount and on
 * org change. Failure is hidden — the chip renders nothing rather than a
 * broken balance.
 */
function CreditsChip() {
  const { organization } = usePlatformOrganization();
  const [balance, setBalance] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    (async () => {
      try {
        const data = await api.get<{
          balance_usd?: number;
          balance_cents?: number;
        }>("/api/v1/credits/balance");
        if (!active) return;
        const usd =
          typeof data.balance_usd === "number"
            ? data.balance_usd
            : (data.balance_cents ?? 0) / 100;
        setBalance(`$${usd.toFixed(2)}`);
      } catch {
        // Fail hidden: never render a broken chip.
      }
    })();
    return () => {
      active = false;
    };
  }, [organization?.id]);

  if (!balance) return null;

  return (
    <NavLink
      to="/billing"
      className={({ isActive }) =>
        cn(
          "flex items-center justify-between rounded-lg px-2.5 py-2 text-[13px] font-medium transition-colors",
          isActive
            ? "bg-[var(--surface-hover)] text-[var(--text-primary)]"
            : "text-[var(--text-secondary)] hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)]"
        )
      }
    >
      <span className="flex items-center gap-3">
        <HugeiconsIcon icon={Wallet01Icon} size={17} />
        Credits
      </span>
      <span className="text-[12px] text-[var(--text-tertiary)]">{balance}</span>
    </NavLink>
  );
}

function RailUserCard({ collapsed }: { collapsed: boolean }) {
  const clerk = useClerk();
  const { user } = usePlatformUser();
  const { organization, membership } = usePlatformOrganization();

  const name =
    [user?.firstName, user?.lastName].filter(Boolean).join(" ") ||
    user?.primaryEmailAddress?.emailAddress ||
    user?.userEmail ||
    "Account";
  const role = membership?.role
    ? membership.role.replace(/^org:/, "")
    : organization
      ? "Member"
      : "Personal";
  const orgName = organization?.name || "Allternit";
  const initials = name
    .split(/\s+/)
    .map((part) => part.charAt(0))
    .slice(0, 2)
    .join("")
    .toUpperCase();

  if (collapsed) {
    return clerk ? (
      <div className="flex justify-center px-2 py-2">
        <UserButton
          afterSignOutUrl="/sign-in"
          appearance={{ elements: { userButtonAvatarBox: "size-8 rounded-lg" } }}
        />
      </div>
    ) : (
      <div className="flex justify-center px-2 py-2">
        <div className="flex size-8 items-center justify-center rounded-lg bg-[var(--accent-primary)]/10 text-[10px] font-semibold text-[var(--accent-primary)]">
          {initials.charAt(0)}
        </div>
      </div>
    );
  }

  return (
    <div className="flex items-center gap-2.5 rounded-xl px-2.5 py-2 transition-colors hover:bg-[var(--surface-hover)]">
      <div className="shrink-0">
        {clerk ? (
          <UserButton
            afterSignOutUrl="/sign-in"
            appearance={{ elements: { userButtonAvatarBox: "size-8 rounded-lg" } }}
          />
        ) : (
          <div className="flex size-8 items-center justify-center rounded-lg bg-[var(--accent-primary)]/10 text-[10px] font-semibold text-[var(--accent-primary)]">
            {initials.charAt(0)}
          </div>
        )}
      </div>
      <div className="min-w-0 flex-1">
        <div className="truncate text-[13px] font-medium text-[var(--text-primary)]">{name}</div>
        <div className="truncate text-[11px] capitalize text-[var(--text-tertiary)]">
          {role} · {orgName}
        </div>
      </div>
      <HugeiconsIcon
        icon={ChevronDownIcon}
        size={14}
        className="shrink-0 text-[var(--text-tertiary)]"
      />
    </div>
  );
}

function SidebarContent({
  onNavigate,
  collapsed,
  onExpand,
  onOpenPalette,
}: {
  onNavigate?: () => void;
  collapsed: boolean;
  onExpand?: () => void;
  onOpenPalette: () => void;
}) {
  const location = useLocation();
  const [openGroups, setOpenGroups] = useState<Record<string, boolean>>({});

  const isGroupOpen = (group: ConsoleNavGroup) => {
    if (groupContainsPath(group, location.pathname)) return true;
    return openGroups[group.label] ?? group.defaultOpen ?? false;
  };

  const toggleGroup = (label: string) => {
    setOpenGroups((prev) => ({
      ...prev,
      [label]:
        !(prev[label] ??
          consoleNav.groups.find((g) => g.label === label)?.defaultOpen ??
          false),
    }));
  };

  if (collapsed) {
    return (
      <>
        <div className="flex flex-1 flex-col items-center gap-1 overflow-y-auto px-2 py-3">
          {consoleNav.top.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              title={item.label}
              className={({ isActive }) =>
                cn(
                  "flex size-10 items-center justify-center rounded-lg transition-colors",
                  isActive
                    ? "bg-[var(--surface-hover)] text-[var(--text-primary)]"
                    : "text-[var(--text-secondary)] hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)]"
                )
              }
            >
              <HugeiconsIcon icon={item.icon} size={19} />
            </NavLink>
          ))}
          {consoleNav.groups.map((group) => (
            <button
              key={group.label}
              type="button"
              title={group.label}
              onClick={onExpand}
              className="flex size-10 items-center justify-center rounded-lg text-[var(--text-secondary)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)]"
            >
              <HugeiconsIcon icon={group.icon} size={19} />
            </button>
          ))}
        </div>
        <div className="border-t border-[var(--border-subtle)] py-2">
          <RailUserCard collapsed />
        </div>
      </>
    );
  }

  return (
    <>
      {/* Search — opens the command palette */}
      <div className="px-3 pt-3">
        <button
          type="button"
          onClick={onOpenPalette}
          className="flex w-full items-center gap-2 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 text-[var(--text-secondary)] transition-colors hover:border-[var(--border-default)]"
        >
          <HugeiconsIcon icon={Search01Icon} size={15} />
          <span className="flex-1 text-left text-[13px] text-[var(--text-tertiary)]">
            Search Console...
          </span>
          <kbd className="inline-flex items-center rounded border border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-1.5 py-0.5 text-[10px] text-[var(--text-tertiary)]">
            ⌘K
          </kbd>
        </button>
      </div>

      {/* Nav */}
      <nav className="flex-1 overflow-y-auto px-3 py-3">
        {consoleNav.top.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            onClick={onNavigate}
            className={({ isActive }) =>
              cn(
                "mb-1 flex items-center gap-3 rounded-lg px-3 py-2 text-[13px] font-medium transition-colors",
                isActive
                  ? "bg-[var(--surface-hover)] text-[var(--text-primary)]"
                  : "text-[var(--text-secondary)] hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)]"
              )
            }
          >
            <HugeiconsIcon icon={item.icon} size={17} />
            {item.label}
          </NavLink>
        ))}

        {consoleNav.groups.map((group) => {
          const open = isGroupOpen(group);
          return (
            <div key={group.label} className="mb-1">
              <button
                type="button"
                onClick={() => toggleGroup(group.label)}
                className={cn(
                  "flex w-full items-center gap-3 rounded-lg px-3 py-2 text-[13px] font-medium transition-colors",
                  groupContainsPath(group, location.pathname)
                    ? "text-[var(--text-primary)]"
                    : "text-[var(--text-secondary)] hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)]"
                )}
              >
                <HugeiconsIcon icon={group.icon} size={17} />
                <span className="flex-1 text-left">{group.label}</span>
                {group.badge && (
                  <span className="rounded-md bg-[var(--accent-highlight-subtle)] px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-[var(--accent-highlight)]">
                    {group.badge}
                  </span>
                )}
                <HugeiconsIcon
                  icon={open ? ChevronDownIcon : ChevronRightIcon}
                  size={14}
                  className="text-[var(--text-tertiary)]"
                />
              </button>
              {open && (
                <div className="mt-0.5 space-y-px pl-[38px]">
                  {group.items.map((item) => (
                    <NavLink
                      key={item.to}
                      to={item.to}
                      onClick={onNavigate}
                      className={({ isActive }) =>
                        cn(
                          "block rounded-md px-3 py-1.5 text-[13px] transition-colors",
                          isActive
                            ? "bg-[var(--surface-hover)] font-medium text-[var(--text-primary)]"
                            : "text-[var(--text-secondary)] hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)]"
                        )
                      }
                    >
                      {item.label}
                    </NavLink>
                  ))}
                </div>
              )}
            </div>
          );
        })}
      </nav>

      {/* Footer — always visible above the collapse control */}
      <div className="border-t border-[var(--border-subtle)] p-2.5">
        <div className="mb-1.5">
          <SidebarOrgSwitcher />
        </div>
        <NavLink
          to="/docs"
          onClick={onNavigate}
          className={({ isActive }) =>
            cn(
              "flex items-center gap-3 rounded-lg px-2.5 py-2 text-[13px] font-medium transition-colors",
              isActive
                ? "bg-[var(--surface-hover)] text-[var(--text-primary)]"
                : "text-[var(--text-secondary)] hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)]"
            )
          }
        >
          <HugeiconsIcon icon={BookOpen02Icon} size={17} />
          Documentation
        </NavLink>
        <CreditsChip />
        <RailUserCard collapsed={false} />
        <div className="mt-1 flex items-center gap-1 px-2.5">
          <a
            href="mailto:support@allternit.com"
            className="inline-flex items-center gap-1 text-[11px] text-[var(--text-tertiary)] hover:text-[var(--text-secondary)] transition-colors"
          >
            <HugeiconsIcon icon={LifebuoyIcon} size={12} /> Support
          </a>
          <span className="text-[var(--border-default)]">·</span>
          <a
            href="https://status.allternit.com"
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex items-center gap-1 text-[11px] text-[var(--text-tertiary)] hover:text-[var(--text-secondary)] transition-colors"
          >
            <SystemStatusDot />{" "}
            System status
          </a>
          <span className="text-[var(--border-default)]">·</span>
          <a
            href="https://allternit.com/changelog"
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex items-center gap-1 text-[11px] text-[var(--text-tertiary)] hover:text-[var(--text-secondary)] transition-colors"
          >
            <HugeiconsIcon icon={ScrollIcon} size={12} /> Changelog
          </a>
        </div>
      </div>
    </>
  );
}


// Live dot for the "System status" link: reads the public status feed, shows a
// neutral dot when the feed can't be reached (never claims "operational" blind).
const STATUS_URL = "https://status.allternit.com/api/v1/status.json";
const STATUS_DOT: Record<string, { cls: string; label: string }> = {
  operational: { cls: "text-[var(--status-success)]", label: "All systems operational" },
  degraded: { cls: "text-[var(--status-warning)]", label: "Some systems degraded" },
  maintenance: { cls: "text-[var(--status-warning)]", label: "Maintenance in progress" },
  down: { cls: "text-[var(--status-error)]", label: "Major outage" },
};

function SystemStatusDot() {
  const [overall, setOverall] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    const load = async () => {
      const ctrl = new AbortController();
      const timer = setTimeout(() => ctrl.abort(), 3000);
      try {
        const res = await fetch(STATUS_URL, { signal: ctrl.signal });
        const body = res.ok ? await res.json() : null;
        if (alive) setOverall(typeof body?.overall === "string" ? body.overall : null);
      } catch {
        if (alive) setOverall(null);
      } finally {
        clearTimeout(timer);
      }
    };
    void load();
    const id = setInterval(() => { if (document.visibilityState === "visible") void load(); }, 5 * 60 * 1000);
    return () => { alive = false; clearInterval(id); };
  }, []);
  const dot = overall ? STATUS_DOT[overall] : undefined;
  return (
    <HugeiconsIcon
      icon={CircleIcon}
      size={8}
      className={dot ? dot.cls : "text-[var(--text-tertiary)]"}
      aria-label={dot ? dot.label : "Status unknown"}
    />
  );
}

export function ConsoleLayout({ children }: { children: React.ReactNode }) {
  const [mobileOpen, setMobileOpen] = useState(false);
  const [collapsed, setCollapsed] = useState(
    () => typeof window !== "undefined" && window.localStorage.getItem("console-rail-collapsed") === "1"
  );
  const [paletteOpen, setPaletteOpen] = useState(false);
  const location = useLocation();
  const pageLabel = location.pathname === "/" ? "Overview" : consoleNavLabelForPath(location.pathname);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setPaletteOpen((prev) => !prev);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const toggleCollapsed = () => {
    setCollapsed((prev) => {
      window.localStorage.setItem("console-rail-collapsed", prev ? "0" : "1");
      return !prev;
    });
  };

  const sidebar = (
    onNavigate: (() => void) | undefined,
    onExpand: (() => void) | undefined
  ) => (
    <SidebarContent
      collapsed={false}
      onNavigate={onNavigate}
      onExpand={onExpand}
      onOpenPalette={() => {
        setPaletteOpen(true);
        setMobileOpen(false);
      }}
    />
  );

  return (
    <div className="flex h-screen w-full overflow-hidden bg-[var(--bg-primary)] text-[var(--text-primary)]">
      {/* Desktop sidebar */}
      <aside
        className={cn(
          "hidden lg:flex flex-col border-r border-[var(--border-subtle)] bg-[var(--bg-secondary)] transition-[width] duration-200",
          collapsed ? "w-[68px]" : "w-64"
        )}
      >
        <div
          className={cn(
            "flex items-center border-b border-[var(--border-subtle)]",
            collapsed ? "justify-center px-2 py-3.5" : "justify-between px-4 py-3.5"
          )}
        >
          {!collapsed && <AllternitWordmark variant="light" height={22} />}
          <button
            type="button"
            onClick={toggleCollapsed}
            title={collapsed ? "Expand navigation" : "Collapse navigation"}
            className="p-1.5 rounded-md text-[var(--text-secondary)] hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)] transition-colors"
          >
            <HugeiconsIcon icon={PanelLeftIcon} size={17} />
          </button>
        </div>

        <SidebarContent
          collapsed={collapsed}
          onExpand={() => setCollapsed(false)}
          onOpenPalette={() => setPaletteOpen(true)}
        />
      </aside>

      {/* Mobile drawer */}
      {mobileOpen && (
        <>
          <div
            className="fixed inset-0 z-40 bg-black/60 lg:hidden"
            onClick={() => setMobileOpen(false)}
            aria-hidden="true"
          />
          <aside className="fixed inset-y-0 left-0 z-50 w-64 flex flex-col border-r border-[var(--border-subtle)] bg-[var(--bg-secondary)] lg:hidden">
            <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--border-subtle)]">
              <AllternitWordmark variant="light" height={24} />
              <button
                type="button"
                onClick={() => setMobileOpen(false)}
                className="p-1.5 rounded-md text-[var(--text-secondary)] hover:bg-[var(--surface-hover)]"
                aria-label="Close navigation"
              >
                <HugeiconsIcon icon={Cancel01Icon} size={20} />
              </button>
            </div>
            {sidebar(() => setMobileOpen(false), undefined)}
          </aside>
        </>
      )}

      <div className="flex flex-1 flex-col min-w-0">
        {/* Header */}
        <header className="flex h-16 items-center justify-between border-b border-[var(--border-subtle)] px-4 lg:px-6 bg-[var(--bg-primary)]/80 backdrop-blur-sm gap-4">
          <div className="flex items-center gap-3 min-w-0">
            <button
              type="button"
              onClick={() => setMobileOpen(true)}
              className="lg:hidden p-1.5 rounded-md text-[var(--text-secondary)] hover:bg-[var(--surface-hover)]"
              aria-label="Open navigation"
            >
              <HugeiconsIcon icon={ListIcon} size={20} />
            </button>

            <nav className="hidden sm:flex items-center gap-2 text-[13px] text-[var(--text-secondary)]">
              <span className="text-[var(--text-tertiary)]">Dashboard</span>
              <HugeiconsIcon icon={ChevronRightIcon} size={14} className="text-[var(--text-tertiary)]" />
              <span className="text-[var(--text-primary)] font-medium">{pageLabel}</span>
            </nav>
          </div>

          <div className="flex items-center gap-3 shrink-0">
            <button
              type="button"
              onClick={() => setPaletteOpen(true)}
              className="relative p-2 rounded-xl text-[var(--text-secondary)] hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)] transition-colors"
              aria-label="Search"
            >
              <HugeiconsIcon icon={Search01Icon} size={20} />
            </button>

            <button
              type="button"
              className="relative p-2 rounded-xl text-[var(--text-secondary)] hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)] transition-colors"
              aria-label="Notifications"
            >
              <HugeiconsIcon icon={Notification01Icon} size={20} />
              <span className="absolute top-1.5 right-1.5 size-2 rounded-full bg-[var(--status-error)] ring-2 ring-[var(--bg-primary)]" />
            </button>

            <a
              href="https://ai.allternit.com/shell"
              target="_blank"
              rel="noopener noreferrer"
              className="hidden md:inline-flex items-center gap-1.5 px-3 py-2 rounded-xl border border-solid border-[var(--accent-primary)]/30 bg-[var(--accent-primary)]/10 text-[13px] font-medium text-[var(--accent-primary)] hover:bg-[var(--accent-primary)]/20 transition-colors"
            >
              <HugeiconsIcon icon={Rocket01Icon} size={14} /> Launch App
            </a>

            <div className="lg:hidden shrink-0">
              <RailUserCard collapsed />
            </div>
          </div>
        </header>

        {/* Main content */}
        <main className="flex-1 overflow-y-auto p-4 lg:p-8">
          <div className="mx-auto max-w-7xl">{children}</div>
        </main>
      </div>

      <CommandPalette open={paletteOpen} onClose={() => setPaletteOpen(false)} />
      <AnnouncementModal />
    </div>
  );
}
