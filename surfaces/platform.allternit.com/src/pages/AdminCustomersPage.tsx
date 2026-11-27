import React, { useCallback, useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { ArrowsClockwise } from "@phosphor-icons/react";
import { usePlatformAuth } from "@/lib/platform-auth-client";
import { formatApiError } from "@/lib/api-client";
import { SkeletonRow } from "@/components/settings/SkeletonRow";
import { QUIET_BUTTON_CLASS } from "@/components/settings/buttonStyles";

// Admin only: everyone who signed up (Clerk), their plan (Stripe), cloud
// computer and the emails we sent them. cloud-api returns 403 to non-admins.

interface CustomerRow {
  userId: string;
  email: string | null;
  name: string | null;
  signedUpAt: string | null;
  planId: string | null;
  planStatus: string | null;
  /** Who bills the plan: stripe, apple or google. Absent on older servers. */
  billingProvider?: string | null;
  stripeCustomerId: string | null;
  computerStatus: string | null;
  emailsSent: string[];
  /** Open Research flag (contract section 6). Absent on older servers. */
  open_research?: OpenResearch | null;
}

interface OpenResearch {
  enabled: boolean;
  contract_ref?: string | null;
  set_at?: string | null;
}

interface CustomersResponse {
  totalSignups: number;
  paying: number;
  customers: CustomerRow[];
}

type Filter = "all" | "paying" | "free";

const EMAIL_LABELS: Record<string, string> = {
  welcome: "Welcome",
  plan_started: "Plan",
  computer_ready: "Computer",
};

function apiBase() {
  return String(import.meta.env.VITE_ALLTERNIT_CLOUD_API_URL || "https://api.allternit.com").replace(/\/$/, "");
}

function OpenResearchCell({
  row,
  onSet,
}: {
  row: CustomerRow;
  onSet: (userId: string, enabled: boolean, contractRef: string, note: string) => Promise<void>;
}) {
  const current = row.open_research;
  const enabled = current?.enabled === true;
  const [open, setOpen] = useState(false);
  const [contractRef, setContractRef] = useState("");
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const ref = contractRef.trim();
  const customer = row.email ?? row.userId;

  async function submit(next: boolean) {
    setBusy(true);
    setErr(null);
    try {
      await onSet(row.userId, next, next ? ref : current?.contract_ref ?? "", note.trim());
      setOpen(false);
      setNote("");
    } catch (e) {
      setErr(formatApiError(e, "Unable to update Open Research"));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="space-y-1">
      <div className="text-[12px]">
        {enabled ? "On" : "Off"}
        {enabled && current?.contract_ref ? ` · ${current.contract_ref}` : ""}
        {enabled && current?.set_at ? ` · ${formatDate(current.set_at)}` : ""}
      </div>
      <button
        type="button"
        className={QUIET_BUTTON_CLASS}
        onClick={() => (enabled ? void submit(false) : setOpen(true))}
        disabled={busy}
        aria-label={`${enabled ? "Turn off" : "Turn on"} Open Research for ${customer}`}
      >
        {enabled ? "Turn off" : "Turn on"}
      </button>
      {err && !open && <p className="text-[12px] text-[var(--status-error)]">{err}</p>}
      {open && (
        <div
          role="dialog"
          aria-modal="true"
          aria-label="Turn on Open Research"
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
          onKeyDown={(e) => {
            if (e.key === "Escape") setOpen(false);
          }}
        >
          <div className="w-full max-w-md space-y-3 rounded-xl border border-[var(--border-subtle)] bg-[var(--bg-primary)] p-5 text-[13px] text-[var(--text-primary)]">
            <h2 className="text-[16px] font-semibold">Turn on Open Research for {customer}?</h2>
            <p className="text-[var(--text-secondary)]">
              This changes what the customer can buy: research models become available to their account. Only turn it on
              once a signed agreement is on file.
            </p>
            <label className="block">
              <span className="text-[12px] text-[var(--text-secondary)]">Contract reference (required)</span>
              <input
                autoFocus
                value={contractRef}
                onChange={(e) => setContractRef(e.target.value)}
                placeholder="OR-2026-001"
                className="mt-1 w-full rounded-md border border-[var(--border-subtle)] bg-transparent px-2 py-1.5 outline-none"
              />
            </label>
            <label className="block">
              <span className="text-[12px] text-[var(--text-secondary)]">Note</span>
              <input
                value={note}
                onChange={(e) => setNote(e.target.value)}
                className="mt-1 w-full rounded-md border border-[var(--border-subtle)] bg-transparent px-2 py-1.5 outline-none"
              />
            </label>
            {err && <p className="text-[var(--status-error)]">{err}</p>}
            <div className="flex justify-end gap-2">
              <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setOpen(false)} disabled={busy}>
                Cancel
              </button>
              <button
                type="button"
                className={QUIET_BUTTON_CLASS}
                onClick={() => void submit(true)}
                disabled={busy || !ref}
              >
                {busy ? "Saving…" : "Turn on"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function isPaying(row: CustomerRow) {
  return row.planStatus === "active" || row.planStatus === "trialing" || row.planStatus === "past_due";
}

function formatDate(value: string | null) {
  if (!value) return "—";
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? "—"
    : date.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

function billedByLabel(row: CustomerRow) {
  if (!row.planId) return "—";
  if (row.billingProvider === "apple") return "Apple";
  if (row.billingProvider === "google") return "Google";
  return "Stripe";
}

function planLabel(row: CustomerRow) {
  if (!row.planId) return "Free";
  const name = row.planId.charAt(0).toUpperCase() + row.planId.slice(1);
  return row.planStatus && row.planStatus !== "active" ? `${name} (${row.planStatus.replace("_", " ")})` : name;
}

export function AdminCustomersPage() {
  const { getToken } = usePlatformAuth();
  const [data, setData] = useState<CustomersResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [filter, setFilter] = useState<Filter>("all");

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const token = await getToken();
      if (!token) throw new Error("Sign in to view customers.");
      const response = await fetch(`${apiBase()}/api/v1/admin/customers`, {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (response.status === 403) throw new Error("This page is for Allternit admins.");
      const payload = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(payload.message || payload.error || `Unable to load customers (${response.status})`);
      setData(payload as CustomersResponse);
    } catch (err) {
      setError(formatApiError(err, "Unable to load customers"));
    } finally {
      setLoading(false);
    }
  }, [getToken]);

  useEffect(() => {
    void load();
  }, [load]);

  const setOpenResearch = useCallback(
    async (userId: string, enabled: boolean, contractRef: string, note: string) => {
      const token = await getToken();
      if (!token) throw new Error("Sign in to change this setting.");
      const response = await fetch(
        `${apiBase()}/api/v1/admin/open-lane/${encodeURIComponent(userId)}/research`,
        {
          method: "PUT",
          headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
          body: JSON.stringify({ enabled, contract_ref: contractRef, note }),
        },
      );
      const payload = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(payload.message || payload.error || `Unable to update (${response.status})`);
      setData((prev) =>
        prev
          ? {
              ...prev,
              customers: prev.customers.map((c) =>
                c.userId === userId
                  ? {
                      ...c,
                      open_research: {
                        enabled: payload.enabled ?? enabled,
                        contract_ref: payload.contract_ref ?? contractRef,
                        set_at: payload.set_at ?? new Date().toISOString(),
                      },
                    }
                  : c,
              ),
            }
          : prev,
      );
    },
    [getToken],
  );

  const rows = useMemo(() => {
    const all = data?.customers ?? [];
    if (filter === "paying") return all.filter(isPaying);
    if (filter === "free") return all.filter((row) => !isPaying(row));
    return all;
  }, [data, filter]);

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="text-[28px] font-bold leading-none tracking-tight text-[var(--text-primary)]">Customers</h1>
          <p className="mt-2 text-[13px] text-[var(--text-secondary)]">
            Everyone who signed up, their plan and cloud computer, and the emails we sent them.{" "}
            <Link to="/admin/funding" className="underline underline-offset-2">Funding</Link>
          </p>
        </div>
        <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => void load()} disabled={loading}>
          <ArrowsClockwise size={14} /> Refresh
        </button>
      </div>

      {data && (
        <div className="flex flex-wrap gap-3">
          {(
            [
              ["all", `All sign-ups · ${data.totalSignups}`],
              ["paying", `Paying · ${data.paying}`],
              ["free", `Free · ${data.totalSignups - data.paying}`],
            ] as Array<[Filter, string]>
          ).map(([value, label]) => (
            <button
              key={value}
              type="button"
              onClick={() => setFilter(value)}
              aria-pressed={filter === value}
              className={`rounded-md border px-3 py-1.5 text-[12px] font-medium ${
                filter === value
                  ? "border-[var(--text-primary)] text-[var(--text-primary)]"
                  : "border-[var(--border-subtle)] text-[var(--text-secondary)]"
              }`}
            >
              {label}
            </button>
          ))}
        </div>
      )}

      {error && <p className="text-[13px] text-[var(--status-error)]">{error}</p>}

      {loading && !data ? (
        <SkeletonRow lines={6} />
      ) : (
        data && (
          <div className="overflow-x-auto rounded-xl border border-[var(--border-subtle)]">
            <table className="w-full min-w-[960px] text-left text-[13px]">
              <thead className="bg-[var(--bg-secondary)] text-[11px] uppercase tracking-[0.08em] text-[var(--text-tertiary)]">
                <tr>
                  <th className="px-3 py-2 font-medium">Customer</th>
                  <th className="px-3 py-2 font-medium">Signed up</th>
                  <th className="px-3 py-2 font-medium">Plan</th>
                  <th className="px-3 py-2 font-medium">Billed by</th>
                  <th className="px-3 py-2 font-medium">Cloud computer</th>
                  <th className="px-3 py-2 font-medium">Emails sent</th>
                  <th className="px-3 py-2 font-medium">Open Research</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => (
                  <tr key={row.userId} className="border-t border-[var(--border-subtle)] text-[var(--text-primary)]">
                    <td className="px-3 py-2">
                      <div>{row.email ?? row.userId}</div>
                      {row.name && <div className="text-[12px] text-[var(--text-secondary)]">{row.name}</div>}
                    </td>
                    <td className="px-3 py-2 tabular-nums">{formatDate(row.signedUpAt)}</td>
                    <td className="px-3 py-2">
                      {row.stripeCustomerId ? (
                        <a
                          href={`https://dashboard.stripe.com/customers/${row.stripeCustomerId}`}
                          target="_blank"
                          rel="noopener noreferrer"
                          className="underline decoration-[var(--border-default)] underline-offset-2"
                        >
                          {planLabel(row)}
                        </a>
                      ) : (
                        planLabel(row)
                      )}
                    </td>
                    <td className="px-3 py-2">{billedByLabel(row)}</td>
                    <td className="px-3 py-2">{row.computerStatus ?? "—"}</td>
                    <td className="px-3 py-2 text-[var(--text-secondary)]">
                      {row.emailsSent.length ? row.emailsSent.map((kind) => EMAIL_LABELS[kind] ?? kind).join(", ") : "—"}
                    </td>
                    <td className="px-3 py-2">
                      <OpenResearchCell row={row} onSet={setOpenResearch} />
                    </td>
                  </tr>
                ))}
                {rows.length === 0 && (
                  <tr>
                    <td colSpan={7} className="px-3 py-6 text-center text-[var(--text-secondary)]">
                      No customers here yet.
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
