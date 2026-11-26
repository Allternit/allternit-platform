import React, { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  ArrowsClockwise,
  CheckCircle,
  CircleNotch,
  Cloud,
  ComputerTower,
  Gauge,
  HardDrives,
  MapPin,
  Plus,
  ShieldCheck,
  Trash,
  X,
} from "@phosphor-icons/react";
import { usePlatformAuth, usePlatformUser } from "@/lib/platform-auth-client";
import {
  createHostedRuntime,
  destroyHostedRuntime,
  getHostedEntitlement,
  listHostedRuntimes,
  startHostedRuntime,
  stopHostedRuntime,
  type HostedRuntime,
  type HostedRuntimeEntitlement,
} from "@/lib/hosted-compute";
import { formatApiError } from "@/lib/api-client";
import { SectionHeading } from "@/components/settings/SectionHeading";
import { EmptyState } from "@/components/settings/EmptyState";
import { SkeletonRow } from "@/components/settings/SkeletonRow";
import { Badge } from "@/components/settings/Badge";
import {
  QUIET_BUTTON_CLASS,
  DESTRUCTIVE_BUTTON_CLASS,
  SETTINGS_SELECT_CLASS,
} from "@/components/settings/buttonStyles";
import { cn } from "@/lib/utils";

const MEMORY_OPTIONS = [512, 1024, 2048, 4096];

function formatHours(seconds: number) {
  if (seconds < 3600) return `${Math.max(0, Math.round(seconds / 60))} min`;
  return `${(seconds / 3600).toFixed(seconds < 36_000 ? 1 : 0)} hr`;
}

function formatMemory(memoryMb: number) {
  return memoryMb >= 1024
    ? `${Number((memoryMb / 1024).toFixed(1))} GB`
    : `${memoryMb} MB`;
}

