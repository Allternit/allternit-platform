import React, { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { ChatsCircle, CircleNotch, Key, PencilSimple, Plus, Robot, Trash, X } from "@phosphor-icons/react";
import { formatApiError } from "@/lib/api-client";
import {
  AGENT_TOOLS,
  AUTONOMY_LEVELS,
  MODEL_PROVIDERS,
  STOCK_VOICES,
  allPages,
  type AgentInput,
  type AgentTool,
  type Autonomy,
  type ModelProvider,
  type V1Account,
  type V1Agent,
  type V1Client,
  type V1ModelKey,
} from "@/lib/platform-v1";
import { Badge, EmptyState, MonoChip, SectionHeading, SkeletonRow } from "@/components/console-ui";
import { DESTRUCTIVE_BUTTON_CLASS, QUIET_BUTTON_CLASS, SETTINGS_SELECT_CLASS } from "@/components/console-ui/buttonStyles";
import { cn } from "@/lib/utils";
import {
  CARD_CLASS,
  ErrorBanner,
  INPUT_CLASS,
  PRIMARY_BUTTON_CLASS,
  PlatformHeader,
  ProjectGate,
  formatDate,
  usePlatformProjects,
} from "./shared";

/** Hosted agents in the selected project, plus the project's model keys. */
export function PlatformAgentsPage() {
  const state = usePlatformProjects();
  return (
    <div className="mx-auto w-full max-w-5xl">
      <PlatformHeader
        title="Agents"
        subtitle="Hosted agents answer conversations, calls and texts for one account in your project."
        state={state}
      />
      <ProjectGate state={state}>{({ project, client }) => <AgentsBody key={project.id} client={client} />}</ProjectGate>
    </div>
  );
}

type Editing = { mode: "new" } | { mode: "edit"; agent: V1Agent } | null;

function AgentsBody({ client }: { client: V1Client }) {
  const [agents, setAgents] = useState<V1Agent[]>([]);
  const [accounts, setAccounts] = useState<V1Account[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<Editing>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [a, accts] = await Promise.all([
        allPages((after) => client.listAgents(after)),
        allPages((after) => client.listAccounts(after)),
      ]);
      setAgents(a);
      setAccounts(accts);
    } catch (err) {
      setError(formatApiError(err, "Unable to load agents"));
    } finally {
      setLoading(false);
    }
  }, [client]);

  useEffect(() => {
    void load();
  }, [load]);

  const remove = async (id: string) => {
    setError(null);
    try {
      await client.deleteAgent(id);
      setConfirmDelete(null);
      await load();
    } catch (err) {
      setError(formatApiError(err, "Unable to delete the agent"));
    }
  };

  const accountName = (id: string) => accounts.find((a) => a.id === id)?.name ?? id;

  return (
    <div className="space-y-4">
      {error && <ErrorBanner message={error} />}

      <div className="flex justify-end">
        <button type="button" className={PRIMARY_BUTTON_CLASS} onClick={() => setEditing({ mode: "new" })} disabled={loading}>
          <Plus size={14} aria-hidden /> New agent
        </button>
      </div>

      {editing && (
        <AgentForm
          key={editing.mode === "edit" ? editing.agent.id : "new"}
          client={client}
          accounts={accounts}
          agent={editing.mode === "edit" ? editing.agent : null}
          onClose={() => setEditing(null)}
          onSaved={async () => {
            setEditing(null);
            await load();
          }}
          onAccountCreated={(a) => setAccounts((cur) => [...cur, a])}
        />
      )}

      <section className={CARD_CLASS} aria-label="Agents">
        {loading ? (
          <div className="px-4" role="status" aria-label="Loading agents">
            <SkeletonRow lines={2} />
            <SkeletonRow lines={2} />
          </div>
        ) : agents.length === 0 ? (
          <EmptyState
            icon={<Robot size={32} aria-hidden />}
            title="No agents yet"
            caption="Create an agent for one of your customer accounts. It gets an AI greeting, a stock voice and the tools you allow."
            ctaLabel="New agent"
            onCtaClick={() => setEditing({ mode: "new" })}
            primaryCta
          />
        ) : (
          <ul className="m-0 list-none divide-y divide-[var(--border-subtle)] p-0">
            {agents.map((a) => (
              <li key={a.id} className="flex flex-wrap items-start gap-3 px-4 py-3">
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="text-[14px] font-semibold text-[var(--text-primary)]">{a.name}</span>
                    <Badge className={a.status === "ready" ? "bg-[var(--status-success)]/10 text-[var(--status-success)]" : undefined}>
                      {a.status === "ready" ? "Ready" : "Not started yet"}
                    </Badge>
                  </div>
                  <div className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 text-[12px] text-[var(--text-secondary)]">
                    <span>Account: {accountName(a.account_id)}</span>
                    <span>Model: {a.model}</span>
                    <span>Voice: {a.voice}</span>
                    <span>Autonomy: {a.autonomy}</span>
                    <span>Created {formatDate(a.created_at)}</span>
                  </div>
                  <div className="mt-1 text-[12px] text-[var(--text-tertiary)]">
                    Tools: {a.tools.length ? a.tools.join(", ") : "none"}
                  </div>
                  <MonoChip className="mt-2 max-w-full truncate">{a.id}</MonoChip>
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <Link to={`/platform/conversations?agent=${encodeURIComponent(a.id)}`} className={QUIET_BUTTON_CLASS}>
                    <ChatsCircle size={14} aria-hidden /> Conversations
                  </Link>
                  <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setEditing({ mode: "edit", agent: a })}>
                    <PencilSimple size={14} aria-hidden /> Edit
                  </button>
                  {confirmDelete === a.id ? (
                    <>
                      <button type="button" className={DESTRUCTIVE_BUTTON_CLASS} onClick={() => void remove(a.id)}>
                        <Trash size={14} aria-hidden /> Delete
                      </button>
                      <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setConfirmDelete(null)}>
                        Cancel
                      </button>
                    </>
                  ) : (
                    <button type="button" className={DESTRUCTIVE_BUTTON_CLASS} onClick={() => setConfirmDelete(a.id)} aria-label={`Delete ${a.name}`}>
                      <Trash size={14} aria-hidden />
                    </button>
                  )}
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>

      <ModelKeys client={client} />
    </div>
  );
}

function AgentForm({
  client,
  accounts,
  agent,
  onClose,
  onSaved,
  onAccountCreated,
}: {
  client: V1Client;
  accounts: V1Account[];
  agent: V1Agent | null;
  onClose: () => void;
  onSaved: () => Promise<void>;
  onAccountCreated: (a: V1Account) => void;
}) {
  const [accountId, setAccountId] = useState(agent?.account_id ?? accounts[0]?.id ?? "");
  const [newAccount, setNewAccount] = useState("");
  const [name, setName] = useState(agent?.name ?? "");
  const [instructions, setInstructions] = useState(agent?.instructions ?? "");
  const [greeting, setGreeting] = useState(agent?.greeting ?? "");
  const [model, setModel] = useState(agent?.model ?? "allternit");
  const [voice, setVoice] = useState(agent?.voice ?? "af_heart");
  const [tools, setTools] = useState<AgentTool[]>(agent?.tools ?? []);
  const [autonomy, setAutonomy] = useState<Autonomy>(agent?.autonomy ?? "ask");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      let account = accountId;
      if (!agent && !account && newAccount.trim()) {
        const created = await client.createAccount({ name: newAccount.trim() });
        onAccountCreated(created);
        account = created.id;
        setAccountId(created.id);
      }
      const input: AgentInput = { name: name.trim(), instructions, model: model.trim() || "allternit", voice, tools, autonomy };
      // An empty greeting on create means "use the default AI greeting".
      if (greeting.trim() || agent) input.greeting = greeting.trim() || undefined;
      if (agent) await client.updateAgent(agent.id, input);
      else await client.createAgent({ ...input, account_id: account });
      await onSaved();
    } catch (err) {
      setError(formatApiError(err, "Unable to save the agent"));
    } finally {
      setSaving(false);
    }
  };

  const needsAccount = !agent && !accountId && !newAccount.trim();

  return (
    <section aria-label={agent ? `Edit ${agent.name}` : "New agent"} className="space-y-3 rounded-xl border border-solid border-[var(--accent-primary)]/25 bg-[var(--accent-primary)]/[0.03] p-4">
      <div className="flex items-center justify-between gap-3">
        <div className="text-[14px] font-semibold text-[var(--text-primary)]">{agent ? `Edit ${agent.name}` : "New agent"}</div>
        <button type="button" onClick={onClose} className="rounded-md p-1 text-[var(--text-tertiary)] hover:bg-[var(--surface-hover)]" aria-label="Close">
          <X size={16} />
        </button>
      </div>
      {error && <ErrorBanner message={error} />}

      <div className="grid gap-3 sm:grid-cols-2">
        {agent ? (
          <div className="text-[12px] font-medium text-[var(--text-secondary)]">
            Account
            <div className="mt-1.5 text-[13px] text-[var(--text-primary)]">{accounts.find((a) => a.id === agent.account_id)?.name ?? agent.account_id}</div>
          </div>
        ) : accounts.length > 0 ? (
          <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
            Account
            <select value={accountId} onChange={(e) => setAccountId(e.target.value)} className={cn(SETTINGS_SELECT_CLASS, "mt-1.5 w-full")}>
              {accounts.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name}
                </option>
              ))}
            </select>
          </label>
        ) : (
          <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
            Account (your customer)
            <input value={newAccount} onChange={(e) => setNewAccount(e.target.value)} placeholder="Lakeside Dental" maxLength={120} className={INPUT_CLASS} />
          </label>
        )}
        <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
          Name
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Front desk" maxLength={120} className={INPUT_CLASS} />
        </label>
      </div>

      <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
        Instructions
        <textarea value={instructions} onChange={(e) => setInstructions(e.target.value)} rows={4} maxLength={32000} placeholder="Book cleanings. Never give medical advice." className={INPUT_CLASS} />
      </label>
      <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
        Greeting (must say it is an AI)
        <input value={greeting} onChange={(e) => setGreeting(e.target.value)} maxLength={500} placeholder={`Hi, this is ${name || "Front desk"}, an AI assistant. How can I help?`} className={INPUT_CLASS} />
      </label>

      <div className="grid gap-3 sm:grid-cols-3">
        <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
          Model
          <input value={model} onChange={(e) => setModel(e.target.value)} placeholder="allternit or provider/model" className={INPUT_CLASS} />
          <span className="mt-1 block text-[11px] font-normal text-[var(--text-tertiary)]">"allternit", or e.g. anthropic/… with a model key below.</span>
        </label>
        <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
          Voice
          <select value={voice} onChange={(e) => setVoice(e.target.value)} className={cn(SETTINGS_SELECT_CLASS, "mt-1.5 w-full")}>
            {STOCK_VOICES.map((v) => (
              <option key={v} value={v}>
                {v}
              </option>
            ))}
          </select>
        </label>
        <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
          Autonomy
          <select value={autonomy} onChange={(e) => setAutonomy(e.target.value as Autonomy)} className={cn(SETTINGS_SELECT_CLASS, "mt-1.5 w-full")}>
            {AUTONOMY_LEVELS.map((l) => (
              <option key={l.value} value={l.value}>
                {l.label}
              </option>
            ))}
          </select>
        </label>
      </div>

      <fieldset>
        <legend className="text-[12px] font-medium text-[var(--text-secondary)]">Tools</legend>
        <div className="mt-1.5 grid grid-cols-1 gap-1.5 sm:grid-cols-2">
          {AGENT_TOOLS.map((t) => (
            <label key={t.value} className={cn("inline-flex items-center gap-1.5 text-[13px]", t.available ? "text-[var(--text-primary)]" : "text-[var(--text-tertiary)]")}>
              <input
                type="checkbox"
                disabled={!t.available}
                checked={tools.includes(t.value)}
                onChange={(e) => setTools((cur) => (e.target.checked ? [...cur, t.value] : cur.filter((x) => x !== t.value)))}
              />
              {t.label}
              {!t.available && <span className="text-[11px]">(coming)</span>}
            </label>
          ))}
        </div>
      </fieldset>

      <button type="button" className={PRIMARY_BUTTON_CLASS} disabled={!name.trim() || needsAccount || saving} onClick={() => void save()}>
        {saving ? <CircleNotch size={14} className="animate-spin" aria-hidden /> : null} {agent ? "Save agent" : "Create agent"}
      </button>
    </section>
  );
}

