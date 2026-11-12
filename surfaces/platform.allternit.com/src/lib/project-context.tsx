/**
 * The selected Platform API project for the whole console.
 *
 * ConsoleLayout mounts <ProjectProvider>. The header switcher, every
 * /platform/* page and Rate limits read the same selection through
 * useProjectContext(), so they never disagree. `client` is the /v1 client for
 * the selected project: it sends `X-Allternit-Project: <id>` on every call, so
 * changing the project here re-scopes every Platform API request.
 *
 * The selection is remembered per organization in localStorage (try/catch:
 * private mode or blocked storage still works for the visit). Archived
 * projects are never listed (listProjects filters them out).
 */
import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { formatApiError } from "@/lib/api-client";
import { isPlatformDisabled, listProjects, type PlatformProject } from "@/lib/platform-projects";
import { v1, type V1Client } from "@/lib/platform-v1";
import { usePlatformOrganization } from "@/lib/platform-auth-client";
import { SETTINGS_SELECT_CLASS } from "@/components/console-ui/buttonStyles";
import { cn } from "@/lib/utils";

/** Pre-Wave-5 key, still read by the API keys page. Written alongside the per-org key. */
const LEGACY_STORAGE_KEY = "allternit.platform.project";

function storageKey(orgId: string | null): string {
  return `${LEGACY_STORAGE_KEY}:${orgId ?? "personal"}`;
}

function readStored(orgId: string | null): string | null {
  try {
    return window.localStorage.getItem(storageKey(orgId)) ?? window.localStorage.getItem(LEGACY_STORAGE_KEY);
  } catch {
    return null;
  }
}

function writeStored(orgId: string | null, id: string): void {
  try {
    window.localStorage.setItem(storageKey(orgId), id);
    window.localStorage.setItem(LEGACY_STORAGE_KEY, id);
  } catch {
    // Private mode or blocked storage: the selection still works for this visit.
  }
}

export interface ProjectsState {
  loading: boolean;
  disabled: boolean;
  error: string | null;
  /** Non-archived projects in the active organization. */
  projects: PlatformProject[];
  project: PlatformProject | null;
  /** /v1 client scoped to `project` (sends X-Allternit-Project). */
  client: V1Client | null;
  select: (id: string) => void;
  reload: () => Promise<void>;
}

const ProjectContext = createContext<ProjectsState | null>(null);

function useProjectsLoader(): ProjectsState {
  const { isLoaded: orgLoaded, organization } = usePlatformOrganization();
  const orgId = organization?.id ?? null;
  const [projects, setProjects] = useState<PlatformProject[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [disabled, setDisabled] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestRef = useRef(0);

  const reload = useCallback(async () => {
    const request = ++requestRef.current;
    setLoading(true);
    setError(null);
    try {
      const list = await listProjects();
      if (request !== requestRef.current) return;
      setProjects(list);
      setDisabled(false);
      setSelected((cur) => {
        const remembered = cur ?? readStored(orgId);
        return remembered && list.some((p) => p.id === remembered) ? remembered : list[0]?.id ?? null;
      });
    } catch (err) {
      if (request !== requestRef.current) return;
      setProjects([]);
      if (isPlatformDisabled(err)) setDisabled(true);
      else setError(formatApiError(err, "Unable to load projects"));
    } finally {
      if (request === requestRef.current) setLoading(false);
    }
  }, [orgId]);

  useEffect(() => {
    if (!orgLoaded) return;
    // A different organization has its own projects and its own remembered pick.
    setSelected(null);
    void reload();
  }, [orgLoaded, reload]);

  const select = useCallback(
    (id: string) => {
      setSelected(id);
      writeStored(orgId, id);
    },
    [orgId],
  );

  const project = projects.find((p) => p.id === selected) ?? null;
  const client = useMemo(() => (project ? v1(project.id) : null), [project]);
  return { loading, disabled, error, projects, project, client, select, reload };
}

export function ProjectProvider({ children }: { children: React.ReactNode }) {
  const state = useProjectsLoader();
  return <ProjectContext.Provider value={state}>{children}</ProjectContext.Provider>;
}

/** The console's selected Platform API project. Must be used under ConsoleLayout. */
export function useProjectContext(): ProjectsState {
  const ctx = useContext(ProjectContext);
  if (!ctx) throw new Error("useProjectContext must be used inside <ProjectProvider> (ConsoleLayout).");
  return ctx;
}

/**
 * Project switcher bound to the context. `compact` is the console header
 * control: no visible label, narrow on phones. With no projects it links to
 * the Projects page instead.
 */
export function ProjectSwitcher({ compact = false, className }: { compact?: boolean; className?: string }) {
  const state = useProjectContext();
  if (state.disabled || state.error) return null;
  // Keep the control on screen while a reload (create/archive) is in flight.
  if (state.loading && state.projects.length === 0) return null;
  if (state.projects.length === 0) {
    return (
      <Link
        to="/platform/projects"
        className={cn(
          "inline-flex shrink-0 items-center whitespace-nowrap rounded-lg border border-dashed border-[var(--border-default)] px-2.5 py-1.5 text-[12px] font-semibold text-[var(--accent-primary)] hover:bg-[var(--surface-hover)]",
          className,
        )}
      >
        Create a project
      </Link>
    );
  }
  return (
    <label
      className={cn(
        "inline-flex min-w-0 max-w-full items-center gap-2 text-[12px] font-medium text-[var(--text-secondary)]",
        className,
      )}
    >
      <span className={compact ? "sr-only md:not-sr-only md:shrink-0" : "shrink-0"}>Project</span>
      <select
        value={state.project?.id ?? ""}
        onChange={(e) => state.select(e.target.value)}
        title={state.project ? `${state.project.name} (${state.project.env === "live" ? "live" : "sandbox"})` : undefined}
        className={cn(
          SETTINGS_SELECT_CLASS,
          compact
            ? "min-w-0 max-w-[132px] truncate py-1.5 sm:max-w-[160px] lg:max-w-[240px]"
            : "min-w-0 max-w-[220px] sm:max-w-[320px]",
        )}
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