function titleCase(value: string) {
  return value.replaceAll("_", " ").replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function safePortalUrl(value?: string): string | null {
  if (!value) return null;
  try {
    const url = new URL(value);
    return url.protocol === "https:" ? url.toString() : null;
  } catch {
    return null;
  }
}

function ProductCard({
  icon,
  title,
  eyebrow,
  description,
  children,
  active,
}: {
  icon: React.ReactNode;
  title: string;
  eyebrow: string;
  description: string;
  children: React.ReactNode;
  active?: boolean;
}) {
  return (
    <div
      className={cn(
        "rounded-xl border border-solid p-4",
        active
          ? "border-[rgba(34,197,94,0.25)] bg-[rgba(34,197,94,0.04)]"
          : "border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40",
      )}
    >
      <div className="flex items-start gap-3">
        <div className="size-9 shrink-0 rounded-lg bg-[var(--bg-secondary)] flex items-center justify-center text-[var(--text-secondary)]">
          {icon}
        </div>
        <div className="min-w-0 flex-1">
          <div className="text-[10px] uppercase tracking-wide text-[var(--text-tertiary)]">{eyebrow}</div>
          <div className="text-[14px] font-semibold text-[var(--text-primary)] mt-0.5">{title}</div>
          <p className="text-[11px] leading-relaxed text-[var(--text-secondary)] mt-1 mb-3">{description}</p>
          {children}
        </div>
      </div>
    </div>
  );
}

export function ComputeBillingPanel() {
  const navigate = useNavigate();
  const { getToken } = usePlatformAuth();
  const { isLoaded, isSignedIn } = usePlatformUser();
  const [entitlement, setEntitlement] = useState<HostedRuntimeEntitlement | null>(null);
  const [runtimes, setRuntimes] = useState<HostedRuntime[]>([]);
  const [loading, setLoading] = useState(true);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [confirmDestroyId, setConfirmDestroyId] = useState<string | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  const [createName, setCreateName] = useState("My hosted runtime");
  const [createRegion, setCreateRegion] = useState("lax");
  const [createMemoryMb, setCreateMemoryMb] = useState(1024);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!isLoaded) return;
    if (!isSignedIn) {
      setEntitlement(null);
      setRuntimes([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const token = await getToken();
      if (!token) throw new Error("A web account session is required to manage hosted compute.");
      const [nextEntitlement, nextRuntimes] = await Promise.all([
        getHostedEntitlement(token),
        listHostedRuntimes(token),
      ]);
      setEntitlement(nextEntitlement);
      setRuntimes(nextRuntimes);
      const allowedRegions = nextEntitlement.allowedRegions?.length
        ? nextEntitlement.allowedRegions
        : ["lax"];
      setCreateRegion((current) => (allowedRegions.includes(current) ? current : allowedRegions[0]));
      const allowedMemory = MEMORY_OPTIONS.filter((value) => value <= nextEntitlement.maxMemoryMb);
      setCreateMemoryMb((current) =>
        allowedMemory.includes(current) ? current : (allowedMemory[allowedMemory.length - 1] ?? nextEntitlement.maxMemoryMb),
      );
    } catch (failure) {
      setError(formatApiError(failure, "Unable to load hosted compute"));
    } finally {
      setLoading(false);
    }
  }, [getToken, isLoaded, isSignedIn]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!runtimes.some((runtime) => ["creating", "starting", "stopping", "destroying"].includes(runtime.status))) return;
    const timer = window.setInterval(() => void load(), 5_000);
    return () => window.clearInterval(timer);
  }, [load, runtimes]);

  const mutate = useCallback(
    async (id: string, action: (token: string) => Promise<unknown>) => {
      setBusyId(id);
      setError(null);
      try {
        const token = await getToken();
        if (!token) throw new Error("A web account session is required.");
        await action(token);
        setConfirmDestroyId(null);
        await load();
      } catch (failure) {
        setError(formatApiError(failure, "Hosted runtime action failed"));
      } finally {
        setBusyId(null);
      }
    },
    [getToken, load],
  );

  const handleCreate = useCallback(async () => {
    setBusyId("create");
    setError(null);
    try {
      const token = await getToken();
      if (!token) throw new Error("A web account session is required.");
      await createHostedRuntime(token, {
        name: createName.trim() || undefined,
        region: createRegion,
        memoryMb: createMemoryMb,
      });
      setShowCreate(false);
      await load();
    } catch (failure) {
      setError(formatApiError(failure, "Unable to create hosted runtime"));
    } finally {
      setBusyId(null);
    }
  }, [createMemoryMb, createName, createRegion, getToken, load]);

  const usagePercent = entitlement?.maxHoursMonthly
    ? Math.min(100, (entitlement.usedSecondsMonthly / (entitlement.maxHoursMonthly * 3600)) * 100)
    : 0;
  const allowedRegions = entitlement?.allowedRegions?.length ? entitlement.allowedRegions : ["lax"];
  const allowedMemory = useMemo(
    () => MEMORY_OPTIONS.filter((value) => value <= (entitlement?.maxMemoryMb ?? 0)),
    [entitlement?.maxMemoryMb],
  );
  const hasCapacity = Boolean(
    entitlement?.canCreateHostedRuntime && entitlement.activeInstances < entitlement.maxHostedRuntimes,
  );

  return (
    <div className="space-y-5">
      <div>
        <SectionHeading className="mb-1">Plans & compute</SectionHeading>
        <p className="text-[12px] text-[var(--text-secondary)] m-0 leading-relaxed">
          Local compute stays the default. Add your own VPS, managed Fly hosting,
          or organization-scoped enterprise BYOC when the workload needs it.
        </p>
      </div>

      <div className="grid grid-cols-1 gap-3">
        <ProductCard
          icon={<ComputerTower size={18} />}
          eyebrow="Included"
          title="Local compute"
          description="Your brain, provider credentials, and local models stay on your computer. Allternit does not meter your machine."
          active
        >
          <div className="flex items-center justify-between gap-3">
            <span className="inline-flex items-center gap-1 text-[11px] font-medium text-[var(--status-success)]">
              <CheckCircle size={13} weight="fill" /> Ready
            </span>
            <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => navigate("/compute")}>
              Manage models
            </button>
          </div>
        </ProductCard>

        <ProductCard
          icon={<HardDrives size={18} />}
          eyebrow="Provider-priced"
          title="Your VPS"
          description="Rent from a supported provider or connect a server you already own. The provider bills the server directly."
        >
          <div className="flex items-center justify-between gap-3">
            <span className="text-[10px] text-[var(--text-tertiary)]">From provider pricing · no Allternit compute markup</span>
            <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => navigate("/compute")}>
              Buy or connect VPS
            </button>
          </div>
        </ProductCard>

        <ProductCard
          icon={<Cloud size={18} />}
          eyebrow="Paid add-on"
          title="Allternit managed hosting"
          description="A private Fly Machine runs your Gizzi brain and stops after inactivity. Plan limits enforce runtime hours, instance count, memory, and spend."
          active={Boolean(entitlement?.canCreateHostedRuntime)}
        >
          {!isLoaded || loading ? (
            <SkeletonRow lines={3} />
          ) : !isSignedIn ? (
            <div className="flex items-center justify-between gap-3 rounded-lg border border-solid border-[var(--border-subtle)] px-3 py-2.5">
              <span className="text-[11px] text-[var(--text-secondary)]">Sign in to view plans and manage hosted runtimes.</span>
              <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => navigate("/sign-in")}>Sign in</button>
            </div>
          ) : (
            <>
              <div className="flex items-center gap-2 flex-wrap mb-3">
                <Badge>{entitlement?.planDisplayName ?? "Free"}</Badge>
                {entitlement?.canCreateHostedRuntime && (
                  <Badge className="text-[var(--status-success)] bg-[var(--status-success)]/10">Hosted enabled</Badge>
                )}
                {entitlement && (
                  <span className="text-[10px] text-[var(--text-tertiary)] ml-auto">
                    {entitlement.activeInstances} / {entitlement.maxHostedRuntimes} runtimes
                  </span>
                )}
              </div>

              {entitlement && entitlement.maxHoursMonthly > 0 && (
                <div className="rounded-lg border border-solid border-[var(--border-subtle)] p-3 mb-3">
                  <div className="flex items-center justify-between gap-3 text-[10px] mb-2">
                    <span className="inline-flex items-center gap-1 text-[var(--text-secondary)]"><Gauge size={12} /> Monthly runtime usage</span>
                    <span className="font-mono text-[var(--text-primary)]">
                      {formatHours(entitlement.usedSecondsMonthly)} / {entitlement.maxHoursMonthly} hr
                    </span>
                  </div>
                  <div className="h-1.5 rounded-full bg-[var(--bg-secondary)] overflow-hidden">
                    <div className="h-full rounded-full bg-[var(--accent-primary)] transition-[width]" style={{ width: `${usagePercent}%` }} />
                  </div>
                  <div className="flex items-center justify-between gap-3 text-[10px] text-[var(--text-tertiary)] mt-2">
                    <span>{formatHours(entitlement.remainingSecondsMonthly)} remaining</span>
                    <span>${entitlement.estimatedCostUsdMonthly.toFixed(2)} estimated infrastructure</span>
                  </div>
                </div>
              )}

              {runtimes.length === 0 && entitlement?.canCreateHostedRuntime && !showCreate && (
                <EmptyState
                  art="connect-scene"
                  size="panel"
                  className="rounded-lg border border-dashed border-[var(--border-subtle)] mb-3"
                  title="No managed runtime yet"
                  caption="Create one within this plan's memory, hour, and instance limits."
                />
              )}

              {runtimes.length > 0 && (
                <div className="space-y-2 mb-3">
                  {runtimes.map((runtime) => (
                    <div key={runtime.id} className="rounded-lg border border-solid border-[var(--border-subtle)] px-3 py-2.5">
                      <div className="flex items-center gap-2">
                        <span className={cn(
                          "size-1.5 rounded-full shrink-0",
                          runtime.status === "running" ? "bg-[var(--status-success)]" :
                            runtime.status === "error" ? "bg-[var(--status-error)]" : "bg-[var(--text-tertiary)]",
                        )} />
                        <div className="min-w-0 flex-1">
                          <div className="text-[12px] font-medium text-[var(--text-primary)] truncate">{runtime.name}</div>
                          <div className="text-[10px] text-[var(--text-tertiary)] mt-0.5">
                            {formatMemory(runtime.memoryMb)} · {runtime.region} · idle stop {runtime.idleTimeoutMinutes} min
                            {runtime.stopReason ? ` · ${titleCase(runtime.stopReason)}` : ""}
                          </div>
                        </div>
                        <Badge>{titleCase(runtime.status)}</Badge>
                      </div>
                      <div className="flex items-center justify-end gap-1.5 mt-2">
                        {runtime.status === "stopped" && (
                          <button type="button" className={QUIET_BUTTON_CLASS} disabled={busyId === runtime.id} onClick={() => void mutate(runtime.id, (token) => startHostedRuntime(token, runtime.id))}>
                            {busyId === runtime.id && <CircleNotch size={12} className="animate-spin" />} Start
                          </button>
                        )}
                        {["running", "starting"].includes(runtime.status) && (
                          <button type="button" className={QUIET_BUTTON_CLASS} disabled={busyId === runtime.id} onClick={() => void mutate(runtime.id, (token) => stopHostedRuntime(token, runtime.id))}>
                            {busyId === runtime.id && <CircleNotch size={12} className="animate-spin" />} Stop
                          </button>
                        )}
                        {confirmDestroyId === runtime.id ? (
                          <>
                            <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setConfirmDestroyId(null)}>Cancel</button>
                            <button type="button" className={DESTRUCTIVE_BUTTON_CLASS} disabled={busyId === runtime.id} onClick={() => void mutate(runtime.id, (token) => destroyHostedRuntime(token, runtime.id))}>
                              Permanently destroy
                            </button>
                          </>
                        ) : (
                          <button type="button" className={DESTRUCTIVE_BUTTON_CLASS} aria-label={`Destroy ${runtime.name}`} disabled={busyId === runtime.id} onClick={() => setConfirmDestroyId(runtime.id)}>
                            <Trash size={13} />
                          </button>
                        )}
                      </div>
                    </div>
                  ))}
                </div>
              )}

              {showCreate && entitlement && (
                <div className="rounded-lg border border-solid border-[var(--accent-primary)]/25 bg-[var(--accent-primary)]/[0.03] p-3 mb-3">
                  <div className="flex items-center justify-between gap-3 mb-3">
                    <div className="text-[12px] font-semibold text-[var(--text-primary)]">Create managed runtime</div>
                    <button type="button" className="p-1 border-none bg-transparent text-[var(--text-tertiary)] cursor-pointer" aria-label="Cancel runtime creation" onClick={() => setShowCreate(false)}><X size={14} /></button>
                  </div>
                  <div className="grid grid-cols-1 sm:grid-cols-3 gap-2">
                    <label className="sm:col-span-3 text-[10px] text-[var(--text-tertiary)]">
                      Name
                      <input
                        value={createName}
                        maxLength={80}
                        onChange={(event) => setCreateName(event.target.value)}
                        className="mt-1 w-full p-2 px-3 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] text-[12px] text-[var(--text-primary)] outline-none focus:border-[var(--accent-primary)]"
                      />
                    </label>
                    <label className="text-[10px] text-[var(--text-tertiary)]">
                      Region
                      <select value={createRegion} onChange={(event) => setCreateRegion(event.target.value)} className={cn(SETTINGS_SELECT_CLASS, "mt-1 w-full")}>
                        {allowedRegions.map((region) => <option key={region} value={region}>{region}</option>)}
                      </select>
                    </label>
                    <label className="text-[10px] text-[var(--text-tertiary)]">
                      Memory
                      <select value={createMemoryMb} onChange={(event) => setCreateMemoryMb(Number(event.target.value))} className={cn(SETTINGS_SELECT_CLASS, "mt-1 w-full")}>
                        {allowedMemory.map((memory) => <option key={memory} value={memory}>{formatMemory(memory)}</option>)}
                      </select>
                    </label>
                    <div className="flex items-end">
                      <button type="button" className={cn(QUIET_BUTTON_CLASS, "w-full justify-center")} disabled={busyId === "create" || !createName.trim()} onClick={() => void handleCreate()}>
                        {busyId === "create" ? <CircleNotch size={13} className="animate-spin" /> : <Plus size={13} />}
                        Create
                      </button>
                    </div>
                  </div>
                  <div className="flex items-center gap-1 mt-2 text-[10px] text-[var(--text-tertiary)]">
                    <MapPin size={11} /> Private Fly Machine · no public ports · auto-stop after {entitlement.idleTimeoutMinutes} minutes idle
                  </div>
                </div>
              )}

              <div className="flex items-center justify-end gap-2 flex-wrap">
                <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => void load()} disabled={loading}>
                  <ArrowsClockwise size={13} /> Refresh
                </button>
                {hasCapacity ? (
                  <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setShowCreate(true)} disabled={showCreate || busyId === "create"}>
                    <Plus size={13} /> Create runtime
                  </button>
                ) : (
                  <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => navigate("/billing")}>
                    View plans
                  </button>
                )}
                {safePortalUrl(entitlement?.billingPortalUrl) && (
                  <button
                    type="button"
                    className={QUIET_BUTTON_CLASS}
                    onClick={() => {
                      const url = safePortalUrl(entitlement?.billingPortalUrl);
                      if (url) window.open(url, "_blank", "noopener,noreferrer");
                    }}
                  >
                    Billing portal
                  </button>
                )}
              </div>
            </>
          )}
        </ProductCard>

        <ProductCard
          icon={<ShieldCheck size={18} />}
          eyebrow="Enterprise · metered"
          title="Bring your own cloud"
          description="Run isolated workloads in your AWS, Google Cloud, or Azure account. Your provider charges infrastructure; Allternit meters the enterprise platform service."
        >
          <div className="flex items-center justify-between gap-3">
            <span className="text-[10px] text-[var(--text-tertiary)]">Organization admin access required</span>
            <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => navigate("/organizations")}>Manage enterprise BYOC</button>
          </div>
        </ProductCard>
      </div>

      {error && (
        <EmptyState icon={<Cloud size={28} />} title="Hosted compute unavailable" caption={error} ctaLabel="Retry" onCtaClick={() => void load()} />
      )}
    </div>
  );
}

export default ComputeBillingPanel;
