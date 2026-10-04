import React, { useCallback, useEffect, useState } from "react";
import {
  Key,
  Plus,
  Copy,
  Trash,
  Check,
  X,
  CircleNotch,
  WarningCircle,
  ShieldCheck,
} from "@phosphor-icons/react";
import { cn } from "@/lib/utils";
import {
  type ApiKey,
  type CreatedApiKey,
  listApiKeys,
  createApiKey,
  revokeApiKey,
} from "@/lib/api-keys";
import { formatApiError } from "@/lib/api-client";
import { EmptyState } from "@/components/settings/EmptyState";
import { QUIET_BUTTON_CLASS, DESTRUCTIVE_BUTTON_CLASS } from "@/components/settings/buttonStyles";
import { GatewayKeysPanel } from "@/pages/console/api-keys/GatewayKeysPanel";
import { PlatformProjectsPanel } from "@/pages/console/api-keys/PlatformProjectsPanel";

const AVAILABLE_SCOPES = [
  { value: "read", label: "Read" },
  { value: "compute", label: "Compute" },
  { value: "billing", label: "Billing" },
  { value: "devices", label: "Devices" },
];

function formatDate(iso?: string | null): string {
  if (!iso) return "—";
  return new Date(iso).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

export function ApiKeysPage() {
  const [tab, setTab] = useState<"scoped" | "gateway" | "platform">("scoped");
  const [keys, setKeys] = useState<ApiKey[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [showCreate, setShowCreate] = useState(false);
  const [newName, setNewName] = useState("");
  const [newScopes, setNewScopes] = useState<string[]>(["read"]);
  const [creating, setCreating] = useState(false);
  const [createdKey, setCreatedKey] = useState<CreatedApiKey | null>(null);

  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [confirmRevokeId, setConfirmRevokeId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await listApiKeys();
      setKeys(data);
    } catch (err) {
      setError(formatApiError(err, "Unable to load API keys"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (tab === "scoped") void load();
  }, [load, tab]);

  const handleCreate = useCallback(async () => {
    if (!newName.trim()) return;
    setCreating(true);
    setError(null);
    try {
      const key = await createApiKey({ name: newName, scopes: newScopes });
      setCreatedKey(key);
      setNewName("");
      setNewScopes(["read"]);
      await load();
    } catch (err) {
      setError(formatApiError(err, "Unable to create API key"));
    } finally {
      setCreating(false);
    }
  }, [newName, newScopes, load]);

  const handleRevoke = useCallback(
    async (id: string) => {
      setError(null);
      try {
        await revokeApiKey(id);
        setConfirmRevokeId(null);
        await load();
      } catch (err) {
        setError(formatApiError(err, "Unable to revoke key"));
      }
    },
    [load]
  );

  const copyToClipboard = useCallback(async (text: string, id: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedId(id);
      window.setTimeout(() => setCopiedId(null), 2000);
    } catch {
      // ignore
    }
  }, []);

  return (
    <div className="space-y-6">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h1 className="text-[22px] font-semibold tracking-tight text-[var(--text-primary)]">
            API Keys
          </h1>
          <p className="text-[13px] text-[var(--text-secondary)] mt-1">
            Create scoped keys for the Allternit cloud API, gateway keys for the LLM gateway, and project keys for the developer Platform API.
          </p>
          <div className="mt-3 inline-flex rounded-lg border border-solid border-[var(--border-subtle)] p-0.5">
            {(
              [
                { id: "scoped", label: "Scoped keys" },
                { id: "gateway", label: "Gateway keys" },
                { id: "platform", label: "Platform API" },
              ] as const
            ).map((item) => (
              <button
                key={item.id}
                type="button"
                onClick={() => setTab(item.id)}
                className={cn(
                  "rounded-md px-3 py-1.5 text-[12px] font-medium transition-colors",
                  tab === item.id
                    ? "bg-[var(--accent-primary)]/10 text-[var(--accent-primary)]"
                    : "text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                )}
              >
                {item.label}
              </button>
            ))}
          </div>
        </div>
        {tab === "scoped" && (
        <button
          type="button"
          onClick={() => {
            setShowCreate(true);
            setCreatedKey(null);
          }}
          className="inline-flex items-center gap-1.5 px-3 py-2 rounded-lg text-[13px] font-semibold bg-[var(--accent-primary)] text-[var(--ui-text-inverse)] hover:brightness-110 transition-all shrink-0"
        >
          <Plus size={14} /> Create key
        </button>
        )}
      </div>

      {tab === "gateway" ? (
        <GatewayKeysPanel />
      ) : tab === "platform" ? (
        <PlatformProjectsPanel />
      ) : (
        <>
      {showCreate && (
        <div className="rounded-xl border border-solid border-[var(--accent-primary)]/25 bg-[var(--accent-primary)]/[0.03] p-4">
          <div className="flex items-center justify-between gap-3 mb-3">
            <div className="text-[14px] font-semibold text-[var(--text-primary)]">Create API key</div>
            <button
              type="button"
              onClick={() => {
                setShowCreate(false);
                setCreatedKey(null);
              }}
              className="p-1 rounded-md text-[var(--text-tertiary)] hover:bg-[var(--surface-hover)]"
              aria-label="Close"
            >
              <X size={16} />
            </button>
          </div>

          {createdKey ? (
            <div className="space-y-3">
              <div className="rounded-lg border border-solid border-[var(--status-success)]/25 bg-[var(--status-success)]/[0.06] p-3">
                <div className="flex items-start gap-2">
                  <ShieldCheck size={16} className="text-[var(--status-success)] mt-0.5 shrink-0" />
                  <div className="min-w-0 flex-1">
                    <div className="text-[13px] font-medium text-[var(--text-primary)]">
                      Copy this token now
                    </div>
                    <p className="text-[12px] text-[var(--text-secondary)] mt-1">
                      It will not be shown again. Store it in a secret manager.
                    </p>
                  </div>
                </div>
              </div>
              <div className="flex items-center gap-2 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2">
                <code className="flex-1 text-[12px] font-mono text-[var(--text-primary)] truncate">
                  {createdKey.token}
                </code>
                <button
                  type="button"
                  onClick={() => void copyToClipboard(createdKey.token, "created")}
                  className="p-1.5 rounded-md text-[var(--text-tertiary)] hover:text-[var(--text-primary)] hover:bg-[var(--surface-hover)]"
                  aria-label="Copy token"
                >
                  {copiedId === "created" ? <Check size={14} className="text-[var(--status-success)]" /> : <Copy size={14} />}
                </button>
              </div>
              <div className="flex justify-end">
                <button
                  type="button"
                  onClick={() => {
                    setShowCreate(false);
                    setCreatedKey(null);
                  }}
                  className={QUIET_BUTTON_CLASS}
                >
                  Done
                </button>
              </div>
            </div>
          ) : (
            <div className="space-y-3">
              <label className="text-[12px] text-[var(--text-secondary)]">
                Key name
                <input
                  value={newName}
                  onChange={(e) => setNewName(e.target.value)}
                  placeholder="Production compute agent"
                  maxLength={80}
                  className="mt-1.5 w-full p-2 px-3 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] text-[13px] text-[var(--text-primary)] outline-none focus:border-[var(--accent-primary)] placeholder:text-[var(--text-tertiary)]"
                />
              </label>
              <div>
                <div className="text-[12px] text-[var(--text-secondary)] mb-1.5">Scopes</div>
                <div className="flex flex-wrap gap-2">
                  {AVAILABLE_SCOPES.map((scope) => (
                    <label
                      key={scope.value}
                      className={cn(
                        "inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-[12px] font-medium border border-solid cursor-pointer transition-colors",
                        newScopes.includes(scope.value)
                          ? "bg-[var(--accent-primary)]/10 border-[var(--accent-primary)]/30 text-[var(--accent-primary)]"
                          : "bg-[var(--bg-primary)] border-[var(--border-subtle)] text-[var(--text-secondary)] hover:border-[var(--border-default)]"
                      )}
                    >
                      <input
                        type="checkbox"
                        className="sr-only"
                        checked={newScopes.includes(scope.value)}
                        onChange={(e) => {
                          setNewScopes((prev) =>
                            e.target.checked
                              ? [...prev, scope.value]
                              : prev.filter((s) => s !== scope.value)
                          );
                        }}
                      />
                      {scope.label}
                    </label>
                  ))}
                </div>
              </div>
              <div className="flex justify-end gap-2">
                <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setShowCreate(false)}>
                  Cancel
                </button>
                <button
                  type="button"
                  onClick={() => void handleCreate()}
                  disabled={creating || !newName.trim()}
                  className="inline-flex items-center gap-1.5 px-3 py-2 rounded-lg text-[13px] font-semibold bg-[var(--accent-primary)] text-[var(--ui-text-inverse)] hover:brightness-110 transition-all disabled:opacity-50"
                >
                  {creating && <CircleNotch size={13} className="animate-spin" />}
                  Create key
                </button>
              </div>
            </div>
          )}
        </div>
      )}

      {error && (
        <div className="flex items-start gap-2 rounded-lg border border-solid border-[var(--status-error)]/25 bg-[var(--status-error)]/[0.06] px-3 py-2.5 text-[13px] text-[var(--status-error)]">
          <WarningCircle size={16} className="shrink-0 mt-0.5" />
          {error}
        </div>
      )}

      {loading ? (
        <div className="space-y-2">
          {Array.from({ length: 3 }).map((_, i) => (
            <div key={i} className="h-16 rounded-xl bg-[var(--bg-secondary)] animate-pulse" />
          ))}
        </div>
      ) : keys.length === 0 ? (
        <EmptyState
          icon={<Key size={32} weight="thin" />}
          title="No API keys yet"
          caption="Create a scoped key to authenticate scripts, agents, or CI pipelines against the Allternit cloud API."
          ctaLabel="Create your first key"
          onCtaClick={() => {
            setShowCreate(true);
            setCreatedKey(null);
          }}
          primaryCta
        />
      ) : (
        <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 overflow-hidden">
          <table className="w-full text-left">
            <thead>
              <tr className="border-b border-[var(--border-subtle)] text-[11px] uppercase tracking-wide text-[var(--text-tertiary)]">
                <th className="px-4 py-3 font-medium">Name</th>
                <th className="px-4 py-3 font-medium hidden sm:table-cell">Prefix</th>
                <th className="px-4 py-3 font-medium hidden md:table-cell">Scopes</th>
                <th className="px-4 py-3 font-medium hidden lg:table-cell">Created</th>
                <th className="px-4 py-3 font-medium text-right">Actions</th>
              </tr>
            </thead>
            <tbody className="text-[13px]">
              {keys.map((key) => (
                <tr key={key.id} className="border-b border-[var(--border-subtle)] last:border-b-0">
                  <td className="px-4 py-3">
                    <div className="font-medium text-[var(--text-primary)]">{key.name}</div>
                    <div className="text-[11px] text-[var(--text-tertiary)] mt-0.5 sm:hidden">
                      {key.prefix}…
                    </div>
                  </td>
                  <td className="px-4 py-3 text-[var(--text-secondary)] hidden sm:table-cell">
                    <code className="text-[12px] font-mono">{key.prefix}…</code>
                  </td>
                  <td className="px-4 py-3 hidden md:table-cell">
                    <div className="flex flex-wrap gap-1">
                      {key.scopes.map((scope) => (
                        <span
                          key={scope}
                          className="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium bg-[var(--bg-primary)] text-[var(--text-secondary)] border border-[var(--border-subtle)]"
                        >
                          {scope}
                        </span>
                      ))}
                    </div>
                  </td>
                  <td className="px-4 py-3 text-[var(--text-secondary)] hidden lg:table-cell">
                    {formatDate(key.createdAt)}
                  </td>
                  <td className="px-4 py-3 text-right">
                    {confirmRevokeId === key.id ? (
                      <div className="flex items-center justify-end gap-2">
                        <button
                          type="button"
                          className={QUIET_BUTTON_CLASS}
                          onClick={() => setConfirmRevokeId(null)}
                        >
                          Cancel
                        </button>
                        <button
                          type="button"
                          className={DESTRUCTIVE_BUTTON_CLASS}
                          onClick={() => void handleRevoke(key.id)}
                        >
                          Revoke
                        </button>
                      </div>
                    ) : (
                      <button
                        type="button"
                        className={DESTRUCTIVE_BUTTON_CLASS}
                        onClick={() => setConfirmRevokeId(key.id)}
                        aria-label={`Revoke ${key.name}`}
                      >
                        <Trash size={14} />
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4">
        <div className="text-[13px] font-semibold text-[var(--text-primary)] mb-2">Example API base URL</div>
        <div className="flex items-center gap-2 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2">
          <code className="flex-1 text-[12px] font-mono text-[var(--text-secondary)] truncate">
            https://api.allternit.com/api/v1
          </code>
          <button
            type="button"
            className="p-1.5 rounded-md text-[var(--text-tertiary)] hover:text-[var(--text-primary)] hover:bg-[var(--surface-hover)]"
            aria-label="Copy URL"
            onClick={() => void copyToClipboard("https://api.allternit.com/api/v1", "base-url")}
          >
            {copiedId === "base-url" ? <Check size={14} className="text-[var(--status-success)]" /> : <Copy size={14} />}
          </button>
        </div>
      </div>
        </>
      )}
    </div>
  );
}
