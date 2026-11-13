import React, { useEffect, useState } from "react";
import { NavLink, useLocation } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Wallet01Icon,
  Search01Icon,
  Rocket01Icon,
  ListIcon,
  Cancel01Icon,
  CircleIcon,
  ChevronRightIcon,
  ChevronDownIcon,
  PanelLeftIcon,
} from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { api } from "@/lib/api-client";
import { usePlatformOrganization } from "@/lib/platform-auth-client";
import {
  consoleNav,
  consoleNavEntryForPath,
} from "@/components/console-ui/navConfig";
import { CommandPalette } from "@/components/console-ui/CommandPalette";
import { AnnouncementModal } from "@/components/console-ui/AnnouncementModal";
import { RailAccountAvatar, RailAccountRow } from "@/components/console-ui/RailAccountRow";
import { AllternitWordmark } from "@/components/AllternitWordmark";
import { ProjectProvider, ProjectSwitcher } from "@/lib/project-context";

/**
 * The rail wordmark, picked by theme with CSS so it follows every theme change
 * (html[data-theme] is the source of truth): the dark mark on the light theme,
 * the cream mark on the dark theme. 17 px keeps the whole mark clear of the
 * collapse button in the 256 px rail.
 */
function RailWordmark() {
  return (
    <span className="flex min-w-0 items-center">
      <AllternitWordmark variant="dark" height={17} className="hidden [html[data-theme=light]_&]:block" />
      <AllternitWordmark variant="light" height={17} className="block [html[data-theme=light]_&]:hidden" />
    </span>
  );
}

/** The A:/ part of the wordmark (same pixel grid), for the 68 px rail. */
function RailMark() {
  const cells: Array<[number, number]> = [
    [0, 20], [0, 30], [0, 40], [10, 10], [10, 20], [20, 0], [30, 10], [30, 20],
    [40, 20], [40, 30], [40, 40], [60, 10], [60, 30], [80, 30], [80, 40], [90, 20],
    [100, 0], [100, 10],
  ];
  return (
    <svg
      viewBox="0 0 110 50"
      height={14}
      role="img"
      aria-label="Allternit"
      className="block text-[var(--text-primary)]"
      style={{ width: "auto" }}
    >
      {cells.map(([x, y]) => (
        <rect key={`${x}-${y}`} x={x + 0.75} y={y + 0.75} width={8.5} height={8.5} rx={1.5} fill="currentColor" />
      ))}
      <rect x={20.75} y={20.75} width={8.5} height={8.5} rx={1.5} fill="#D97757" />
    </svg>
  );
}

/**
 * Credits balance chip. Fetched via the platform api client on mount and on
 * org change. Failure is hidden — the chip renders nothing rather than a
 * broken balance.
 */
function CreditsChip({ onNavigate }: { onNavigate?: () => void }) {
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
      onClick={onNavigate}
      className={({ isActive }) =>
        cn(
          "mb-1 flex items-center justify-between rounded-lg px-2 py-1 text-[12px] transition-colors",
          isActive
            ? "bg-[var(--surface-hover)] text-[var(--text-primary)]"
            : "text-[var(--text-secondary)] hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)]"
        )
      }
    >
      <span className="flex items-center gap-2">
        <HugeiconsIcon icon={Wallet01Icon} size={14} />
        Credits
      </span>
      <span className="tabular-nums text-[var(--text-tertiary)]">{balance}</span>
    </NavLink>
  );
}

const footerLinkClass =
  "inline-flex shrink-0 items-center gap-1 text-[11px] text-[var(--text-tertiary)] transition-colors hover:text-[var(--text-secondary)]";