/** The project's own provider keys: masked, set or replace, delete. */
function ModelKeys({ client }: { client: V1Client }) {
  const [keys, setKeys] = useState<V1ModelKey[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [provider, setProvider] = useState<ModelProvider>("anthropic");
  const [secret, setSecret] = useState("");
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setKeys((await client.listModelKeys()).data);
    } catch (err) {
      setError(formatApiError(err, "Unable to load model keys"));
    } finally {
      setLoading(false);
    }
  }, [client]);

  useEffect(() => {
    void load();
  }, [load]);

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      await client.putModelKey(provider, secret.trim());
      setSecret("");
      await load();
    } catch (err) {
      setError(formatApiError(err, "Unable to save the model key"));
    } finally {
      setSaving(false);
    }
  };

  const remove = async (p: ModelProvider) => {
    setError(null);
    try {
      await client.deleteModelKey(p);
      await load();
    } catch (err) {
      setError(formatApiError(err, "Unable to delete the model key"));
    }
  };

  return (
    <section aria-label="Model keys">
      <SectionHeading>Model keys</SectionHeading>
      <p className="m-0 mb-3 text-[13px] text-[var(--text-secondary)]">
        Agents whose model is <code className="font-mono">provider/…</code> run on the project's own key. Keys are stored encrypted and only the last four characters are shown.
      </p>
      {error && <ErrorBanner message={error} className="mb-3" />}
      <div className={CARD_CLASS}>
        {loading ? (
          <div className="px-4" role="status" aria-label="Loading model keys">
            <SkeletonRow />
          </div>
        ) : keys.length === 0 ? (
          <p className="m-0 px-4 py-4 text-[13px] text-[var(--text-secondary)]">No model keys. Agents use the Allternit model.</p>
        ) : (
          <ul className="m-0 list-none divide-y divide-[var(--border-subtle)] p-0">
            {keys.map((k) => (
              <li key={k.provider} className="flex flex-wrap items-center gap-3 px-4 py-3">
                <Key size={16} className="text-[var(--text-tertiary)]" aria-hidden />
                <span className="text-[13px] font-medium text-[var(--text-primary)]">{MODEL_PROVIDERS.find((p) => p.value === k.provider)?.label ?? k.provider}</span>
                <code className="font-mono text-[12px] text-[var(--text-secondary)]">{k.masked}</code>
                <span className="flex-1 text-[12px] text-[var(--text-tertiary)]">updated {formatDate(k.updated_at)}</span>
                <button type="button" className={DESTRUCTIVE_BUTTON_CLASS} onClick={() => void remove(k.provider)} aria-label={`Delete ${k.provider} key`}>
                  <Trash size={14} aria-hidden /> Delete
                </button>
              </li>
            ))}
          </ul>
        )}
        <div className="flex flex-col gap-2 border-t border-solid border-[var(--border-subtle)] p-4 sm:flex-row sm:items-end">
          <label className="flex flex-col text-[12px] font-medium text-[var(--text-secondary)]">
            Provider
            <select value={provider} onChange={(e) => setProvider(e.target.value as ModelProvider)} className={cn(SETTINGS_SELECT_CLASS, "mt-1.5 w-full sm:w-auto")}>
              {MODEL_PROVIDERS.map((p) => (
                <option key={p.value} value={p.value}>
                  {p.label}
                </option>
              ))}
            </select>
          </label>
          <label className="block flex-1 text-[12px] font-medium text-[var(--text-secondary)]">
            API key
            <input type="password" autoComplete="off" value={secret} onChange={(e) => setSecret(e.target.value)} placeholder="Paste the provider key" className={INPUT_CLASS} />
          </label>
          <button type="button" className={PRIMARY_BUTTON_CLASS} disabled={secret.trim().length < 8 || saving} onClick={() => void save()}>
            {saving ? <CircleNotch size={14} className="animate-spin" aria-hidden /> : null} Save key
          </button>
        </div>
      </div>
    </section>
  );
}
