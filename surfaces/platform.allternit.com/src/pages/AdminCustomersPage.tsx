import React, { useCallback, useEffect, useMemo, useState } from "react";
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
  stripeCustomerId: string | null;
  computerStatus: string | null;
  emailsSent: string[];
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
            Everyone who signed up, their plan and cloud computer, and the emails we sent them.
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
            <table className="w-full min-w-[760px] text-left text-[13px]">
              <thead className="bg-[var(--bg-secondary)] text-[11px] uppercase tracking-[0.08em] text-[var(--text-tertiary)]">
                <tr>
                  <th className="px-3 py-2 font-medium">Customer</th>
                  <th className="px-3 py-2 font-medium">Signed up</th>
                  <th className="px-3 py-2 font-medium">Plan</th>
                  <th className="px-3 py-2 font-medium">Cloud computer</th>
                  <th className="px-3 py-2 font-medium">Emails sent</th>
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
                    <td className="px-3 py-2">{row.computerStatus ?? "—"}</td>
                    <td className="px-3 py-2 text-[var(--text-secondary)]">
                      {row.emailsSent.length ? row.emailsSent.map((kind) => EMAIL_LABELS[kind] ?? kind).join(", ") : "—"}
                    </td>
                  </tr>
                ))}
                {rows.length === 0 && (
                  <tr>
                    <td colSpan={5} className="px-3 py-6 text-center text-[var(--text-secondary)]">
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
