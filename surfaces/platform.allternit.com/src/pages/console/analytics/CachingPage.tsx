import React, { useCallback, useEffect, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  Cancel01Icon,
  DatabaseIcon,
} from "@hugeicons/core-free-icons";
import { api, formatApiError } from "@/lib/api-client";
import {
  ListPage,
  EmptyState,
  MonoChip,
  SkeletonRow,
  StatCard,
  GaugeCard,
  CodeTemplateBlock,
  QUIET_BUTTON_CLASS,
  SETTINGS_SELECT_CLASS,
} from "@/components/console-ui";
import {
  formatTokenCount,
  getGatewayCaching,
  microdollarsToUsd,
  type CachingResponse,
} from "@/lib/gateway-analytics";
import {
  ensureConsoleGatewayKey,
  gatewayAuthOptions,
} from "@/lib/console-gateway";

interface ContextCacheListItem {
  id: string;
  object: string;
  name: string | null;
  message_count: number;
  expires_at: number | null;
  created_at: number;
}

const PERIODS = ["7d", "30d", "90d"];

function gatewayBase(): string {
  return String(
    import.meta.env.VITE_ALLTERNIT_GATEWAY_URL || "https://api.allternit.com"
  ).replace(/\/+$/, "");
}

const CACHE_CONTROL_CURL = `curl ${gatewayBase()}/v1/chat/completions \\
  -H "Authorization: Bearer $ALLTERNIT_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{
    "model": "deepseek-v4.1-flash",
    "messages": [
      {
        "role": "system",
        "content": "Your long, stable system prompt or document context goes here.",
        "cache_control": { "type": "ephemeral" }
      },
      { "role": "user", "content": "Summarize the above." }
    ]
  }'

# Mark the stable prefix of a request with cache_control and the gateway
# reuses the cached prefix on subsequent calls — billed as cached tokens.`;

function formatUnix(secs: number | null | undefined): string {
  if (!secs) return "—";
  return new Date(secs * 1000).toLocaleString();
}

