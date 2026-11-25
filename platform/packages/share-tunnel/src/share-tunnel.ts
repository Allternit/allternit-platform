import { homedir } from "node:os";
import { join } from "node:path";
import {
  create as createOpenTunnelClient,
  OpenTunnelStorage,
  type OpenTunnelClientEvent,
  type OpenTunnelPromiseClient,
} from "@opentunnel/client";
import {
  DENIED_PORTS,
  ShareTunnelError,
  type LiveShare,
  type LiveShareStatus,
  type GateShare,
  type ShareGate,
  type ShareTunnelEvent,
  type StartShareInput,
  type StopReason,
} from "./types.js";

/** The part of the OpenTunnel SDK client this package uses (injectable for tests). */
export type TunnelClient = {
  readonly tunnel: Pick<OpenTunnelPromiseClient["tunnel"], "get" | "connect" | "remove">;
  readonly dispose: () => Promise<void>;
};

export interface ShareTunnelOptions {
  /** OpenTunnel relay API, e.g. `https://api.live.allternitusercontent.com`. */
  relayApi: string;
  /** cloud-api origin for heartbeats, e.g. `https://api.allternit.com`. */
  cloudApi: string;
  /** The caller's cloud-api bearer (Clerk session, device token, gizzi session). */
  getAuthToken: () => Promise<string>;
  gate: ShareGate;
  /** Where tunnel identities live; default `~/.allternit/share-tunnel` (OpenTunnel layout, one profile per link). */
  dataDir?: string;
  /** Default 60 s. */
  heartbeatMs?: number;
  /** Ports refused on top of `DENIED_PORTS`. */
  extraDeniedPorts?: readonly number[];
  onEvent?: (event: ShareTunnelEvent) => void;
  fetch?: typeof fetch;
  /** Builds the SDK client for an API URL; tests pass a fake. */
  openTunnel?: (api: string) => TunnelClient;
}

export interface ShareTunnel {
  start(input: StartShareInput): Promise<LiveShare>;
  /** Live links started by this instance and not yet stopped. */
  list(): LiveShare[];
  get(linkId: string): LiveShare | undefined;
  stopAll(reason?: StopReason): Promise<void>;
}

/** The SDK's API base with the admission credential as a path prefix (relay patch 0001). */
export function admissionApi(relayApi: string, credential: string): string {
  return `${relayApi.replace(/\/+$/, "")}/admit/${encodeURIComponent(credential)}`;
}

