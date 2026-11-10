import React, { useCallback, useEffect, useMemo, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Briefcase01Icon,
  AlertCircleIcon,
  Cancel01Icon,
  DocumentCodeIcon,
} from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { api, formatApiError } from "@/lib/api-client";
import {
  ListPage,
  EmptyState,
  MonoChip,
  Badge,
  SkeletonRow,
  CodeTemplateBlock,
  QUIET_BUTTON_CLASS,
} from "@/components/console-ui";
import {
  ensureConsoleGatewayKey,
  gatewayAuthOptions,
  type OpenAiBatch,
  type OpenAiListResponse,
} from "@/lib/console-gateway";

const STATUSES = [
  "validating",
  "in_progress",
  "finalizing",
  "completed",
  "failed",
  "expired",
  "cancelling",
  "cancelled",
];

function formatDate(unixSeconds: number | null | undefined): string {
  if (!unixSeconds) return "—";
  return new Date(unixSeconds * 1000).toLocaleString();
}

function gatewayBase(): string {
  return String(
    import.meta.env.VITE_ALLTERNIT_GATEWAY_URL || "https://api.allternit.com"
  ).replace(/\/+$/, "");
}

const EMPTY_CURL = `curl ${gatewayBase()}/v1/batches \\
  -H "Authorization: Bearer $ALLTERNIT_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{
    "endpoint": "/v1/chat/completions",
    "input_file_id": "file_...",
    "completion_window": "24h"
  }'

# The input file is JSONL — one chat completion request per line.`;

const REQUESTS_PLACEHOLDER = `[
  {
    "model": "kimi-k3",
    "messages": [{ "role": "user", "content": "Hello" }],
    "max_tokens": 100
  }
]`;

type CreateMode = "requests" | "file";

