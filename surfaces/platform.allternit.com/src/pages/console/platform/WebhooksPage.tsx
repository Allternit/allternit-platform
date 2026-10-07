import React, { useCallback, useEffect, useState } from "react";
import { ArrowClockwise, Check, CircleNotch, Copy, PaperPlaneTilt, Plus, ShieldCheck, Trash, WebhooksLogo, X } from "@phosphor-icons/react";
import { formatApiError } from "@/lib/api-client";
import { WEBHOOK_EVENT_TYPES, allPages, type V1Client, type V1Webhook, type V1WebhookDelivery } from "@/lib/platform-v1";
import { Badge, EmptyState, MonoChip, SkeletonRow } from "@/components/console-ui";
import { DESTRUCTIVE_BUTTON_CLASS, QUIET_BUTTON_CLASS } from "@/components/console-ui/buttonStyles";
import { cn } from "@/lib/utils";
import {
  CARD_CLASS,
  ErrorBanner,
  INPUT_CLASS,
  PRIMARY_BUTTON_CLASS,
  PlatformHeader,
  ProjectGate,
  formatDateTime,
  usePlatformProjects,
} from "./shared";

/** Platform API webhook endpoints for the selected project: create, test, delete, delivery log, redeliver. */
export function PlatformWebhooksPage() {
  const state = usePlatformProjects();
  return (
    <div className="mx-auto w-full max-w-5xl">
      <PlatformHeader
        title="Webhooks"
        subtitle="Endpoints that receive this project's events, signed with an allternit-signature header."
        state={state}
      />
      <ProjectGate state={state}>{({ project, client }) => <WebhooksBody key={project.id} client={client} />}</ProjectGate>
    </div>
  );
}

