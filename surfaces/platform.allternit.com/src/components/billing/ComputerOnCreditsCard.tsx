import React, { useCallback, useEffect, useState } from "react";
import { ArrowSquareOut, Desktop, Warning } from "@phosphor-icons/react";
import { usePlatformAuth } from "@/lib/platform-auth-client";
import { formatApiError } from "@/lib/api-client";
import { QUIET_BUTTON_CLASS } from "@/components/settings/buttonStyles";

type CreditSize = {
  id: string;
  cpu: number;
  memory_gb: number;
  disk_gb: number;
  price_per_hour_usd: number;
  stopped_price_per_hour_usd: number;
};

type ComputeStatus = {
  state: "none" | "paired" | "provisioning" | "hosted" | "suspended";
  computer: { id: string; kind: string; provider: string | null; status: string } | null;
  paths?: {
    credits?: {
      price_per_hour_usd: number;
      stopped_price_per_hour_usd: number;
      balance_usd: number;
      min_start_usd: number;
      sizes?: CreditSize[];
    } | null;
  };
  docs_url?: string;
};

const DOCS_URL = "https://docs.allternit.com/guides/credits-computer";

function apiBase(): string {
  return String(import.meta.env.VITE_ALLTERNIT_CLOUD_API_URL || "https://api.allternit.com").replace(/\/$/, "");
}

function usd(value: number): string {
  return `$${Number(value.toFixed(3))}`;
}

/** Runway = balance / running rate, as "about N hours" or "about N days". */
export function formatRunway(balanceUsd: number, pricePerHour: number): string {
  if (!(pricePerHour > 0)) return "";
  const hours = balanceUsd / pricePerHour;
  if (hours < 1) return `about ${Math.max(0, Math.round(hours * 60))} minutes`;
  if (hours < 48) return `about ${Math.round(hours)} hours`;
  return `about ${Math.round(hours / 24)} days`;
}

/**
 * "Computer on credits": a hosted computer billed per minute from the credit
 * balance. Hidden unless the server reports a credits path (flag off = null).
 * Prices come from /api/v1/account/compute-status, never hard-coded.
 */
