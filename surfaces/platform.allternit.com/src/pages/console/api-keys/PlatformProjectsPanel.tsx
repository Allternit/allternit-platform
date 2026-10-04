import React, { useCallback, useEffect, useState } from "react";
import { Check, CircleNotch, Copy, Cube, Key, Plus, ShieldCheck, Trash, WarningCircle, X } from "@phosphor-icons/react";
import { cn } from "@/lib/utils";
import { formatApiError } from "@/lib/api-client";
import { EmptyState } from "@/components/settings/EmptyState";
import { DESTRUCTIVE_BUTTON_CLASS, QUIET_BUTTON_CLASS } from "@/components/settings/buttonStyles";
import {
  type CreatedProjectKey,
  type PlatformProject,
  type ProjectEnv,
  type ProjectKey,
  PROJECT_KEY_SCOPES,
  createProject,
  createProjectKey,
  isPlatformDisabled,
  listProjectKeys,
  listProjects,
  revokeProjectKey,
} from "@/lib/platform-projects";

const INPUT_CLASS =
  "mt-1.5 w-full p-2 px-3 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] text-[13px] text-[var(--text-primary)] outline-none focus:border-[var(--accent-primary)] placeholder:text-[var(--text-tertiary)]";
const PRIMARY_BUTTON_CLASS =
  "inline-flex items-center gap-1.5 px-3 py-2 rounded-lg text-[13px] font-semibold bg-[var(--accent-primary)] text-[var(--ui-text-inverse)] hover:brightness-110 transition-all disabled:opacity-50 disabled:cursor-not-allowed";