export function createShareTunnel(options: ShareTunnelOptions): ShareTunnel {
  const doFetch = options.fetch ?? fetch;
  const denied = new Set([...DENIED_PORTS, ...(options.extraDeniedPorts ?? [])]);
  const heartbeatMs = options.heartbeatMs ?? 60_000;
  const emit = (event: ShareTunnelEvent) => {
    try {
      options.onEvent?.(event);
    } catch {
      // a listener's error never stops a share
    }
  };
  const openTunnel =
    options.openTunnel ??
    ((api: string): TunnelClient => {
      const dataDir = options.dataDir ?? join(homedir(), ".allternit", "share-tunnel");
      return createOpenTunnelClient({
        api,
        store: OpenTunnelStorage.xdg({ env: { ...process.env, XDG_DATA_HOME: dataDir } }),
      });
    });
  const shares = new Map<string, LiveShare>();

  async function heartbeat(linkId: string): Promise<{ stop?: StopReason; status?: number; error?: string }> {
    const response = await doFetch(
      `${options.cloudApi.replace(/\/+$/, "")}/api/v1/share-links/${encodeURIComponent(linkId)}/heartbeat`,
      { method: "POST", headers: { authorization: `Bearer ${await options.getAuthToken()}` } },
    );
    const body = (await response.json().catch(() => ({}))) as { error?: string; status?: string };
    if (response.status === 410) return { stop: body.error === "revoked" ? "revoked" : "killed", status: 410 };
    if (response.status === 404) return { stop: "not_found", status: 404 };
    if (!response.ok) return { status: response.status, error: body.error ?? `HTTP ${response.status}` };
    if (body.status === "paused") return { stop: "paused", status: response.status };
    if (body.status === "revoked" || body.status === "killed") return { stop: body.status, status: response.status };
    return { status: response.status };
  }

  async function start(input: StartShareInput): Promise<LiveShare> {
    const { link, port } = input;
    if (!Number.isInteger(port) || port < 1 || port > 65_535) {
      throw new ShareTunnelError("bad_port", `Port ${port} is not a TCP port.`);
    }
    if (denied.has(port)) {
      throw new ShareTunnelError("port_denied", `Port ${port} runs Allternit itself and can't be shared.`);
    }
    if (shares.has(link.id)) return shares.get(link.id)!;
    const profile = link.subdomain;

    // A saved identity needs no credential; a first start does.
    let client = openTunnel(options.relayApi);
    const saved = await client.tunnel.get({ profile }).catch(() => undefined);
    if (!saved) {
      await client.dispose();
      const credential = typeof input.admission === "function" ? await input.admission() : input.admission;
      if (!credential) {
        throw new ShareTunnelError("admission_required", "This link needs a relay credential from cloud-api.");
      }
      client = openTunnel(admissionApi(options.relayApi, credential));
    }

    let gateShare: GateShare;
    try {
      gateShare = await options.gate.open({
        linkId: link.id,
        port,
        authMode: input.authMode ?? "token",
        ...(input.subject ? { subject: input.subject } : {}),
        ...(input.label ? { label: input.label } : {}),
      });
    } catch (error) {
      await client.dispose();
      throw error instanceof ShareTunnelError
        ? error
        : new ShareTunnelError("gate_failed", "The share gate did not open.", error);
    }

    const status: LiveShareStatus = { state: "connected", connections: 0 };
    const onEvent = (event: OpenTunnelClientEvent) => {
      if (event.type === "connection-opened") status.connections += 1;
      else if (event.type === "connection-closed") status.connections = Math.max(0, status.connections - 1);
      else if (event.type === "reconnecting") {
        status.state = "reconnecting";
        emit({ type: "reconnecting", linkId: link.id, attempt: event.attempt });
      } else if (event.type === "connected") {
        status.state = "connected";
        emit({ type: "connected", linkId: link.id });
      } else if (event.type === "disconnected") status.lastError = event.reason;
    };

    let connection: Awaited<ReturnType<TunnelClient["tunnel"]["connect"]>>;
    try {
      connection = await client.tunnel.connect({
        profile,
        routes: { "@": gateShare.target },
        onRouteConflict: "fail",
        onEvent,
      });
    } catch (error) {
      await options.gate.close(gateShare.id).catch(() => undefined);
      await client.dispose();
      const message = error instanceof Error ? error.message : String(error);
      throw new ShareTunnelError(
        /admission refused/i.test(message) ? "admission_refused" : "relay_failed",
        message,
        error,
      );
    }
    if (connection.tunnel.id !== link.subdomain) {
      await connection.close().catch(() => undefined);
      await options.gate.close(gateShare.id).catch(() => undefined);
      await client.dispose();
      throw new ShareTunnelError(
        "name_mismatch",
        `Relay named the tunnel ${connection.tunnel.id}, expected ${link.subdomain}.`,
      );
    }

    let resolveStopped!: (reason: StopReason) => void;
    const stopped = new Promise<StopReason>((resolve) => (resolveStopped = resolve));
    let stopping: Promise<StopReason> | undefined;
    let timer: ReturnType<typeof setInterval> | undefined;

    const stop = (reason: StopReason = "stopped", error?: string): Promise<StopReason> => {
      if (stopping) return stopping;
      stopping = (async () => {
        if (timer) clearInterval(timer);
        status.state = "stopped";
        status.stopReason = reason;
        if (error) status.lastError = error;
        shares.delete(link.id);
        await connection.close().catch(() => undefined);
        await options.gate.close(gateShare.id).catch(() => undefined);
        if (reason === "killed" || reason === "revoked" || reason === "not_found") {
          // The relay tunnel is gone or must go: drop the local identity too.
          await client.tunnel.remove({ profile }).catch(() => undefined);
        }
        await client.dispose().catch(() => undefined);
        emit({ type: "stopped", linkId: link.id, reason, ...(error ? { error } : {}) });
        resolveStopped(reason);
        return reason;
      })();
      return stopping;
    };

    const beat = async () => {
      try {
        const result = await heartbeat(link.id);
        status.lastHeartbeatAt = Date.now();
        emit({
          type: "heartbeat",
          linkId: link.id,
          ok: !result.error,
          ...(result.status ? { status: result.status } : {}),
          ...(result.error ? { error: result.error } : {}),
        });
        if (result.stop) await stop(result.stop);
      } catch (error) {
        // Offline or cloud-api down: keep serving, try again next beat.
        emit({ type: "heartbeat", linkId: link.id, ok: false, error: String(error) });
      }
    };
    timer = setInterval(() => void beat(), heartbeatMs);
    timer.unref?.();
    connection.closed.then(
      () => void stop("relay_closed"),
      (error: unknown) => void stop("error", error instanceof Error ? error.message : String(error)),
    );

    const share: LiveShare = {
      linkId: link.id,
      subdomain: link.subdomain,
      url: `https://${connection.tunnel.hostname}`,
      port,
      get token() {
        return gateShare.token;
      },
      async pause(paused = true) {
        if (!options.gate.pause) throw new ShareTunnelError("gate_failed", "This share gate cannot pause.");
        await options.gate.pause(gateShare.id, paused);
      },
      async rotate() {
        if (!options.gate.rotate) throw new ShareTunnelError("gate_failed", "This share gate cannot rotate.");
        gateShare = { ...gateShare, token: await options.gate.rotate(gateShare.id) };
        return gateShare.token;
      },
      status: () => ({ ...status }),
      stop: (reason) => stop(reason ?? "stopped"),
      stopped,
    };
    shares.set(link.id, share);
    emit({ type: "started", linkId: link.id, url: share.url });
    void beat();
    return share;
  }

  return {
    start,
    list: () => [...shares.values()],
    get: (linkId) => shares.get(linkId),
    stopAll: async (reason = "stopped") => {
      await Promise.all([...shares.values()].map((share) => share.stop(reason)));
    },
  };
}
