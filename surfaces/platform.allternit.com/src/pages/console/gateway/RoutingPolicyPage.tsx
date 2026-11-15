import React, { useCallback, useEffect, useMemo, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  CheckIcon,
  Route01Icon,
  SparklesIcon,
} from "@hugeicons/core-free-icons";
import { formatApiError, AllternitApiError } from "@/lib/api-client";
import {
  type DataCollection,
  type ModelRoutingOverride,
  type PolicySource,
  type ProviderRoutingPolicy,
  type ProviderRoutingSort,
  type ResolveRoutingAnswer,
  exportProviderRoutingHermes,
  getProviderRouting,
  putProviderRouting,
  resolveProviderRouting,
} from "@/lib/gateway-routing-policy";
import { usePlatformAuth } from "@/lib/platform-auth-client";
import { hasOrganizationAdminAccess } from "@/components/settings/OrganizationAccessPanel";
import {
  ListPage,
  EmptyState,
  SkeletonRow,
  Badge,
  MonoChip,
  QUIET_BUTTON_CLASS,
} from "@/components/console-ui";
import { cn } from "@/lib/utils";

type TriState = "" | "true" | "false";
type SortChoice = "" | ProviderRoutingSort;
type DataCollectionChoice = "" | DataCollection;

interface ModelDraft {
  model: string;
  sort: SortChoice;
  only: string;
  ignore: string;
  order: string;
  require_parameters: TriState;
  data_collection: DataCollectionChoice;
}

interface PolicyDraft {
  sort: SortChoice;
  only: string;
  ignore: string;
  order: string;
  require_parameters: TriState;
  data_collection: DataCollectionChoice;
  models: ModelDraft[];
}

const EMPTY_DRAFT: PolicyDraft = {
  sort: "",
  only: "",
  ignore: "",
  order: "",
  require_parameters: "",
  data_collection: "",
  models: [],
};

const INPUT_CLASS =
  "w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]";
const SELECT_CLASS =
  "w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none focus:border-[var(--accent-primary)]";
const LABEL_CLASS = "mb-1 block text-[12px] font-semibold text-[var(--text-secondary)]";
const TH_CLASS =
  "text-left text-[11px] font-semibold uppercase tracking-wide text-[var(--text-tertiary)] px-3 py-2";

const SOURCE_TONE: Record<PolicySource, string> = {
  tenant: "text-[var(--status-success)] bg-[var(--status-success)]/10",
  global: "text-[var(--status-warning)] bg-[var(--status-warning)]/10",
  none: "text-[var(--text-tertiary)] bg-[var(--text-tertiary)]/10",
};

function parseList(raw: string): string[] {
  return raw
    .split(",")
    .map((part) => part.trim())
    .filter(Boolean);
}

function joinList(values: string[] | undefined): string {
  return (values ?? []).join(", ");
}

function draftFromPolicy(policy: ProviderRoutingPolicy | null): PolicyDraft {
  if (!policy) return { ...EMPTY_DRAFT, models: [] };
  return {
    sort: policy.sort ?? "",
    only: joinList(policy.only),
    ignore: joinList(policy.ignore),
    order: joinList(policy.order),
    require_parameters:
      policy.require_parameters === undefined
        ? ""
        : policy.require_parameters
          ? "true"
          : "false",
    data_collection: policy.data_collection ?? "",
    models: Object.entries(policy.models ?? {}).map(([model, override]) => ({
      model,
      sort: override.sort ?? "",
      only: joinList(override.only),
      ignore: joinList(override.ignore),
      order: joinList(override.order),
      require_parameters:
        override.require_parameters === undefined
          ? ""
          : override.require_parameters
            ? "true"
            : "false",
      data_collection: override.data_collection ?? "",
    })),
  };
}

function policyFromDraft(draft: PolicyDraft): ProviderRoutingPolicy {
  const models: Record<string, ModelRoutingOverride> = {};
  for (const row of draft.models) {
    const model = row.model.trim();
    if (!model) continue;
    const override: ModelRoutingOverride = {
      only: parseList(row.only),
      ignore: parseList(row.ignore),
      order: parseList(row.order),
    };
    if (row.sort) override.sort = row.sort;
    if (row.require_parameters) override.require_parameters = row.require_parameters === "true";
    if (row.data_collection) override.data_collection = row.data_collection;
    models[model] = override;
  }
  const policy: ProviderRoutingPolicy = {
    only: parseList(draft.only),
    ignore: parseList(draft.ignore),
    order: parseList(draft.order),
    models,
  };
  if (draft.sort) policy.sort = draft.sort;
  if (draft.require_parameters)
    policy.require_parameters = draft.require_parameters === "true";
  if (draft.data_collection) policy.data_collection = draft.data_collection;
  return policy;
}

