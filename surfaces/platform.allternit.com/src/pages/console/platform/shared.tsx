/**
 * Shared pieces for the Platform API console pages (Projects, Agents,
 * Conversations, Usage, Webhooks): the selected project, the project picker,
 * and the private-beta / no-project / error states every page needs.
 */
import React, { useCallback, useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { Cube, WarningCircle } from "@phosphor-icons/react";
import { formatApiError } from "@/lib/api-client";
import { isPlatformDisabled, listProjects, type PlatformProject } from "@/lib/platform-projects";
import { v1, type V1Client } from "@/lib/platform-v1";
import { EmptyState, SkeletonCard } from "@/components/console-ui";
import { SETTINGS_SELECT_CLASS } from "@/components/console-ui/buttonStyles";
import { cn } from "@/lib/utils";

const STORAGE_KEY = "allternit.platform.project";

function readStored(): string | null {
  try {
    return window.localStorage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
}

function writeStored(id: string): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, id);
  } catch {
    // Private mode or blocked storage: the picker still works for this visit.
  }
}

export interface ProjectsState {
  loading: boolean;
  disabled: boolean;
  error: string | null;
  projects: PlatformProject[];
  project: PlatformProject | null;
  client: V1Client | null;
  select: (id: string) => void;
  reload: () => Promise<void>;
}

/** The signed-in user's projects and the one this browser last picked. */
export function usePlatformProjects(): ProjectsState {
  const [projects, setProjects] = useState<PlatformProject[]>([]);
  const [selected, setSelected] = useState<string | null>(() => readStored());
  const [loading, setLoading] = useState(true);
  const [disabled, setDisabled] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await listProjects();
      setProjects(list);
      setDisabled(false);
      setSelected((cur) => (cur && list.some((p) => p.id === cur) ? cur : list[0]?.id ?? null));
    } catch (err) {
      if (isPlatformDisabled(err)) setDisabled(true);
      else setError(formatApiError(err, "Unable to load projects"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const select = useCallback((id: string) => {
    setSelected(id);
    writeStored(id);
  }, []);

  const project = projects.find((p) => p.id === selected) ?? null;
  const client = useMemo(() => (project ? v1(project.id) : null), [project]);
  return { loading, disabled, error, projects, project, client, select, reload };
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

export function ProjectPicker({ state }: { state: ProjectsState }) {
  if (state.projects.length === 0) return null;
  return (
    <label className="inline-flex max-w-full items-center gap-2 text-[12px] font-medium text-[var(--text-secondary)]">
      Project
      <select
        value={state.project?.id ?? ""}
        onChange={(e) => state.select(e.target.value)}
        className={cn(SETTINGS_SELECT_CLASS, "min-w-0 max-w-[220px] sm:max-w-[320px]")}
      >
        {state.projects.map((p) => (
          <option key={p.id} value={p.id}>
            {p.name} ({p.env === "live" ? "live" : "sandbox"})
          </option>
        ))}
      </select>
    </label>
  );
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

/** Page header: title + subtitle on the left, project picker (and actions) on the right. */
export function PlatformHeader({
  title,
  subtitle,
  state,
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
        {state && <ProjectPicker state={state} />}
        {actions}
      </div>
    </div>
  );
}