export function CachingPage(): React.ReactNode {
  const [data, setData] = useState<CachingResponse | null>(null);
  const [caches, setCaches] = useState<ContextCacheListItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [cachesLoading, setCachesLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [cachesError, setCachesError] = useState<string | null>(null);
  const [period, setPeriod] = useState("30d");
  const [showCreate, setShowCreate] = useState(false);
  const [createName, setCreateName] = useState("");
  const [createTtl, setCreateTtl] = useState("3600");
  const [createMessages, setCreateMessages] = useState("");
  const [creating, setCreating] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setData(await getGatewayCaching(period));
    } catch (err) {
      setError(formatApiError(err, "Unable to load caching stats"));
      setData(null);
    } finally {
      setLoading(false);
    }
  }, [period]);

  const loadCaches = useCallback(async () => {
    setCachesLoading(true);
    setCachesError(null);
    try {
      const key = await ensureConsoleGatewayKey();
      const data = await apiGetCaches(key);
      setCaches(data);
    } catch (err) {
      setCachesError(formatApiError(err, "Unable to load your caches"));
      setCaches([]);
    } finally {
      setCachesLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    void loadCaches();
  }, [loadCaches]);

  const handleCreate = useCallback(async () => {
    setCreating(true);
    setCachesError(null);
    let messages: unknown;
    try {
      messages = JSON.parse(createMessages || "[]");
    } catch (err) {
      setCachesError(
        `Messages JSON is invalid: ${err instanceof Error ? err.message : String(err)}`
      );
      setCreating(false);
      return;
    }
    try {
      const key = await ensureConsoleGatewayKey();
      const ttl = Number(createTtl);
      await apiCreateCache(key, {
        name: createName.trim() || undefined,
        ttl_seconds: Number.isFinite(ttl) && ttl > 0 ? ttl : undefined,
        messages,
      });
      setShowCreate(false);
      setCreateName("");
      setCreateTtl("3600");
      setCreateMessages("");
      await loadCaches();
    } catch (err) {
      setCachesError(formatApiError(err, "Unable to create cache"));
    } finally {
      setCreating(false);
    }
  }, [createName, createTtl, createMessages, loadCaches]);

  const handleDelete = useCallback(
    async (id: string) => {
      if (!window.confirm(`Delete cache ${id}? Requests referencing it will no longer reuse its messages.`)) return;
      setBusyId(id);
      setCachesError(null);
      try {
        const key = await ensureConsoleGatewayKey();
        await apiDeleteCache(key, id);
        await loadCaches();
        await load();
      } catch (err) {
        setCachesError(formatApiError(err, "Delete failed"));
      } finally {
        setBusyId(null);
      }
    },
    [load, loadCaches]
  );

  const totals = data?.totals ?? null;
  const hitRateDenominator = Math.max(1, totals?.requests_with_context_cache ?? 0);
  const hitRate = Math.min(100, Math.round(((totals?.context_cache_hits ?? 0) / hitRateDenominator) * 100));

  return (
    <ListPage
      title="Caching"
      subtitle="Context-cache reuse and cached-token savings across your gateway traffic."
      filters={
        <select
          value={period}
          onChange={(e) => setPeriod(e.target.value)}
          aria-label="Period"
          className={SETTINGS_SELECT_CLASS}
        >
          {PERIODS.map((p) => (
            <option key={p} value={p}>
              Last {p}
            </option>
          ))}
        </select>
      }
    >
      {error && (
        <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      )}

      {loading && !data ? (
        <SkeletonRow lines={6} />
      ) : !data || !totals ? null : (
        <div className="space-y-4">
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
            <StatCard
              label="Cached tokens"
              value={formatTokenCount(totals.cached_tokens)}
              hint={`${totals.requests.toLocaleString()} requests in this window`}
            />
            <StatCard
              label="Estimated savings"
              value={microdollarsToUsd(totals.estimated_savings_microdollars)}
              hint={
                totals.savings_unpriced_models > 0
                  ? `${totals.savings_unpriced_models} model(s) unpriced — excluded from the estimate`
                  : "Estimate, not a billed figure"
              }
            />
            <GaugeCard
              label="Cache hit rate"
              used={totals.context_cache_hits}
              limit={Math.max(1, totals.requests_with_context_cache)}
              resetHint={`${hitRate}% of cache-eligible requests reused a context cache`}
            />
          </div>

          {data.by_model.length > 0 && (
            <div className="overflow-x-auto rounded-xl border border-solid border-[var(--border-subtle)]">
              <table className="w-full border-collapse text-left text-[13px]">
                <thead>
                  <tr className="border-b border-solid border-[var(--border-subtle)] text-[11px] uppercase tracking-wide text-[var(--text-tertiary)]">
                    <th className="px-3 py-2 font-medium">Model</th>
                    <th className="px-3 py-2 font-medium">Cached tokens</th>
                    <th className="px-3 py-2 font-medium">Estimated savings</th>
                  </tr>
                </thead>
                <tbody>
                  {data.by_model.map((row) => (
                    <tr
                      key={`${row.provider_id}/${row.model_id}`}
                      className="border-b border-solid border-[var(--border-subtle)] last:border-b-0 hover:bg-[var(--surface-hover)]"
                    >
                      <td className="px-3 py-2.5 font-mono text-[12px] text-[var(--text-primary)]">
                        {row.provider_id ? `${row.provider_id}/` : ""}
                        {row.model_id}
                      </td>
                      <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                        {formatTokenCount(row.cached_tokens)}
                      </td>
                      <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                        {row.estimated_savings_microdollars === null
                          ? "Not priced"
                          : microdollarsToUsd(row.estimated_savings_microdollars)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
            {data.savings_estimate_basis}
          </p>
        </div>
      )}

      <div className="mt-8">
        <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
          <div>
            <h2 className="m-0 text-[16px] font-semibold text-[var(--text-primary)]">
              Your context caches
            </h2>
            <p className="m-0 mt-0.5 text-[12px] text-[var(--text-tertiary)]">
              Reusable message prefixes attached to your API key. Reference an
              ID as <span className="font-mono">context_cache_id</span> on a chat
              completion to prepend its messages.
            </p>
          </div>
          <button
            type="button"
            onClick={() => setShowCreate((v) => !v)}
            className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110"
          >
            Create cache
          </button>
        </div>

        {cachesError && (
          <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
            <HugeiconsIcon icon={AlertCircleIcon} size={14} />
            {cachesError}
          </p>
        )}

        {showCreate && (
          <div className="mb-4 space-y-3 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <div className="space-y-1.5">
                <label
                  htmlFor="cache-name"
                  className="text-[12px] font-semibold text-[var(--text-secondary)]"
                >
                  Name
                </label>
                <input
                  id="cache-name"
                  type="text"
                  value={createName}
                  onChange={(e) => setCreateName(e.target.value)}
                  placeholder="support-agent-context"
                  className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 font-mono text-[12px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]"
                />
              </div>
              <div className="space-y-1.5">
                <label
                  htmlFor="cache-ttl"
                  className="text-[12px] font-semibold text-[var(--text-secondary)]"
                >
                  TTL (seconds, 0 = no expiry)
                </label>
                <input
                  id="cache-ttl"
                  type="number"
                  min={0}
                  value={createTtl}
                  onChange={(e) => setCreateTtl(e.target.value)}
                  className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 font-mono text-[12px] text-[var(--text-primary)] outline-none focus:border-[var(--border-default)]"
                />
              </div>
            </div>
            <div className="space-y-1.5">
              <label
                htmlFor="cache-messages"
                className="text-[12px] font-semibold text-[var(--text-secondary)]"
              >
                Messages (JSON array — typically a long system prompt or document context)
              </label>
              <textarea
                id="cache-messages"
                value={createMessages}
                onChange={(e) => setCreateMessages(e.target.value)}
                placeholder={'[\n  { "role": "system", "content": "…long stable context…" }\n]'}
                rows={6}
                className="w-full resize-y rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 font-mono text-[12px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]"
              />
            </div>
            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={() => void handleCreate()}
                disabled={creating}
                className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {creating ? "Creating…" : "Create cache"}
              </button>
              <button
                type="button"
                onClick={() => setShowCreate(false)}
                className={QUIET_BUTTON_CLASS}
              >
                Cancel
              </button>
            </div>
          </div>
        )}

        {cachesLoading ? (
          <SkeletonRow lines={4} />
        ) : caches.length === 0 ? (
          <div className="space-y-6">
            <EmptyState
              icon={<HugeiconsIcon icon={DatabaseIcon} size={32} />}
              title="No context caches"
              caption="Create one above, or start marking the stable prefix of your requests with cache_control and the gateway will serve repeated prefixes as cached tokens."
            />
            <div className="mx-auto max-w-xl">
              <CodeTemplateBlock language="bash" code={CACHE_CONTROL_CURL} />
            </div>
          </div>
        ) : (
          <div className="overflow-x-auto rounded-xl border border-solid border-[var(--border-subtle)]">
            <table className="w-full border-collapse text-left text-[13px]">
              <thead>
                <tr className="border-b border-solid border-[var(--border-subtle)] text-[11px] uppercase tracking-wide text-[var(--text-tertiary)]">
                  <th className="px-3 py-2 font-medium">ID</th>
                  <th className="px-3 py-2 font-medium">Name</th>
                  <th className="px-3 py-2 font-medium">Messages</th>
                  <th className="px-3 py-2 font-medium">Created</th>
                  <th className="px-3 py-2 font-medium">Expires</th>
                  <th className="px-3 py-2 text-right font-medium">Actions</th>
                </tr>
              </thead>
              <tbody>
                {caches.map((cache) => (
                  <tr
                    key={cache.id}
                    className="border-b border-solid border-[var(--border-subtle)] last:border-b-0 hover:bg-[var(--surface-hover)]"
                  >
                    <td className="px-3 py-2.5">
                      <MonoChip>{cache.id}</MonoChip>
                    </td>
                    <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                      {cache.name ?? "—"}
                    </td>
                    <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                      {cache.message_count}
                    </td>
                    <td className="px-3 py-2.5 whitespace-nowrap text-[var(--text-secondary)]">
                      {formatUnix(cache.created_at)}
                    </td>
                    <td className="px-3 py-2.5 whitespace-nowrap text-[var(--text-secondary)]">
                      {cache.expires_at === null ? "Never" : formatUnix(cache.expires_at)}
                    </td>
                    <td className="px-3 py-2.5 text-right">
                      <button
                        type="button"
                        onClick={() => void handleDelete(cache.id)}
                        disabled={busyId === cache.id}
                        className={QUIET_BUTTON_CLASS + " px-2.5 py-1 text-[12px] hover:text-[var(--status-error)] hover:border-[var(--status-error)]/30"}
                      >
                        <HugeiconsIcon icon={Cancel01Icon} size={12} />
                        {busyId === cache.id ? "Deleting…" : "Delete"}
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </ListPage>
  );
}

// Thin wrappers so every network call funnels through the `api` client with
// the virtual-key Authorization override (llm_gateway/mod.rs:86,91-95).
function apiGetCaches(key: string): Promise<ContextCacheListItem[]> {
  return api
    .get<{ data?: ContextCacheListItem[] } | ContextCacheListItem[]>(
      "/v1/context-caches",
      gatewayAuthOptions(key)
    )
    .then((data) => (Array.isArray(data) ? data : data.data ?? []));
}

function apiCreateCache(
  key: string,
  body: { name?: string; ttl_seconds?: number; messages: unknown }
): Promise<unknown> {
  return api.post("/v1/context-caches", body, gatewayAuthOptions(key));
}

function apiDeleteCache(key: string, id: string): Promise<unknown> {
  return api.delete(`/v1/context-caches/${encodeURIComponent(id)}`, gatewayAuthOptions(key));
}

export default CachingPage;