export function BatchesPage(): React.ReactNode {
  const [batches, setBatches] = useState<OpenAiBatch[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [statusFilter, setStatusFilter] = useState("");
  const [showCreate, setShowCreate] = useState(false);
  const [createMode, setCreateMode] = useState<CreateMode>("requests");
  const [requestsJson, setRequestsJson] = useState("");
  const [inputFileId, setInputFileId] = useState("");
  const [creating, setCreating] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<OpenAiBatch | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [results, setResults] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const key = await ensureConsoleGatewayKey();
      const data = await api.get<OpenAiListResponse<OpenAiBatch>>(
        "/v1/batches",
        gatewayAuthOptions(key)
      );
      setBatches(data.data ?? []);
    } catch (err) {
      setError(formatApiError(err, "Unable to load batches"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const filtered = useMemo(
    () =>
      statusFilter
        ? batches.filter((b) => b.status === statusFilter)
        : batches,
    [batches, statusFilter]
  );

  const openDetail = useCallback(async (id: string) => {
    setSelectedId(id);
    setDetail(null);
    setResults(null);
    setDetailLoading(true);
    try {
      const key = await ensureConsoleGatewayKey();
      const data = await api.get<OpenAiBatch>(
        `/v1/batches/${id}`,
        gatewayAuthOptions(key)
      );
      setDetail(data);
    } catch (err) {
      setError(formatApiError(err, "Unable to load batch details"));
      setSelectedId(null);
    } finally {
      setDetailLoading(false);
    }
  }, []);

  const closeDetail = useCallback(() => {
    setSelectedId(null);
    setDetail(null);
    setResults(null);
  }, []);

  const handleCreate = useCallback(async () => {
    setCreating(true);
    setError(null);
    try {
      const key = await ensureConsoleGatewayKey();
      const body =
        createMode === "requests"
          ? { requests: JSON.parse(requestsJson || "[]") }
          : {
              input_file_id: inputFileId.trim(),
              endpoint: "/v1/chat/completions",
              completion_window: "24h",
            };
      await api.post("/v1/batches", body, gatewayAuthOptions(key));
      setShowCreate(false);
      setRequestsJson("");
      setInputFileId("");
      await load();
    } catch (err) {
      setError(
        err instanceof SyntaxError
          ? `Requests JSON is invalid: ${err.message}`
          : formatApiError(err, "Unable to create batch")
      );
    } finally {
      setCreating(false);
    }
  }, [createMode, requestsJson, inputFileId, load]);

  const handleCancel = useCallback(
    async (id: string) => {
      if (!window.confirm(`Cancel batch ${id}?`)) return;
      setBusy(id);
      setError(null);
      try {
        const key = await ensureConsoleGatewayKey();
        await api.post(
          `/v1/batches/${id}/cancel`,
          {},
          gatewayAuthOptions(key)
        );
        await load();
        if (selectedId === id) await openDetail(id);
      } catch (err) {
        setError(formatApiError(err, "Cancel failed"));
      } finally {
        setBusy(null);
      }
    },
    [load, selectedId, openDetail]
  );

  const handleResults = useCallback(async (id: string) => {
    setBusy(`results-${id}`);
    setError(null);
    try {
      const key = await ensureConsoleGatewayKey();
      const data = await api.get<OpenAiListResponse<unknown>>(
        `/v1/batches/${id}/results`,
        gatewayAuthOptions(key)
      );
      setResults(JSON.stringify(data.data ?? [], null, 2));
    } catch (err) {
      setError(formatApiError(err, "Unable to load results"));
    } finally {
      setBusy(null);
    }
  }, []);

  return (
    <ListPage
      title="Batches"
      subtitle="Submit large workloads through the Batches API and retrieve results within 24 hours."
      filters={
        <select
          value={statusFilter}
          onChange={(e) => setStatusFilter(e.target.value)}
          aria-label="Filter by status"
          className="rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-2 px-3 text-[13px] text-[var(--text-primary)]"
        >
          <option value="">All statuses</option>
          {STATUSES.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </select>
      }
      primaryAction={{
        label: "Create batch",
        onClick: () => setShowCreate((v) => !v),
      }}
    >
      {error && (
        <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      )}

      {showCreate && (
        <div className="mb-4 space-y-3 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
          <div className="inline-flex rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] p-0.5">
            {(
              [
                ["requests", "From requests (JSON)"],
                ["file", "From input file"],
              ] as [CreateMode, string][]
            ).map(([mode, label]) => (
              <button
                key={mode}
                type="button"
                onClick={() => setCreateMode(mode)}
                className={cn(
                  "rounded-lg px-3 py-1 text-[12px] font-semibold transition-colors",
                  createMode === mode
                    ? "bg-[var(--accent-primary)] text-[var(--ui-text-inverse)]"
                    : "text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                )}
              >
                {label}
              </button>
            ))}
          </div>

          {createMode === "requests" ? (
            <div className="space-y-1.5">
              <label
                htmlFor="batch-requests"
                className="text-[12px] font-semibold text-[var(--text-secondary)]"
              >
                Requests (JSON array — one chat completion request per entry)
              </label>
              <textarea
                id="batch-requests"
                value={requestsJson}
                onChange={(e) => setRequestsJson(e.target.value)}
                placeholder={REQUESTS_PLACEHOLDER}
                rows={8}
                className="w-full resize-y rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 font-mono text-[12px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]"
              />
            </div>
          ) : (
            <div className="space-y-1.5">
              <label
                htmlFor="batch-file-id"
                className="text-[12px] font-semibold text-[var(--text-secondary)]"
              >
                Input file ID
              </label>
              <input
                id="batch-file-id"
                type="text"
                value={inputFileId}
                onChange={(e) => setInputFileId(e.target.value)}
                placeholder="file_..."
                className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 font-mono text-[12px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]"
              />
              <p className="m-0 text-[11px] text-[var(--text-tertiary)]">
                The file must contain one JSONL chat-completion request per
                line. Upload it on the Files page first.
              </p>
            </div>
          )}

          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={() => void handleCreate()}
              disabled={creating}
              className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {creating ? "Creating…" : "Create batch"}
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

      {loading ? (
        <SkeletonRow lines={5} />
      ) : filtered.length === 0 ? (
        <div className="space-y-6">
          <EmptyState
            icon={<HugeiconsIcon icon={Briefcase01Icon} size={32} />}
            title="No batches yet"
            caption={
              statusFilter
                ? `No batches with status "${statusFilter}".`
                : "Submit a batch of requests and retrieve the results within 24 hours, or create one from the command line."
            }
            ctaLabel={statusFilter ? undefined : "Create batch"}
            onCtaClick={statusFilter ? undefined : () => setShowCreate(true)}
          />
          {!statusFilter && (
            <div className="mx-auto max-w-xl">
              <CodeTemplateBlock language="bash" code={EMPTY_CURL} />
            </div>
          )}
        </div>
      ) : (
        <div className="overflow-x-auto rounded-xl border border-solid border-[var(--border-subtle)]">
          <table className="w-full border-collapse text-left text-[13px]">
            <thead>
              <tr className="border-b border-solid border-[var(--border-subtle)] text-[11px] uppercase tracking-wide text-[var(--text-tertiary)]">
                <th className="px-3 py-2 font-medium">ID</th>
                <th className="px-3 py-2 font-medium">Status</th>
                <th className="px-3 py-2 font-medium">Endpoint</th>
                <th className="px-3 py-2 font-medium">Progress</th>
                <th className="px-3 py-2 font-medium">Created</th>
                <th className="px-3 py-2 text-right font-medium">Actions</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((batch) => (
                <tr
                  key={batch.id}
                  className="border-b border-solid border-[var(--border-subtle)] last:border-b-0 hover:bg-[var(--surface-hover)]"
                >
                  <td className="px-3 py-2.5">
                    <button
                      type="button"
                      onClick={() => void openDetail(batch.id)}
                      className="border-0 bg-transparent p-0"
                    >
                      <MonoChip>{batch.id}</MonoChip>
                    </button>
                  </td>
                  <td className="px-3 py-2.5">
                    <Badge>{batch.status}</Badge>
                  </td>
                  <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                    {batch.endpoint}
                  </td>
                  <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                    {batch.request_counts?.completed ?? 0}/
                    {batch.request_counts?.total ?? 0} done
                    {(batch.request_counts?.failed ?? 0) > 0 &&
                      `, ${batch.request_counts.failed} failed`}
                  </td>
                  <td className="px-3 py-2.5 text-[var(--text-secondary)]">
                    {formatDate(batch.created_at)}
                  </td>
                  <td className="px-3 py-2.5 text-right">
                    <div className="inline-flex gap-1.5">
                      <button
                        type="button"
                        onClick={() => void handleResults(batch.id)}
                        disabled={busy === `results-${batch.id}`}
                        className={cn(QUIET_BUTTON_CLASS, "px-2.5 py-1 text-[12px]")}
                      >
                        <HugeiconsIcon icon={DocumentCodeIcon} size={12} />
                        {busy === `results-${batch.id}` ? "Loading…" : "Results"}
                      </button>
                      {["validating", "in_progress", "finalizing"].includes(
                        batch.status
                      ) && (
                        <button
                          type="button"
                          onClick={() => void handleCancel(batch.id)}
                          disabled={busy === batch.id}
                          className={cn(QUIET_BUTTON_CLASS, "px-2.5 py-1 text-[12px]")}
                        >
                          <HugeiconsIcon icon={Cancel01Icon} size={12} />
                          Cancel
                        </button>
                      )}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {selectedId && (
        <div
          className="fixed inset-0 z-50 flex justify-end bg-black/40"
          onClick={closeDetail}
        >
          <div
            className="h-full w-full max-w-lg overflow-y-auto border-l border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] p-5"
            onClick={(e) => e.stopPropagation()}
            role="dialog"
            aria-label={`Batch ${selectedId} details`}
          >
            <div className="mb-4 flex items-start justify-between gap-3">
              <div>
                <h2 className="m-0 text-[16px] font-semibold text-[var(--text-primary)]">
                  Batch details
                </h2>
                <p className="m-0 mt-1 font-mono text-[12px] text-[var(--text-tertiary)]">
                  {selectedId}
                </p>
              </div>
              <button
                type="button"
                onClick={closeDetail}
                className={cn(QUIET_BUTTON_CLASS, "px-2.5 py-1 text-[12px]")}
              >
                Close
              </button>
            </div>

            {detailLoading ? (
              <SkeletonRow lines={6} />
            ) : detail ? (
              <div className="space-y-4">
                <dl className="m-0 space-y-2 text-[13px]">
                  {(
                    [
                      ["Status", detail.status],
                      ["Endpoint", detail.endpoint],
                      ["Input file", detail.input_file_id],
                      ["Completion window", detail.completion_window],
                      ["Progress", `${detail.request_counts?.completed ?? 0}/${detail.request_counts?.total ?? 0} completed, ${detail.request_counts?.failed ?? 0} failed`],
                      ["Created", formatDate(detail.created_at)],
                      ["Completed", formatDate(detail.completed_at)],
                      [
                        "Output file",
                        detail.output_file_id ?? "— (available when completed)",
                      ],
                    ] as [string, string][]
                  ).map(([label, value]) => (
                    <div key={label} className="flex justify-between gap-4">
                      <dt className="text-[var(--text-tertiary)]">{label}</dt>
                      <dd className="m-0 text-right font-mono text-[12px] text-[var(--text-secondary)]">
                        {value}
                      </dd>
                    </div>
                  ))}
                </dl>
                <div className="flex gap-2">
                  <button
                    type="button"
                    onClick={() => void handleResults(detail.id)}
                    disabled={busy === `results-${detail.id}`}
                    className={QUIET_BUTTON_CLASS}
                  >
                    <HugeiconsIcon icon={DocumentCodeIcon} size={13} />
                    {busy === `results-${detail.id}` ? "Loading…" : "Load results"}
                  </button>
                  {["validating", "in_progress", "finalizing"].includes(
                    detail.status
                  ) && (
                    <button
                      type="button"
                      onClick={() => void handleCancel(detail.id)}
                      disabled={busy === detail.id}
                      className={QUIET_BUTTON_CLASS}
                    >
                      <HugeiconsIcon icon={Cancel01Icon} size={13} />
                      Cancel batch
                    </button>
                  )}
                </div>
              </div>
            ) : null}

            {results !== null && (
              <div className="mt-5">
                <CodeTemplateBlock language="json" code={results} />
              </div>
            )}
          </div>
        </div>
      )}
    </ListPage>
  );
}

export default BatchesPage;