/** One line of links under the account row. Must not wrap at 256 px. */
function RailFooterLinks({ onNavigate }: { onNavigate?: () => void }) {
  return (
    <div className="mt-1 flex items-center gap-3 overflow-hidden whitespace-nowrap px-2 pt-1">
      <NavLink to="/docs" onClick={onNavigate} className={footerLinkClass}>
        Docs
      </NavLink>
      <a href="mailto:support@allternit.com" className={footerLinkClass}>
        Support
      </a>
      <a
        href="https://status.allternit.com"
        target="_blank"
        rel="noopener noreferrer"
        className={footerLinkClass}
      >
        <SystemStatusDot />
        Status
      </a>
      <a
        href="https://allternit.com/changelog"
        target="_blank"
        rel="noopener noreferrer"
        className={footerLinkClass}
      >
        Changelog
      </a>
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
  const activeGroup = consoleNavEntryForPath(location.pathname)?.group ?? null;
  // Only one group is open: the one holding the current page, until the user
  // opens another (which closes the rest) or closes the open one.
  const [openGroup, setOpenGroup] = useState<string | null>(activeGroup);

  useEffect(() => {
    setOpenGroup(activeGroup);
  }, [activeGroup]);

  const toggleGroup = (label: string) => {
    setOpenGroup((prev) => (prev === label ? null : label));
  };

  if (collapsed) {
    return (
      <>
        <div className="flex flex-1 flex-col items-center gap-1 overflow-y-auto px-2 py-3">
          {consoleNav.top.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              end={item.to === "/"}
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
          <div className="my-1 h-px w-6 bg-[var(--border-subtle)]" />
          {consoleNav.groups.map((group) => (
            <button
              key={group.label}
              type="button"
              title={group.label}
              onClick={() => {
                setOpenGroup(group.label);
                onExpand?.();
              }}
              className={cn(
                "flex size-10 items-center justify-center rounded-lg transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)]",
                group.label === activeGroup
                  ? "bg-[var(--surface-hover)] text-[var(--text-primary)]"
                  : "text-[var(--text-secondary)]"
              )}
            >
              <HugeiconsIcon icon={group.icon} size={19} />
            </button>
          ))}
        </div>
        <div className="border-t border-[var(--border-subtle)] py-2">
          <RailAccountAvatar />
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
            end={item.to === "/"}
            onClick={onNavigate}
            className={({ isActive }) =>
              cn(
                "mb-0.5 flex items-center gap-3 rounded-lg px-3 py-1.5 text-[13px] font-medium transition-colors",
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

        <div className="h-2" />

        {consoleNav.groups.map((group) => {
          const open = openGroup === group.label;
          const holdsPage = group.label === activeGroup;
          return (
            <div key={group.label} className="mb-0.5">
              <button
                type="button"
                onClick={() => toggleGroup(group.label)}
                aria-expanded={open}
                className={cn(
                  "flex w-full items-center gap-3 rounded-lg px-3 py-1.5 text-[13px] font-medium transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)]",
                  holdsPage ? "text-[var(--text-primary)]" : "text-[var(--text-secondary)]"
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
                <div className="mb-1 mt-0.5 space-y-px pl-[38px]">
                  {group.items.map((item) => (
                    <NavLink
                      key={item.to}
                      to={item.to}
                      onClick={onNavigate}
                      className={({ isActive }) =>
                        cn(
                          "flex items-center justify-between gap-2 rounded-md px-3 py-1 text-[13px] transition-colors",
                          isActive
                            ? "bg-[var(--surface-hover)] font-medium text-[var(--text-primary)]"
                            : "text-[var(--text-secondary)] hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)]"
                        )
                      }
                    >
                      <span className="truncate">{item.label}</span>
                      {item.badge && (
                        <span className="shrink-0 rounded-md bg-[var(--accent-highlight-subtle)] px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-[var(--accent-highlight)]">
                          {item.badge}
                        </span>
                      )}
                    </NavLink>
                  ))}
                </div>
              )}
            </div>
          );
        })}
      </nav>

      {/* Footer — credits, one account row, one line of links */}
      <div className="border-t border-[var(--border-subtle)] p-2.5">
        <CreditsChip onNavigate={onNavigate} />
        <RailAccountRow />
        <RailFooterLinks onNavigate={onNavigate} />
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
  const pageEntry = consoleNavEntryForPath(location.pathname);
  const pageLabel = pageEntry?.label ?? "Console";
  const pageGroup = pageEntry?.group ?? null;

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
    <ProjectProvider>
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
            "flex border-b border-[var(--border-subtle)]",
            collapsed
              ? "flex-col items-center gap-2 px-2 py-3"
              : "h-[57px] items-center justify-between gap-3 pl-4 pr-3"
          )}
        >
          {collapsed ? <RailMark /> : <RailWordmark />}
          <button
            type="button"
            onClick={toggleCollapsed}
            title={collapsed ? "Expand navigation" : "Collapse navigation"}
            aria-label={collapsed ? "Expand navigation" : "Collapse navigation"}
            className="shrink-0 p-1.5 rounded-md text-[var(--text-secondary)] hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)] transition-colors"
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
            <div className="flex h-[57px] items-center justify-between gap-3 pl-4 pr-3 border-b border-[var(--border-subtle)]">
              <RailWordmark />
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

            <nav className="hidden sm:flex min-w-0 items-center gap-2 text-[13px] text-[var(--text-secondary)]">
              {pageGroup && (
                <>
                  <span className="shrink-0 text-[var(--text-tertiary)]">{pageGroup}</span>
                  <HugeiconsIcon icon={ChevronRightIcon} size={14} className="shrink-0 text-[var(--text-tertiary)]" />
                </>
              )}
              <span className="truncate text-[var(--text-primary)] font-medium">{pageLabel}</span>
            </nav>

            {/* Platform API project scope (within the active organization): every /platform page and Rate limits follow it. */}
            <div className="flex min-w-0 shrink-0 items-center empty:hidden sm:border-0 sm:border-l sm:border-solid sm:border-[var(--border-subtle)] sm:pl-4">
              <ProjectSwitcher compact className="min-w-0" />
            </div>
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

            <a
              href="https://ai.allternit.com/shell"
              target="_blank"
              rel="noopener noreferrer"
              className="hidden md:inline-flex items-center gap-1.5 px-3 py-2 rounded-xl border border-solid border-[var(--accent-primary)]/30 bg-[var(--accent-primary)]/10 text-[13px] font-medium text-[var(--accent-primary)] hover:bg-[var(--accent-primary)]/20 transition-colors"
            >
              <HugeiconsIcon icon={Rocket01Icon} size={14} /> Launch App
            </a>

            <div className="lg:hidden shrink-0">
              <RailAccountAvatar />
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
    </ProjectProvider>
  );
}
