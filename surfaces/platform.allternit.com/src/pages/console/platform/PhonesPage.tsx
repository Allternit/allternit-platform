/**
 * Platform API → Phones: the project owner's own phones, linked to their
 * Allternit Desktop.
 *
 * Lists the phones (`GET /v1/phones`), a phone's recent runs and steps
 * (`GET /v1/phones/:serial/history`, optionally one run), and a live view of
 * its screen (`POST /v1/phones/:serial/live` → `viewer_url` in an iframe).
 * Frames and screen text stay on the Desktop; the history here is the
 * redacted ledger the API returns.
 */
import React, { useCallback, useEffect, useState } from "react";
import { ArrowClockwise, CircleNotch, DeviceMobile, Play, X } from "@phosphor-icons/react";
import { AllternitApiError, formatApiError } from "@/lib/api-client";
import type { V1Client, V1Phone, V1PhoneHistory, V1PhoneLive, V1PhoneRun } from "@/lib/platform-v1";
import { Badge, EmptyState, MonoChip, SkeletonRow } from "@/components/console-ui";
import { QUIET_BUTTON_CLASS } from "@/components/console-ui/buttonStyles";
import { cn } from "@/lib/utils";
import { CARD_CLASS, ErrorBanner, PlatformHeader, PRIMARY_BUTTON_CLASS, ProjectGate, formatDateTime, usePlatformProjects } from "./shared";

export function PlatformPhonesPage() {
  const state = usePlatformProjects();
  return (
    <div className="mx-auto w-full max-w-5xl">
      <PlatformHeader
        title="Phones"
        subtitle="Your own phones, linked to Allternit Desktop. Your app drives them with the phones scope; runs and steps show here."
        state={state}
      />
      <ProjectGate state={state}>{({ project, client }) => <PhonesBody key={project.id} client={client} />}</ProjectGate>
    </div>
  );
}

function phoneError(err: unknown, fallback: string): string {
  if (err instanceof AllternitApiError) {
    if (err.code === "phone_desktop_offline") return "Your Allternit Desktop is offline. Open it on the computer your phone is linked to.";
    if (err.code === "phone_gateway_unavailable") return "The phone gateway on your Desktop isn't running. Open Phones in Allternit Desktop.";
    if (err.code === "phone_offline") return "This phone isn't connected to your Desktop right now.";
  }
  return formatApiError(err, fallback);
}

function PhonesBody({ client }: { client: V1Client }) {
  const [phones, setPhones] = useState<V1Phone[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const page = await client.listPhones();
      setPhones(page.data);
      setSelected((cur) => (cur && page.data.some((p) => p.serial === cur) ? cur : page.data[0]?.serial ?? null));
    } catch (err) {
      setError(phoneError(err, "Unable to load your phones"));
    } finally {
      setLoading(false);
    }
  }, [client]);

  useEffect(() => {
    void load();
  }, [load]);

  if (loading && !phones) {
    return (
      <div className={cn(CARD_CLASS, "space-y-3 p-4")} role="status" aria-label="Loading phones">
        <SkeletonRow />
        <SkeletonRow />
      </div>
    );
  }
  if (error && !phones) {
    return (
      <div className="space-y-3">
        <ErrorBanner message={error} />
        <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => void load()}>
          <ArrowClockwise size={14} aria-hidden /> Try again
        </button>
      </div>
    );
  }
  if (!phones || phones.length === 0) {
    return (
      <div className={CARD_CLASS}>
        <EmptyState
          icon={<DeviceMobile size={32} aria-hidden />}
          title="No linked phones"
          caption="Link a phone in Allternit Desktop (Phones → Link a phone) with Phone Link, adb or a USB Agent. It shows here once your Desktop sees it."
        />
      </div>
    );
  }

  const phone = phones.find((p) => p.serial === selected) ?? null;
  const desktopOffline = phones.every((p) => !p.desktop_online);

  return (
    <div className="space-y-4">
      {error && <ErrorBanner message={error} />}
      {desktopOffline && (
        <div className="rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-3 text-[13px] text-[var(--text-secondary)]">
          Your Allternit Desktop is offline, so these are the phones the cloud last saw. Open Desktop to drive them or see their history.
        </div>
      )}
      <div className="flex items-center justify-between gap-2">
        <h2 className="m-0 text-[14px] font-semibold text-[var(--text-primary)]">Linked phones</h2>
        <button type="button" className={QUIET_BUTTON_CLASS} disabled={loading} onClick={() => void load()}>
          {loading ? <CircleNotch size={14} className="animate-spin" aria-hidden /> : <ArrowClockwise size={14} aria-hidden />} Refresh
        </button>
      </div>
      <ul className="m-0 grid list-none gap-3 p-0 sm:grid-cols-2">
        {phones.map((p) => (
          <li key={p.serial}>
            <button
              type="button"
              onClick={() => setSelected(p.serial)}
              aria-pressed={p.serial === selected}
              className={cn(
                CARD_CLASS,
                "w-full bg-transparent p-4 text-left transition-colors hover:bg-[var(--bg-secondary)]",
                p.serial === selected && "border-[var(--accent-primary)]/40",
              )}
            >
              <div className="flex flex-wrap items-center gap-2">
                <DeviceMobile size={16} className="text-[var(--text-secondary)]" aria-hidden />
                <span className="min-w-0 truncate text-[14px] font-semibold text-[var(--text-primary)]">{p.name || p.model || p.serial}</span>
                <Badge className={p.online ? "bg-[var(--status-success)]/10 text-[var(--status-success)]" : undefined}>{p.online ? "Online" : "Offline"}</Badge>
                {p.call_bridge?.enabled && <Badge>Calls: {p.call_bridge.botName ?? "bot"}</Badge>}
              </div>
              <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-[12px] text-[var(--text-secondary)]">
                {p.model && p.name && <span>{p.model}</span>}
                <span>Last seen {formatDateTime(p.last_seen_at)}</span>
              </div>
              <MonoChip className="mt-2 max-w-full truncate">{p.serial}</MonoChip>
            </button>
          </li>
        ))}
      </ul>

      {phone && (
        <>
          <LivePanel key={`live-${phone.serial}`} client={client} phone={phone} />
          <HistoryPanel key={`history-${phone.serial}`} client={client} phone={phone} />
        </>
      )}
    </div>
  );
}

