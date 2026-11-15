import React, { useCallback, useEffect, useState } from "react";
import {
  HardDrives,
  Key,
  Receipt,
  CircleNotch,
  ArrowsClockwise,
  WarningCircle,
  CheckCircle,
} from "@phosphor-icons/react";
import { cn } from "@/lib/utils";
import { Link } from "react-router-dom";
import { Coins } from "@phosphor-icons/react";
import { formatCreditsUsd, getOrgCreditsBalance, type OrgCreditsBalance } from "@/lib/credits";
import {
  type NodeRecord,
  type LeaseRecord,
  type ChargeEvent,
  osApi,
  formatOSApiError,
  isOSConfigured,
} from "@/lib/allternitos";
import { EmptyState } from "@/components/settings/EmptyState";
import { SkeletonRow } from "@/components/settings/SkeletonRow";
import { Badge } from "@/components/settings/Badge";
import { QUIET_BUTTON_CLASS } from "@/components/settings/buttonStyles";

function formatLastSeen(iso?: string | null): string {
  if (!iso) return "Never";
  const date = new Date(iso);
  const diff = Date.now() - date.getTime();
  if (diff < 60_000) return "Just now";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} min ago`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} hr ago`;
  return date.toLocaleDateString();
}

/** Organization compute-credits balance (fabric ledger) with a top-up link. */
function OrgCreditsCard() {
  const [balance, setBalance] = useState<OrgCreditsBalance | null>(null);
  const [unavailable, setUnavailable] = useState(false);

  useEffect(() => {
    let active = true;
    getOrgCreditsBalance()
      .then((result) => {
        if (!active) return;
        if (result === null) setUnavailable(true);
        else setBalance(result);
      })
      .catch(() => {
        if (active) setUnavailable(true);
      });
    return () => {
      active = false;
    };
  }, []);

  if (unavailable || !balance) return null;

  return (
    <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4">
      <div className="flex flex-wrap items-center gap-4">
        <div className="flex items-center gap-3 min-w-0">
          <div className="size-9 shrink-0 rounded-lg bg-[var(--accent-primary)]/10 text-[var(--accent-primary)] flex items-center justify-center">
            <Coins size={18} />
          </div>
          <div>
            <div className="text-[14px] font-semibold text-[var(--text-primary)]">
              Organization compute credits
            </div>
            <div className="text-[12px] text-[var(--text-secondary)]">
              Provisioning holds and metered usage charge this balance.
            </div>
          </div>
        </div>
        <div className="flex items-center gap-4 ml-auto">
          <div className="text-right">
            <div className="text-[10px] uppercase tracking-wide text-[var(--text-tertiary)]">Balance</div>
            <div className="text-[16px] font-semibold font-mono text-[var(--text-primary)]">
              {formatCreditsUsd(balance.balance_cents / 100)}
            </div>
          </div>
          <div className="text-right">
            <div className="text-[10px] uppercase tracking-wide text-[var(--text-tertiary)]">Available</div>
            <div className="text-[16px] font-semibold font-mono text-[var(--text-primary)]">
              {formatCreditsUsd(balance.available_cents / 100)}
            </div>
          </div>
          {balance.held_cents > 0 && (
            <div className="text-right">
              <div className="text-[10px] uppercase tracking-wide text-[var(--text-tertiary)]">Held</div>
              <div className="text-[16px] font-semibold font-mono text-[var(--text-primary)]">
                {formatCreditsUsd(balance.held_cents / 100)}
              </div>
            </div>
          )}
          <Link
            to="/billing"
            className={QUIET_BUTTON_CLASS}
          >
            Top up
          </Link>
        </div>
      </div>
    </div>
  );
}