function WebhooksBody({ client }: { client: V1Client }) {
  const [hooks, setHooks] = useState<V1Webhook[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showNew, setShowNew] = useState(false);
  const [open, setOpen] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setHooks(await allPages(() => client.listWebhooks()));
    } catch (err) {
      setError(formatApiError(err, "Unable to load webhooks"));
    } finally {
      setLoading(false);
    }
  }, [client]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <div className="space-y-4">
      {error && <ErrorBanner message={error} />}
      <div className="flex justify-end">
        <button type="button" className={PRIMARY_BUTTON_CLASS} onClick={() => setShowNew(true)} disabled={loading}>
          <Plus size={14} aria-hidden /> Add endpoint
        </button>
      </div>
      {showNew && <NewWebhook client={client} onClose={() => setShowNew(false)} onCreated={load} />}

      <section className={CARD_CLASS} aria-label="Webhook endpoints">
        {loading ? (
          <div className="px-4" role="status" aria-label="Loading webhooks">
            <SkeletonRow lines={2} />
          </div>
        ) : hooks.length === 0 ? (
          <EmptyState
            icon={<WebhooksLogo size={32} aria-hidden />}
            title="No endpoints"
            caption="Add a public https URL to receive message, status and registration events."
            ctaLabel="Add endpoint"
            onCtaClick={() => setShowNew(true)}
            primaryCta
          />
        ) : (
          <ul className="m-0 list-none divide-y divide-[var(--border-subtle)] p-0">
            {hooks.map((h) => (
              <WebhookRow key={h.id} client={client} hook={h} open={open === h.id} onToggle={() => setOpen((cur) => (cur === h.id ? null : h.id))} onDeleted={load} onError={setError} />
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

function NewWebhook({ client, onClose, onCreated }: { client: V1Client; onClose: () => void; onCreated: () => Promise<void> }) {
  const [url, setUrl] = useState("");
  const [events, setEvents] = useState<string[]>(["*"]);
  const [description, setDescription] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [secret, setSecret] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const all = events.includes("*");
  const toggle = (e: string, on: boolean) =>
    setEvents((cur) => {
      const rest = cur.filter((x) => x !== "*" && x !== e);
      return on ? [...rest, e] : rest;
    });

  const create = async () => {
    setSaving(true);
    setError(null);
    try {
      const created = await client.createWebhook({ url: url.trim(), events, ...(description.trim() ? { description: description.trim() } : {}) });
      setSecret(created.secret);
      await onCreated();
    } catch (err) {
      setError(formatApiError(err, "Unable to add the endpoint"));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section aria-label="New webhook endpoint" className="space-y-3 rounded-xl border border-solid border-[var(--accent-primary)]/25 bg-[var(--accent-primary)]/[0.03] p-4">
      <div className="flex items-center justify-between gap-3">
        <div className="text-[14px] font-semibold text-[var(--text-primary)]">New endpoint</div>
        <button type="button" onClick={onClose} className="rounded-md p-1 text-[var(--text-tertiary)] hover:bg-[var(--surface-hover)]" aria-label="Close">
          <X size={16} />
        </button>
      </div>
      {error && <ErrorBanner message={error} />}
      {secret ? (
        <div className="space-y-3">
          <div className="flex items-start gap-2 rounded-lg border border-solid border-[var(--status-success)]/25 bg-[var(--status-success)]/[0.06] p-3">
            <ShieldCheck size={16} className="mt-0.5 shrink-0 text-[var(--status-success)]" aria-hidden />
            <div className="text-[13px] text-[var(--text-primary)]">Copy the signing secret now. It is shown once. Use it to verify the allternit-signature header.</div>
          </div>
          <div className="flex items-center gap-2 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2">
            <code className="flex-1 truncate font-mono text-[12px] text-[var(--text-primary)]">{secret}</code>
            <button
              type="button"
              className={QUIET_BUTTON_CLASS}
              onClick={() =>
                void navigator.clipboard
                  .writeText(secret)
                  .then(() => {
                    setCopied(true);
                    window.setTimeout(() => setCopied(false), 2000);
                  })
                  .catch(() => {})
              }
            >
              {copied ? <Check size={14} aria-hidden /> : <Copy size={14} aria-hidden />} {copied ? "Copied" : "Copy"}
            </button>
          </div>
        </div>
      ) : (
        <>
          <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
            URL
            <input value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://example.com/allternit/webhooks" inputMode="url" className={INPUT_CLASS} />
          </label>
          <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
            Description (optional)
            <input value={description} onChange={(e) => setDescription(e.target.value)} maxLength={200} className={INPUT_CLASS} />
          </label>
          <fieldset>
            <legend className="text-[12px] font-medium text-[var(--text-secondary)]">Events</legend>
            <div className="mt-1.5 grid grid-cols-1 gap-1.5 sm:grid-cols-2">
              <label className="inline-flex items-center gap-1.5 text-[13px] text-[var(--text-primary)]">
                <input type="checkbox" checked={all} onChange={(e) => setEvents(e.target.checked ? ["*"] : [])} />
                All events
              </label>
              {WEBHOOK_EVENT_TYPES.map((e) => (
                <label key={e} className="inline-flex items-center gap-1.5 font-mono text-[12px] text-[var(--text-primary)]">
                  <input type="checkbox" checked={all || events.includes(e)} disabled={all} onChange={(ev) => toggle(e, ev.target.checked)} />
                  {e}
                </label>
              ))}
            </div>
          </fieldset>
          <button type="button" className={PRIMARY_BUTTON_CLASS} disabled={!url.trim().startsWith("https://") || events.length === 0 || saving} onClick={() => void create()}>
            {saving ? <CircleNotch size={14} className="animate-spin" aria-hidden /> : <Plus size={14} aria-hidden />} Add endpoint
          </button>
        </>
      )}
    </section>
  );
}

function stateBadge(state: string) {
  const ok = state === "delivered";
  const bad = state === "failed" || state === "dead";
  return (
    <Badge className={cn(ok && "bg-[var(--status-success)]/10 text-[var(--status-success)]", bad && "bg-[var(--status-error)]/10 text-[var(--status-error)]")}>{state}</Badge>
  );
}

function WebhookRow({
  client,
  hook,
  open,
  onToggle,
  onDeleted,
  onError,
}: {
  client: V1Client;
  hook: V1Webhook;
  open: boolean;
  onToggle: () => void;
  onDeleted: () => Promise<void>;
  onError: (m: string | null) => void;
}) {
  const [confirm, setConfirm] = useState(false);
  const [note, setNote] = useState<string | null>(null);

  const test = async () => {
    onError(null);
    try {
      await client.testWebhook(hook.id);
      setNote("Test event queued. It appears in the delivery log.");
    } catch (err) {
      onError(formatApiError(err, "Unable to send a test event"));
    }
  };
  const remove = async () => {
    onError(null);
    try {
      await client.deleteWebhook(hook.id);
      await onDeleted();
    } catch (err) {
      onError(formatApiError(err, "Unable to delete the endpoint"));
    }
  };

  return (
    <li className="px-4 py-3">
      <div className="flex flex-wrap items-start gap-3">
        <div className="min-w-0 flex-1">
          <div className="break-all text-[13px] font-medium text-[var(--text-primary)]">{hook.url}</div>
          {hook.description && <div className="text-[12px] text-[var(--text-secondary)]">{hook.description}</div>}
          <div className="mt-1 flex flex-wrap items-center gap-1.5">
            {hook.events.map((e) => (
              <Badge key={e} className="font-mono">{e === "*" ? "all events" : e}</Badge>
            ))}
          </div>
          <MonoChip className="mt-2 max-w-full truncate">{hook.id}</MonoChip>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <button type="button" className={QUIET_BUTTON_CLASS} onClick={onToggle} aria-expanded={open}>
            Deliveries
          </button>
          <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => void test()}>
            <PaperPlaneTilt size={14} aria-hidden /> Send test
          </button>
          {confirm ? (
            <>
              <button type="button" className={DESTRUCTIVE_BUTTON_CLASS} onClick={() => void remove()}>
                <Trash size={14} aria-hidden /> Delete
              </button>
              <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setConfirm(false)}>
                Cancel
              </button>
            </>
          ) : (
            <button type="button" className={DESTRUCTIVE_BUTTON_CLASS} onClick={() => setConfirm(true)} aria-label={`Delete ${hook.url}`}>
              <Trash size={14} aria-hidden />
            </button>
          )}
        </div>
      </div>
      {note && (
        <p role="status" className="m-0 mt-2 text-[12px] text-[var(--text-secondary)]">
          {note}
        </p>
      )}
      {open && <Deliveries client={client} hookId={hook.id} />}
    </li>
  );
}

function Deliveries({ client, hookId }: { client: V1Client; hookId: string }) {
  const [items, setItems] = useState<V1WebhookDelivery[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [queued, setQueued] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const all = await allPages((after) => client.listDeliveries(hookId, after));
      setItems(all.reverse()); // newest first
    } catch (err) {
      setError(formatApiError(err, "Unable to load deliveries"));
    } finally {
      setLoading(false);
    }
  }, [client, hookId]);

  useEffect(() => {
    void load();
  }, [load]);

  const redeliver = async (id: string) => {
    setError(null);
    try {
      await client.redeliver(hookId, id);
      setQueued(id);
      await load();
    } catch (err) {
      setError(formatApiError(err, "Unable to redeliver"));
    }
  };

  return (
    <div className="mt-3 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]">
      <div className="flex items-center justify-between gap-2 border-b border-solid border-[var(--border-subtle)] px-3 py-2">
        <span className="text-[12px] font-medium text-[var(--text-secondary)]">Delivery log</span>
        <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => void load()} disabled={loading} aria-label="Refresh deliveries">
          <ArrowClockwise size={14} aria-hidden /> Refresh
        </button>
      </div>
      {error && <ErrorBanner message={error} className="m-3" />}
      {loading ? (
        <div className="px-3" role="status" aria-label="Loading deliveries">
          <SkeletonRow lines={2} />
        </div>
      ) : items.length === 0 ? (
        <p className="m-0 p-3 text-[12px] text-[var(--text-secondary)]">No deliveries yet. Send a test event to try the endpoint.</p>
      ) : (
        <ul className="m-0 list-none divide-y divide-[var(--border-subtle)] p-0">
          {items.map((d) => (
            <li key={d.id} className="flex flex-wrap items-center gap-2 px-3 py-2 text-[12px]">
              {stateBadge(d.state)}
              <span className="font-mono text-[var(--text-primary)]">{d.event_type}</span>
              <span className="text-[var(--text-secondary)]">
                {d.attempts} {d.attempts === 1 ? "attempt" : "attempts"}
                {d.last_status ? ` · HTTP ${d.last_status}` : ""}
              </span>
              <span className="flex-1 text-[var(--text-tertiary)]">{formatDateTime(d.created_at)}</span>
              {d.last_error && <span className="w-full break-words text-[var(--status-error)]">{d.last_error}</span>}
              <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => void redeliver(d.id)} aria-label={`Redeliver ${d.id}`}>
                <ArrowClockwise size={14} aria-hidden /> {queued === d.id ? "Queued" : "Redeliver"}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
