/**
 * Platform API → Computers (hosted computer driver).
 *
 * Shows the project's `hosted_driver_enabled` flag (read-only; Allternit turns
 * it on), the project's computers (`GET /v1/computers`) with start / stop /
 * delete, the computer settings (`GET/PATCH /v1/computer_settings`), and
 * `computer_minute` / `computer_action` usage from `GET /v1/usage`.
 */
import React, { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { Desktop } from "@phosphor-icons/react";
import { AllternitApiError, formatApiError } from "@/lib/api-client";
import {
  allPages,
  USAGE_METER_LABELS,
  type ApprovalMode,
  type V1Client,
  type V1Computer,
  type V1ComputerSettings,
  type V1UsageRow,
} from "@/lib/platform-v1";
import { EmptyState, SkeletonRow } from "@/components/console-ui";
import { DESTRUCTIVE_BUTTON_CLASS, QUIET_BUTTON_CLASS, SETTINGS_SELECT_CLASS } from "@/components/console-ui/buttonStyles";
import { cn } from "@/lib/utils";
import { CARD_CLASS, ErrorBanner, PlatformHeader, PRIMARY_BUTTON_CLASS, ProjectGate, formatDateTime, usePlatformProjects } from "./shared";

const COMPUTER_METERS = ["computer_minute", "computer_action"] as const;

/** White field on a neutral border (no tan surfaces). */
const FIELD_CLASS =
  "mt-1.5 w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-2 px-3 text-[13px] text-[var(--text-primary)] outline-none focus:border-[var(--accent-primary)]";

function isDriverDisabled(err: unknown): boolean {
  return err instanceof AllternitApiError && err.statusCode === 404 && err.code === "hosted_driver_disabled";
}

export function PlatformComputersPage() {
  const state = usePlatformProjects();
  return (
    <div className="mx-auto w-full max-w-5xl">
      <PlatformHeader
        title="Computers"
        subtitle="Hosted computers your app drives through the computer and browser toolsets."
        state={state}
      />
      <ProjectGate state={state}>{({ project, client }) => <ComputersBody key={project.id} client={client} />}</ProjectGate>
    </div>
  );
}

function ComputersBody({ client }: { client: V1Client }) {
  const [settings, setSettings] = useState<V1ComputerSettings | null>(null);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  const [driverOff, setDriverOff] = useState(false);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let active = true;
    setLoading(true);
    client
      .getComputerSettings()
      .then((s) => {
        if (!active) return;
        setSettings(s);
        setDriverOff(!s.hosted_driver_enabled);
      })
      .catch((err) => {
        if (!active) return;
        if (isDriverDisabled(err)) setDriverOff(true);
        else setSettingsError(formatApiError(err, "Unable to load computer settings"));
      })
      .finally(() => active && setLoading(false));
    return () => {
      active = false;
    };
  }, [client]);

  if (loading) {
    return (
      <div className={cn(CARD_CLASS, "px-4")} role="status" aria-label="Loading computers">
        <SkeletonRow lines={3} />
      </div>
    );
  }

  return (
    <div className="space-y-5">
      <DriverFlag enabled={!driverOff} />
      {settingsError && <ErrorBanner message={settingsError} />}
      {!driverOff && (
        <>
          <ComputerList client={client} />
          {settings && <SettingsForm client={client} settings={settings} onSaved={setSettings} />}
          <ComputerUsage client={client} />
        </>
      )}
      <KeysNote />
    </div>
  );
}

function DriverFlag({ enabled }: { enabled: boolean }) {
  return (
    <section className={cn(CARD_CLASS, "bg-[var(--bg-secondary)] p-4")} aria-label="Hosted driver status">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-[13px] font-semibold text-[var(--text-primary)]">Hosted driver</span>
        <span
          className={cn(
            "rounded-full px-2 py-0.5 text-[11px] font-semibold",
            enabled
              ? "bg-[var(--status-success)]/[0.12] text-[var(--status-success)]"
              : "bg-[var(--neutral-fill,var(--surface-hover))] text-[var(--text-secondary)]",
          )}
        >
          {enabled ? "On" : "Off"}
        </span>
      </div>
      <p className="m-0 mt-1.5 text-[13px] text-[var(--text-secondary)]">
        {enabled
          ? "This project can create and drive hosted computers through /v1/computers."
          : "Allternit turns the hosted driver on per project. You can't switch it on here. Until it is on, every /v1/computers call returns 404 hosted_driver_disabled."}
      </p>
    </section>
  );
}

const STATUS_TONE: Record<string, string> = {
  running: "text-[var(--status-success)]",
  error: "text-[var(--status-error)]",
  provisioning: "text-[var(--status-warning)]",
  starting: "text-[var(--status-warning)]",
  stopping: "text-[var(--status-warning)]",
};

