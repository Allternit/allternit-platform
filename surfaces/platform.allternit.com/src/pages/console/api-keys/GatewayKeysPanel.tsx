import React, { useCallback, useEffect, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  CheckIcon,
  CopyIcon,
} from "@hugeicons/core-free-icons";
import { formatApiError } from "@/lib/api-client";
import {
  type GatewayKey,
  type CreatedGatewayKey,
  listGatewayKeys,
  createGatewayKey,
  updateGatewayKey,
  revokeGatewayKey,
} from "@/lib/gateway-keys";
import {
  EmptyState,
  SkeletonRow,
  Badge,
  MonoChip,
  QUIET_BUTTON_CLASS,
  DESTRUCTIVE_BUTTON_CLASS,
} from "@/components/console-ui";
import { cn } from "@/lib/utils";

const TH_CLASS =
  "text-left text-[11px] font-semibold uppercase tracking-wide text-[var(--text-tertiary)] px-3 py-2";
const TD_CLASS = "px-3 py-2.5 text-[13px] text-[var(--text-primary)]";

function formatDate(iso?: string | null): string {
  if (!iso) return "—";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

function formatBudget(cents: number | null): string {
  if (cents === null) return "No budget";
  return (cents / 100).toLocaleString(undefined, { style: "currency", currency: "USD" }) + "/mo";
}

interface EditingDraft {
  name: string;
  budget: string;
  rpm: string;
}

/**
 * Gateway keys — `ak-…` virtual keys for the /v1/* LLM gateway. This is the
 * second tab on the API keys page; the scoped-keys tab remains the default.
 */
export function GatewayKeysPanel(): React.ReactNode {
  const [keys, setKeys] = useState<GatewayKey[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [showCreate, setShowCreate] = useState(false);
  const [formName, setFormName] = useState("");
  const [formBudget, setFormBudget] = useState("");
  const [formRpm, setFormRpm] = useState("");
  const [saving, setSaving] = useState(false);
  const [createdKey, setCreatedKey] = useState<CreatedGatewayKey | null>(null);

  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState<EditingDraft>({ name: "", budget: "", rpm: "" });

  const [confirmRevokeId, setConfirmRevokeId] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setKeys(await listGatewayKeys());
    } catch (err) {
      setError(formatApiError(err, "Unable to load gateway keys."));
      setKeys([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const copyText = useCallback(async (text: string, id: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedId(id);
      window.setTimeout(() => setCopiedId(null), 2000);
    } catch {
      // leave selectable
    }
  }, []);

  const parseOptionalInt = (raw: string): number | null | "invalid" => {
    const trimmed = raw.trim();
    if (trimmed === "") return null;
    const parsed = Number(trimmed);
    if (!Number.isInteger(parsed)) return "invalid";
    return parsed;
  };

  const handleCreate = useCallback(async () => {
    const budget = parseOptionalInt(formBudget);
    const rpm = parseOptionalInt(formRpm);
    if (budget === "invalid" || rpm === "invalid") {
      setError("Budget and rate limit must be whole numbers.");
      return;
    }
    if (budget !== null && budget < 0) {
      setError("Monthly budget must be 0 or greater (USD cents).");
      return;
    }
    if (rpm !== null && rpm < 1) {
      setError("Rate limit must be at least 1 request per minute.");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const created = await createGatewayKey({
        name: formName,
        ...(budget !== null ? { monthly_budget_cents: budget } : {}),
        ...(rpm !== null ? { rate_limit_rpm: rpm } : {}),
      });
      setCreatedKey(created);
      setShowCreate(false);
      setFormName("");
      setFormBudget("");
      setFormRpm("");
      await load();
    } catch (err) {
      setError(formatApiError(err, "Unable to create the gateway key."));
    } finally {
      setSaving(false);
    }
  }, [formName, formBudget, formRpm, load]);

  const startEdit = useCallback((key: GatewayKey) => {
    setEditingId(key.id);
    setDraft({
      name: key.name ?? "",
      budget: key.monthly_budget_cents !== null ? String(key.monthly_budget_cents) : "",
      rpm: key.rate_limit_rpm !== null ? String(key.rate_limit_rpm) : "",
    });
  }, []);

  const handleSaveEdit = useCallback(
    async (key: GatewayKey) => {
      const budget = parseOptionalInt(draft.budget);
      const rpm = parseOptionalInt(draft.rpm);
      if (budget === "invalid" || rpm === "invalid") {
        setError("Budget and rate limit must be whole numbers.");
        return;
      }
      if (budget !== null && budget < 0) {
        setError("Monthly budget must be 0 or greater (USD cents).");
        return;
      }
      if (rpm !== null && rpm < 1) {
        setError("Rate limit must be at least 1 request per minute.");
        return;
      }
      setBusyId(key.id);
      setError(null);
      try {
        const body: { name?: string | null; monthly_budget_cents?: number | null; rate_limit_rpm?: number | null } = {};
        const nextName = draft.name.trim() || null;
        if (nextName !== (key.name ?? null)) body.name = nextName;
        if (budget !== (key.monthly_budget_cents ?? null)) body.monthly_budget_cents = budget;
        if (rpm !== (key.rate_limit_rpm ?? null)) body.rate_limit_rpm = rpm;
        if (Object.keys(body).length > 0) {
          await updateGatewayKey(key.id, body);
        }
        setEditingId(null);
        await load();
      } catch (err) {
        setError(formatApiError(err, "Unable to update the gateway key."));
      } finally {
        setBusyId(null);
      }
    },
    [draft, load]
  );

  const handleRevoke = useCallback(
    async (key: GatewayKey) => {
      setBusyId(key.id);
      setError(null);
      try {
        await revokeGatewayKey(key.id);
        setConfirmRevokeId(null);
        await load();
      } catch (err) {
        setError(formatApiError(err, "Unable to revoke the key."));
      } finally {
        setBusyId(null);
      }
    },
    [load]
  );

  return (
    <div className="space-y-4">
      {error && (
        <p className="flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      )}

      {createdKey && (
        <div className="rounded-xl border border-solid border-[var(--status-success)]/25 bg-[var(--status-success)]/[0.06] p-4">
          <div className="mb-3 flex items-start gap-2">
            <HugeiconsIcon icon={CheckIcon} size={16} className="mt-0.5 shrink-0 text-[var(--status-success)]" />
            <div className="min-w-0 flex-1">
              <div className="text-[13px] font-semibold text-[var(--text-primary)]">
                Gateway key created — copy it now
              </div>
              <p className="m-0 mt-0.5 text-[12px] text-[var(--text-secondary)]">
                {createdKey.warning}
              </p>
            </div>
          </div>
          <div className="flex items-center gap-2 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2">
            <code className="flex-1 truncate font-mono text-[12px] text-[var(--text-primary)]">
              {createdKey.key}
            </code>
            <button
              type="button"
              aria-label="Copy gateway key"
              className="p-1 text-[var(--text-tertiary)] transition-colors hover:text-[var(--text-primary)]"
              onClick={() => void copyText(createdKey.key, "created")}
            >
              <HugeiconsIcon
                icon={copiedId === "created" ? CheckIcon : CopyIcon}
                size={13}
                className={copiedId === "created" ? "text-[var(--status-success)]" : undefined}
              />
            </button>
          </div>
          <div className="mt-3 flex justify-end">
            <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setCreatedKey(null)}>
              Done
            </button>
          </div>
        </div>
      )}

      <div className="flex items-center justify-end">
        <button
          type="button"
          className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110"
          onClick={() => {
            setShowCreate((prev) => !prev);
            setCreatedKey(null);
          }}
        >
          {showCreate ? "Close" : "Create gateway key"}
        </button>
      </div>

      {showCreate && (
        <div className="max-w-2xl rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
            <label className="block sm:col-span-3">
              <span className="mb-1 block text-[12px] font-semibold text-[var(--text-secondary)]">
                Name
              </span>
              <input
                value={formName}
                onChange={(event) => setFormName(event.target.value)}
                placeholder="Production inference"
                maxLength={128}
                className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]"
              />
            </label>
            <label className="block">
              <span className="mb-1 block text-[12px] font-semibold text-[var(--text-secondary)]">
                Monthly budget (USD cents)
              </span>
              <input
                type="number"
                min={0}
                value={formBudget}
                onChange={(event) => setFormBudget(event.target.value)}
                placeholder="No budget"
                className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 font-mono text-[12px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]"
              />
            </label>
            <label className="block">
              <span className="mb-1 block text-[12px] font-semibold text-[var(--text-secondary)]">
                Rate limit (rpm)
              </span>
              <input
                type="number"
                min={1}
                value={formRpm}
                onChange={(event) => setFormRpm(event.target.value)}
                placeholder="No limit"
                className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 font-mono text-[12px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]"
              />
            </label>
            <div className="flex items-end justify-end gap-2 pb-0.5">
              <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setShowCreate(false)}>
                Cancel
              </button>
              <button
                type="button"
                disabled={saving}
                onClick={() => void handleCreate()}
                className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {saving ? "Creating…" : "Create key"}
              </button>
            </div>
          </div>
          <p className="m-0 mt-2 text-[11px] text-[var(--text-tertiary)]">
            Gateway keys authenticate against the LLM gateway (/v1/* routes) as Bearer
            tokens. Budget is in USD cents per calendar month; leave blank for no cap.
          </p>
        </div>
      )}

      {loading && keys.length === 0 ? (
        <SkeletonRow lines={4} />
      ) : keys.length === 0 ? (
        <EmptyState
          art="key"
          title="No gateway keys yet"
          caption="Gateway keys are ak-… tokens your applications use to call models through the Allternit LLM gateway."
          ctaLabel="Create gateway key"
          onCtaClick={() => {
            setShowCreate(true);
            setCreatedKey(null);
          }}
        />
      ) : (
        <div className="overflow-hidden rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40">
          <table className="w-full">
            <thead>
              <tr className="border-b border-solid border-[var(--border-subtle)]">
                <th className={TH_CLASS}>Name</th>
                <th className={TH_CLASS}>Prefix</th>
                <th className={cn(TH_CLASS, "hidden lg:table-cell")}>Budget</th>
                <th className={cn(TH_CLASS, "hidden lg:table-cell")}>Rate limit</th>
                <th className={cn(TH_CLASS, "hidden md:table-cell")}>Last used</th>
                <th className={TH_CLASS}>Status</th>
                <th className={cn(TH_CLASS, "text-right")}>Actions</th>
              </tr>
            </thead>
            <tbody>
              {keys.map((key) =>
                editingId === key.id ? (
                  <tr key={key.id} className="border-b border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/60">
                    <td className={cn(TD_CLASS, "align-top")} colSpan={3}>
                      <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
                        <input
                          value={draft.name}
                          onChange={(event) => setDraft((prev) => ({ ...prev, name: event.target.value }))}
                          placeholder="Name"
                          maxLength={128}
                          className="rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none focus:border-[var(--border-default)]"
                        />
                        <input
                          type="number"
                          min={0}
                          value={draft.budget}
                          onChange={(event) => setDraft((prev) => ({ ...prev, budget: event.target.value }))}
                          placeholder="Budget (cents, blank = none)"
                          className="rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 font-mono text-[12px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]"
                        />
                        <input
                          type="number"
                          min={1}
                          value={draft.rpm}
                          onChange={(event) => setDraft((prev) => ({ ...prev, rpm: event.target.value }))}
                          placeholder="RPM (blank = none)"
                          className="rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 font-mono text-[12px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]"
                        />
                      </div>
                    </td>
                    <td className={cn(TD_CLASS, "hidden lg:table-cell")} />
                    <td className={cn(TD_CLASS, "hidden md:table-cell")} />
                    <td className={TD_CLASS} />
                    <td className={cn(TD_CLASS, "align-top text-right")}>
                      <span className="inline-flex gap-2">
                        <button
                          type="button"
                          className={QUIET_BUTTON_CLASS}
                          disabled={busyId === key.id}
                          onClick={() => setEditingId(null)}
                        >
                          Cancel
                        </button>
                        <button
                          type="button"
                          disabled={busyId === key.id}
                          onClick={() => void handleSaveEdit(key)}
                          className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3 py-1.5 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50"
                        >
                          Save
                        </button>
                      </span>
                    </td>
                  </tr>
                ) : (
                  <tr key={key.id} className="border-b border-solid border-[var(--border-subtle)] last:border-b-0">
                    <td className={cn(TD_CLASS, "font-medium")}>{key.name ?? "—"}</td>
                    <td className={TD_CLASS}>
                      <button
                        type="button"
                        className="p-0"
                        aria-label={`Copy key prefix for ${key.name ?? key.id}`}
                        onClick={() => key.key_prefix && void copyText(key.key_prefix, key.id)}
                      >
                        <MonoChip>
                          {key.key_prefix ?? key.id}
                          <HugeiconsIcon
                            icon={copiedId === key.id ? CheckIcon : CopyIcon}
                            size={11}
                            className={cn("ml-1.5", copiedId === key.id && "text-[var(--status-success)]")}
                          />
                        </MonoChip>
                      </button>
                    </td>
                    <td className={cn(TD_CLASS, "hidden text-[var(--text-secondary)] lg:table-cell")}>
                      {formatBudget(key.monthly_budget_cents)}
                    </td>
                    <td className={cn(TD_CLASS, "hidden text-[var(--text-secondary)] lg:table-cell")}>
                      {key.rate_limit_rpm !== null ? `${key.rate_limit_rpm} rpm` : "No limit"}
                    </td>
                    <td className={cn(TD_CLASS, "hidden text-[var(--text-secondary)] md:table-cell")}>
                      {formatDate(key.last_used_at)}
                    </td>
                    <td className={TD_CLASS}>
                      {key.revoked ? (
                        <Badge className="text-[var(--status-error)] bg-[var(--status-error)]/10">revoked</Badge>
                      ) : (
                        <Badge className="text-[var(--status-success)] bg-[var(--status-success)]/10">active</Badge>
                      )}
                    </td>
                    <td className={cn(TD_CLASS, "text-right")}>
                      {key.revoked ? null : confirmRevokeId === key.id ? (
                        <span className="inline-flex items-center gap-2">
                          <span className="text-[12px] text-[var(--text-secondary)]">Revoke?</span>
                          <button
                            type="button"
                            className={QUIET_BUTTON_CLASS}
                            disabled={busyId === key.id}
                            onClick={() => setConfirmRevokeId(null)}
                          >
                            Cancel
                          </button>
                          <button
                            type="button"
                            className={DESTRUCTIVE_BUTTON_CLASS}
                            disabled={busyId === key.id}
                            onClick={() => void handleRevoke(key)}
                          >
                            {busyId === key.id ? "Revoking…" : "Revoke"}
                          </button>
                        </span>
                      ) : (
                        <span className="inline-flex items-center gap-1">
                          <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => startEdit(key)}>
                            Edit
                          </button>
                          <button
                            type="button"
                            className={DESTRUCTIVE_BUTTON_CLASS}
                            onClick={() => setConfirmRevokeId(key.id)}
                          >
                            Revoke
                          </button>
                        </span>
                      )}
                    </td>
                  </tr>
                )
              )}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

export default GatewayKeysPanel;
