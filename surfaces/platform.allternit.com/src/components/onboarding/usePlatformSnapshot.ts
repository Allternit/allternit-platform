import { useEffect, useState } from "react";
import { listGatewayKeys } from "@/lib/gateway-keys";
import { listApiKeys } from "@/lib/api-keys";
import { listProjects, isPlatformDisabled } from "@/lib/platform-projects";
import { fetchConsoleModels, type ConsoleModel } from "@/lib/console-models";
import { listRuntimeDevices } from "@/lib/devices";
import { listHostedRuntimes } from "@/lib/hosted-compute";
import { listRuns } from "@/lib/runs";
import { listSchedules } from "@/lib/schedules";
import { listWebhookSubscriptions } from "@/lib/webhooks";

/** A count the console loaded, or why it could not. */
export type Count =
  | { state: "loading" }
  | { state: "ok"; n: number; names?: string[] }
  | { state: "off"; note: string }
  | { state: "error" };

export interface PlatformSnapshot {
  projects: Count;
  gatewayKeys: Count;
  scopedKeys: Count;
  models: Count;
  modelList: ConsoleModel[];
  suppliers: Count;
  devices: Count;
  runtimes: Count;
  runs: Count;
  schedules: Count;
  webhooks: Count;
}

const LOADING: Count = { state: "loading" };

function initial(): PlatformSnapshot {
  return {
    projects: LOADING,
    gatewayKeys: LOADING,
    scopedKeys: LOADING,
    models: LOADING,
    modelList: [],
    suppliers: LOADING,
    devices: LOADING,
    runtimes: LOADING,
    runs: LOADING,
    schedules: LOADING,
    webhooks: LOADING,
  };
}

function settle<T>(p: Promise<T[]>, names?: (x: T) => string): Promise<Count> {
  return p.then(
    (list) => ({ state: "ok", n: list.length, names: names ? list.slice(0, 3).map(names) : undefined }) as Count,
    () => ({ state: "error" }) as Count,
  );
}

/**
 * Everything the connected view shows, read from the console's existing
 * clients. Every value is either a real count or an explicit "couldn't load".
 */
export function usePlatformSnapshot(
  enabled: boolean,
  getToken: () => Promise<string | null>,
  refreshKey: number,
): PlatformSnapshot {
  const [snap, setSnap] = useState<PlatformSnapshot>(initial);

  useEffect(() => {
    if (!enabled) return;
    let active = true;
    const put = (patch: Partial<PlatformSnapshot>) => {
      if (active) setSnap((s) => ({ ...s, ...patch }));
    };
    setSnap(initial());

    listProjects().then(
      (list) => put({ projects: { state: "ok", n: list.length, names: list.slice(0, 3).map((p) => p.name) } }),
      (err) =>
        put({
          projects: isPlatformDisabled(err)
            ? { state: "off", note: "Not enabled on this deployment" }
            : { state: "error" },
        }),
    );
    void settle(listGatewayKeys()).then((c) => put({ gatewayKeys: c }));
    void settle(listApiKeys().then((ks) => ks.filter((k) => !k.revokedAt))).then((c) => put({ scopedKeys: c }));
    fetchConsoleModels().then(
      (list) => {
        const owners = Array.from(new Set(list.map((m) => m.owned_by).filter((o) => o && o !== "unknown")));
        put({
          modelList: list,
          models: { state: "ok", n: list.length, names: list.slice(0, 3).map((m) => m.id) },
          suppliers: { state: "ok", n: owners.length, names: owners.slice(0, 3) },
        });
      },
      () => put({ models: { state: "error" }, suppliers: { state: "error" } }),
    );
    void settle(listRuntimeDevices().then((ds) => ds.filter((d) => d.status !== "revoked"))).then((c) =>
      put({ devices: c }),
    );
    getToken()
      .then((token) => {
        if (!token) throw new Error("no session");
        return listHostedRuntimes(token);
      })
      .then(
        (list) => put({ runtimes: { state: "ok", n: list.length } }),
        () => put({ runtimes: { state: "error" } }),
      );
    void settle(listRuns()).then((c) => put({ runs: c }));
    void settle(listSchedules()).then((c) => put({ schedules: c }));
    void settle(listWebhookSubscriptions()).then((c) => put({ webhooks: c }));

    return () => {
      active = false;
    };
  }, [enabled, getToken, refreshKey]);

  return snap;
}

export function countText(c: Count, noun: string, plural = `${noun}s`): string {
  switch (c.state) {
    case "loading":
      return "Loading…";
    case "error":
      return "Couldn't load";
    case "off":
      return c.note;
    case "ok":
      return c.n === 0 ? `No ${plural} yet` : `${c.n} ${c.n === 1 ? noun : plural}`;
  }
}

export function hasAny(c: Count): boolean {
  return c.state === "ok" && c.n > 0;
}
