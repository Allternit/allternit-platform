/**
 * Shared pieces for the Platform API console pages (Projects, Agents,
 * Conversations, Usage, Webhooks): the selected project, the project picker,
 * and the private-beta / no-project / error states every page needs.
 */
import React from "react";
import { Link } from "react-router-dom";
import { Cube, WarningCircle } from "@phosphor-icons/react";
import { EmptyState, SkeletonCard } from "@/components/console-ui";
import { ProjectSwitcher, useProjectContext, type ProjectsState } from "@/lib/project-context";
import type { PlatformProject } from "@/lib/platform-projects";
import type { V1Client } from "@/lib/platform-v1";
import { cn } from "@/lib/utils";

export type { ProjectsState };

/**
 * The signed-in user's projects and the selected one. This is the console-wide
 * selection from src/lib/project-context.tsx (the header switcher), so every
 * Platform page and its X-Allternit-Project header follow the switcher.
 */
export function usePlatformProjects(): ProjectsState {
  return useProjectContext();
}

export function ErrorBanner({ message, className }: { message: string; className?: string }) {
  return (
    <div
      role="alert"
      className={cn(
        "flex items-start gap-2 rounded-lg border border-solid border-[var(--status-error)]/25 bg-[var(--status-error)]/[0.06] p-3 text-[13px] text-[var(--text-primary)]",
        className,
      )}
    >
      <WarningCircle size={16} className="mt-0.5 shrink-0 text-[var(--status-error)]" aria-hidden />
      <span className="min-w-0 break-words">{message}</span>
    </div>
  );
}

/** The project picker, bound to the console-wide selection (same control as the header switcher). */
export function ProjectPicker(_props: { state?: ProjectsState }) {
  return <ProjectSwitcher />;
}

/**
 * Renders the shared loading / beta / error / no-project states, or the page
 * body with a ready client for the selected project.
 */
export function ProjectGate({
  state,
  children,
}: {
  state: ProjectsState;
  children: (ctx: { project: PlatformProject; client: V1Client }) => React.ReactNode;
}) {
  if (state.loading) {
    return (
      <div className="space-y-3" role="status" aria-label="Loading">
        <SkeletonCard />
        <SkeletonCard rows={2} />
      </div>
    );
  }
  if (state.disabled) {
    return (
      <EmptyState
        icon={<Cube size={32} aria-hidden />}
        title="Platform API is in private beta"
        caption="Projects, agents and usage for the developer Platform API appear here once it is switched on for your account."
      />
    );
  }
  if (state.error) return <ErrorBanner message={state.error} />;
  if (!state.project || !state.client) {
    return (
      <div className="rounded-xl border border-solid border-[var(--border-subtle)]">
        <EmptyState
          icon={<Cube size={32} aria-hidden />}
          title="No projects yet"
          caption="A project is your app. It holds its accounts, agents, numbers and keys. Create one to start."
        />
        <div className="-mt-6 pb-8 text-center">
          <Link to="/platform/projects" className="text-[13px] font-semibold text-[var(--accent-primary)] hover:underline">
            Create a project
          </Link>
        </div>
      </div>
    );
  }
  return <>{children({ project: state.project, client: state.client })}</>;
}

export const INPUT_CLASS =
  "mt-1.5 w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] p-2 px-3 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--accent-primary)]";

export const PRIMARY_BUTTON_CLASS =
  "inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-all hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-50";

export const CARD_CLASS = "rounded-xl border border-solid border-[var(--border-subtle)]";

export function formatDate(iso?: string | null): string {
  if (!iso) return "—";
  return new Date(iso).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

export function formatDateTime(iso?: string | null): string {
  if (!iso) return "—";
  return new Date(iso).toLocaleString(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
}

/**
 * Page header: title + subtitle on the left, actions on the right. The project
 * is picked once, in the console header switcher; `state` is accepted for the
 * existing call sites and no longer renders a second picker.
 */
export function PlatformHeader({
  title,
  subtitle,
  actions,
}: {
  title: string;
  subtitle: string;
  state?: ProjectsState;
  actions?: React.ReactNode;
}) {
  return (
    <div className="mb-5 flex flex-wrap items-start justify-between gap-3">
      <div className="min-w-0">
        <h1 className="m-0 text-[20px] font-semibold tracking-tight text-[var(--text-primary)]">{title}</h1>
        <p className="m-0 mt-1 text-[13px] text-[var(--text-secondary)]">{subtitle}</p>
      </div>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        {actions}
      </div>
    </div>
  );
}
