import React, { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { ShieldCheck } from "@phosphor-icons/react";
import { formatApiError } from "@/lib/api-client";
import type { ApprovalSource, ApprovalStatus, V1Approval, V1Client } from "@/lib/platform-v1";
import { approvalStatusLabel, APPROVAL_SOURCE_LABEL, summarizeProposedAction } from "@/lib/approvals";
import { Badge, EmptyState, SkeletonRow } from "@/components/console-ui";
import { QUIET_BUTTON_CLASS, SETTINGS_SELECT_CLASS } from "@/components/console-ui/buttonStyles";
import { cn } from "@/lib/utils";
import { CARD_CLASS, ErrorBanner, PlatformHeader, ProjectGate, formatDateTime, usePlatformProjects } from "@/pages/console/platform/shared";

function statusColor(status: string): string {
  switch (status) {
    case "approved":
      return "text-[var(--status-success)] bg-[var(--status-success)]/10";
    case "denied":
      return "text-[var(--status-error)] bg-[var(--status-error)]/10";
    case "pending":
      return "text-[var(--accent-highlight)] bg-[var(--accent-highlight)]/10";
    default:
      return "text-[var(--text-tertiary)] bg-[var(--bg-primary)]";
  }
}

const APPROVE_CLASS =
  "inline-flex items-center gap-1 px-2.5 py-1 rounded-md text-[12px] font-medium bg-[var(--status-success)]/10 text-[var(--status-success)] hover:bg-[var(--status-success)]/20 transition-colors disabled:opacity-50";
const DENY_CLASS =
  "inline-flex items-center gap-1 px-2.5 py-1 rounded-md text-[12px] font-medium text-[var(--text-secondary)] hover:text-[var(--status-error)] hover:bg-[var(--status-error)]/10 transition-colors disabled:opacity-50";

export function ApprovalsPage() {
  const state = usePlatformProjects();
  return (
    <div className="mx-auto w-full max-w-6xl">
      <PlatformHeader title="Approvals" subtitle="Actions your agents proposed that need the owner's OK." state={state} />
      <ProjectGate state={state}>{({ project, client }) => <ApprovalsBody key={project.id} client={client} />}</ProjectGate>
    </div>
  );
}

function ApprovalsBody({ client }: { client: V1Client }) {
  const [status, setStatus] = useState<ApprovalStatus | "">("pending");
  const [source, setSource] = useState<ApprovalSource | "">("");
  const [items, setItems] = useState<V1Approval[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);

  const load = useCallback(
    async (after: string | null) => {
      setLoading(true);
      setError(null);
      try {
        const page = await client.listApprovals({ status: status || undefined, source: source || undefined, after });
        setItems((cur) => (after ? [...cur, ...page.data] : page.data));
        setCursor(page.next_cursor);
        setHasMore(page.has_more);
      } catch (err) {
        setError(formatApiError(err, "Unable to load approvals"));
      } finally {
        setLoading(false);
      }
    },
    [client, status, source],
  );

  useEffect(() => {
    void load(null);
  }, [load]);

  const decide = async (id: string, decision: "approve" | "deny") => {
    setBusyId(id);
    setError(null);
    try {
      const updated = await client.decideApproval(id, decision);
      setItems((cur) => {
        // A row that no longer matches the status filter leaves the list.
        if (status && updated.status !== status) return cur.filter((a) => a.id !== id);
        return cur.map((a) => (a.id === id ? updated : a));
      });
    } catch (err) {
      setError(formatApiError(err, "Unable to record the decision"));
    } finally {
      setBusyId(null);
    }
  };

  const selected = items.find((a) => a.id === selectedId) ?? null;

  const actions = (a: V1Approval, size: "sm" | "md") =>
    a.status === "pending" ? (
      <div className="flex items-center gap-2">
        <button type="button" disabled={busyId === a.id} onClick={(e) => { e.stopPropagation(); void decide(a.id, "approve"); }} className={cn(APPROVE_CLASS, size === "md" && "px-3 py-1.5 text-[13px]")}>
          Approve
        </button>
        <button type="button" disabled={busyId === a.id} onClick={(e) => { e.stopPropagation(); void decide(a.id, "deny"); }} className={cn(DENY_CLASS, size === "md" && "px-3 py-1.5 text-[13px]")}>
          Deny
        </button>
      </div>
    ) : null;

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-3">
        <label className="inline-flex items-center gap-2 text-[12px] font-medium text-[var(--text-secondary)]">
          Status
          <select value={status} onChange={(e) => setStatus(e.target.value as ApprovalStatus | "")} className={cn(SETTINGS_SELECT_CLASS, "min-w-0")}>
            <option value="pending">Pending</option>
            <option value="approved">Approved</option>
            <option value="denied">Denied</option>
            <option value="expired">Expired</option>
            <option value="consumed">Consumed</option>
            <option value="">All</option>
          </select>
        </label>
        <label className="inline-flex items-center gap-2 text-[12px] font-medium text-[var(--text-secondary)]">
          Source
          <select value={source} onChange={(e) => setSource(e.target.value as ApprovalSource | "")} className={cn(SETTINGS_SELECT_CLASS, "min-w-0")}>
            <option value="">All</option>
            <option value="twin">Twin</option>
            <option value="computer">Computer</option>
          </select>
        </label>
        <button type="button" onClick={() => void load(null)} disabled={loading} className={QUIET_BUTTON_CLASS}>
          Refresh
        </button>
      </div>

      {error && <ErrorBanner message={error} />}

      <section className={CARD_CLASS} aria-label="Approvals">
        {items.length === 0 && loading ? (
          <div className="px-4" role="status" aria-label="Loading approvals">
            <SkeletonRow lines={4} />
          </div>
        ) : items.length === 0 && !error ? (
          <EmptyState
            icon={<ShieldCheck size={32} aria-hidden />}
            title="No approvals"
            caption="Approvals appear here when an agent's action needs the owner's OK."
          />
        ) : (
          <ul className="m-0 list-none divide-y divide-[var(--border-subtle)] p-0">
            {items.map((a) => (
              <li key={a.id}>
                <div
                  role="button"
                  tabIndex={0}
                  onClick={() => setSelectedId(a.id === selectedId ? null : a.id)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      setSelectedId(a.id === selectedId ? null : a.id);
                    }
                  }}
                  className={cn("flex flex-wrap items-center justify-between gap-3 px-4 py-3 cursor-pointer hover:bg-[var(--surface-hover)]", a.id === selectedId && "bg-[var(--surface-hover)]")}
                >
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2 text-[13px]">
                      <Badge>{APPROVAL_SOURCE_LABEL[a.source] ?? a.source}</Badge>
                      <span className="font-medium text-[var(--text-primary)]">{a.kind}</span>
                      <Badge className={statusColor(a.status)}>{approvalStatusLabel(a.status)}</Badge>
                    </div>
                    <div className="mt-1 truncate text-[13px] text-[var(--text-secondary)]">{summarizeProposedAction(a.proposed_action)}</div>
                    <div className="mt-1 flex flex-wrap gap-x-3 text-[12px] text-[var(--text-tertiary)]">
                      {a.account_id && <span className="font-mono">{a.account_id}</span>}
                      <span>Created {formatDateTime(a.created_at)}</span>
                      {a.expires_at && <span>Expires {formatDateTime(a.expires_at)}</span>}
                    </div>
                  </div>
                  {actions(a, "sm")}
                </div>
              </li>
            ))}
          </ul>
        )}
        {hasMore && (
          <div className="border-t border-solid border-[var(--border-subtle)] p-3 text-center">
            <button type="button" className={QUIET_BUTTON_CLASS} disabled={loading} onClick={() => void load(cursor)}>
              {loading ? "Loading…" : "Load more"}
            </button>
          </div>
        )}
      </section>

      {selected && (
        <div className={cn(CARD_CLASS, "p-5")}>
          <div className="mb-4 flex items-center justify-between gap-4">
            <h2 className="m-0 text-[14px] font-semibold text-[var(--text-primary)]">Approval details</h2>
            <button type="button" onClick={() => setSelectedId(null)} className="text-[12px] text-[var(--text-tertiary)] hover:text-[var(--text-secondary)]">
              Close
            </button>
          </div>
          <dl className="m-0 grid grid-cols-1 gap-4 text-[13px] sm:grid-cols-2">
            <Field label="Approval" mono>{selected.id}</Field>
            <Field label="Status"><Badge className={statusColor(selected.status)}>{approvalStatusLabel(selected.status)}</Badge></Field>
            <Field label="Source">{APPROVAL_SOURCE_LABEL[selected.source] ?? selected.source} <span className="font-mono text-[12px] text-[var(--text-tertiary)]">{selected.source_ref}</span></Field>
            <Field label="Kind">{selected.kind}</Field>
            <Field label="Account" mono>{selected.account_id ?? "—"}</Field>
            <Field label="Expires">{selected.expires_at ? formatDateTime(selected.expires_at) : "No deadline"}</Field>
            {selected.conversation_id && (
              <Field label="Conversation" mono>
                <Link to={`/platform/conversations?conversation=${encodeURIComponent(selected.conversation_id)}`} className="text-[var(--accent-primary)] hover:underline">
                  {selected.conversation_id}
                </Link>
              </Field>
            )}
            {selected.response_id && <Field label="Response" mono>{selected.response_id}</Field>}
            {selected.resolved_at && <Field label="Resolved">{formatDateTime(selected.resolved_at)}{selected.resolved_by ? ` by ${selected.resolved_by}` : ""}</Field>}
          </dl>
          <div className="mt-4">
            <div className="text-[11px] text-[var(--text-tertiary)]">Proposed action</div>
            <pre className="m-0 mt-1 max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] p-3 font-mono text-[12px] text-[var(--text-primary)]">
              {JSON.stringify(selected.proposed_action, null, 2)}
            </pre>
          </div>
          {selected.status === "pending" && <div className="mt-4">{actions(selected, "md")}</div>}
        </div>
      )}
    </div>
  );
}

function Field({ label, mono, children }: { label: string; mono?: boolean; children: React.ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-[11px] text-[var(--text-tertiary)]">{label}</dt>
      <dd className={cn("m-0 break-words text-[var(--text-primary)]", mono && "font-mono text-[12px]")}>{children}</dd>
    </div>
  );
}