function LivePanel({ client, phone }: { client: V1Client; phone: V1Phone }) {
  const [live, setLive] = useState<V1PhoneLive | null>(null);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [expired, setExpired] = useState(false);

  useEffect(() => {
    if (!live) return;
    const ms = new Date(live.expires_at).getTime() - Date.now();
    if (!Number.isFinite(ms)) return;
    const t = window.setTimeout(() => setExpired(true), Math.max(0, ms));
    return () => window.clearTimeout(t);
  }, [live]);

  const start = async () => {
    setStarting(true);
    setError(null);
    setExpired(false);
    try {
      setLive(await client.phoneLive(phone.serial));
    } catch (err) {
      setLive(null);
      setError(phoneError(err, "Unable to start the live view"));
    } finally {
      setStarting(false);
    }
  };

  return (
    <section className={cn(CARD_CLASS, "p-4")} aria-labelledby="phone-live-title">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="min-w-0">
          <h2 id="phone-live-title" className="m-0 text-[14px] font-semibold text-[var(--text-primary)]">
            Live view
          </h2>
          <p className="m-0 mt-1 text-[12px] text-[var(--text-secondary)]">
            The phone's screen, streamed from your Desktop. The link is short-lived and only works for you.
          </p>
        </div>
        {live && !expired ? (
          <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setLive(null)}>
            <X size={14} aria-hidden /> Close
          </button>
        ) : (
          <button type="button" className={PRIMARY_BUTTON_CLASS} disabled={!phone.online || starting} onClick={() => void start()}>
            {starting ? <CircleNotch size={14} className="animate-spin" aria-hidden /> : <Play size={14} aria-hidden />}
            {expired ? "Start again" : "Start live view"}
          </button>
        )}
      </div>
      {!phone.online && !live && <p className="m-0 mt-3 text-[12px] text-[var(--text-tertiary)]">Live view needs the phone online and your Desktop open.</p>}
      {error && <ErrorBanner message={error} className="mt-3" />}
      {expired && <p className="m-0 mt-3 text-[12px] text-[var(--text-secondary)]">The live view link expired. Start it again to keep watching.</p>}
      {live && !expired && (
        <div className="mt-3">
          <div className="mx-auto aspect-[9/19] w-full max-w-[360px] overflow-hidden rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]">
            <iframe
              src={live.viewer_url}
              title={`Live view of ${phone.name || phone.serial}`}
              className="h-full w-full border-0"
              allow="fullscreen; clipboard-write"
              referrerPolicy="no-referrer"
            />
          </div>
          <p className="m-0 mt-2 text-center text-[12px] text-[var(--text-tertiary)]">Link expires {formatDateTime(live.expires_at)}</p>
        </div>
      )}
    </section>
  );
}

function runLabel(run: V1PhoneRun): string {
  return run.source ? `${run.source} run` : "Run";
}

