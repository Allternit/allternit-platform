import React, { useCallback, useEffect, useState } from "react";
import {
  Desktop,
  HardDrives,
  Cloud,
  Trash,
  CheckCircle,
  X,
  CircleNotch,
  ArrowsClockwise,
  ShieldCheck,
  WarningCircle,
} from "@phosphor-icons/react";
import { cn } from "@/lib/utils";
import {
  type RuntimeDevice,
  type PairingRequest,
  listRuntimeDevices,
  revokeRuntimeDevice,
  getPairingInfo,
  approvePairing,
  denyPairing,
} from "@/lib/devices";
import { EmptyState } from "@/components/settings/EmptyState";
import { SkeletonRow } from "@/components/settings/SkeletonRow";
import { Badge } from "@/components/settings/Badge";
import { QUIET_BUTTON_CLASS, DESTRUCTIVE_BUTTON_CLASS } from "@/components/settings/buttonStyles";
import { formatApiError } from "@/lib/api-client";

function formatLastSeen(iso?: string | null): string {
  if (!iso) return "Never";
  const date = new Date(iso);
  const diff = Date.now() - date.getTime();
  if (diff < 60_000) return "Just now";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} min ago`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} hr ago`;
  return date.toLocaleDateString();
}

function deviceIcon(runtimeType: string) {
  if (runtimeType === "hosted") return <Cloud size={18} />;
  if (runtimeType === "vps") return <HardDrives size={18} />;
  return <Desktop size={18} />;
}