function formatDate(iso?: string | null): string {
  if (!iso) return "—";
  return new Date(iso).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

/**
 * Platform API projects and their project keys (`alt_test_…` / `alt_live_…`).
 * One project per app; keys are bound to a project and, optionally, one account.
 */
export function PlatformProjectsPanel() {
  const [projects, setProjects] = useState<PlatformProject[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [disabled, setDisabled] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [showNewProject, setShowNewProject] = useState(false);
  const [projectName, setProjectName] = useState("");
  const [projectEnv, setProjectEnv] = useState<ProjectEnv>("sandbox");
  const [savingProject, setSavingProject] = useState(false);

  const load = useCallback(async () => {
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
    void load();
  }, [load]);

  const handleCreateProject = useCallback(async () => {
    if (!projectName.trim()) return;
    setSavingProject(true);
    setError(null);
    try {
      const p = await createProject({ name: projectName.trim(), env: projectEnv });
      setProjectName("");
      setProjectEnv("sandbox");
      setShowNewProject(false);
      await load();
      setSelected(p.id);
    } catch (err) {
      setError(formatApiError(err, "Unable to create project"));
    } finally {
      setSavingProject(false);
    }
  }, [projectName, projectEnv, load]);

  if (loading) {
    return (
      <div className="flex items-center gap-2 py-10 justify-center text-[13px] text-[var(--text-secondary)]" role="status">
        <CircleNotch size={16} className="animate-spin" aria-hidden /> Loading projects…
      </div>
    );
  }

  if (disabled) {
    return (
      <EmptyState
        icon={<Cube size={32} aria-hidden />}
        title="Platform API is in private beta"
        caption="Projects and project keys for the developer Platform API will appear here once it is switched on for your account."
      />
    );
  }

  const project = projects.find((p) => p.id === selected) ?? null;

  return (
    <div className="space-y-4">
      {error && (
        <div role="alert" className="flex items-start gap-2 rounded-lg border border-solid border-[var(--status-error)]/25 bg-[var(--status-error)]/[0.06] p-3 text-[13px] text-[var(--text-primary)]">
          <WarningCircle size={16} className="text-[var(--status-error)] mt-0.5 shrink-0" aria-hidden />
          <span>{error}</span>
        </div>
      )}

      <div className="flex flex-wrap items-center gap-2">
        {projects.map((p) => (
          <button
            key={p.id}
            type="button"
            onClick={() => setSelected(p.id)}
            aria-pressed={p.id === selected}
            className={cn(
              "inline-flex items-center gap-1.5 rounded-lg border border-solid px-3 py-1.5 text-[13px] font-medium transition-colors",
              p.id === selected
                ? "border-[var(--accent-primary)]/40 bg-[var(--accent-primary)]/10 text-[var(--accent-primary)]"
                : "border-[var(--border-subtle)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]",
            )}
          >
            {p.name}
            <span className="text-[11px] uppercase tracking-wide opacity-70">{p.env === "live" ? "live" : "sandbox"}</span>
          </button>
        ))}
        <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setShowNewProject(true)}>
          <Plus size={14} aria-hidden /> New project
        </button>
      </div>

      {showNewProject && (
        <div className="rounded-xl border border-solid border-[var(--accent-primary)]/25 bg-[var(--accent-primary)]/[0.03] p-4 space-y-3">
          <div className="flex items-center justify-between gap-3">
            <div className="text-[14px] font-semibold text-[var(--text-primary)]">New project</div>
            <button type="button" onClick={() => setShowNewProject(false)} className="p-1 rounded-md text-[var(--text-tertiary)] hover:bg-[var(--surface-hover)]" aria-label="Close">
              <X size={16} />
            </button>
          </div>
          <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
            Name
            <input value={projectName} onChange={(e) => setProjectName(e.target.value)} placeholder="Front desk app" maxLength={80} className={INPUT_CLASS} />
          </label>
          <fieldset>
            <legend className="text-[12px] font-medium text-[var(--text-secondary)]">Environment</legend>
            <div className="mt-1.5 flex flex-wrap gap-3 text-[13px] text-[var(--text-primary)]">
              {(["sandbox", "live"] as const).map((env) => (
                <label key={env} className="inline-flex items-center gap-1.5">
                  <input type="radio" name="project-env" checked={projectEnv === env} onChange={() => setProjectEnv(env)} />
                  {env === "sandbox" ? "Sandbox: test keys, no real numbers or calls" : "Live: production traffic"}
                </label>
              ))}
            </div>
          </fieldset>
          <button type="button" className={PRIMARY_BUTTON_CLASS} disabled={!projectName.trim() || savingProject} onClick={() => void handleCreateProject()}>
            {savingProject ? <CircleNotch size={14} className="animate-spin" aria-hidden /> : <Plus size={14} aria-hidden />} Create project
          </button>
        </div>
      )}

      {projects.length === 0 && !showNewProject ? (
        <EmptyState
          icon={<Cube size={32} aria-hidden />}
          title="No projects yet"
          caption="A project is your app. It holds its accounts, numbers, agents and keys. Start with a sandbox project to test."
          ctaLabel="New project"
          onCtaClick={() => setShowNewProject(true)}
          primaryCta
        />
      ) : project ? (
        <ProjectKeys key={project.id} project={project} />
      ) : null}
    </div>
  );
}

function ProjectKeys({ project }: { project: PlatformProject }) {
  const [keys, setKeys] = useState<ProjectKey[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  const [name, setName] = useState("");
  const [scopes, setScopes] = useState<string[]>(["agents"]);
  const [accountId, setAccountId] = useState("");
  const [creating, setCreating] = useState(false);
  const [created, setCreated] = useState<CreatedProjectKey | null>(null);
  const [copied, setCopied] = useState(false);
  const [confirmRevoke, setConfirmRevoke] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setKeys(await listProjectKeys(project.id));
    } catch (err) {
      setError(formatApiError(err, "Unable to load keys"));
    } finally {
      setLoading(false);
    }
  }, [project.id]);

  useEffect(() => {
    void load();
  }, [load]);

  const handleCreate = useCallback(async () => {
    if (!name.trim() || scopes.length === 0) return;
    setCreating(true);
    setError(null);
    try {
      const key = await createProjectKey(project.id, { name: name.trim(), scopes, ...(accountId.trim() ? { account_id: accountId.trim() } : {}) });
      setCreated(key);
      setName("");
      setScopes(["agents"]);
      setAccountId("");
      await load();
    } catch (err) {
      setError(formatApiError(err, "Unable to create key"));
    } finally {
      setCreating(false);
    }
  }, [project.id, name, scopes, accountId, load]);

  const handleRevoke = useCallback(
    async (keyId: string) => {
      setError(null);
      try {
        await revokeProjectKey(project.id, keyId);
        setConfirmRevoke(null);
        await load();
      } catch (err) {
        setError(formatApiError(err, "Unable to revoke key"));
      }
    },
    [project.id, load],
  );

  const prefix = project.env === "live" ? "alt_live_" : "alt_test_";

  return (
    <section aria-label={`Keys for ${project.name}`} className="rounded-xl border border-solid border-[var(--border-subtle)]">
      <div className="flex items-center justify-between gap-3 border-b border-solid border-[var(--border-subtle)] px-4 py-3">
        <div>
          <div className="text-[14px] font-semibold text-[var(--text-primary)]">{project.name}</div>
          <div className="text-[12px] text-[var(--text-secondary)]">
            {project.env === "live" ? "Live" : "Sandbox"} · plan {project.plan} · <code className="font-mono">{project.id}</code>
          </div>
        </div>
        <button type="button" className={PRIMARY_BUTTON_CLASS} onClick={() => { setShowCreate(true); setCreated(null); }}>
          <Plus size={14} aria-hidden /> Create key
        </button>
      </div>

      {error && (
        <div role="alert" className="m-4 flex items-start gap-2 rounded-lg border border-solid border-[var(--status-error)]/25 bg-[var(--status-error)]/[0.06] p-3 text-[13px] text-[var(--text-primary)]">
          <WarningCircle size={16} className="text-[var(--status-error)] mt-0.5 shrink-0" aria-hidden />
          <span>{error}</span>
        </div>
      )}

      {showCreate && (
        <div className="m-4 rounded-xl border border-solid border-[var(--accent-primary)]/25 bg-[var(--accent-primary)]/[0.03] p-4 space-y-3">
          <div className="flex items-center justify-between gap-3">
            <div className="text-[14px] font-semibold text-[var(--text-primary)]">Create {prefix}… key</div>
            <button type="button" onClick={() => { setShowCreate(false); setCreated(null); }} className="p-1 rounded-md text-[var(--text-tertiary)] hover:bg-[var(--surface-hover)]" aria-label="Close">
              <X size={16} />
            </button>
          </div>
          {created ? (
            <div className="space-y-3">
              <div className="flex items-start gap-2 rounded-lg border border-solid border-[var(--status-success)]/25 bg-[var(--status-success)]/[0.06] p-3">
                <ShieldCheck size={16} className="text-[var(--status-success)] mt-0.5 shrink-0" aria-hidden />
                <div className="text-[13px] text-[var(--text-primary)]">
                  Copy this key now. It will not be shown again. Store it in a secret manager.
                </div>
              </div>
              <div className="flex items-center gap-2 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2">
                <code className="flex-1 text-[12px] font-mono text-[var(--text-primary)] truncate">{created.token}</code>
                <button
                  type="button"
                  className={QUIET_BUTTON_CLASS}
                  onClick={() => void navigator.clipboard.writeText(created.token).then(() => { setCopied(true); window.setTimeout(() => setCopied(false), 2000); }).catch(() => {})}
                  aria-label="Copy key"
                >
                  {copied ? <Check size={14} aria-hidden /> : <Copy size={14} aria-hidden />} {copied ? "Copied" : "Copy"}
                </button>
              </div>
            </div>
          ) : (
            <>
              <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
                Name
                <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Backend server" maxLength={80} className={INPUT_CLASS} />
              </label>
              <fieldset>
                <legend className="text-[12px] font-medium text-[var(--text-secondary)]">Scopes</legend>
                <div className="mt-1.5 grid grid-cols-1 sm:grid-cols-2 gap-1.5">
                  {PROJECT_KEY_SCOPES.map((s) => (
                    <label key={s.value} className="inline-flex items-start gap-1.5 text-[13px] text-[var(--text-primary)]">
                      <input
                        type="checkbox"
                        className="mt-0.5"
                        checked={scopes.includes(s.value)}
                        onChange={(e) => setScopes((cur) => (e.target.checked ? [...cur, s.value] : cur.filter((x) => x !== s.value)))}
                      />
                      <span>
                        {s.label} <span className="text-[var(--text-tertiary)]">· {s.hint}</span>
                      </span>
                    </label>
                  ))}
                </div>
              </fieldset>
              <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
                Account (optional)
                <input value={accountId} onChange={(e) => setAccountId(e.target.value)} placeholder="acct_… — limits the key to one customer" className={INPUT_CLASS} />
              </label>
              <button type="button" className={PRIMARY_BUTTON_CLASS} disabled={!name.trim() || scopes.length === 0 || creating} onClick={() => void handleCreate()}>
                {creating ? <CircleNotch size={14} className="animate-spin" aria-hidden /> : <Key size={14} aria-hidden />} Create key
              </button>
            </>
          )}
        </div>
      )}

      {loading ? (
        <div className="flex items-center gap-2 p-6 justify-center text-[13px] text-[var(--text-secondary)]" role="status">
          <CircleNotch size={16} className="animate-spin" aria-hidden /> Loading keys…
        </div>
      ) : keys.length === 0 ? (
        <EmptyState icon={<Key size={28} aria-hidden />} title="No keys yet" caption={`Create a ${prefix}… key to call the Platform API from your server.`} />
      ) : (
        <ul className="divide-y divide-[var(--border-subtle)]">
          {keys.map((k) => (
            <li key={k.id} className="flex flex-wrap items-center gap-3 px-4 py-3">
              <div className="min-w-0 flex-1">
                <div className="text-[13px] font-medium text-[var(--text-primary)]">{k.name}</div>
                <div className="text-[12px] text-[var(--text-secondary)]">
                  <code className="font-mono">{k.prefix}…</code> · {k.scopes.join(", ")}
                  {k.account_id ? <> · account <code className="font-mono">{k.account_id}</code></> : null} · created {formatDate(k.created_at)} · last used {formatDate(k.last_used_at)}
                </div>
              </div>
              {confirmRevoke === k.id ? (
                <div className="flex items-center gap-2">
                  <button type="button" className={DESTRUCTIVE_BUTTON_CLASS} onClick={() => void handleRevoke(k.id)}>
                    <Trash size={14} aria-hidden /> Revoke
                  </button>
                  <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setConfirmRevoke(null)}>Cancel</button>
                </div>
              ) : (
                <button type="button" className={DESTRUCTIVE_BUTTON_CLASS} onClick={() => setConfirmRevoke(k.id)} aria-label={`Revoke ${k.name}`}>
                  <Trash size={14} aria-hidden /> Revoke
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
