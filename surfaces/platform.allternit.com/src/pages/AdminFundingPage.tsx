import React, { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { ArrowsClockwise } from "@phosphor-icons/react";
import { usePlatformAuth } from "@/lib/platform-auth-client";
import { formatApiError } from "@/lib/api-client";
import { SkeletonRow } from "@/components/settings/SkeletonRow";
import { QUIET_BUTTON_CLASS } from "@/components/settings/buttonStyles";

// Admin only: prepaid balance at each inference provider vs. what customers
// hold in their wallets. cloud-api returns 403 to non-admins.
// Backed by GET/PUT /api/v1/admin/funding (services/upstream_funding.rs).

interface ProviderFunding {
  provider: string;
  balanceUsd: number | null;
  source: "api" | "manual" | string;
  reserveDays: number;
  floorUsd: number;
  manualBalanceUsd: number | null;
  manualSetAt: string | null;
  burn24hUsd: number;
  lastError: string | null;
  refreshedAt: string | null;
  daysLeft: number | null;
  belowFloor: boolean;
  short: boolean;
  configured: boolean;
}

interface FundingResponse {
  providers: ProviderFunding[];
  upstreamTotalUsd: number;
  walletLiabilityUsd: number;
  coverageRatio: number | null;
  burn24hTotalUsd: number;
}

interface Draft {
  balanceUsd: string;
  reserveDays: string;
  floorUsd: string;
}

function apiBase() {
  return String(import.meta.env.VITE_ALLTERNIT_CLOUD_API_URL || "https://api.allternit.com").replace(/\/$/, "");
}

const money = (value: number | null | undefined) =>
  value == null ? "—" : value.toLocaleString(undefined, { style: "currency", currency: "USD" });

function formatTime(value: string | null) {
  if (!value) return "—";
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? "—"
    : date.toLocaleString(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
}

function daysLabel(row: ProviderFunding) {
  if (row.balanceUsd == null) return "—";
  if (row.daysLeft == null) return "No spend";
  return `${row.daysLeft.toFixed(1)} days`;
}

export function AdminFundingPage() {
  const { getToken } = usePlatformAuth();
  const [data, setData] = useState<FundingResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft>({ balanceUsd: "", reserveDays: "", floorUsd: "" });
  const [saving, setSaving] = useState(false);

  const request = useCallback(
    async (path: string, init?: RequestInit) => {
      const token = await getToken();
      if (!token) throw new Error("Sign in to view funding.");
      const response = await fetch(`${apiBase()}${path}`, {
        ...init,
        headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
      });
      if (response.status === 403) throw new Error("This page is for Allternit admins.");
      const payload = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(payload.message || payload.error || `Request failed (${response.status})`);
      return payload;
    },
    [getToken],
  );

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setData((await request("/api/v1/admin/funding")) as FundingResponse);
    } catch (err) {
      setError(formatApiError(err, "Unable to load funding"));
    } finally {
      setLoading(false);
    }
  }, [request]);

  useEffect(() => {
    void load();
  }, [load]);

  const startEdit = (row: ProviderFunding) => {
    setEditing(row.provider);
    setDraft({
      balanceUsd: "",
      reserveDays: String(row.reserveDays),
      floorUsd: String(row.floorUsd),
    });
  };

  const save = async (provider: string) => {
    setSaving(true);
    setError(null);
    try {
      const body: Record<string, number> = {};
      if (draft.balanceUsd.trim()) body.balanceUsd = Number(draft.balanceUsd);
      if (draft.reserveDays.trim()) body.reserveDays = Math.round(Number(draft.reserveDays));
      if (draft.floorUsd.trim()) body.floorUsd = Number(draft.floorUsd);
      if (Object.values(body).some((value) => !Number.isFinite(value))) throw new Error("Enter numbers only.");
      await request(`/api/v1/admin/funding/${encodeURIComponent(provider)}`, {
        method: "PUT",
        body: JSON.stringify(body),
      });
      setEditing(null);
      await load();
    } catch (err) {
      setError(formatApiError(err, "Unable to save"));
    } finally {
      setSaving(false);
    }
  };

  const inputClass =
    "w-24 rounded-md border border-[var(--border-subtle)] bg-[var(--bg-primary)] px-2 py-1 text-[13px] text-[var(--text-primary)]";

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="text-[28px] font-bold leading-none tracking-tight text-[var(--text-primary)]">Funding</h1>
          <p className="mt-2 text-[13px] text-[var(--text-secondary)]">
            Prepaid balance at each model provider against what customers hold in their wallets. Refreshes every 10
            minutes. <Link to="/admin/customers" className="underline underline-offset-2">Customers</Link>
          </p>
        </div>
        <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => void load()} disabled={loading}>
          <ArrowsClockwise size={14} /> Refresh
        </button>
      </div>

      {data && (
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          {(
            [
              ["At providers", money(data.upstreamTotalUsd)],
              ["Customers hold", money(data.walletLiabilityUsd)],
              ["Coverage", data.coverageRatio == null ? "—" : `${data.coverageRatio.toFixed(2)}×`],
              ["Spent last 24 h", money(data.burn24hTotalUsd)],
            ] as Array<[string, string]>
          ).map(([label, value]) => (
            <div key={label} className="rounded-xl border border-[var(--border-subtle)] px-3 py-2">
              <div className="text-[11px] uppercase tracking-[0.08em] text-[var(--text-tertiary)]">{label}</div>
              <div className="mt-1 text-[18px] font-semibold tabular-nums text-[var(--text-primary)]">{value}</div>
            </div>
          ))}
        </div>
      )}

      {error && <p className="text-[13px] text-[var(--status-error)]">{error}</p>}

      {loading && !data ? (
        <SkeletonRow lines={5} />
      ) : (
        data && (
          <div className="overflow-x-auto rounded-xl border border-[var(--border-subtle)]">
            <table className="w-full min-w-[820px] text-left text-[13px]">
              <thead className="bg-[var(--bg-secondary)] text-[11px] uppercase tracking-[0.08em] text-[var(--text-tertiary)]">
                <tr>
                  <th className="px-3 py-2 font-medium">Provider</th>
                  <th className="px-3 py-2 font-medium">Balance</th>
                  <th className="px-3 py-2 font-medium">Last 24 h</th>
                  <th className="px-3 py-2 font-medium">Days left</th>
                  <th className="px-3 py-2 font-medium">Reserve / floor</th>
                  <th className="px-3 py-2 font-medium">Updated</th>
                  <th className="px-3 py-2 font-medium" />
                </tr>
              </thead>
              <tbody>
                {data.providers.map((row) => (
                  <tr key={row.provider} className="border-t border-[var(--border-subtle)] align-top text-[var(--text-primary)]">
                    <td className="px-3 py-2">
                      <div className="font-medium">{row.provider}</div>
                      <div className="text-[12px] text-[var(--text-secondary)]">
                        {row.source === "api" ? "Read from provider" : "Entered by hand"}
                        {!row.configured && " · no key here"}
                      </div>
                    </td>
                    <td className="px-3 py-2 tabular-nums">
                      {editing === row.provider ? (
                        <input
                          aria-label={`${row.provider} balance now (USD)`}
                          className={inputClass}
                          inputMode="decimal"
                          placeholder={row.balanceUsd == null ? "0.00" : row.balanceUsd.toFixed(2)}
                          value={draft.balanceUsd}
                          onChange={(e) => setDraft({ ...draft, balanceUsd: e.target.value })}
                        />
                      ) : (
                        <span className={row.short ? "text-[var(--status-error)]" : undefined}>{money(row.balanceUsd)}</span>
                      )}
                      {row.belowFloor && editing !== row.provider && (
                        <div className="text-[12px] text-[var(--status-error)]">Below floor, routed around</div>
                      )}
                      {row.lastError && (
                        <div className="text-[12px] text-[var(--status-error)]" title={row.lastError}>
                          Last refresh failed
                        </div>
                      )}
                    </td>
                    <td className="px-3 py-2 tabular-nums">{money(row.burn24hUsd)}</td>
                    <td className="px-3 py-2 tabular-nums">{daysLabel(row)}</td>
                    <td className="px-3 py-2 tabular-nums">
                      {editing === row.provider ? (
                        <div className="flex gap-2">
                          <input
                            aria-label={`${row.provider} reserve days`}
                            className={inputClass}
                            inputMode="numeric"
                            value={draft.reserveDays}
                            onChange={(e) => setDraft({ ...draft, reserveDays: e.target.value })}
                          />
                          <input
                            aria-label={`${row.provider} floor (USD)`}
                            className={inputClass}
                            inputMode="decimal"
                            value={draft.floorUsd}
                            onChange={(e) => setDraft({ ...draft, floorUsd: e.target.value })}
                          />
                        </div>
                      ) : (
                        `${row.reserveDays} days / ${money(row.floorUsd)}`
                      )}
                    </td>
                    <td className="px-3 py-2 text-[var(--text-secondary)]">{formatTime(row.refreshedAt)}</td>
                    <td className="px-3 py-2 text-right">
                      {editing === row.provider ? (
                        <div className="flex justify-end gap-2">
                          <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setEditing(null)} disabled={saving}>
                            Cancel
                          </button>
                          <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => void save(row.provider)} disabled={saving}>
                            {saving ? "Saving…" : "Save"}
                          </button>
                        </div>
                      ) : (
                        <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => startEdit(row)}>
                          Edit
                        </button>
                      )}
                    </td>
                  </tr>
                ))}
                {data.providers.length === 0 && (
                  <tr>
                    <td colSpan={7} className="px-3 py-6 text-center text-[var(--text-secondary)]">
                      No provider balances yet. The first refresh runs when cloud-api starts.
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>
        )
      )}
    </div>
  );
}
