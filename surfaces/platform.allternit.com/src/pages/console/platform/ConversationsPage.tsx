import React, { useCallback, useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { ArrowLeft, ChatsCircle } from "@phosphor-icons/react";
import { formatApiError } from "@/lib/api-client";
import { allPages, type V1Agent, type V1Client, type V1Conversation } from "@/lib/platform-v1";
import { EmptyState, MonoChip, SkeletonRow } from "@/components/console-ui";
import { QUIET_BUTTON_CLASS, SETTINGS_SELECT_CLASS } from "@/components/console-ui/buttonStyles";
import { cn } from "@/lib/utils";
import { CARD_CLASS, ErrorBanner, PlatformHeader, ProjectGate, formatDateTime, usePlatformProjects } from "./shared";

/** Conversations per agent, and the messages of the one you open (read-only). */
export function PlatformConversationsPage() {
  const state = usePlatformProjects();
  return (
    <div className="mx-auto w-full max-w-6xl">
      <PlatformHeader title="Conversations" subtitle="What your agents and the people they talk to said, per agent. Read-only." state={state} />
      <ProjectGate state={state}>{({ project, client }) => <ConversationsBody key={project.id} client={client} />}</ProjectGate>
    </div>
  );
}

function ConversationsBody({ client }: { client: V1Client }) {
  const [params, setParams] = useSearchParams();
  const [agents, setAgents] = useState<V1Agent[]>([]);
  const [loadingAgents, setLoadingAgents] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const agentId = params.get("agent");
  const convId = params.get("conversation");

  useEffect(() => {
    let active = true;
    setLoadingAgents(true);
    allPages((after) => client.listAgents(after))
      .then((list) => {
        if (!active) return;
        setAgents(list);
        if (!agentId && list[0]) setParams((p) => { p.set("agent", list[0].id); return p; }, { replace: true });
      })
      .catch((err) => active && setError(formatApiError(err, "Unable to load agents")))
      .finally(() => active && setLoadingAgents(false));
    return () => {
      active = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client]);

  const pickAgent = (id: string) => setParams({ agent: id });
  const open = (id: string | null) =>
    setParams((p) => {
      if (id) p.set("conversation", id);
      else p.delete("conversation");
      return p;
    });

  if (loadingAgents) {
    return (
      <div className={cn(CARD_CLASS, "px-4")} role="status" aria-label="Loading">
        <SkeletonRow lines={2} />
      </div>
    );
  }
  if (error) return <ErrorBanner message={error} />;
  if (agents.length === 0) {
    return (
      <div className={CARD_CLASS}>
        <EmptyState icon={<ChatsCircle size={32} aria-hidden />} title="No agents yet" caption="Conversations appear here once you create an agent and send it a message." />
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <label className="inline-flex max-w-full items-center gap-2 text-[12px] font-medium text-[var(--text-secondary)]">
        Agent
        <select value={agentId ?? ""} onChange={(e) => pickAgent(e.target.value)} className={cn(SETTINGS_SELECT_CLASS, "min-w-0 max-w-[260px]")}>
          {agents.map((a) => (
            <option key={a.id} value={a.id}>
              {a.name}
            </option>
          ))}
        </select>
      </label>

      {agentId && (
        <div className="grid gap-4 lg:grid-cols-[minmax(0,320px)_minmax(0,1fr)]">
          <div className={cn(convId ? "hidden lg:block" : "block")}>
            <ConversationList key={agentId} client={client} agentId={agentId} selected={convId} onOpen={open} />
          </div>
          <div className={cn(convId ? "block" : "hidden lg:block")}>
            {convId ? (
              <ConversationView key={convId} client={client} id={convId} onBack={() => open(null)} />
            ) : (
              <div className={cn(CARD_CLASS, "flex min-h-[200px] items-center justify-center p-6 text-[13px] text-[var(--text-secondary)]")}>
                Pick a conversation to read it.
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function ConversationList({
  client,
  agentId,
  selected,
  onOpen,
}: {
  client: V1Client;
  agentId: string;
  selected: string | null;
  onOpen: (id: string) => void;
}) {
  const [items, setItems] = useState<V1Conversation[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(
    async (after: string | null) => {
      setLoading(true);
      setError(null);
      try {
        const page = await client.listConversations(agentId, after);
        setItems((cur) => (after ? [...cur, ...page.data] : page.data));
        setCursor(page.next_cursor);
        setHasMore(page.has_more);
      } catch (err) {
        setError(formatApiError(err, "Unable to load conversations"));
      } finally {
        setLoading(false);
      }
    },
    [client, agentId],
  );

  useEffect(() => {
    void load(null);
  }, [load]);

  return (
    <section className={CARD_CLASS} aria-label="Conversations">
      {error && <ErrorBanner message={error} className="m-3" />}
      {items.length === 0 && loading ? (
        <div className="px-4" role="status" aria-label="Loading conversations">
          <SkeletonRow lines={3} />
        </div>
      ) : items.length === 0 && !error ? (
        <EmptyState icon={<ChatsCircle size={28} aria-hidden />} title="No conversations" caption="Start one with POST /v1/agents/{id}/conversations." />
      ) : (
        <ul className="m-0 list-none divide-y divide-[var(--border-subtle)] p-0">
          {items.map((c) => (
            <li key={c.id}>
              <button
                type="button"
                onClick={() => onOpen(c.id)}
                aria-current={c.id === selected ? "true" : undefined}
                className={cn(
                  "w-full px-4 py-3 text-left transition-colors hover:bg-[var(--surface-hover)]",
                  c.id === selected && "bg-[var(--accent-primary)]/[0.06]",
                )}
              >
                <div className="truncate font-mono text-[12px] text-[var(--text-primary)]">{c.id}</div>
                <div className="mt-0.5 text-[12px] text-[var(--text-secondary)]">Started {formatDateTime(c.created_at)}</div>
              </button>
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
  );
}

function ConversationView({ client, id, onBack }: { client: V1Client; id: string; onBack: () => void }) {
  const [conv, setConv] = useState<V1Conversation | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    client
      .getConversation(id)
      .then((c) => active && setConv(c))
      .catch((err) => active && setError(formatApiError(err, "Unable to load the conversation")));
    return () => {
      active = false;
    };
  }, [client, id]);

  const messages = conv?.messages ?? [];

  return (
    <section className={CARD_CLASS} aria-label={`Conversation ${id}`}>
      <div className="flex flex-wrap items-center gap-2 border-b border-solid border-[var(--border-subtle)] px-4 py-3">
        <button type="button" onClick={onBack} className={cn(QUIET_BUTTON_CLASS, "lg:hidden")} aria-label="Back to conversations">
          <ArrowLeft size={14} aria-hidden /> Back
        </button>
        <MonoChip className="max-w-full truncate">{id}</MonoChip>
        {conv && <span className="text-[12px] text-[var(--text-secondary)]">Started {formatDateTime(conv.created_at)}</span>}
      </div>
      {error ? (
        <ErrorBanner message={error} className="m-4" />
      ) : !conv ? (
        <div className="px-4" role="status" aria-label="Loading messages">
          <SkeletonRow lines={3} />
        </div>
      ) : messages.length === 0 ? (
        <p className="m-0 p-6 text-center text-[13px] text-[var(--text-secondary)]">No messages yet.</p>
      ) : (
        <ol className="m-0 list-none space-y-3 p-4">
          {messages.map((m) => (
            <li key={m.id} className={cn("flex", m.role === "user" ? "justify-end" : "justify-start")}>
              <div
                className={cn(
                  "max-w-[85%] rounded-2xl px-3.5 py-2.5 text-[13px] leading-relaxed",
                  m.role === "user"
                    ? "bg-[var(--accent-primary)]/10 text-[var(--text-primary)]"
                    : "border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] text-[var(--text-primary)]",
                )}
              >
                <div className="mb-1 text-[11px] font-medium uppercase tracking-wide text-[var(--text-tertiary)]">
                  {m.role === "user" ? "User" : "Agent"} · {formatDateTime(m.created_at)}
                </div>
                <div className="whitespace-pre-wrap break-words">{m.content || (m.status === "failed" ? "" : "…")}</div>
                {m.status === "failed" && (
                  <div className="mt-1 text-[12px] text-[var(--status-error)]">Failed: {m.error ?? "the agent's turn failed"}</div>
                )}
              </div>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
