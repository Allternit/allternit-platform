import React, { useCallback, useEffect, useRef, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  Archive02Icon,
  ArrowLeft01Icon,
  PauseIcon,
  SendIcon,
} from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { api, formatApiError } from "@/lib/api-client";
import {
  type SessionOutput,
  type SessionRecord,
  type SessionStreamEvent,
  type SessionTurn,
  archiveSession,
  formatBytes,
  formatDate,
  formatTime,
  getSession,
  listSessionOutputs,
  listSessionThreads,
  listSessionTurns,
  sendSessionEvents,
} from "@/lib/managed-agents";
import { MonoChip, Badge, SkeletonCard } from "@/components/console-ui";
import { EmptyInline } from "@/components/console-ui/EmptyState";

type Tab = "events" | "turns" | "threads" | "outputs";

const TABS: Array<{ id: Tab; label: string }> = [
  { id: "events", label: "Events" },
  { id: "turns", label: "Turns" },
  { id: "threads", label: "Threads" },
  { id: "outputs", label: "Outputs" },
];

const EVENT_TYPE_CLASS: Record<string, string> = {
  "session.created": "text-[var(--accent-primary)]",
  "session.running": "text-[var(--status-success)]",
  "session.idle": "text-[var(--text-secondary)]",
  "session.failed": "text-[var(--status-error)]",
  "turn.started": "text-[var(--text-primary)]",
  "turn.completed": "text-[var(--status-success)]",
  "turn.failed": "text-[var(--status-error)]",
  "agent.tool_use": "text-[var(--status-warning)]",
  "user.interrupt": "text-[var(--status-warning)]",
};

function statusClass(status?: string): string {
  switch (status) {
    case "running":
      return "text-[var(--status-success)]";
    case "waiting":
      return "text-[var(--status-warning)]";
    case "failed":
      return "text-[var(--status-error)]";
    case "archived":
      return "text-[var(--text-tertiary)]";
    default:
      return "text-[var(--text-secondary)]";
  }
}

/**
 * Live session detail. The event stream is GET /sessions/:id/events/stream,
 * consumed through api.stream() — each SSE `data:` payload is a translated
 * public event ({id, sequence, type, session_id, created_at, data}); the
 * `type` lives inside the JSON body, so no event-name tracking is needed.
 */