export function ComputerOnCreditsCard({ onChanged }: { onChanged?: () => void }) {
  const { getToken, isSignedIn } = usePlatformAuth();
  const [status, setStatus] = useState<ComputeStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const call = useCallback(
    async (path: string, init?: RequestInit) => {
      const token = await getToken(); // fresh Clerk token per request
      if (!token) throw new Error("A web account session is required.");
      return fetch(`${apiBase()}${path}`, {
        ...init,
        headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json", ...(init?.headers ?? {}) },
      });
    },
    [getToken],
  );

  const load = useCallback(async () => {
    try {
      const response = await call("/api/v1/account/compute-status");
      if (!response.ok) {
        setStatus(null);
        return;
      }
      setStatus((await response.json()) as ComputeStatus);
    } catch {
      setStatus(null);
    }
  }, [call]);

  useEffect(() => {
    if (isSignedIn) void load();
  }, [isSignedIn, load]);

  const starting = status?.computer?.status === "starting" || status?.state === "provisioning";
  useEffect(() => {
    if (!starting) return;
    const timer = window.setInterval(() => void load(), 5000);
    return () => window.clearInterval(timer);
  }, [starting, load]);

  const credits = status?.paths?.credits;
  if (!credits) return null;
  const size = credits.sizes?.[0];
  const price = size?.price_per_hour_usd ?? credits.price_per_hour_usd;
  const stoppedPrice = size?.stopped_price_per_hour_usd ?? credits.stopped_price_per_hour_usd;
  const computer = status?.computer?.kind === "credits" ? status.computer : null;
  const running = computer && (computer.status === "running" || computer.status === "sleeping" || computer.status === "starting");
  const outOfCredits = status?.state === "suspended" && computer?.kind === "credits";

  const act = async (path: string, body?: unknown) => {
    setBusy(true);
    setError(null);
    try {
      const response = await call(path, { method: "POST", body: body ? JSON.stringify(body) : undefined });
      if (!response.ok) {
        const payload = await response.json().catch(() => ({}));
        const code = payload?.error?.code;
        if (code === "insufficient_credits") {
          setError(`Starting needs at least ${usd(payload.error.min_start_usd ?? credits.min_start_usd)} in credits.`);
        } else {
          setError(payload?.error?.message || `Request failed (${response.status})`);
        }
        return;
      }
      await load();
      onChanged?.();
    } catch (err) {
      setError(formatApiError(err, "Request failed"));
    } finally {
      setBusy(false);
    }
  };

  let stateLine = "No credit computer yet.";
  if (outOfCredits) stateLine = "Your credits ran out and the computer stopped. Add credits to start it again.";
  else if (computer?.status === "starting") stateLine = "Starting your computer. This takes about a minute.";
  else if (computer?.status === "sleeping") stateLine = "Your computer is asleep. It wakes when an agent needs it.";
  else if (computer?.status === "running") stateLine = "Your computer is ready.";
  else if (computer?.status === "stopped") stateLine = "Your computer is stopped.";
  else if (computer?.status === "error") stateLine = "The computer did not start. Try again.";

  return (
    <div className="rounded-xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4" data-testid="computer-on-credits">
      <div className="flex items-center gap-2">
        <Desktop size={18} className="text-[var(--text-secondary)]" />
        <div className="text-[14px] font-semibold text-[var(--text-primary)]">Computer on credits</div>
      </div>
      <p className="mt-1 text-[13px] text-[var(--text-secondary)]">
        {size ? `${size.cpu} cores, ${size.memory_gb} GB. ` : ""}
        {usd(price)} an hour while it runs, {usd(stoppedPrice)} an hour while stopped.
      </p>
      <dl className="mt-3 grid grid-cols-2 gap-3 text-[13px]">
        <div>
          <dt className="text-[var(--text-tertiary)]">Balance</dt>
          <dd className="font-semibold text-[var(--text-primary)]">${credits.balance_usd.toFixed(2)}</dd>
        </div>
        <div>
          <dt className="text-[var(--text-tertiary)]">Runway while running</dt>
          <dd className="font-semibold text-[var(--text-primary)]">{formatRunway(credits.balance_usd, price) || "n/a"}</dd>
        </div>
      </dl>
      <p className="mt-3 text-[13px] text-[var(--text-primary)]" role="status">{stateLine}</p>
      {error && (
        <div className="mt-2 flex items-center gap-2 text-[13px] text-[var(--status-error)]" role="alert">
          <Warning size={14} /> {error}
        </div>
      )}
      <div className="mt-3 flex flex-wrap items-center gap-3">
        {running ? (
          <button type="button" disabled={busy} className={QUIET_BUTTON_CLASS} onClick={() => void act("/api/v1/provisioned-instances/credit/stop")}>
            Stop
          </button>
        ) : (
          <button
            type="button"
            disabled={busy || credits.balance_usd < credits.min_start_usd}
            className={QUIET_BUTTON_CLASS}
            onClick={() => void act("/api/v1/provisioned-instances/credit", { size: size?.id ?? "medium" })}
          >
            Start
          </button>
        )}
        {!running && credits.balance_usd < credits.min_start_usd && (
          <span className="text-[12px] text-[var(--text-tertiary)]">
            Starting needs at least {usd(credits.min_start_usd)} in credits.
          </span>
        )}
        <a href={status?.docs_url?.includes("credits-computer") ? status.docs_url : DOCS_URL} target="_blank" rel="noopener noreferrer" className="inline-flex items-center gap-1 text-[13px] text-[var(--text-secondary)] underline">
          Docs <ArrowSquareOut size={12} />
        </a>
      </div>
      <p className="mt-3 text-[12px] text-[var(--text-tertiary)]">
        Hosted computers run on a third-party cloud provider, managed by Allternit. They sleep when idle.
      </p>
    </div>
  );
}
