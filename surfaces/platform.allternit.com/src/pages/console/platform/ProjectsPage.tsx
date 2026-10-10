import React, { useState } from "react";
import { Link } from "react-router-dom";
import { Archive, CircleNotch, Cube, Key, PencilSimple, Plus, X, CreditCard } from "@phosphor-icons/react";
import { formatApiError } from "@/lib/api-client";
import { createProject, updateProject, type PlatformProject, type ProjectEnv } from "@/lib/platform-projects";
import { Badge, EmptyState, MonoChip, SkeletonCard } from "@/components/console-ui";
import { DESTRUCTIVE_BUTTON_CLASS, QUIET_BUTTON_CLASS } from "@/components/console-ui/buttonStyles";
import { cn } from "@/lib/utils";
import {
  CARD_CLASS,
  ErrorBanner,
  INPUT_CLASS,
  PRIMARY_BUTTON_CLASS,
  PlatformHeader,
  formatDate,
  usePlatformProjects,
} from "./shared";

const PLAN_LABELS: Record<string, string> = {
  sandbox: "Sandbox (free)",
  payg: "Pay as you go",
  growth: "Growth",
  enterprise: "Enterprise",
};

function dollars(cents: number): string {
  return `$${(cents / 100).toLocaleString(undefined, { maximumFractionDigits: 2 })}`;
}