export function SessionDetailPage(): React.ReactNode {
  const { id } = useParams<{ id: string }>();
  const [session, setSession] = useState<SessionRecord | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("events");

  const [events, setEvents] = useState<SessionStreamEvent[]>([]);
  const [streamError, setStreamError] = useState<string | null>(null);
  const [streamLive, setStreamLive] = useState(false);
  const abortRef = useRef<AbortController | null>(null);

  const [turns, setTurns] = useState<SessionTurn[] | null>(null);
  const [threads, setThreads] = useState<SessionRecord[] | null>(null);
  const [outputs, setOutputs] = useState<SessionOutput[] | null>(null);

  const [followUp, setFollowUp] = useState("");
  const [sending, setSending] = useState(false);
  const [archiving, setArchiving] = useState(false);

  const stopStream = useCallback(() => {
    abortRef.current?.abort();
    abortRef.current = null;
    setStreamLive(false);
  }, []);

  const startStream = useCallback(async () => {
    if (!id) return;
    stopStream();
    const controller = new AbortController();
    abortRef.current = controller;
    setStreamError(null);
    try {
      const stream = api.stream<SessionStreamEvent>(
        `/api/v1/sessions/${encodeURIComponent(id)}/events/stream`,
        undefined,
        { method: "GET", signal: controller.signal }
      );
      setStreamLive(true);
      for await (const event of stream) {
        if (controller.signal.aborted) break;
        setEvents((prev) => {
          const key = event.id ?? `${event.sequence}-${event.type}`;
          return prev.some((e) => (e.id ?? `${e.sequence}-${e.type}`) === key) ? prev : [...prev, event];
        });
      }
    } catch (err) {
      if ((err as Error).name !== "AbortError") {
        setStreamError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (abortRef.current === controller) {
        abortRef.current = null;
        setStreamLive(false);
      }
    }
  }, [id, stopStream]);

  useEffect(() => {
    if (!id) return;
    let active = true;
    setLoading(true);
    getSession(id)
      .then((loaded) => {
        if (active) setSession(loaded);
      })
      .catch((err) => {
        if (active) setError(formatApiError(err, "Unable to load session"));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [id]);

  useEffect(() => {
    void startStream();
    return () => stopStream();
  }, [startStream, stopStream]);

  useEffect(() => {
    if (!id || tab === "events") return;
    let active = true;
    const load = async (): Promise<void> => {
      try {
        if (tab === "turns") {
          const data = await listSessionTurns(id);
          if (active) setTurns(data);
        } else if (tab === "threads") {
          const data = await listSessionThreads(id);
          if (active) setThreads(data);
        } else {
          const data = await listSessionOutputs(id);
          if (active) setOutputs(data);
        }
      } catch (err) {
        if (active) setError(formatApiError(err, `Unable to load ${tab}`));
      }
    };
    void load();
    return () => {
      active = false;
    };
  }, [id, tab]);

  const refreshSession = useCallback(async () => {
    if (!id) return;
    try {
      setSession(await getSession(id));
    } catch {
      // Keep the last known session shape — the stream already reports failures.
    }
  }, [id]);

  const handleFollowUp = useCallback(async () => {
    if (!id || !followUp.trim()) return;
    setSending(true);
    setError(null);
    try {
      await sendSessionEvents(id, [{ type: "user.message", content: followUp.trim() }]);
      setFollowUp("");
      await refreshSession();
    } catch (err) {
      setError(formatApiError(err, "Message failed"));
    } finally {
      setSending(false);
    }
  }, [id, followUp, refreshSession]);

  const handleInterrupt = useCallback(async () => {
    if (!id) return;
    setError(null);
    try {
      await sendSessionEvents(id, [{ type: "user.interrupt" }]);
      await refreshSession();
    } catch (err) {
      setError(formatApiError(err, "Interrupt failed"));
    }
  }, [id, refreshSession]);

  const handleArchive = useCallback(async () => {
    if (!id || !session) return;
    if (!window.confirm("Archive this session? Archived sessions cannot accept new events.")) return;
    setArchiving(true);
    setError(null);
    try {
      await archiveSession(id);
      stopStream();
      await refreshSession();
    } catch (err) {
      setError(formatApiError(err, "Archive failed"));
    } finally {
      setArchiving(false);
    }
  }, [id, session, refreshSession, stopStream]);

  if (loading) {
    return (
      <div className="space-y-4">
        <SkeletonCard rows={3} />
        <SkeletonCard rows={6} />
      </div>
    );
  }

  if (error && !session) {
    return (
      <div className="space-y-4">
        <Link
          to="/sessions"
          className="inline-flex items-center gap-1.5 text-[13px] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
        >
          <HugeiconsIcon icon={ArrowLeft01Icon} size={14} />
          Back to sessions
        </Link>
        <p className="flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      </div>
    );
  }

  if (!session) return null;

  const archived = session.status === "archived";

  return (
    <div className="space-y-5">
      <Link
        to="/sessions"
        className="inline-flex items-center gap-1.5 text-[13px] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
      >
        <HugeiconsIcon icon={ArrowLeft01Icon} size={14} />
        Back to sessions
      </Link>

      {error && (
        <p className="flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      )}

      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 space-y-2">
          <div className="flex flex-wrap items-center gap-2">
            <h1 className="m-0 text-[20px] font-semibold tracking-tight text-[var(--text-primary)]">
              {session.name ?? "Session"}
            </h1>
            <Badge className={statusClass(session.status)}>{session.status ?? "—"}</Badge>
            {streamLive && !archived && (
              <Badge className="text-[var(--status-success)]">live</Badge>
            )}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <MonoChip>{session.id}</MonoChip>
            <span className="text-[12px] text-[var(--text-secondary)]">
              computer {session.computer?.kind ?? "none"} · created {formatTime(session.created_at)}
            </span>
          </div>
        </div>
        <div className="flex shrink-0 flex-wrap items-center gap-4">
          <div className="text-right text-[12px] text-[var(--text-secondary)]">
            <div>
              {session.budget?.estimated_cost_usd != null
                ? `$${Number(session.budget.estimated_cost_usd).toFixed(4)} estimated`
                : "—"}
            </div>
            <div>
              {session.budget?.tokens_used != null ? `${session.budget.tokens_used} tokens used` : "no token usage yet"}
              {session.budget?.charged === false ? " · not charged" : ""}
            </div>
          </div>
          <div className="flex gap-2">
            <button
              type="button"
              onClick={() => void handleInterrupt()}
              disabled={archived}
              className="inline-flex items-center gap-1.5 rounded-lg border border-solid border-[var(--border-subtle)] px-3.5 py-2 text-[13px] font-medium text-[var(--text-primary)] transition-colors hover:bg-[var(--bg-secondary)] disabled:opacity-50 disabled:cursor-not-allowed"
            >
              <HugeiconsIcon icon={PauseIcon} size={13} />
              Interrupt
            </button>
            <button
              type="button"
              onClick={() => void handleArchive()}
              disabled={archiving || archived}
              className="inline-flex items-center gap-1.5 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3.5 py-2 text-[13px] font-medium text-[var(--text-secondary)] transition-colors hover:text-[var(--status-error)] hover:border-[var(--status-error)]/30 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              <HugeiconsIcon icon={Archive02Icon} size={13} />
              {archiving ? "Archiving…" : "Archive"}
            </button>
          </div>
        </div>
      </div>

      {!archived && (
        <div className="flex gap-2">
          <input
            type="text"
            value={followUp}
            onChange={(e) => setFollowUp(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.nativeEvent.isComposing) {
                e.preventDefault();
                void handleFollowUp();
              }
            }}
            placeholder="Send a follow-up message…"
            className="flex-1 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--accent-primary)]"
          />
          <button
            type="button"
            onClick={() => void handleFollowUp()}
            disabled={sending || !followUp.trim()}
            className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            <HugeiconsIcon icon={SendIcon} size={13} />
            {sending ? "Sending…" : "Send"}
          </button>
        </div>
      )}

      <div className="flex items-center gap-1 border-b border-solid border-[var(--border-subtle)]">
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            onClick={() => setTab(t.id)}
            className={cn(
              "border-b-2 px-3 py-2 text-[13px] font-medium transition-colors",
              tab === t.id
                ? "border-[var(--accent-primary)] text-[var(--text-primary)]"
                : "border-transparent text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
            )}
          >
            {t.label}
          </button>
        ))}
      </div>

      {tab === "events" && (
        <div className="space-y-2">
          {streamError && (
            <p className="flex items-center gap-2 rounded-lg border border-solid border-[var(--status-warning)]/30 bg-[var(--status-warning)]/10 px-3 py-2 text-[12px] text-[var(--status-warning)]">
              <HugeiconsIcon icon={AlertCircleIcon} size={13} />
              Event stream interrupted: {streamError}
              <button
                type="button"
                onClick={() => void startStream()}
                className="ml-auto font-semibold underline underline-offset-2"
              >
                Reconnect
              </button>
            </p>
          )}
          {!streamLive && !streamError && !archived && (
            <p className="text-[12px] text-[var(--text-tertiary)]">Stream idle — waiting for events.</p>
          )}
          {events.length === 0 ? (
            <EmptyInline art="log">
              {archived
                ? "This session is archived — it can no longer accept events, but its history still streams below."
                : "No events yet. Send a message or wait for the agent to act — events stream here in real time."}
            </EmptyInline>
          ) : (
            <ol className="max-h-[480px] space-y-1 overflow-y-auto rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-3 font-mono text-[12px]">
              {events.map((event, index) => (
                <li key={event.id ?? index} className="flex items-baseline gap-3">
                  <span className="shrink-0 text-[var(--text-tertiary)]">
                    {event.created_at ? formatTime(event.created_at) : `#${event.sequence ?? index}`}
                  </span>
                  <span className={cn("font-semibold", EVENT_TYPE_CLASS[event.type ?? ""] ?? "text-[var(--text-primary)]")}>
                    {event.type ?? "event"}
                  </span>
                  {event.data && Object.keys(event.data).length > 0 && (
                    <span className="truncate text-[var(--text-tertiary)]">{JSON.stringify(event.data)}</span>
                  )}
                </li>
              ))}
            </ol>
          )}
        </div>
      )}

      {tab === "turns" && (
        <div>
          {turns === null ? (
            <p className="text-[13px] text-[var(--text-tertiary)]">Loading turns…</p>
          ) : turns.length === 0 ? (
            <p className="text-[13px] text-[var(--text-tertiary)]">
              No turns yet — turns appear once the agent starts work.
            </p>
          ) : (
            <div className="divide-y divide-solid divide-[var(--border-subtle)] rounded-xl border border-solid border-[var(--border-subtle)]">
              {turns.map((turn, index) => (
                <div key={turn.id ?? index} className="flex items-center gap-3 px-3 py-2 text-[13px]">
                  <span className="flex-1 text-[var(--text-primary)]">Turn {index + 1}</span>
                  <Badge
                    className={
                      turn.status === "completed"
                        ? "text-[var(--status-success)]"
                        : turn.status === "failed"
                          ? "text-[var(--status-error)]"
                          : "text-[var(--status-warning)]"
                    }
                  >
                    {turn.status ?? "—"}
                  </Badge>
                  <span className="text-[12px] text-[var(--text-tertiary)]">
                    {formatDate(turn.started_at)} → {turn.completed_at ? formatDate(turn.completed_at) : "…"}
                  </span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {tab === "threads" && (
        <div>
          {threads === null ? (
            <p className="text-[13px] text-[var(--text-tertiary)]">Loading threads…</p>
          ) : threads.length === 0 ? (
            <p className="text-[13px] text-[var(--text-tertiary)]">
              No child sessions — threads appear when follow-up work is forked from this session.
            </p>
          ) : (
            <div className="divide-y divide-solid divide-[var(--border-subtle)] rounded-xl border border-solid border-[var(--border-subtle)]">
              {threads.map((thread) => (
                <Link
                  key={thread.id}
                  to={`/sessions/${thread.id}`}
                  className="flex items-center gap-3 px-3 py-2 text-[13px] transition-colors hover:bg-[var(--surface-hover)]"
                >
                  <MonoChip>{thread.id.slice(0, 8)}</MonoChip>
                  <span className="flex-1 text-[var(--text-primary)]">{thread.name ?? thread.id.slice(0, 8)}</span>
                  <Badge className={statusClass(thread.status)}>{thread.status ?? "—"}</Badge>
                  <span className="text-[12px] text-[var(--text-tertiary)]">{formatTime(thread.created_at)}</span>
                </Link>
              ))}
            </div>
          )}
        </div>
      )}

      {tab === "outputs" && (
        <div>
          {outputs === null ? (
            <p className="text-[13px] text-[var(--text-tertiary)]">Loading outputs…</p>
          ) : outputs.length === 0 ? (
            <p className="text-[13px] text-[var(--text-tertiary)]">
              No files produced yet — files the agent writes land here.
            </p>
          ) : (
            <div className="divide-y divide-solid divide-[var(--border-subtle)] rounded-xl border border-solid border-[var(--border-subtle)]">
              {outputs.map((output) => (
                <div key={output.id} className="flex items-center gap-3 px-3 py-2 text-[13px]">
                  <span className="flex-1 font-medium text-[var(--text-primary)]">{output.filename}</span>
                  <span className="text-[12px] text-[var(--text-tertiary)]">{output.mime_type ?? "file"}</span>
                  <span className="text-[12px] text-[var(--text-secondary)]">{formatBytes(output.size_bytes)}</span>
                  <span className="text-[12px] text-[var(--text-tertiary)]">{formatTime(output.created_at)}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

export default SessionDetailPage;
