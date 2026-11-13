import React, { useCallback, useEffect, useState } from "react";
import { ArrowLeft, Scales } from "@phosphor-icons/react";
import { formatApiError } from "@/lib/api-client";
import type { V1Client } from "@/lib/platform-v1";
import {
  OUTCOME_STATUSES,
  REFUSAL_LABELS,
  formatPct,
  hasOutcomes,
  topAnswer,
  type DecisionAnswer,
  type DecisionOutcome,
  type OutcomeStatus,
  type V1Decision,
} from "@/lib/decisions";
import { EmptyState, MonoChip, SkeletonRow } from "@/components/console-ui";
import { QUIET_BUTTON_CLASS, SETTINGS_SELECT_CLASS } from "@/components/console-ui/buttonStyles";
import { cn } from "@/lib/utils";
import { CARD_CLASS, ErrorBanner, INPUT_CLASS, PRIMARY_BUTTON_CLASS, PlatformHeader, ProjectGate, formatDateTime, usePlatformProjects } from "./shared";

const DOCS_URL = "https://docs.allternit.com/api/platform/decisions";

/** Decisions made with POST /v1/decisions, their answers, and the outcomes you record. */
export function PlatformDecisionsPage() {
  const state = usePlatformProjects();
  return (
    <div className="mx-auto w-full max-w-6xl">
      <PlatformHeader title="Decisions" subtitle="Answers your project got from POST /v1/decisions, and what happened afterwards." state={state} />
      <ProjectGate state={state}>{({ project, client }) => <DecisionsBody key={project.id} client={client} />}</ProjectGate>
    </div>
  );
}