function ComputerList({ client }: { client: V1Client }) {
  const [computers, setComputers] = useState<V1Computer[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const all = await allPages((after) => client.listComputers(after));
      setComputers(all.filter((c) => c.status !== "deleted"));
    } catch (err) {
      setError(formatApiError(err, "Unable to load computers"));
    } finally {
      setLoading(false);
    }
  }, [client]);

  useEffect(() => {
    void load();
  }, [load]);

  const act = async (c: V1Computer, action: "start" | "stop" | "delete") => {
    if (action === "delete" && !window.confirm(`Delete ${c.name || c.id}? Its disk is removed and this can't be undone.`)) return;
    setBusy(`${c.id}:${action}`);
    setError(null);
    try {
      const updated =
        action === "start" ? await client.startComputer(c.id) : action === "stop" ? await client.stopComputer(c.id) : await client.deleteComputer(c.id);
      setComputers((list) =>
        updated.status === "deleted" ? list.filter((x) => x.id !== c.id) : list.map((x) => (x.id === c.id ? updated : x)),
      );
    } catch (err) {
      setError(formatApiError(err, `Unable to ${action} computer`));
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className={cn(CARD_CLASS, "overflow-hidden bg-[var(--bg-secondary)]")} aria-label="Computers" aria-busy={loading}>
      <div className="flex items-center justify-between border-b border-solid border-[var(--border-subtle)] px-4 py-3">
        <h2 className="m-0 text-[14px] font-semibold text-[var(--text-primary)]">Computers</h2>
        <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => void load()} disabled={loading}>
          Refresh
        </button>
      </div>
      {error && <ErrorBanner message={error} className="m-3" />}
      {loading ? (
        <div className="px-4" role="status" aria-label="Loading computers">
          <SkeletonRow lines={3} />
        </div>
      ) : computers.length === 0 ? (
        <EmptyState
          icon={<Desktop size={32} aria-hidden />}
          title="No computers yet"
          caption="Your app creates computers with POST /v1/computers using a key with the computers scope. They show up here."
        />
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full min-w-[640px] border-collapse text-left text-[13px]">
            <thead>
              <tr className="border-b border-solid border-[var(--border-subtle)] text-[12px] text-[var(--text-tertiary)]">
                <th scope="col" className="px-4 py-2.5 font-medium">Name</th>
                <th scope="col" className="px-4 py-2.5 font-medium">Status</th>
                <th scope="col" className="px-4 py-2.5 font-medium">Account</th>
                <th scope="col" className="px-4 py-2.5 font-medium">Created</th>
                <th scope="col" className="px-4 py-2.5 text-right font-medium">Actions</th>
              </tr>
            </thead>
            <tbody>
              {computers.map((c) => {
                const running = c.status === "running" || c.status === "starting" || c.status === "provisioning";
                return (
                  <tr key={c.id} className="border-b border-solid border-[var(--border-subtle)] last:border-0">
                    <td className="px-4 py-2.5">
                      <div className="text-[var(--text-primary)]">{c.name || "Untitled"}</div>
                      <div className="font-mono text-[11px] text-[var(--text-tertiary)]">{c.id}</div>
                    </td>
                    <td className={cn("px-4 py-2.5 capitalize", STATUS_TONE[c.status] ?? "text-[var(--text-secondary)]")}>{c.status}</td>
                    <td className="px-4 py-2.5 font-mono text-[12px] text-[var(--text-secondary)]">{c.account_id ?? "—"}</td>
                    <td className="px-4 py-2.5 text-[var(--text-secondary)]">{formatDateTime(c.created_at)}</td>
                    <td className="px-4 py-2.5">
                      <div className="flex justify-end gap-2">
                        {running ? (
                          <button type="button" className={QUIET_BUTTON_CLASS} disabled={busy !== null} onClick={() => void act(c, "stop")}>
                            {busy === `${c.id}:stop` ? "Stopping…" : "Stop"}
                          </button>
                        ) : (
                          <button type="button" className={QUIET_BUTTON_CLASS} disabled={busy !== null} onClick={() => void act(c, "start")}>
                            {busy === `${c.id}:start` ? "Starting…" : "Start"}
                          </button>
                        )}
                        <button type="button" className={DESTRUCTIVE_BUTTON_CLASS} disabled={busy !== null} onClick={() => void act(c, "delete")}>
                          {busy === `${c.id}:delete` ? "Deleting…" : "Delete"}
                        </button>
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function SettingsForm({
  client,
  settings,
  onSaved,
}: {
  client: V1Client;
  settings: V1ComputerSettings;
  onSaved: (s: V1ComputerSettings) => void;
}) {
  const [approvalMode, setApprovalMode] = useState<ApprovalMode>(settings.approval_mode);
  const [concurrency, setConcurrency] = useState(String(settings.per_key_concurrency));
  const [browser, setBrowser] = useState(settings.browser_toolset);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  const parsed = Number(concurrency);
  const valid = Number.isInteger(parsed) && parsed >= 1;
  const dirty =
    approvalMode !== settings.approval_mode || parsed !== settings.per_key_concurrency || browser !== settings.browser_toolset;

  const save = async () => {
    if (!valid) return;
    setSaving(true);
    setError(null);
    setSaved(false);
    try {
      const next = await client.updateComputerSettings({
        approval_mode: approvalMode,
        per_key_concurrency: parsed,
        browser_toolset: browser,
      });
      onSaved(next);
      setSaved(true);
    } catch (err) {
      setError(formatApiError(err, "Unable to save settings"));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className={cn(CARD_CLASS, "bg-[var(--bg-secondary)] p-4")} aria-label="Computer settings">
      <h2 className="m-0 text-[14px] font-semibold text-[var(--text-primary)]">Settings</h2>
      <div className="mt-3 grid gap-4 sm:grid-cols-2">
        <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
          Who approves risky actions
          <select
            value={approvalMode}
            onChange={(e) => setApprovalMode(e.target.value as ApprovalMode)}
            className={cn(SETTINGS_SELECT_CLASS, "mt-1.5 w-full")}
          >
            <option value="owner">Project owner, in this console</option>
            <option value="api_key">API keys may approve (POST …/approvals/&#123;id&#125;)</option>
          </select>
        </label>
        <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
          Running computers per key
          <input
            type="number"
            min={1}
            step={1}
            inputMode="numeric"
            value={concurrency}
            onChange={(e) => setConcurrency(e.target.value)}
            aria-invalid={!valid}
            className={FIELD_CLASS}
          />
          <span className="mt-1 block font-normal text-[var(--text-tertiary)]">Past this, POST /v1/computers returns 429 concurrency_limit.</span>
        </label>
      </div>
      <label className="mt-4 flex items-start gap-2 text-[13px] text-[var(--text-primary)]">
        <input type="checkbox" checked={browser} onChange={(e) => setBrowser(e.target.checked)} className="mt-0.5" />
        <span>
          Browser toolset
          <span className="block text-[12px] text-[var(--text-tertiary)]">Lets keys call the browser toolset (navigate, read page, fill forms) as well as the computer toolset.</span>
        </span>
      </label>
      {error && <ErrorBanner message={error} className="mt-3" />}
      <div className="mt-4 flex items-center gap-3">
        <button type="button" className={PRIMARY_BUTTON_CLASS} disabled={!dirty || !valid || saving} onClick={() => void save()}>
          {saving ? "Saving…" : "Save settings"}
        </button>
        {saved && !dirty && <span className="text-[12px] text-[var(--text-secondary)]" role="status">Saved.</span>}
      </div>
    </section>
  );
}

function ComputerUsage({ client }: { client: V1Client }) {
  const [rows, setRows] = useState<V1UsageRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    const to = new Date();
    const from = new Date(to.getTime() - 30 * 86_400_000);
    client
      .usage({ group_by: "meter", from: from.toISOString(), to: to.toISOString() })
      .then((u) => active && setRows(u.data.filter((r) => (COMPUTER_METERS as readonly string[]).includes(r.meter))))
      .catch((err) => active && setError(formatApiError(err, "Unable to load usage")));
    return () => {
      active = false;
    };
  }, [client]);

  return (
    <section className={cn(CARD_CLASS, "bg-[var(--bg-secondary)] p-4")} aria-label="Computer usage" aria-busy={rows === null && !error}>
      <div className="flex items-center justify-between">
        <h2 className="m-0 text-[14px] font-semibold text-[var(--text-primary)]">Usage, last 30 days</h2>
        <Link to="/platform/usage" className="text-[12px] font-semibold text-[var(--accent-primary)] hover:underline">
          All usage
        </Link>
      </div>
      {error ? (
        <ErrorBanner message={error} className="mt-3" />
      ) : rows === null ? (
        <div role="status" aria-label="Loading usage">
          <SkeletonRow lines={2} />
        </div>
      ) : (
        <div className="mt-3 grid gap-3 sm:grid-cols-2">
          {COMPUTER_METERS.map((meter) => {
            const row = rows.find((r) => r.meter === meter);
            return (
              <div key={meter} className="rounded-lg bg-[var(--neutral-fill,var(--surface-hover))] p-3">
                <div className="text-[12px] text-[var(--text-secondary)]">{USAGE_METER_LABELS[meter]}</div>
                <div className="mt-1 text-[18px] font-semibold tabular-nums text-[var(--text-primary)]">
                  {(row?.quantity ?? 0).toLocaleString(undefined, { maximumFractionDigits: 2 })}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </section>
  );
}

function KeysNote() {
  return (
    <p className="m-0 text-[13px] text-[var(--text-secondary)]">
      Keys need the <code className="font-mono text-[12px]">computers</code> scope to call /v1/computers.{" "}
      <Link to="/api-keys" className="font-semibold text-[var(--accent-primary)] hover:underline">
        Manage project keys
      </Link>
      {" · "}
      <a href="https://docs.allternit.com/api/platform/hosted-computers" className="font-semibold text-[var(--accent-primary)] hover:underline">
        Hosted driver docs
      </a>
    </p>
  );
}