function formatDateTime(iso?: string | null): string {
  if (!iso) return "—";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

/**
 * Provider routing policy — which backend provider serves a model when the
 * platform proxies to an OpenAI-compatible aggregator. Owner/admin gated by
 * the admin API (403 for non-admin org members); organization-less callers
 * manage their own tenant-scoped row.
 */
export function RoutingPolicyPage(): React.ReactNode {
  const auth = usePlatformAuth();
  const isOrgAdmin = hasOrganizationAdminAccess(auth.orgRole);
  const canEdit = !auth.orgId || isOrgAdmin;

  const [draft, setDraft] = useState<PolicyDraft>(EMPTY_DRAFT);
  const [updatedAt, setUpdatedAt] = useState<string | null>(null);
  const [hasStoredPolicy, setHasStoredPolicy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [forbidden, setForbidden] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [flash, setFlash] = useState<string | null>(null);

  const [resolveModel, setResolveModel] = useState("");
  const [resolveProvider, setResolveProvider] = useState("");
  const [resolving, setResolving] = useState(false);
  const [resolveAnswer, setResolveAnswer] = useState<ResolveRoutingAnswer | null>(null);
  const [resolveError, setResolveError] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const state = await getProviderRouting();
      setDraft(draftFromPolicy(state.policy));
      setUpdatedAt(state.updated_at);
      setHasStoredPolicy(state.policy !== null);
      setForbidden(false);
    } catch (err) {
      if (err instanceof AllternitApiError && err.statusCode === 403) {
        setForbidden(true);
      } else {
        setError(formatApiError(err, "Unable to load the provider routing policy."));
      }
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const showFlash = useCallback((message: string) => {
    setFlash(message);
    window.setTimeout(() => setFlash(null), 3000);
  }, []);

  const handleSave = useCallback(async () => {
    setSaving(true);
    setError(null);
    try {
      const state = await putProviderRouting(policyFromDraft(draft));
      setDraft(draftFromPolicy(state.policy));
      setUpdatedAt(state.updated_at);
      setHasStoredPolicy(state.policy !== null);
      showFlash("Provider routing policy saved.");
    } catch (err) {
      setError(formatApiError(err, "Unable to save the provider routing policy."));
    } finally {
      setSaving(false);
    }
  }, [draft, showFlash]);

  const handleResolve = useCallback(async () => {
    const model = resolveModel.trim();
    if (!model) {
      setResolveError("Enter a model name to preview resolution.");
      return;
    }
    setResolving(true);
    setResolveError(null);
    try {
      setResolveAnswer(
        await resolveProviderRouting({ model, provider: resolveProvider || undefined })
      );
    } catch (err) {
      setResolveAnswer(null);
      setResolveError(formatApiError(err, "Unable to resolve this model."));
    } finally {
      setResolving(false);
    }
  }, [resolveModel, resolveProvider]);

  const handleExport = useCallback(async () => {
    setExporting(true);
    setError(null);
    try {
      const yaml = await exportProviderRoutingHermes();
      const blob = new Blob([yaml], { type: "text/yaml;charset=utf-8" });
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = "provider-routing.hermes.yaml";
      document.body.appendChild(anchor);
      anchor.click();
      document.body.removeChild(anchor);
      URL.revokeObjectURL(url);
    } catch (err) {
      setError(formatApiError(err, "Unable to export the Hermes YAML."));
    } finally {
      setExporting(false);
    }
  }, []);

  const resolvedEntries = useMemo(() => {
    if (!resolveAnswer?.provider) return [];
    const provider = resolveAnswer.provider;
    const order: Array<[string, unknown]> = [
      ["sort", provider.sort],
      ["only", provider.only],
      ["ignore", provider.ignore],
      ["order", provider.order],
      ["require_parameters", provider.require_parameters],
      ["data_collection", provider.data_collection],
    ];
    return order.filter(([, value]) => value !== undefined);
  }, [resolveAnswer]);

  const updateModelRow = useCallback(
    (index: number, patch: Partial<ModelDraft>) => {
      setDraft((prev) => ({
        ...prev,
        models: prev.models.map((row, i) => (i === index ? { ...row, ...patch } : row)),
      }));
    },
    []
  );

  return (
    <ListPage
      title="Routing policy"
      subtitle="Control which backend provider serves each model — sort, allow/deny lists, priority order, and per-model pins. Preview resolution before you save."
      primaryAction={
        canEdit
          ? {
              label: exporting ? "Exporting…" : "Export Hermes YAML",
              onClick: () => void handleExport(),
            }
          : undefined
      }
    >
      {error && (
        <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      )}
      {flash && (
        <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-success)]/30 bg-[var(--status-success)]/10 px-3 py-2 text-[13px] text-[var(--status-success)]">
          <HugeiconsIcon icon={CheckIcon} size={14} />
          {flash}
        </p>
      )}

      {loading ? (
        <SkeletonRow lines={6} />
      ) : forbidden ? (
        <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
          <p className="m-0 text-[13px] text-[var(--text-secondary)]">
            Provider routing is managed by organization owners and admins. Ask an organization
            admin to review or change the policy.
          </p>
        </div>
      ) : (
        <>
          {/* Preview resolution — the reason this page exists. */}
          <section className="mb-8 rounded-xl border border-solid border-[var(--accent-primary)]/40 bg-[var(--accent-primary)]/5 p-4">
            <div className="flex items-center gap-2">
              <HugeiconsIcon icon={SparklesIcon} size={16} className="text-[var(--accent-primary)]" />
              <h2 className="m-0 text-[15px] font-semibold text-[var(--text-primary)]">
                Preview resolution
              </h2>
            </div>
            <p className="m-0 mb-3 mt-1 text-[12px] text-[var(--text-tertiary)]">
              See which provider chain a model resolves to under the current policy — before you
              save any change.
            </p>
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-[1fr_200px_auto]">
              <label className="block">
                <span className={LABEL_CLASS}>Model name</span>
                <input
                  value={resolveModel}
                  onChange={(event) => setResolveModel(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") void handleResolve();
                  }}
                  placeholder="claude-sonnet-5.5"
                  className={cn(INPUT_CLASS, "font-mono")}
                />
              </label>
              <label className="block">
                <span className={LABEL_CLASS}>Provider (optional)</span>
                <input
                  value={resolveProvider}
                  onChange={(event) => setResolveProvider(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") void handleResolve();
                  }}
                  placeholder="anthropic"
                  className={cn(INPUT_CLASS, "font-mono")}
                />
              </label>
              <div className="flex items-end">
                <button
                  type="button"
                  disabled={resolving || !resolveModel.trim()}
                  onClick={() => void handleResolve()}
                  className="inline-flex w-full items-center justify-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed sm:w-auto"
                >
                  {resolving ? "Resolving…" : "Resolve"}
                </button>
              </div>
            </div>

            {resolveError && (
              <p className="mb-0 mt-3 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
                <HugeiconsIcon icon={AlertCircleIcon} size={14} />
                {resolveError}
              </p>
            )}

            {resolveAnswer && (
              <div className="mt-3 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-3">
                <div className="flex flex-wrap items-center gap-2">
                  <Badge className={SOURCE_TONE[resolveAnswer.source]}>
                    policy: {resolveAnswer.source}
                  </Badge>
                  {resolveAnswer.matched_override && (
                    <span className="flex items-center gap-1.5 text-[12px] text-[var(--text-secondary)]">
                      pinned by <MonoChip>{resolveAnswer.matched_override}</MonoChip>
                    </span>
                  )}
                </div>
                {resolvedEntries.length === 0 ? (
                  <p className="m-0 mt-2 text-[13px] text-[var(--text-secondary)]">
                    No provider routing applies — requests for this model go out with no provider
                    pin.
                  </p>
                ) : (
                  <div className="mt-2 space-y-1.5">
                    {resolvedEntries.map(([key, value]) => (
                      <div key={key} className="flex flex-wrap items-baseline gap-2">
                        <span className="w-44 shrink-0 font-mono text-[12px] text-[var(--text-tertiary)]">
                          {key}
                        </span>
                        <MonoChip>
                          {Array.isArray(value) ? value.join(", ") : String(value)}
                        </MonoChip>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            )}
          </section>

          <section className="mb-8">
            <div className="flex flex-wrap items-baseline justify-between gap-2">
              <h2 className="m-0 mb-1 text-[15px] font-semibold text-[var(--text-primary)]">
                Default policy
              </h2>
              <span className="text-[12px] text-[var(--text-tertiary)]">
                {hasStoredPolicy
                  ? `Last updated ${formatDateTime(updatedAt)}`
                  : "No tenant policy stored yet — the platform-global row may still apply."}
              </span>
            </div>
            <p className="m-0 mb-3 text-[12px] text-[var(--text-tertiary)]">
              Applies to every model without a per-model pin. Leave a field unset to fall through
              to the provider default.
            </p>

            <div className="max-w-3xl rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
              <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
                <label className="block">
                  <span className={LABEL_CLASS}>Sort</span>
                  <select
                    value={draft.sort}
                    disabled={!canEdit}
                    onChange={(event) =>
                      setDraft((prev) => ({ ...prev, sort: event.target.value as SortChoice }))
                    }
                    className={SELECT_CLASS}
                  >
                    <option value="">Unset</option>
                    <option value="price">price</option>
                    <option value="throughput">throughput</option>
                    <option value="latency">latency</option>
                  </select>
                </label>
                <label className="block">
                  <span className={LABEL_CLASS}>Require parameters</span>
                  <select
                    value={draft.require_parameters}
                    disabled={!canEdit}
                    onChange={(event) =>
                      setDraft((prev) => ({
                        ...prev,
                        require_parameters: event.target.value as TriState,
                      }))
                    }
                    className={SELECT_CLASS}
                  >
                    <option value="">Unset</option>
                    <option value="true">true</option>
                    <option value="false">false</option>
                  </select>
                </label>
                <label className="block">
                  <span className={LABEL_CLASS}>Data collection</span>
                  <select
                    value={draft.data_collection}
                    disabled={!canEdit}
                    onChange={(event) =>
                      setDraft((prev) => ({
                        ...prev,
                        data_collection: event.target.value as DataCollectionChoice,
                      }))
                    }
                    className={SELECT_CLASS}
                  >
                    <option value="">Unset</option>
                    <option value="allow">allow</option>
                    <option value="deny">deny</option>
                  </select>
                </label>
                <label className="block sm:col-span-2 lg:col-span-1">
                  <span className={LABEL_CLASS}>Only (comma-separated slugs)</span>
                  <input
                    value={draft.only}
                    disabled={!canEdit}
                    onChange={(event) =>
                      setDraft((prev) => ({ ...prev, only: event.target.value }))
                    }
                    placeholder="anthropic, google"
                    className={cn(INPUT_CLASS, "font-mono")}
                  />
                </label>
                <label className="block">
                  <span className={LABEL_CLASS}>Ignore (comma-separated slugs)</span>
                  <input
                    value={draft.ignore}
                    disabled={!canEdit}
                    onChange={(event) =>
                      setDraft((prev) => ({ ...prev, ignore: event.target.value }))
                    }
                    placeholder="together"
                    className={cn(INPUT_CLASS, "font-mono")}
                  />
                </label>
                <label className="block">
                  <span className={LABEL_CLASS}>Order (priority, first wins)</span>
                  <input
                    value={draft.order}
                    disabled={!canEdit}
                    onChange={(event) =>
                      setDraft((prev) => ({ ...prev, order: event.target.value }))
                    }
                    placeholder="anthropic, amazon-bedrock"
                    className={cn(INPUT_CLASS, "font-mono")}
                  />
                </label>
              </div>
              <p className="m-0 mt-3 text-[12px] text-[var(--text-tertiary)]">
                `only` and `ignore` must not overlap; `order` providers cannot also be ignored.
              </p>
            </div>
          </section>

          <section className="mb-8">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <h2 className="m-0 mb-1 text-[15px] font-semibold text-[var(--text-primary)]">
                Per-model overrides
              </h2>
              {canEdit && (
                <button
                  type="button"
                  className={QUIET_BUTTON_CLASS}
                  onClick={() =>
                    setDraft((prev) => ({
                      ...prev,
                      models: [
                        ...prev.models,
                        {
                          model: "",
                          sort: "",
                          only: "",
                          ignore: "",
                          order: "",
                          require_parameters: "",
                          data_collection: "",
                        },
                      ],
                    }))
                  }
                >
                  Add model pin
                </button>
              )}
            </div>
            <p className="m-0 mb-3 text-[12px] text-[var(--text-tertiary)]">
              A pin replaces any flat value it sets; unset fields fall through to the defaults
              above. Matching is spelling-tolerant — keys may include or omit the provider
              prefix.
            </p>

            {draft.models.length === 0 ? (
              <div className="rounded-xl border border-dashed border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-6">
                <EmptyState
                  icon={<HugeiconsIcon icon={Route01Icon} size={32} />}
                  title="No per-model pins"
                  caption="Models without a pin resolve purely from the default policy above."
                />
              </div>
            ) : (
              <div className="overflow-x-auto rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]">
                <table className="w-full border-collapse">
                  <thead>
                    <tr className="border-b border-[var(--border-subtle)]">
                      <th className={TH_CLASS}>Model</th>
                      <th className={TH_CLASS}>Sort</th>
                      <th className={TH_CLASS}>Only</th>
                      <th className={TH_CLASS}>Ignore</th>
                      <th className={TH_CLASS}>Order</th>
                      <th className={TH_CLASS}>Req. params</th>
                      <th className={TH_CLASS}>Data</th>
                      {canEdit && <th className={TH_CLASS} />}
                    </tr>
                  </thead>
                  <tbody>
                    {draft.models.map((row, index) => (
                      <tr
                        key={index}
                        className="border-b border-[var(--border-subtle)] last:border-b-0"
                      >
                        <td className="min-w-[200px] px-3 py-2">
                          <input
                            value={row.model}
                            disabled={!canEdit}
                            onChange={(event) =>
                              updateModelRow(index, { model: event.target.value })
                            }
                            placeholder="claude-sonnet-5.5"
                            className={cn(INPUT_CLASS, "font-mono text-[12px]")}
                          />
                        </td>
                        <td className="min-w-[130px] px-3 py-2">
                          <select
                            value={row.sort}
                            disabled={!canEdit}
                            onChange={(event) =>
                              updateModelRow(index, {
                                sort: event.target.value as SortChoice,
                              })
                            }
                            className={SELECT_CLASS}
                          >
                            <option value="">Inherit</option>
                            <option value="price">price</option>
                            <option value="throughput">throughput</option>
                            <option value="latency">latency</option>
                          </select>
                        </td>
                        <td className="min-w-[150px] px-3 py-2">
                          <input
                            value={row.only}
                            disabled={!canEdit}
                            onChange={(event) =>
                              updateModelRow(index, { only: event.target.value })
                            }
                            placeholder="anthropic"
                            className={cn(INPUT_CLASS, "font-mono text-[12px]")}
                          />
                        </td>
                        <td className="min-w-[150px] px-3 py-2">
                          <input
                            value={row.ignore}
                            disabled={!canEdit}
                            onChange={(event) =>
                              updateModelRow(index, { ignore: event.target.value })
                            }
                            placeholder="together"
                            className={cn(INPUT_CLASS, "font-mono text-[12px]")}
                          />
                        </td>
                        <td className="min-w-[170px] px-3 py-2">
                          <input
                            value={row.order}
                            disabled={!canEdit}
                            onChange={(event) =>
                              updateModelRow(index, { order: event.target.value })
                            }
                            placeholder="anthropic, together"
                            className={cn(INPUT_CLASS, "font-mono text-[12px]")}
                          />
                        </td>
                        <td className="min-w-[130px] px-3 py-2">
                          <select
                            value={row.require_parameters}
                            disabled={!canEdit}
                            onChange={(event) =>
                              updateModelRow(index, {
                                require_parameters: event.target.value as TriState,
                              })
                            }
                            className={SELECT_CLASS}
                          >
                            <option value="">Inherit</option>
                            <option value="true">true</option>
                            <option value="false">false</option>
                          </select>
                        </td>
                        <td className="min-w-[120px] px-3 py-2">
                          <select
                            value={row.data_collection}
                            disabled={!canEdit}
                            onChange={(event) =>
                              updateModelRow(index, {
                                data_collection: event.target.value as DataCollectionChoice,
                              })
                            }
                            className={SELECT_CLASS}
                          >
                            <option value="">Inherit</option>
                            <option value="allow">allow</option>
                            <option value="deny">deny</option>
                          </select>
                        </td>
                        {canEdit && (
                          <td className="px-3 py-2 text-right">
                            <button
                              type="button"
                              className={QUIET_BUTTON_CLASS}
                              onClick={() =>
                                setDraft((prev) => ({
                                  ...prev,
                                  models: prev.models.filter((_, i) => i !== index),
                                }))
                              }
                            >
                              Remove
                            </button>
                          </td>
                        )}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </section>

          {canEdit ? (
            <div className="flex items-center justify-end gap-2">
              <button
                type="button"
                disabled={saving}
                onClick={() => void handleSave()}
                className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {saving ? "Saving…" : "Save policy"}
              </button>
            </div>
          ) : (
            <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
              You are viewing this policy as an organization member. Owners and admins can edit
              and save it.
            </p>
          )}
        </>
      )}
    </ListPage>
  );
}

export default RoutingPolicyPage;