function DecisionsBody({ client }: { client: V1Client }) {
  const [items, setItems] = useState<V1Decision[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);

  const load = useCallback(
    async (after: string | null) => {
      setLoading(true);
      setError(null);
      try {
        const page = await client.listDecisions(after);
        setItems((cur) => (after ? [...cur, ...page.data] : page.data));
        setCursor(page.next_cursor);
        setHasMore(page.has_more);
      } catch (err) {
        setError(formatApiError(err, "Unable to load decisions"));
      } finally {
        setLoading(false);
      }
    },
    [client],
  );

  useEffect(() => {
    void load(null);
  }, [load]);

  const updateItem = (d: V1Decision) => setItems((cur) => cur.map((x) => (x.id === d.id ? d : x)));
  const current = items.find((d) => d.id === selected) ?? null;

  if (items.length === 0 && loading) {
    return (
      <div className={cn(CARD_CLASS, "px-4")} role="status" aria-label="Loading decisions">
        <SkeletonRow lines={3} />
      </div>
    );
  }
  if (items.length === 0 && error) return <ErrorBanner message={error} />;
  if (items.length === 0) {
    return (
      <div className={CARD_CLASS}>
        <EmptyState
          icon={<Scales size={32} aria-hidden />}
          title="No decisions yet"
          caption="Decisions you make with POST /v1/decisions show up here."
        />
        <div className="space-y-2 px-4 pb-5">
          <pre className="m-0 overflow-x-auto rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-3 font-mono text-[12px] text-[var(--text-primary)]">{`curl https://api.allternit.com/v1/decisions \\
  -H "Authorization: Bearer $ALLTERNIT_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{"input":"My parcel arrived broken","questions":[{"name":"is_damaged","type":"predicate","prompt":"Is the item damaged?"}]}'`}</pre>
          <a href={DOCS_URL} target="_blank" rel="noreferrer" className="text-[13px] font-medium text-[var(--accent-primary)] hover:underline">
            Decisions API docs
          </a>
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {error && <ErrorBanner message={error} />}
      <div className="grid gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,420px)]">
        <div className={cn(selected ? "hidden lg:block" : "block")}>
          <section className={CARD_CLASS} aria-label="Decisions">
            <ul className="m-0 list-none divide-y divide-[var(--border-subtle)] p-0">
              {items.map((d) => (
                <li key={d.id}>
                  <button
                    type="button"
                    onClick={() => setSelected(d.id)}
                    aria-current={d.id === selected ? "true" : undefined}
                    className={cn(
                      "w-full px-4 py-3 text-left transition-colors hover:bg-[var(--surface-hover)]",
                      d.id === selected && "bg-[var(--accent-primary)]/[0.06]",
                    )}
                  >
                    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[12px] text-[var(--text-secondary)]">
                      <span>{formatDateTime(d.created_at)}</span>
                      {d.backend && <MonoChip>{d.backend}</MonoChip>}
                      {typeof d.latency_ms === "number" && <span>{d.latency_ms} ms</span>}
                      <span>{hasOutcomes(d) ? "Outcomes recorded" : "No outcomes"}</span>
                    </div>
                    <div className="mt-1.5 flex flex-wrap gap-x-4 gap-y-1 text-[13px] text-[var(--text-primary)]">
                      {d.answers.map((a) => (
                        <span key={a.name} className="min-w-0 break-words">
                          <span className="font-mono text-[12px] text-[var(--text-secondary)]">{a.name}</span> {topAnswer(a)}
                        </span>
                      ))}
                    </div>
                  </button>
                </li>
              ))}
            </ul>
            {hasMore && (
              <div className="border-t border-solid border-[var(--border-subtle)] p-3 text-center">
                <button type="button" className={QUIET_BUTTON_CLASS} disabled={loading} onClick={() => void load(cursor)}>
                  {loading ? "Loading…" : "Load more"}
                </button>
              </div>
            )}
          </section>
        </div>
        <div className={cn(selected ? "block" : "hidden lg:block")}>
          {current ? (
            <DecisionDetail key={current.id} client={client} decision={current} onSaved={updateItem} onBack={() => setSelected(null)} />
          ) : (
            <div className={cn(CARD_CLASS, "flex min-h-[200px] items-center justify-center p-6 text-[13px] text-[var(--text-secondary)]")}>
              Pick a decision to see its answers.
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function AnswerCard({ answer }: { answer: DecisionAnswer }) {
  const refusal = answer.type === "refusal";
  return (
    <li className="rounded-lg border border-solid border-[var(--border-subtle)] p-3">
      <div className="flex flex-wrap items-center gap-2">
        <span className="font-mono text-[12px] font-semibold text-[var(--text-primary)]">{answer.name}</span>
        <MonoChip>{answer.type}</MonoChip>
        {answer.backend && <MonoChip>{answer.backend}</MonoChip>}
      </div>
      <div className="mt-1 text-[13px] text-[var(--text-primary)]">{topAnswer(answer)}</div>
      {refusal ? (
        <div className="mt-1 text-[12px] text-[var(--status-error)]">Refused: {REFUSAL_LABELS[answer.reason ?? ""] ?? answer.reason ?? "no reason given"}</div>
      ) : (
        typeof answer.confidence === "number" && <div className="mt-1 text-[12px] text-[var(--text-secondary)]">Confidence {formatPct(answer.confidence)}</div>
      )}
      {answer.probabilities && answer.probabilities.length > 0 && (
        <ul className="m-0 mt-2 list-none space-y-1.5 p-0">
          {answer.probabilities.map((p) => {
            const name = p.label ?? String(p.value);
            return (
              <li key={`${p.value}`} className="grid grid-cols-[minmax(0,90px)_minmax(0,1fr)_48px] items-center gap-2 text-[12px] text-[var(--text-secondary)]">
                <span className="truncate" title={name}>{name}</span>
                <span className="h-1.5 overflow-hidden rounded-full bg-[var(--bg-secondary)]" role="img" aria-label={`${name} ${formatPct(p.probability)}`}>
                  <span className="block h-full rounded-full bg-[var(--accent-primary)]" style={{ width: `${Math.max(0, Math.min(1, p.probability)) * 100}%` }} />
                </span>
                <span className="text-right tabular-nums">{formatPct(p.probability)}</span>
              </li>
            );
          })}
        </ul>
      )}
    </li>
  );
}

type Draft = Record<string, { status: OutcomeStatus | ""; label: string }>;

function DecisionDetail({
  client,
  decision,
  onSaved,
  onBack,
}: {
  client: V1Client;
  decision: V1Decision;
  onSaved: (d: V1Decision) => void;
  onBack: () => void;
}) {
  const initial = (d: V1Decision): Draft =>
    Object.fromEntries(
      d.answers.map((a) => {
        const o = d.outcomes?.[a.name];
        return [a.name, { status: (o?.status ?? "") as OutcomeStatus | "", label: o?.label ?? "" }];
      }),
    );
  const [draft, setDraft] = useState<Draft>(() => initial(decision));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  const save = async () => {
    const outcomes: Record<string, DecisionOutcome> = {};
    for (const [name, v] of Object.entries(draft)) {
      if (!v.status) continue;
      const prev = decision.outcomes?.[name];
      outcomes[name] = { status: v.status, ...(v.label.trim() ? { label: v.label.trim() } : {}), ...(prev?.detail ? { detail: prev.detail } : {}) };
    }
    setSaving(true);
    setError(null);
    setSaved(false);
    try {
      const updated = await client.recordOutcomes(decision.id, outcomes);
      onSaved(updated);
      setDraft(initial(updated));
      setSaved(true);
    } catch (err) {
      setError(formatApiError(err, "Unable to save outcomes"));
    } finally {
      setSaving(false);
    }
  };

  const meta = decision.metadata ?? {};
  const saveable = Object.values(draft).some((v) => v.status);

  return (
    <section className={CARD_CLASS} aria-label={`Decision ${decision.id}`}>
      <div className="flex flex-wrap items-center gap-2 border-b border-solid border-[var(--border-subtle)] px-4 py-3">
        <button type="button" onClick={onBack} className={cn(QUIET_BUTTON_CLASS, "lg:hidden")} aria-label="Back to decisions">
          <ArrowLeft size={14} aria-hidden /> Back
        </button>
        <MonoChip className="max-w-full truncate">{decision.id}</MonoChip>
        <span className="text-[12px] text-[var(--text-secondary)]">{formatDateTime(decision.created_at)}</span>
      </div>
      <div className="space-y-5 p-4">
        <div>
          <h3 className="m-0 mb-2 text-[13px] font-semibold text-[var(--text-primary)]">Answers</h3>
          <ul className="m-0 list-none space-y-2 p-0">
            {decision.answers.map((a) => (
              <AnswerCard key={a.name} answer={a} />
            ))}
          </ul>
        </div>

        <div>
          <h3 className="m-0 mb-2 text-[13px] font-semibold text-[var(--text-primary)]">Usage and metadata</h3>
          <dl className="m-0 grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1 text-[12px] text-[var(--text-secondary)]">
            {typeof decision.usage?.input_tokens === "number" && (<><dt>Input tokens</dt><dd className="m-0 text-[var(--text-primary)]">{decision.usage.input_tokens}</dd></>)}
            {typeof decision.usage?.questions === "number" && (<><dt>Questions</dt><dd className="m-0 text-[var(--text-primary)]">{decision.usage.questions}</dd></>)}
            {decision.backend && (<><dt>Backend</dt><dd className="m-0 text-[var(--text-primary)]">{decision.backend}</dd></>)}
            {typeof decision.latency_ms === "number" && (<><dt>Latency</dt><dd className="m-0 text-[var(--text-primary)]">{decision.latency_ms} ms</dd></>)}
          </dl>
          {Object.keys(meta).length > 0 && (
            <pre className="m-0 mt-2 overflow-x-auto rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-2 font-mono text-[12px] text-[var(--text-primary)]">{JSON.stringify(meta, null, 2)}</pre>
          )}
        </div>

        <div>
          <h3 className="m-0 mb-2 text-[13px] font-semibold text-[var(--text-primary)]">Outcomes</h3>
          <ul className="m-0 list-none space-y-3 p-0">
            {decision.answers.map((a) => (
              <li key={a.name} className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_130px_minmax(0,1fr)] sm:items-center">
                <span className="truncate font-mono text-[12px] text-[var(--text-primary)]">{a.name}</span>
                <select
                  aria-label={`Outcome status for ${a.name}`}
                  value={draft[a.name]?.status ?? ""}
                  onChange={(e) => setDraft((d) => ({ ...d, [a.name]: { ...d[a.name], status: e.target.value as OutcomeStatus | "" } }))}
                  className={SETTINGS_SELECT_CLASS}
                >
                  <option value="">Not recorded</option>
                  {OUTCOME_STATUSES.map((s) => (
                    <option key={s} value={s}>{s}</option>
                  ))}
                </select>
                <input
                  aria-label={`Outcome label for ${a.name}`}
                  placeholder="Label (optional)"
                  maxLength={128}
                  value={draft[a.name]?.label ?? ""}
                  onChange={(e) => setDraft((d) => ({ ...d, [a.name]: { ...d[a.name], label: e.target.value } }))}
                  className={cn(INPUT_CLASS, "mt-0")}
                />
              </li>
            ))}
          </ul>
          {error && <ErrorBanner message={error} className="mt-3" />}
          <div className="mt-3 flex items-center gap-3">
            <button type="button" className={PRIMARY_BUTTON_CLASS} disabled={saving || !saveable} onClick={() => void save()}>
              {saving ? "Saving…" : "Save outcomes"}
            </button>
            {saved && <span className="text-[12px] text-[var(--text-secondary)]" role="status">Saved</span>}
          </div>
          {hasOutcomes(decision) && (
            <ul className="m-0 mt-3 list-none space-y-1 p-0 text-[12px] text-[var(--text-secondary)]" aria-label="Saved outcomes">
              {Object.entries(decision.outcomes ?? {}).map(([name, o]) => (
                <li key={name}>
                  <span className="font-mono">{name}</span>: {o.status}
                  {o.label ? ` · ${o.label}` : ""}
                  {o.detail ? ` · ${o.detail}` : ""}
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </section>
  );
}