/** Platform API projects: one per app, sandbox or live. Keys live on the API keys page. */
export function PlatformProjectsPage() {
  const state = usePlatformProjects();
  const [showNew, setShowNew] = useState(false);
  const [name, setName] = useState("");
  const [env, setEnv] = useState<ProjectEnv>("sandbox");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const create = async () => {
    if (!name.trim()) return;
    setSaving(true);
    setError(null);
    try {
      const p = await createProject({ name: name.trim(), env });
      setName("");
      setEnv("sandbox");
      setShowNew(false);
      await state.reload();
      state.select(p.id);
    } catch (err) {
      setError(formatApiError(err, "Unable to create the project"));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="mx-auto w-full max-w-5xl">
      <PlatformHeader
        title="Projects"
        subtitle="A project is one app: its accounts, agents, numbers and API keys. Sandbox projects use test keys and simulated numbers."
        actions={
          !state.disabled && !state.loading ? (
            <button type="button" className={PRIMARY_BUTTON_CLASS} onClick={() => setShowNew(true)}>
              <Plus size={14} aria-hidden /> New project
            </button>
          ) : null
        }
      />

      {error && <ErrorBanner message={error} className="mb-4" />}

      {showNew && (
        <div className="mb-4 space-y-3 rounded-xl border border-solid border-[var(--accent-primary)]/25 bg-[var(--accent-primary)]/[0.03] p-4">
          <div className="flex items-center justify-between gap-3">
            <div className="text-[14px] font-semibold text-[var(--text-primary)]">New project</div>
            <button type="button" onClick={() => setShowNew(false)} className="rounded-md p-1 text-[var(--text-tertiary)] hover:bg-[var(--surface-hover)]" aria-label="Close">
              <X size={16} />
            </button>
          </div>
          <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
            Name
            <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Front desk app" maxLength={120} className={INPUT_CLASS} />
          </label>
          <fieldset>
            <legend className="text-[12px] font-medium text-[var(--text-secondary)]">Environment</legend>
            <div className="mt-1.5 flex flex-col gap-1.5 text-[13px] text-[var(--text-primary)] sm:flex-row sm:gap-4">
              {(["sandbox", "live"] as const).map((e) => (
                <label key={e} className="inline-flex items-center gap-1.5">
                  <input type="radio" name="new-project-env" checked={env === e} onChange={() => setEnv(e)} />
                  {e === "sandbox" ? "Sandbox: test keys, simulated numbers" : "Live: real traffic, card on file"}
                </label>
              ))}
            </div>
          </fieldset>
          <button type="button" className={PRIMARY_BUTTON_CLASS} disabled={!name.trim() || saving} onClick={() => void create()}>
            {saving ? <CircleNotch size={14} className="animate-spin" aria-hidden /> : <Plus size={14} aria-hidden />} Create project
          </button>
        </div>
      )}

      {state.loading ? (
        <div className="space-y-3" role="status" aria-label="Loading projects">
          <SkeletonCard />
          <SkeletonCard rows={2} />
        </div>
      ) : state.disabled ? (
        <EmptyState
          icon={<Cube size={32} aria-hidden />}
          title="Platform API is in private beta"
          caption="Projects for the developer Platform API appear here once it is switched on for your account."
        />
      ) : state.error ? (
        <ErrorBanner message={state.error} />
      ) : state.projects.length === 0 ? (
        <div className={CARD_CLASS}>
          <EmptyState
            icon={<Cube size={32} aria-hidden />}
            title="No projects yet"
            caption="Start with a sandbox project. It is free and uses test keys."
            ctaLabel="New project"
            onCtaClick={() => setShowNew(true)}
            primaryCta
          />
        </div>
      ) : (
        <ul className="m-0 list-none space-y-3 p-0">
          {state.projects.map((p) => (
            <ProjectRow key={p.id} project={p} current={p.id === state.project?.id} onSelect={() => state.select(p.id)} onChanged={state.reload} onError={setError} />
          ))}
        </ul>
      )}
    </div>
  );
}

function ProjectRow({
  project,
  current,
  onSelect,
  onChanged,
  onError,
}: {
  project: PlatformProject;
  current: boolean;
  onSelect: () => void;
  onChanged: () => Promise<void>;
  onError: (msg: string | null) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(project.name);
  const [cap, setCap] = useState(String(project.spend_cap_cents / 100));
  const [busy, setBusy] = useState(false);
  const [confirmArchive, setConfirmArchive] = useState(false);

  const save = async () => {
    const capCents = Math.round(Number(cap) * 100);
    if (!name.trim() || !Number.isFinite(capCents) || capCents < 0) {
      onError("Enter a name and a spend cap of $0 or more.");
      return;
    }
    setBusy(true);
    onError(null);
    try {
      await updateProject(project.id, { name: name.trim(), spend_cap_cents: capCents });
      setEditing(false);
      await onChanged();
    } catch (err) {
      onError(formatApiError(err, "Unable to save the project"));
    } finally {
      setBusy(false);
    }
  };

  const archive = async () => {
    setBusy(true);
    onError(null);
    try {
      await updateProject(project.id, { archived: true });
      await onChanged();
    } catch (err) {
      onError(formatApiError(err, "Unable to archive the project"));
      setBusy(false);
    }
  };

  return (
    <li className={cn(CARD_CLASS, "p-4", current && "border-[var(--accent-primary)]/40")}>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-[15px] font-semibold text-[var(--text-primary)]">{project.name}</span>
            <Badge className={project.env === "live" ? "bg-[var(--status-success)]/10 text-[var(--status-success)]" : undefined}>
              {project.env === "live" ? "Live" : "Sandbox"}
            </Badge>
            {current && <Badge>Selected</Badge>}
          </div>
          <div className="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1 text-[12px] text-[var(--text-secondary)]">
            <span>Plan: {PLAN_LABELS[project.plan] ?? project.plan}</span>
            <span>Monthly spend cap: {dollars(project.spend_cap_cents)}</span>
            <span>Created {formatDate(project.created_at)}</span>
          </div>
          <MonoChip className="mt-2 max-w-full truncate">{project.id}</MonoChip>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {!current && (
            <button type="button" className={QUIET_BUTTON_CLASS} onClick={onSelect}>
              Use this project
            </button>
          )}
          <Link to="/api-keys" onClick={onSelect} className={QUIET_BUTTON_CLASS}>
            <Key size={14} aria-hidden /> Keys
          </Link>
          <Link to={`/platform/billing?project=${encodeURIComponent(project.id)}`} onClick={onSelect} className={QUIET_BUTTON_CLASS}>
            <CreditCard size={14} aria-hidden /> Billing
          </Link>
          <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setEditing((v) => !v)} aria-expanded={editing}>
            <PencilSimple size={14} aria-hidden /> Edit
          </button>
        </div>
      </div>

      {editing && (
        <div className="mt-4 grid gap-3 border-t border-solid border-[var(--border-subtle)] pt-4 sm:grid-cols-2">
          <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
            Name
            <input value={name} onChange={(e) => setName(e.target.value)} maxLength={120} className={INPUT_CLASS} />
          </label>
          <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
            Monthly spend cap (USD)
            <input value={cap} onChange={(e) => setCap(e.target.value)} inputMode="decimal" className={INPUT_CLASS} />
          </label>
          <div className="flex flex-wrap items-center gap-2 sm:col-span-2">
            <button type="button" className={PRIMARY_BUTTON_CLASS} disabled={busy} onClick={() => void save()}>
              {busy ? <CircleNotch size={14} className="animate-spin" aria-hidden /> : null} Save
            </button>
            {confirmArchive ? (
              <>
                <span className="text-[12px] text-[var(--text-secondary)]">Archive stops every key in this project.</span>
                <button type="button" className={DESTRUCTIVE_BUTTON_CLASS} disabled={busy} onClick={() => void archive()}>
                  <Archive size={14} aria-hidden /> Archive
                </button>
                <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setConfirmArchive(false)}>
                  Cancel
                </button>
              </>
            ) : (
              <button type="button" className={DESTRUCTIVE_BUTTON_CLASS} onClick={() => setConfirmArchive(true)}>
                <Archive size={14} aria-hidden /> Archive project
              </button>
            )}
          </div>
        </div>
      )}
    </li>
  );
}