export function DevicesPage() {
  const [devices, setDevices] = useState<RuntimeDevice[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [confirmId, setConfirmId] = useState<string | null>(null);

  const [pairingCode, setPairingCode] = useState("");
  const [pairingInfo, setPairingInfo] = useState<PairingRequest | null>(null);
  const [pairingLoading, setPairingLoading] = useState(false);
  const [pairingError, setPairingError] = useState<string | null>(null);
  const [pairingBusy, setPairingBusy] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const runtimes = await listRuntimeDevices();
      setDevices(runtimes);
    } catch (err) {
      setError(formatApiError(err, "Unable to load devices"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const handleRevoke = useCallback(async (id: string) => {
    setBusyId(id);
    setError(null);
    try {
      await revokeRuntimeDevice(id);
      setConfirmId(null);
      await load();
    } catch (err) {
      setError(formatApiError(err, "Unable to revoke device"));
    } finally {
      setBusyId(null);
    }
  }, [load]);

  const lookupPairing = useCallback(async () => {
    const code = pairingCode.trim().replace(/-/g, "").toUpperCase();
    if (code.length < 4) {
      setPairingError("Enter a pairing code");
      return;
    }
    setPairingLoading(true);
    setPairingError(null);
    setPairingInfo(null);
    try {
      const info = await getPairingInfo(code);
      setPairingInfo(info);
    } catch (err) {
      setPairingError(formatApiError(err, "Pairing request not found"));
    } finally {
      setPairingLoading(false);
    }
  }, [pairingCode]);

  const handleApprove = useCallback(async () => {
    const code = pairingCode.trim().replace(/-/g, "").toUpperCase();
    if (!code) return;
    setPairingBusy(true);
    setPairingError(null);
    try {
      await approvePairing(code);
      setPairingInfo(null);
      setPairingCode("");
      await load();
    } catch (err) {
      setPairingError(formatApiError(err, "Unable to approve pairing"));
    } finally {
      setPairingBusy(false);
    }
  }, [pairingCode, load]);

  const handleDeny = useCallback(async () => {
    const code = pairingCode.trim().replace(/-/g, "").toUpperCase();
    if (!code) return;
    setPairingBusy(true);
    setPairingError(null);
    try {
      await denyPairing(code);
      setPairingInfo(null);
      setPairingCode("");
    } catch (err) {
      setPairingError(formatApiError(err, "Unable to deny pairing"));
    } finally {
      setPairingBusy(false);
    }
  }, [pairingCode]);

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-[22px] font-semibold tracking-tight text-[var(--text-primary)]">
          Devices
        </h1>
        <p className="text-[13px] text-[var(--text-secondary)] mt-1">
          Manage paired desktops, VPS runtimes, and hosted machines.
        </p>
      </div>

      <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4">
        <div className="flex items-start gap-3">
          <div className="size-9 shrink-0 rounded-lg bg-[var(--accent-primary)]/10 text-[var(--accent-primary)] flex items-center justify-center">
            <ShieldCheck size={18} />
          </div>
          <div className="min-w-0 flex-1">
            <div className="text-[14px] font-semibold text-[var(--text-primary)]">Approve a pairing</div>
            <p className="text-[12px] text-[var(--text-secondary)] mt-1 mb-3">
              Enter the code shown on the runtime you want to pair.
            </p>
            <div className="flex flex-col sm:flex-row gap-2">
              <input
                value={pairingCode}
                onChange={(e) => setPairingCode(e.target.value)}
                placeholder="ABCD-1234"
                maxLength={12}
                className="flex-1 min-w-0 p-2 px-3 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] text-[13px] text-[var(--text-primary)] outline-none focus:border-[var(--accent-primary)] placeholder:text-[var(--text-tertiary)]"
              />
              <button
                type="button"
                onClick={() => void lookupPairing()}
                disabled={pairingLoading}
                className={cn(QUIET_BUTTON_CLASS, "justify-center")}
              >
                {pairingLoading && <CircleNotch size={13} className="animate-spin" />}
                Look up
              </button>
            </div>
            {pairingError && !pairingInfo && (
              <div className="mt-2 text-[12px] text-[var(--status-error)]">{pairingError}</div>
            )}
            {pairingInfo && (
              <div className="mt-3 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] p-3">
                <div className="flex items-center justify-between gap-3">
                  <div>
                    <div className="text-[13px] font-semibold text-[var(--text-primary)]">{pairingInfo.name}</div>
                    <div className="text-[11px] text-[var(--text-secondary)] mt-0.5">
                      {pairingInfo.runtimeType} · {pairingInfo.platform || "unknown platform"}
                    </div>
                  </div>
                  <Badge>{pairingInfo.status}</Badge>
                </div>
                <div className="flex items-center gap-2 mt-3">
                  <button
                    type="button"
                    onClick={() => void handleApprove()}
                    disabled={pairingBusy}
                    className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-[12px] font-medium bg-[var(--status-success)]/10 text-[var(--status-success)] hover:bg-[var(--status-success)]/20 transition-colors"
                  >
                    {pairingBusy && <CircleNotch size={12} className="animate-spin" />}
                    <CheckCircle size={13} /> Approve
                  </button>
                  <button
                    type="button"
                    onClick={() => void handleDeny()}
                    disabled={pairingBusy}
                    className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-[12px] font-medium text-[var(--text-secondary)] hover:bg-[var(--surface-hover)] transition-colors"
                  >
                    <X size={13} /> Deny
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>
      </div>

      {loading ? (
        <SkeletonRow lines={4} />
      ) : error ? (
        <EmptyState
          icon={<WarningCircle size={28} />}
          title="Devices unavailable"
          caption={error}
          ctaLabel="Retry"
          onCtaClick={() => void load()}
        />
      ) : devices.length === 0 ? (
        <EmptyState
          art="connect-scene"
          title="No paired devices"
          caption="Pair a desktop, VPS, or hosted runtime to see it here. Use the pairing code shown in the app."
          ctaLabel="Refresh"
          onCtaClick={() => void load()}
        />
      ) : (
        <div className="space-y-2">
          {devices.map((device) => (
            <div
              key={device.id}
              className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4"
            >
              <div className="flex items-start gap-3">
                <div className="size-9 shrink-0 rounded-lg bg-[var(--accent-primary)]/10 text-[var(--accent-primary)] flex items-center justify-center">
                  {deviceIcon(device.runtimeType)}
                </div>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2 flex-wrap">
                    <span className="text-[14px] font-semibold text-[var(--text-primary)]">
                      {device.name}
                    </span>
                    <Badge
                      className={cn(
                        device.status === "online" && "text-[var(--status-success)] bg-[var(--status-success)]/10",
                        device.status === "offline" && "text-[var(--text-tertiary)] bg-[var(--bg-secondary)]",
                        device.status === "revoked" && "text-[var(--status-error)] bg-[var(--status-error)]/10"
                      )}
                    >
                      {device.status}
                    </Badge>
                  </div>
                  <div className="text-[11px] text-[var(--text-tertiary)] mt-1">
                    {device.platform || "Unknown platform"}
                    {device.hostname ? ` · ${device.hostname}` : ""}
                    {device.version ? ` · v${device.version}` : ""}
                  </div>
                  <div className="flex flex-wrap gap-1.5 mt-2">
                    {device.capabilities.slice(0, 6).map((capability) => (
                      <span
                        key={capability}
                        className="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium bg-[var(--bg-primary)] text-[var(--text-secondary)] border border-[var(--border-subtle)]"
                      >
                        {capability}
                      </span>
                    ))}
                    {device.capabilities.length > 6 && (
                      <span className="text-[10px] text-[var(--text-tertiary)] self-center">
                        +{device.capabilities.length - 6} more
                      </span>
                    )}
                  </div>
                  <div className="flex items-center justify-between gap-3 mt-3 pt-3 border-t border-[var(--border-subtle)]">
                    <span className="text-[11px] text-[var(--text-tertiary)]">
                      Last seen {formatLastSeen(device.lastSeenAt)}
                    </span>
                    {confirmId === device.id ? (
                      <div className="flex items-center gap-2">
                        <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setConfirmId(null)}>
                          Cancel
                        </button>
                        <button
                          type="button"
                          className={DESTRUCTIVE_BUTTON_CLASS}
                          disabled={busyId === device.id}
                          onClick={() => void handleRevoke(device.id)}
                        >
                          {busyId === device.id && <CircleNotch size={12} className="animate-spin" />}
                          Revoke
                        </button>
                      </div>
                    ) : (
                      <button
                        type="button"
                        className={DESTRUCTIVE_BUTTON_CLASS}
                        onClick={() => setConfirmId(device.id)}
                      >
                        <Trash size={13} /> Revoke
                      </button>
                    )}
                  </div>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}

      {!loading && (
        <div className="flex justify-end">
          <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => void load()} disabled={loading}>
            <ArrowsClockwise size={13} /> Refresh
          </button>
        </div>
      )}
    </div>
  );
}
