import React, { useEffect, useMemo, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import { InformationCircleIcon, Tick02Icon } from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { Badge } from "@/components/console-ui";
import type { ConsoleModel } from "@/lib/console-models";
import type { AgentRecord } from "@/lib/managed-agents";
import {
  ACCESS_LABEL,
  ALLTERNIT_RUNTIME,
  CATEGORY_LABEL,
  DEFAULT_RUNTIME_ID,
  cachedRuntimes,
  fetchRuntimes,
  planProvider,
  PLAN_NAME,
  allowed,
  findRuntime,
  fits,
  planRefusal,
  planOf,
  providerFor,
  type AccessKind,
  type RuntimeEntry,
} from "@/lib/runtimes";

/**
 * Runtime + Brain picker for console agents (Harness & Bots 2.0, approved
 * design A5 in the console's own style).
 *
 *   Runtime · How it works   — grid of registry runtimes, category + status badges
 *   Brain   · How it thinks  — the Cloud model list, marked by `fits`
 *   Access                   — how the brain is paid for (plan only where `allowed`)
 *
 * Picking a runtime that can't use the chosen brain switches to the first
 * allowed brain (or off a plan) and shows a one-line note. Server 422 errors
 * (`runtime_unknown`, `brain_not_fit`, `plan_not_allowed_on_runtime`) show
 * under the matching field.
 */

const INPUT_CLASS =
  "w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]";

const LABEL_CLASS = "text-[12px] font-semibold text-[var(--text-secondary)]";

export type RuntimeField = "runtime" | "brain" | "access";
export interface RuntimeFieldError {
  field: RuntimeField;
  message: string;
}

export const FIELD_OF_CODE: Record<string, RuntimeField> = {
  runtime_unknown: "runtime",
  brain_not_fit: "brain",
  plan_not_allowed_on_runtime: "access",
};

/** Vendor provider of a console model: `owned_by`, else the `vendor/` id prefix, else allternit. */
export function modelProvider(m: Pick<ConsoleModel, "id" | "owned_by"> | null | undefined, fallbackId?: string): string {
  const owned = m?.owned_by;
  if (owned && owned !== "unknown") return owned.toLowerCase();
  const id = m?.id ?? fallbackId ?? "";
  const prefix = id.split("/")[0];
  return prefix && prefix !== id ? prefix.toLowerCase() : "allternit";
}

/** Access kinds a runtime offers for a brain from `provider`. */
export function accessChoices(runtime: RuntimeEntry, provider: string): AccessKind[] {
  const base: AccessKind[] = runtime.access.length > 0 ? runtime.access : ["credits", "key", "plan"];
  return base.filter((a) => a !== "plan" || Boolean(planOf(provider)));
}

export function accessPhrase(access: AccessKind, provider: string): string {
  switch (access) {
    case "plan": {
      const plan = planOf(provider);
      return plan ? `on your ${PLAN_NAME[plan] ?? plan} plan` : "on your plan";
    }
    case "key":
      return "with your API key";
    case "local":
      return "on the computer it runs on";
    case "credits":
      return "with Allternit credits";
  }
}

const ACCESS_TONE: Record<AccessKind, string> = {
  plan: "text-[var(--status-success)]",
  key: "text-[var(--status-warning)]",
  local: "text-[var(--accent-primary)]",
  credits: "text-[var(--text-primary)]",
};

export function AccessBadge({ access }: { access: AccessKind }): React.ReactNode {
  return <Badge className={ACCESS_TONE[access]}>{ACCESS_LABEL[access]}</Badge>;
}

/** The registry for read-only labels: cached copy at once, then a refresh; Allternit only on failure. */
export function useRuntimes(): RuntimeEntry[] {
  const [list, setList] = useState<RuntimeEntry[]>(() => cachedRuntimes() ?? [ALLTERNIT_RUNTIME]);
  useEffect(() => {
    let active = true;
    fetchRuntimes()
      .then((next) => {
        if (active) setList(next);
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, []);
  return list;
}

/** `config.botBrain` of an agent record (allternit-api stores runtime/access there, like bots). */
export function agentBotBrain(agent: Pick<AgentRecord, "config">): { runtime?: string; access?: string } {
  const cfg = (agent.config ?? {}) as { botBrain?: unknown };
  const bb = cfg.botBrain;
  return bb && typeof bb === "object" ? (bb as { runtime?: string; access?: string }) : {};
}

/** An agent's runtime id: `config.botBrain.runtime`, else allternit. */
export function agentRuntimeId(agent: AgentRecord): string {
  const rt = agentBotBrain(agent).runtime;
  return typeof rt === "string" && rt ? rt : DEFAULT_RUNTIME_ID;
}

/** An agent's access: `config.botBrain.access`, else plan for a plan provider, else credits. */
export function agentAccess(agent: AgentRecord, runtimes: readonly RuntimeEntry[] = []): AccessKind {
  const raw = agentBotBrain(agent).access;
  if (raw === "plan" || raw === "key" || raw === "local" || raw === "credits") return raw;
  return planProvider(agent.provider ?? "", runtimes) ? "plan" : "credits";
}

/** Runtime name + access badge, for the agent list and detail pages. */
export function RuntimeLabel({
  runtimes,
  runtimeId,
  access,
}: {
  runtimes: readonly RuntimeEntry[];
  runtimeId: string | null | undefined;
  access: AccessKind | null | undefined;
}): React.ReactNode {
  const id = runtimeId || DEFAULT_RUNTIME_ID;
  const name = findRuntime(runtimes, id)?.name ?? (id === DEFAULT_RUNTIME_ID ? "Allternit" : id);
  return (
    <span className="inline-flex items-center gap-1.5">
      <span>{name}</span>
      {access ? <AccessBadge access={access} /> : null}
    </span>
  );
}

function FieldError({ id, message }: { id: string; message: string | null | undefined }): React.ReactNode {
  if (!message) return null;
  return (
    <p id={id} role="alert" className="m-0 text-[12px] text-[var(--status-error)]">
      {message}
    </p>
  );
}

interface Props {
  runtimes: readonly RuntimeEntry[];
  runtimesError: string | null;
  runtimeId: string;
  onRuntimeChange: (id: string) => void;
  models: readonly ConsoleModel[];
  modelsError: string | null;
  model: string;
  onModelChange: (id: string) => void;
  defaultModelId: string;
  access: AccessKind;
  onAccessChange: (access: AccessKind) => void;
  fieldError: RuntimeFieldError | null;
  onClearFieldError: () => void;
}

export function RuntimeBrainPicker({
  runtimes,
  runtimesError,
  runtimeId,
  onRuntimeChange,
  models,
  modelsError,
  model,
  onModelChange,
  defaultModelId,
  access,
  onAccessChange,
  fieldError,
  onClearFieldError,
}: Props): React.ReactNode {
  const [note, setNote] = useState<string | null>(null);
  const [planHint, setPlanHint] = useState<string | null>(null);
  const [showMore, setShowMore] = useState(false);

  // A saved runtime the registry doesn't list (route not deployed, or removed)
  // stays visible as its own tile instead of silently reading as Allternit.
  const known = findRuntime(runtimes, runtimeId);
  const unlisted: RuntimeEntry | null =
    known || !runtimeId
      ? null
      : { ...ALLTERNIT_RUNTIME, id: runtimeId, name: runtimeId, vendor: runtimeId, category: "general", tool: runtimeId, plans: undefined, status: { label: "Not in the runtime list", tone: "warn" } };
  const allRuntimes: readonly RuntimeEntry[] = unlisted ? [...runtimes, unlisted] : runtimes;
  const runtime = known ?? unlisted ?? runtimes[0];
  const currentModel = models.find((m) => m.id === model);
  const brainId = model || defaultModelId;
  const provider = modelProvider(currentModel, brainId);
  const brainName = currentModel?.display_name ?? (model || defaultModelId);

  // Default first, then the catalog order: the auto-switch picks from this.
  const orderedModels = useMemo(() => {
    const def = models.find((m) => m.id === defaultModelId);
    return def ? [def, ...models.filter((m) => m.id !== def.id)] : [...models];
  }, [models, defaultModelId]);

  const visibleRuntimes = useMemo(() => {
    const list = allRuntimes.filter((r) => showMore || !r.more || r.id === runtime?.id);
    return list;
  }, [allRuntimes, showMore, runtime?.id]);
  const hiddenCount = allRuntimes.filter((r) => r.more && r.id !== runtime?.id).length;

  if (!runtime) return null;

  const pickRuntime = (next: RuntimeEntry): void => {
    onClearFieldError();
    setPlanHint(null);
    onRuntimeChange(next.id);
    const notes: string[] = [];
    let nextProvider = provider;
    if (!fits(next, provider) && orderedModels.length > 0) {
      const first = orderedModels.find((m) => allowed(next, modelProvider(m), runtimes));
      if (first) {
        onModelChange(first.id);
        nextProvider = modelProvider(first);
        const runs = next.brains === "any" ? "" : next.brains.join(", ");
        notes.push(
          `${next.name} runs ${runs || "its own"} models only, so the brain changed to ${first.display_name ?? first.id}.`
        );
      }
    }
    const choices = accessChoices(next, nextProvider);
    const planBlocked = access === "plan" && !allowed(next, providerFor(nextProvider, "plan"), runtimes);
    if (planBlocked || !choices.includes(access)) {
      const fallback = choices.find((a) => a !== "plan") ?? choices[0];
      if (fallback && fallback !== access) {
        onAccessChange(fallback);
        if (access === "plan") {
          const plan = planOf(nextProvider);
          notes.push(
            `${plan ? PLAN_NAME[plan] ?? plan : "That"} plan can't be used inside ${next.name}, so access changed to ${ACCESS_LABEL[fallback]}.`
          );
        }
      }
    }
    setNote(notes.length > 0 ? notes.join(" ") : null);
  };

  const pickModel = (id: string): void => {
    onClearFieldError();
    setNote(null);
    setPlanHint(null);
    onModelChange(id);
    const p = modelProvider(models.find((m) => m.id === id), id);
    const choices = accessChoices(runtime, p);
    if (!choices.includes(access) || (access === "plan" && !allowed(runtime, providerFor(p, "plan"), runtimes))) {
      const fallback = choices.find((a) => a !== "plan") ?? choices[0];
      if (fallback) onAccessChange(fallback);
    }
  };

  const pickAccess = (a: AccessKind): void => {
    onClearFieldError();
    if (a === "plan" && !allowed(runtime, providerFor(provider, "plan"), runtimes)) {
      const plan = planOf(provider);
      setPlanHint(plan ? planRefusal(runtimes, runtime, providerFor(provider, "plan")) : null);
      return;
    }
    setPlanHint(null);
    onAccessChange(a);
  };

  const errorFor = (field: RuntimeField): string | null =>
    fieldError && fieldError.field === field ? fieldError.message : null;
  const choices = accessChoices(runtime, provider);

  return (
    <div className="space-y-4 rounded-xl border border-solid border-[var(--border-subtle)] p-3">
      {/* Runtime */}
      <div className="space-y-2">
        <div className="flex items-baseline justify-between gap-2">
          <span id="agent-runtime-label" className={LABEL_CLASS}>
            Runtime <span className="font-normal text-[var(--text-tertiary)]">· How it works</span>
          </span>
          {runtimesError && (
            <span className="text-[11px] text-[var(--text-tertiary)]">Runtime list unavailable — showing Allternit.</span>
          )}
        </div>
        <div
          role="group"
          aria-labelledby="agent-runtime-label"
          aria-describedby={errorFor("runtime") ? "agent-runtime-error" : undefined}
          className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3"
        >
          {visibleRuntimes.map((r) => {
            const selected = r.id === runtime.id;
            const canRun = fits(r, provider) || orderedModels.some((m) => allowed(r, modelProvider(m), runtimes));
            return (
              <button
                key={r.id}
                type="button"
                aria-pressed={selected}
                disabled={!canRun}
                onClick={() => pickRuntime(r)}
                className={cn(
                  "flex min-h-[44px] items-start gap-2 rounded-lg border border-solid px-3 py-2 text-left transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-[var(--accent-primary)]",
                  selected
                    ? "border-[var(--text-primary)] bg-[var(--bg-secondary)]"
                    : "border-[var(--border-subtle)] bg-transparent",
                  canRun ? "cursor-pointer" : "cursor-not-allowed opacity-45"
                )}
              >
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[13px] font-semibold text-[var(--text-primary)]">{r.name}</span>
                  <span className="mt-1 flex flex-wrap gap-1">
                    <Badge>{CATEGORY_LABEL[r.category] ?? r.category}</Badge>
                    {r.status && (
                      <Badge
                        className={
                          r.status.tone === "warn" ? "text-[var(--status-warning)]" : "text-[var(--accent-primary)]"
                        }
                      >
                        {r.status.label}
                      </Badge>
                    )}
                  </span>
                </span>
                {selected && (
                  <span
                    aria-hidden
                    className="grid h-[18px] w-[18px] flex-none place-items-center rounded-full bg-[var(--text-primary)] text-[var(--bg-primary)]"
                  >
                    <HugeiconsIcon icon={Tick02Icon} size={12} />
                  </span>
                )}
              </button>
            );
          })}
        </div>
        {hiddenCount > 0 && (
          <button
            type="button"
            aria-expanded={showMore}
            onClick={() => setShowMore((v) => !v)}
            className="text-[12px] font-semibold text-[var(--accent-primary)]"
          >
            {showMore ? "Show fewer runtimes" : `Show ${hiddenCount} more runtime${hiddenCount === 1 ? "" : "s"}`}
          </button>
        )}
        <FieldError id="agent-runtime-error" message={errorFor("runtime")} />
        {runtime.id !== DEFAULT_RUNTIME_ID && (
          <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
            Runs on a computer that has {runtime.name} installed.
          </p>
        )}
      </div>

      {/* Brain */}
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="space-y-1">
          <label htmlFor="agent-model" className={LABEL_CLASS}>
            Brain <span className="font-normal text-[var(--text-tertiary)]">· How it thinks</span>
          </label>
          {models.length > 0 ? (
            <select
              id="agent-model"
              value={model}
              onChange={(e) => pickModel(e.target.value)}
              aria-invalid={Boolean(errorFor("brain"))}
              aria-describedby={errorFor("brain") ? "agent-model-error" : undefined}
              className={INPUT_CLASS}
            >
              <option value="">Cloud default ({defaultModelId})</option>
              {!models.some((m) => m.id === model) && model && <option value={model}>{model} (current)</option>}
              {models.map((m) => {
                const ok = fits(runtime, modelProvider(m));
                return (
                  <option key={m.id} value={m.id} disabled={!ok}>
                    {m.display_name ?? m.id} ({m.id}){m.default ? " · default" : ""}
                    {ok ? "" : ` · not on ${runtime.name}`}
                  </option>
                );
              })}
            </select>
          ) : (
            <input
              id="agent-model"
              type="text"
              value={model}
              onChange={(e) => pickModel(e.target.value)}
              placeholder={`model id, e.g. ${defaultModelId}`}
              aria-invalid={Boolean(errorFor("brain"))}
              aria-describedby={errorFor("brain") ? "agent-model-error" : undefined}
              className={INPUT_CLASS}
            />
          )}
          {modelsError && (
            <p className="m-0 text-[12px] text-[var(--status-warning)]">
              Live model catalog unavailable — enter a model id manually.
            </p>
          )}
          <FieldError id="agent-model-error" message={errorFor("brain")} />
        </div>

        {/* Access */}
        <div className="space-y-1">
          <span id="agent-access-label" className={LABEL_CLASS}>
            Access
          </span>
          <div
            role="group"
            aria-labelledby="agent-access-label"
            aria-describedby={planHint || errorFor("access") ? "agent-access-error" : undefined}
            className="flex flex-wrap gap-1.5"
          >
            {choices.map((a) => {
              const blocked = a === "plan" && !allowed(runtime, providerFor(provider, "plan"), runtimes);
              const selected = access === a;
              return (
                <button
                  key={a}
                  type="button"
                  aria-pressed={selected}
                  aria-disabled={blocked || undefined}
                  onClick={() => pickAccess(a)}
                  className={cn(
                    "min-h-[36px] rounded-lg border border-solid px-3 py-1.5 text-[12px] font-semibold transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-[var(--accent-primary)]",
                    selected
                      ? "border-[var(--text-primary)] bg-[var(--bg-secondary)] text-[var(--text-primary)]"
                      : "border-[var(--border-subtle)] text-[var(--text-secondary)]",
                    blocked && "opacity-45"
                  )}
                >
                  {a === "plan" && planOf(provider) ? `${PLAN_NAME[planOf(provider)!]} plan` : ACCESS_LABEL[a]}
                </button>
              );
            })}
          </div>
          <FieldError id="agent-access-error" message={errorFor("access") ?? planHint} />
        </div>
      </div>

      {note && (
        <div
          role="status"
          className="flex gap-2 rounded-lg bg-[var(--bg-secondary)] px-3 py-2 text-[12px] text-[var(--accent-primary)]"
        >
          <HugeiconsIcon icon={InformationCircleIcon} size={14} className="mt-px flex-none" />
          <span>{note}</span>
        </div>
      )}

      <p className="m-0 flex items-center gap-2 border-t border-solid border-[var(--border-subtle)] pt-3 text-[13px] text-[var(--text-secondary)]">
        <span aria-hidden className="h-1.5 w-1.5 flex-none rounded-full bg-[var(--status-success)]" />
        This agent works with {runtime.name} and thinks with {brainName}, {accessPhrase(access, provider)}.
      </p>
    </div>
  );
}
