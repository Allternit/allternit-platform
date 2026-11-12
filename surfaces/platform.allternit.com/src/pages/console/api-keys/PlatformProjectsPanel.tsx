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
  revokeProjectKey,
  updateProjectKey,
} from "@/lib/platform-projects";
import { allPages, v1, type V1Account } from "@/lib/platform-v1";
import { SETTINGS_SELECT_CLASS } from "@/components/settings/buttonStyles";
import { ACCEPTABLE_USE_URL, DEVELOPER_TERMS_URL } from "@/lib/platform-billing";
import { Link } from "react-router-dom";
import { useProjectContext } from "@/lib/project-context";
import {
  EXPIRY_PRESET_DAYS,
  expiryFromDate,
  expiryFromDays,
  getOrgPolicy,
  keyStatus,
  parseCidrList,
  validateExpiry,
  type OrgPolicy,
} from "@/lib/governance";

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
  // The selected project lives in the console's project context (header switcher).
  const { projects, project, select, reload, loading, disabled, error: loadError } = useProjectContext();
  const [actionError, setActionError] = useState<string | null>(null);
  const error = actionError ?? loadError;
  const setError = setActionError;

  const [showNewProject, setShowNewProject] = useState(false);
  const [projectName, setProjectName] = useState("");
  const [projectEnv, setProjectEnv] = useState<ProjectEnv>("sandbox");
  const [savingProject, setSavingProject] = useState(false);

  const handleCreateProject = useCallback(async () => {
    if (!projectName.trim()) return;
    setSavingProject(true);
    setError(null);
    try {
      const p = await createProject({ name: projectName.trim(), env: projectEnv });
      setProjectName("");
      setProjectEnv("sandbox");
      setShowNewProject(false);
      await reload();
      select(p.id);
    } catch (err) {
      setError(formatApiError(err, "Unable to create project"));
    } finally {
      setSavingProject(false);
    }
  }, [projectName, projectEnv, reload, select]);

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
            onClick={() => select(p.id)}
            aria-pressed={p.id === project?.id}
            className={cn(
              "inline-flex items-center gap-1.5 rounded-lg border border-solid px-3 py-1.5 text-[13px] font-medium transition-colors",
              p.id === project?.id
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

type ExpiryChoice = "keep" | "none" | "custom" | `${number}`;

/** Resolves the picker to an ISO time, `null` (never) or `undefined` (leave unchanged). */
function resolveExpiry(choice: ExpiryChoice, customDate: string): string | null | undefined {
  if (choice === "keep") return undefined;
  if (choice === "none") return null;
  if (choice === "custom") return expiryFromDate(customDate);
  return expiryFromDays(Number(choice));
}

function ExpiryField({
  choice,
  customDate,
  onChoice,
  onCustomDate,
  policy,
  allowKeep,
}: {
  choice: ExpiryChoice;
  customDate: string;
  onChoice: (c: ExpiryChoice) => void;
  onCustomDate: (d: string) => void;
  policy: OrgPolicy | null;
  allowKeep?: boolean;
}) {
  const max = policy?.max_key_ttl_days ?? null;
  const presets = EXPIRY_PRESET_DAYS.filter((d) => !max || d <= max);
  const maxDate = max ? new Date(Date.now() + max * 86400000).toISOString().slice(0, 10) : undefined;
  const minDate = new Date(Date.now() + 86400000).toISOString().slice(0, 10);
  return (
    <fieldset>
      <legend className="text-[12px] font-medium text-[var(--text-secondary)]">Expiry</legend>
      <div className="mt-1.5 flex flex-wrap items-center gap-x-4 gap-y-1.5 text-[13px] text-[var(--text-primary)]">
        {allowKeep && (
          <label className="inline-flex items-center gap-1.5">
            <input type="radio" checked={choice === "keep"} onChange={() => onChoice("keep")} /> Keep current
          </label>
        )}
        {!policy?.require_key_expiry && (
          <label className="inline-flex items-center gap-1.5">
            <input type="radio" checked={choice === "none"} onChange={() => onChoice("none")} /> No expiry
          </label>
        )}
        {presets.map((d) => (
          <label key={d} className="inline-flex items-center gap-1.5">
            <input type="radio" checked={choice === String(d)} onChange={() => onChoice(String(d) as ExpiryChoice)} /> {d} days
          </label>
        ))}
        <label className="inline-flex items-center gap-1.5">
          <input type="radio" checked={choice === "custom"} onChange={() => onChoice("custom")} /> Pick a date
        </label>
      </div>
      {choice === "custom" && (
        <input
          type="date"
          value={customDate}
          min={minDate}
          max={maxDate}
          onChange={(e) => onCustomDate(e.target.value)}
          aria-label="Expiry date"
          className={cn(INPUT_CLASS, "max-w-[200px]")}
        />
      )}
      {policy?.require_key_expiry || max ? (
        <p className="m-0 mt-1 text-[11px] text-[var(--text-tertiary)]">
          Your organization policy {policy?.require_key_expiry ? "requires keys to expire" : "limits key lifetime"}
          {max ? `, up to ${max} days` : ""}.
        </p>
      ) : null}
    </fieldset>
  );
}

function CidrField({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const { invalid } = parseCidrList(value);
  return (
    <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
      Allowed IP addresses (optional)
      <textarea
        value={value}
        onChange={(e) => onChange(e.target.value)}
        rows={2}
        placeholder="203.0.113.0/24, 2001:db8::/32"
        aria-invalid={invalid.length > 0}
        className={cn(INPUT_CLASS, "font-mono")}
      />
      {invalid.length > 0 ? (
        <span role="alert" className="mt-1 block text-[11px] font-normal text-[var(--status-error)]">
          Not a valid address or CIDR range: {invalid.join(", ")}
        </span>
      ) : (
        <span className="mt-1 block text-[11px] font-normal text-[var(--text-tertiary)]">
          Separate with commas or new lines. Leave blank to allow any address. Requests from other addresses are rejected.
        </span>
      )}
    </label>
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
  const [acceptTerms, setAcceptTerms] = useState(false);
  const [creating, setCreating] = useState(false);
  const [created, setCreated] = useState<CreatedProjectKey | null>(null);
  const [copied, setCopied] = useState(false);
  const [confirmRevoke, setConfirmRevoke] = useState<string | null>(null);
  // The project's accounts, for binding a key to one customer. null = couldn't load (free text instead).
  const [accounts, setAccounts] = useState<V1Account[] | null>([]);
  const [expiryChoice, setExpiryChoice] = useState<ExpiryChoice>("none");
  const [customDate, setCustomDate] = useState("");
  const [cidrText, setCidrText] = useState("");
  const [policy, setPolicy] = useState<OrgPolicy | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [editExpiry, setEditExpiry] = useState<ExpiryChoice>("keep");
  const [editDate, setEditDate] = useState("");
  const [editCidrs, setEditCidrs] = useState("");
  const [saving, setSaving] = useState(false);

  // Organization policy shapes the expiry choices. Only admins can read it; for others the server enforces it.
  useEffect(() => {
    let active = true;
    getOrgPolicy()
      .then((p) => {
        if (!active) return;
        setPolicy(p);
        if (p.require_key_expiry) setExpiryChoice((cur) => (cur === "none" ? (p.max_key_ttl_days && p.max_key_ttl_days < 90 ? "custom" : "90") : cur));
      })
      .catch(() => active && setPolicy(null));
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    const client = v1(project.id);
    allPages((after) => client.listAccounts(after))
      .then((list) => active && setAccounts(list))
      .catch(() => active && setAccounts(null));
    return () => {
      active = false;
    };
  }, [project.id]);

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
    if (!name.trim() || scopes.length === 0 || !acceptTerms) return;
    const expiresAt = resolveExpiry(expiryChoice, customDate) ?? null;
    if (expiryChoice === "custom" && !expiresAt) {
      setError("Pick an expiry date.");
      return;
    }
    const expiryError = validateExpiry(expiresAt, policy);
    if (expiryError) {
      setError(expiryError);
      return;
    }
    const { cidrs, invalid } = parseCidrList(cidrText);
    if (invalid.length > 0) {
      setError(`Fix the IP allowlist: ${invalid.join(", ")} is not a valid address or CIDR range.`);
      return;
    }
    setCreating(true);
    setError(null);
    try {
      const key = await createProjectKey(project.id, {
        name: name.trim(),
        scopes,
        accept_terms: acceptTerms,
        ...(accountId.trim() ? { account_id: accountId.trim() } : {}),
        ...(expiresAt ? { expires_at: expiresAt } : {}),
        ...(cidrs.length > 0 ? { allowed_cidrs: cidrs } : {}),
      });
      setCreated(key);
      setName("");
      setScopes(["agents"]);
      setAccountId("");
      setCidrText("");
      setCustomDate("");
      setExpiryChoice(policy?.require_key_expiry ? "90" : "none");
      await load();
    } catch (err) {
      setError(formatApiError(err, "Unable to create key"));
    } finally {
      setCreating(false);
    }
  }, [project.id, name, scopes, accountId, acceptTerms, expiryChoice, customDate, cidrText, policy, load]);

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

  const startEdit = (k: ProjectKey) => {
    setEditing(k.id);
    setEditExpiry("keep");
    setEditDate("");
    setEditCidrs((k.allowed_cidrs ?? []).join("\n"));
  };

  const handleSaveEdit = useCallback(
    async (keyId: string) => {
      const expiresAt = resolveExpiry(editExpiry, editDate);
      if (editExpiry === "custom" && !expiresAt) {
        setError("Pick an expiry date.");
        return;
      }
      if (expiresAt !== undefined) {
        const expiryError = validateExpiry(expiresAt, policy);
        if (expiryError) {
          setError(expiryError);
          return;
        }
      }
      const { cidrs, invalid } = parseCidrList(editCidrs);
      if (invalid.length > 0) {
        setError(`Fix the IP allowlist: ${invalid.join(", ")} is not a valid address or CIDR range.`);
        return;
      }
      setSaving(true);
      setError(null);
      try {
        await updateProjectKey(project.id, keyId, {
          ...(expiresAt !== undefined ? { expires_at: expiresAt } : {}),
          allowed_cidrs: cidrs,
        });
        setEditing(null);
        await load();
      } catch (err) {
        setError(formatApiError(err, "Unable to update the key"));
      } finally {
        setSaving(false);
      }
    },
    [project.id, editExpiry, editDate, editCidrs, policy, load],
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
              {accounts && accounts.length > 0 ? (
                <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
                  Account (optional)
                  <select value={accountId} onChange={(e) => setAccountId(e.target.value)} className={cn(SETTINGS_SELECT_CLASS, "mt-1.5 w-full")}>
                    <option value="">All accounts in the project</option>
                    {accounts.map((a) => (
                      <option key={a.id} value={a.id}>
                        {a.name} ({a.id})
                      </option>
                    ))}
                  </select>
                  <span className="mt-1 block text-[11px] font-normal text-[var(--text-tertiary)]">A key bound to one account only sees that customer's data.</span>
                </label>
              ) : (
                <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
                  Account (optional)
                  <input value={accountId} onChange={(e) => setAccountId(e.target.value)} placeholder="acct_… — limits the key to one customer" className={INPUT_CLASS} />
                </label>
              )}
              <ExpiryField choice={expiryChoice} customDate={customDate} onChoice={setExpiryChoice} onCustomDate={setCustomDate} policy={policy} />
              <CidrField value={cidrText} onChange={setCidrText} />
              <label className="flex items-start gap-2 text-[13px] text-[var(--text-primary)]">
                <input type="checkbox" className="mt-0.5" checked={acceptTerms} onChange={(e) => setAcceptTerms(e.target.checked)} />
                <span>
                  I accept the{" "}
                  <a href={DEVELOPER_TERMS_URL} target="_blank" rel="noreferrer" className="font-semibold text-[var(--accent-primary)] hover:underline">Developer Terms</a>{" "}
                  and the{" "}
                  <a href={ACCEPTABLE_USE_URL} target="_blank" rel="noreferrer" className="font-semibold text-[var(--accent-primary)] hover:underline">Acceptable Use Policy</a>.
                </span>
              </label>
              <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
                Keys work for reading and setup right away. Billable requests need a card on file first:{" "}
                <Link to={`/platform/billing?project=${encodeURIComponent(project.id)}`} className="font-semibold text-[var(--accent-primary)] hover:underline">add a payment method</Link>.
              </p>
              <button type="button" className={PRIMARY_BUTTON_CLASS} disabled={!name.trim() || scopes.length === 0 || !acceptTerms || creating} onClick={() => void handleCreate()}>
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
          {keys.map((k) => {
            const status = keyStatus(k);
            const cidrs = k.allowed_cidrs ?? [];
            return (
              <li key={k.id} className="px-4 py-3">
                <div className="flex flex-wrap items-center gap-3">
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2 text-[13px] font-medium text-[var(--text-primary)]">
                      {k.name}
                      <span
                        className={cn(
                          "rounded-full px-2 py-0.5 text-[11px] font-semibold",
                          status.state === "expired" && "bg-[var(--status-error)]/10 text-[var(--status-error)]",
                          status.state === "expiring" && "bg-[var(--status-warning)]/15 text-[var(--status-warning)]",
                          status.state === "active" && "bg-[var(--status-success)]/10 text-[var(--status-success)]",
                        )}
                      >
                        {status.label}
                      </span>
                    </div>
                    <div className="text-[12px] text-[var(--text-secondary)] break-words">
                      <code className="font-mono">{k.prefix}…</code> · {k.scopes.join(", ")}
                      {k.account_id ? <> · account <code className="font-mono">{k.account_id}</code></> : null} · created {formatDate(k.created_at)} · last used {k.last_used_at ? formatDate(k.last_used_at) : "never"}
                      {k.expires_at ? <> · expires {formatDate(k.expires_at)}</> : null}
                    </div>
                    <div className="text-[12px] text-[var(--text-secondary)] break-words">
                      {cidrs.length > 0 ? <>Allowed IPs: <code className="font-mono">{cidrs.join(", ")}</code></> : "Any IP address"}
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
                    <div className="flex items-center gap-2">
                      <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => (editing === k.id ? setEditing(null) : startEdit(k))} aria-label={`Edit ${k.name}`}>
                        Edit
                      </button>
                      <button type="button" className={DESTRUCTIVE_BUTTON_CLASS} onClick={() => setConfirmRevoke(k.id)} aria-label={`Revoke ${k.name}`}>
                        <Trash size={14} aria-hidden /> Revoke
                      </button>
                    </div>
                  )}
                </div>
                {editing === k.id && (
                  <div className="mt-3 space-y-3 rounded-lg border border-solid border-[var(--border-subtle)] p-3">
                    <ExpiryField choice={editExpiry} customDate={editDate} onChoice={setEditExpiry} onCustomDate={setEditDate} policy={policy} allowKeep />
                    <CidrField value={editCidrs} onChange={setEditCidrs} />
                    <div className="flex items-center gap-2">
                      <button type="button" className={PRIMARY_BUTTON_CLASS} disabled={saving} onClick={() => void handleSaveEdit(k.id)}>
                        {saving ? <CircleNotch size={14} className="animate-spin" aria-hidden /> : <Check size={14} aria-hidden />} Save
                      </button>
                      <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setEditing(null)}>Cancel</button>
                    </div>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