function HistoryPanel({ client, phone }: { client: V1Client; phone: V1Phone }) {
  const [runId, setRunId] = useState<string | null>(null);
  const [history, setHistory] = useState<V1PhoneHistory | null>(null);
  const [runs, setRuns] = useState<V1PhoneRun[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const h = await client.phoneHistory(phone.serial, runId);
      setHistory(h);
      // Keep the full run list while one run is open.
      if (!runId) setRuns(h.runs);
    } catch (err) {
      setError(phoneError(err, "Unable to load this phone's history"));
    } finally {
      setLoading(false);
    }
  }, [client, phone.serial, runId]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <section className={cn(CARD_CLASS, "p-4")} aria-labelledby="phone-history-title">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="min-w-0">
          <h2 id="phone-history-title" className="m-0 text-[14px] font-semibold text-[var(--text-primary)]">
            Runs and history
          </h2>
          <p className="m-0 mt-1 text-[12px] text-[var(--text-secondary)]">
            Tasks, skills and tool calls on this phone
            {history?.retention_days ? `, kept ${history.retention_days} days on your Desktop` : ""}. Typed text and numbers are redacted.
          </p>
        </div>
        <button type="button" className={QUIET_BUTTON_CLASS} disabled={loading} onClick={() => void load()}>
          {loading ? <CircleNotch size={14} className="animate-spin" aria-hidden /> : <ArrowClockwise size={14} aria-hidden />} Refresh
        </button>
      </div>

      {error && <ErrorBanner message={error} className="mt-3" />}

      {loading && !history ? (
        <div className="mt-3 space-y-2" role="status" aria-label="Loading history">
          <SkeletonRow />
          <SkeletonRow />
        </div>
      ) : history && runs.length === 0 && history.steps.length === 0 ? (
        <EmptyState
          icon={<DeviceMobile size={28} aria-hidden />}
          title="No runs yet"
          caption="Run a task or skill on this phone (POST /v1/phones/{serial}/task) and its steps show here."
        />
      ) : history ? (
        <div className="mt-3 grid gap-4 md:grid-cols-[minmax(0,1fr)_minmax(0,1.4fr)]">
          <div className="min-w-0">
            <div className="mb-2 text-[12px] font-medium text-[var(--text-secondary)]">Runs</div>
            {runs.length === 0 ? (
              <p className="m-0 text-[12px] text-[var(--text-tertiary)]">No task or skill runs, only single tool calls.</p>
            ) : (
              <ul className="m-0 list-none space-y-1.5 p-0">
                <li>
                  <button
                    type="button"
                    onClick={() => setRunId(null)}
                    aria-pressed={runId === null}
                    className={cn(
                      "w-full rounded-lg border border-solid border-transparent bg-transparent px-2.5 py-2 text-left text-[13px] text-[var(--text-primary)] hover:bg-[var(--bg-secondary)]",
                      runId === null && "border-[var(--border-subtle)] bg-[var(--bg-secondary)]",
                    )}
                  >
                    All recent steps
                  </button>
                </li>
                {runs.map((r) => (
                  <li key={r.run_id}>
                    <button
                      type="button"
                      onClick={() => setRunId(r.run_id)}
                      aria-pressed={runId === r.run_id}
                      className={cn(
                        "w-full rounded-lg border border-solid border-transparent bg-transparent px-2.5 py-2 text-left hover:bg-[var(--bg-secondary)]",
                        runId === r.run_id && "border-[var(--border-subtle)] bg-[var(--bg-secondary)]",
                      )}
                    >
                      <div className="flex flex-wrap items-center gap-2 text-[13px] text-[var(--text-primary)]">
                        <span className="capitalize">{runLabel(r)}</span>
                        {r.failed > 0 ? (
                          <Badge className="bg-[var(--status-error)]/10 text-[var(--status-error)]">{r.failed} failed</Badge>
                        ) : (
                          <Badge>{r.steps} steps</Badge>
                        )}
                      </div>
                      <div className="mt-0.5 text-[12px] text-[var(--text-secondary)]">{formatDateTime(r.started_at)}</div>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
          <div className="min-w-0">
            <div className="mb-2 text-[12px] font-medium text-[var(--text-secondary)]">{runId ? "Steps in this run" : "Recent steps"}</div>
            {history.steps.length === 0 ? (
              <p className="m-0 text-[12px] text-[var(--text-tertiary)]">No steps.</p>
            ) : (
              <ol className="m-0 list-none space-y-1.5 p-0">
                {history.steps.map((s) => (
                  <li key={s.id} className="rounded-lg border border-solid border-[var(--border-subtle)] px-2.5 py-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="min-w-0 truncate text-[13px] text-[var(--text-primary)]">{s.label || s.tool}</span>
                      <MonoChip>{s.tool}</MonoChip>
                      {!s.ok && <Badge className="bg-[var(--status-error)]/10 text-[var(--status-error)]">Failed</Badge>}
                    </div>
                    <div className="mt-0.5 flex flex-wrap gap-x-3 text-[12px] text-[var(--text-secondary)]">
                      <span>{formatDateTime(s.at)}</span>
                      <span>{Math.round(s.duration_ms)} ms</span>
                      {s.source && <span>{s.source}</span>}
                    </div>
                    {s.error && <div className="mt-1 break-words text-[12px] text-[var(--status-error)]">{s.error}</div>}
                  </li>
                ))}
              </ol>
            )}
          </div>
        </div>
      ) : null}
    </section>
  );
}