export function FabricPage() {
  const [nodes, setNodes] = useState<NodeRecord[]>([]);
  const [leases, setLeases] = useState<LeaseRecord[]>([]);
  const [chargeEvents, setChargeEvents] = useState<ChargeEvent[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [ackBusy, setAckBusy] = useState<string | null>(null);

  const configured = isOSConfigured();

  const load = useCallback(async () => {
    if (!configured) {
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const [nodeResult, leaseResult, events] = await Promise.all([
        osApi.listNodes(),
        osApi.listLeases(),
        osApi.queryChargeEvents(),
      ]);
      setNodes(nodeResult.nodes || []);
      setLeases(leaseResult.leases || []);
      setChargeEvents(events || []);
    } catch (err) {
      setError(formatOSApiError(err, "Unable to reach AllternitOS control plane"));
    } finally {
      setLoading(false);
    }
  }, [configured]);

  useEffect(() => {
    void load();
  }, [load]);

  const handleAck = useCallback(async (idempotencyKey: string) => {
    setAckBusy(idempotencyKey);
    try {
      await osApi.acknowledgeChargeEvents([idempotencyKey]);
      setChargeEvents((prev) =>
        prev.map((ev) =>
          ev.idempotency_key === idempotencyKey
            ? { ...ev, acknowledged_at: new Date().toISOString() }
            : ev
        )
      );
    } catch (err) {
      setError(formatOSApiError(err, "Unable to acknowledge charge event"));
    } finally {
      setAckBusy(null);
    }
  }, []);

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-[22px] font-semibold tracking-tight text-[var(--text-primary)]">
          Fabric
        </h1>
        <p className="text-[13px] text-[var(--text-secondary)] mt-1">
          Inspect AllternitOS nodes, capability leases, and charge events.
        </p>
      </div>

      <OrgCreditsCard />

      {!configured && (
        <EmptyState
          icon={<HardDrives size={32} weight="thin" />}
          title="Fabric is not available"
          caption="This console is not connected to an AllternitOS control plane."
        />
      )}

      {error && (
        <div className="rounded-xl border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 p-3 text-[13px] text-[var(--status-error)]">
          {error}
        </div>
      )}

      {!configured ? null : loading ? (
        <SkeletonRow lines={6} />
      ) : (
        <>
          <section className="space-y-3">
            <div className="flex items-center gap-2">
              <HardDrives size={18} className="text-[var(--accent-primary)]" />
              <h2 className="text-[14px] font-semibold text-[var(--text-primary)]">
                Nodes ({nodes.length})
              </h2>
            </div>
            {nodes.length === 0 ? (
              <EmptyState
                icon={<HardDrives size={32} weight="thin" />}
                title="No nodes enrolled"
                caption="Start allternitosd on a machine and enroll it with the control plane."
              />
            ) : (
              <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
                {nodes.map((node) => (
                  <div
                    key={node.node_id}
                    className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4"
                  >
                    <div className="flex items-center justify-between gap-2">
                      <span className="text-[13px] font-semibold text-[var(--text-primary)] truncate font-mono">
                        {node.node_id}
                      </span>
                      <Badge
                        className={cn(
                          node.health === "healthy" &&
                            "text-[var(--status-success)] bg-[var(--status-success)]/10",
                          node.health !== "healthy" &&
                            "text-[var(--status-warning)] bg-[var(--status-warning)]/10"
                        )}
                      >
                        {node.health || "unknown"}
                      </Badge>
                    </div>
                    <div className="text-[11px] text-[var(--text-tertiary)] mt-1">
                      {node.region || "—"} · {node.zone || "—"} · {node.rack || "—"}
                    </div>
                    <div className="text-[11px] text-[var(--text-tertiary)] mt-1">
                      Last seen {formatLastSeen(node.last_heartbeat_at)}
                    </div>
                    <div className="flex flex-wrap gap-1.5 mt-3">
                      {node.capability_record?.workers?.capabilities?.map((cap) => (
                        <span
                          key={cap.capability}
                          className="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium bg-[var(--bg-primary)] text-[var(--text-secondary)] border border-[var(--border-subtle)]"
                          title={cap.actions.join(", ")}
                        >
                          {cap.capability}
                        </span>
                      ))}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </section>

          <section className="space-y-3">
            <div className="flex items-center gap-2">
              <Key size={18} className="text-[var(--accent-primary)]" />
              <h2 className="text-[14px] font-semibold text-[var(--text-primary)]">
                Leases ({leases.length})
              </h2>
            </div>
            {leases.length === 0 ? (
              <EmptyState
                icon={<Key size={32} weight="thin" />}
                title="No leases"
                caption="Capability leases issued by workloads will appear here."
              />
            ) : (
              <div className="space-y-2">
                {leases.map((lease) => (
                  <div
                    key={lease.lease_id}
                    className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4"
                  >
                    <div className="flex items-center justify-between gap-2">
                      <span className="text-[13px] font-semibold text-[var(--text-primary)] font-mono">
                        {lease.lease_id}
                      </span>
                      <Badge
                        className={cn(
                          lease.state === "active" &&
                            "text-[var(--accent-primary)] bg-[var(--accent-primary)]/10",
                          lease.state === "revoked" &&
                            "text-[var(--text-tertiary)] bg-[var(--bg-secondary)]"
                        )}
                      >
                        {lease.state}
                      </Badge>
                    </div>
                    <div className="text-[12px] text-[var(--text-secondary)] mt-1">
                      {lease.capability} → {lease.node_id || "unassigned"}
                    </div>
                    <div className="text-[11px] text-[var(--text-tertiary)] mt-1">
                      Requester {lease.requester_principal_id} · Expires{" "}
                      {new Date(lease.not_after).toLocaleString()}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </section>

          <section className="space-y-3">
            <div className="flex items-center gap-2">
              <Receipt size={18} className="text-[var(--accent-primary)]" />
              <h2 className="text-[14px] font-semibold text-[var(--text-primary)]">
                Charge Events ({chargeEvents.length})
              </h2>
            </div>
            {chargeEvents.length === 0 ? (
              <EmptyState
                icon={<Receipt size={32} weight="thin" />}
                title="No charge events"
                caption="Usage events reconciled by the control plane will appear here."
              />
            ) : (
              <div className="space-y-2">
                {chargeEvents.map((ev) => (
                  <div
                    key={ev.id}
                    className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40 p-4"
                  >
                    <div className="flex items-center justify-between gap-2">
                      <span className="text-[13px] font-semibold text-[var(--text-primary)] font-mono">
                        {ev.event_type}
                      </span>
                      <Badge
                        className={cn(
                          ev.acknowledged_at
                            ? "text-[var(--status-success)] bg-[var(--status-success)]/10"
                            : "text-[var(--status-warning)] bg-[var(--status-warning)]/10"
                        )}
                      >
                        {ev.acknowledged_at ? "acknowledged" : "unacknowledged"}
                      </Badge>
                    </div>
                    <div className="text-[12px] text-[var(--text-secondary)] mt-1">
                      {ev.quantity} {ev.unit} · {ev.amount} {ev.currency} · {ev.resource_id}
                    </div>
                    <div className="text-[11px] text-[var(--text-tertiary)] mt-1">
                      Measured {new Date(ev.measured_at).toLocaleString()}
                    </div>
                    {!ev.acknowledged_at && (
                      <button
                        type="button"
                        onClick={() => void handleAck(ev.idempotency_key)}
                        disabled={ackBusy === ev.idempotency_key}
                        className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-solid border-[var(--accent-primary)]/30 bg-[var(--accent-primary)]/10 text-[13px] font-medium text-[var(--accent-primary)] cursor-pointer hover:bg-[var(--accent-primary)]/20 transition-colors disabled:opacity-50 disabled:cursor-not-allowed mt-3"
                      >
                        {ackBusy === ev.idempotency_key && (
                          <CircleNotch size={12} className="animate-spin" />
                        )}
                        <CheckCircle size={13} /> Acknowledge
                      </button>
                    )}
                  </div>
                ))}
              </div>
            )}
          </section>

          <div className="flex justify-end">
            <button
              type="button"
              className={QUIET_BUTTON_CLASS}
              onClick={() => void load()}
              disabled={loading}
            >
              {loading && <CircleNotch size={13} className="animate-spin" />}
              <ArrowsClockwise size={13} /> Refresh
            </button>
          </div>
        </>
      )}
    </div>
  );
}
