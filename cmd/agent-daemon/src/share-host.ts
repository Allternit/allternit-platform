import { randomBytes } from 'node:crypto';
import { createServer, type IncomingMessage, type Server } from 'node:http';
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import {
  createShareTunnel,
  httpShareGate,
  ShareTunnelError,
  type AuthMode,
  type LiveShare,
  type ShareTunnel,
  type StopReason,
} from '@allternit/share-tunnel';

/**
 * Live-link host for a cloud computer. The daemon holds the cloud-api device token; gizzi/allternit-api on the
 * same box ask for a link over a loopback-only control API. Active link ids are saved in `dataDir`, so a daemon
 * restart re-admits each one with a fresh relay credential (cloud-api `POST /share-links/:id/relay-credential`).
 *
 *   POST   /share-host/links           {linkId, subdomain, port, authMode?, admission?} -> {id, url, token, port}
 *   GET    /share-host/links           -> {links: [{id, url, port, state, connections}]}
 *   POST   /share-host/links/:id/pause {paused?}
 *   DELETE /share-host/links/:id       (also POST .../stop)
 *
 * `startShare` is the relay-forwarded host command `POST /host/shares/start` (index.ts routes it here; it is not an
 * allternit-api route). Two vocabularies meet here: cloud-api share-links say subject port|artifact and auth_mode
 * public|signed_in; the local allternit-api gate (packages/share-tunnel) says subject app|mcp and auth_mode token|public.
 */
export type ShareHostOptions = {
  cloudApi: string;
  relayApi: string;
  gatewayUrl: string;
  /** Reads the current device token (it rotates). */
  getAuthToken: () => Promise<string>;
  /** This computer's runtime device id (cloud-api `runtime_device_id` on live links). */
  getRuntimeDeviceId?: () => string | undefined;
  dataDir: string;
  controlPort: number;
  log?: (message: string) => void;
};

type SavedLink = { id: string; subdomain: string; port: number; authMode: AuthMode };

/** Heartbeat answers that mean cloud-api no longer wants this link (410 killed / revoked / unknown). */
const DROP_REASONS: ReadonlySet<StopReason> = new Set(['killed', 'revoked', 'not_found', 'stopped', 'paused']);
const RETRY_MS = 60_000;

export type StartShareResult = { status: number; body: Record<string, unknown> };

export function startShareHost(options: ShareHostOptions): {
  stop(): Promise<void>;
  startShare(input: { port?: unknown; label?: unknown; artifact_id?: unknown }): Promise<StartShareResult>;
} {
  const log = options.log ?? ((m: string) => console.log(`[ShareHost] ${m}`));
  const file = join(options.dataDir, 'active-links.json');
  const saved = new Map<string, SavedLink>();
  let shuttingDown = false;
  let retry: ReturnType<typeof setTimeout> | undefined;

  const persist = async () => {
    await mkdir(dirname(file), { recursive: true, mode: 0o700 });
    const tmp = `${file}.tmp`;
    await writeFile(tmp, JSON.stringify([...saved.values()], null, 2), { mode: 0o600 });
    await rename(tmp, file);
  };
  const forget = (id: string) => {
    if (saved.delete(id)) void persist().catch(() => undefined);
  };

  // The last answer the local gate gave to a failed call, so startShare can answer 403/409/429 like the gate did.
  let gateFailure: { status: number; error?: string; message?: string } | undefined;
  const gateFetch: typeof fetch = async (input, init) => {
    const response = await fetch(input, init);
    if (!response.ok) {
      const body = (await response.clone().json().catch(() => ({}))) as { error?: string; message?: string };
      gateFailure = { status: response.status, error: body.error, message: body.message };
    }
    return response;
  };

  const tunnel: ShareTunnel = createShareTunnel({
    relayApi: options.relayApi,
    cloudApi: options.cloudApi,
    getAuthToken: options.getAuthToken,
    gate: httpShareGate({ baseUrl: options.gatewayUrl, auth: { bearer: options.getAuthToken }, fetch: gateFetch }),
    dataDir: join(options.dataDir, 'tunnel'),
    onEvent: (event) => {
      if (event.type !== 'stopped') return;
      log(`link ${event.linkId} stopped: ${event.reason}`);
      // A restart stops every link too; those stay saved so the next start brings them back.
      if (!shuttingDown && DROP_REASONS.has(event.reason)) forget(event.linkId);
    },
  });

  const credential = async (linkId: string): Promise<string> => {
    const response = await fetch(
      `${options.cloudApi}/api/v1/share-links/${encodeURIComponent(linkId)}/relay-credential`,
      { method: 'POST', headers: { Authorization: `Bearer ${await options.getAuthToken()}` } },
    );
    if (response.status === 404 || response.status === 410 || response.status === 403) {
      throw new ShareTunnelError('admission_refused', `cloud-api no longer serves link ${linkId} (${response.status})`);
    }
    if (!response.ok) throw new Error(`relay credential for ${linkId} failed (${response.status})`);
    const body = (await response.json()) as { relay_credential?: string };
    if (!body.relay_credential) throw new Error(`relay credential for ${linkId} missing`);
    return body.relay_credential;
  };

  const serve = async (link: SavedLink, admission?: string): Promise<LiveShare> => {
    const live = await tunnel.start({
      link: { id: link.id, subdomain: link.subdomain },
      port: link.port,
      authMode: link.authMode,
      admission: admission ?? (() => credential(link.id)),
    });
    saved.set(link.id, link);
    await persist();
    return live;
  };

  const restore = async () => {
    let links: SavedLink[] = [];
    try {
      links = JSON.parse(await readFile(file, 'utf8')) as SavedLink[];
    } catch {
      return;
    }
    let again = false;
    for (const link of links) {
      saved.set(link.id, link);
      if (tunnel.get(link.id)) continue;
      try {
        await serve(link, await credential(link.id));
        log(`link ${link.id} back up`);
      } catch (error) {
        if (error instanceof ShareTunnelError && error.code === 'admission_refused') {
          log(`link ${link.id} dropped: ${error.message}`);
          forget(link.id);
        } else {
          log(`link ${link.id} not restored, retrying: ${error instanceof Error ? error.message : error}`);
          again = true;
        }
      }
    }
    if (again && !shuttingDown) retry = setTimeout(() => void restore(), RETRY_MS);
  };

  const cloud = async (method: string, path: string, body?: unknown) => {
    const response = await fetch(`${options.cloudApi}${path}`, {
      method,
      headers: {
        Authorization: `Bearer ${await options.getAuthToken()}`,
        ...(body === undefined ? {} : { 'content-type': 'application/json' }),
      },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    const json = (await response.json().catch(() => ({}))) as any;
    return { ok: response.ok, status: response.status, json };
  };

  const startShare = async (input: { port?: unknown; label?: unknown; artifact_id?: unknown }): Promise<StartShareResult> => {
    const port = input.port;
    if (typeof port !== 'number' || !Number.isInteger(port) || port < 1 || port > 65_535) {
      return { status: 400, body: { error: 'invalid_port', message: 'port must be an integer from 1 to 65535.' } };
    }
    const deviceId = options.getRuntimeDeviceId?.();
    if (!deviceId) return { status: 502, body: { error: 'not_paired', message: 'This computer is not paired yet.' } };
    const label = typeof input.label === 'string' && input.label.trim() ? input.label.trim().slice(0, 80) : `localhost:${port}`;
    const artifactId = typeof input.artifact_id === 'string' && input.artifact_id ? input.artifact_id : undefined;
    // The gate checks the port (denied ports, caps) when the tunnel opens it; a refusal there revokes this link.
    const subdomain = `l${randomBytes(5).toString('hex')}`;
    const made = await cloud('POST', '/api/v1/share-links', {
      kind: 'live',
      subject: artifactId ? 'artifact' : 'port',
      ...(artifactId ? { artifact_id: artifactId } : {}),
      runtime_device_id: deviceId,
      target_port: port,
      auth_mode: 'signed_in',
      subdomain,
      label,
    });
    const link = made.json?.link;
    if (!made.ok || !link?.id) {
      const status = [403, 409, 429].includes(made.status) ? made.status : 502;
      return { status, body: { error: made.json?.error ?? 'cloud_refused', message: made.json?.message ?? `cloud-api answered ${made.status}` } };
    }
    gateFailure = undefined;
    try {
      const live = await serve(
        { id: String(link.id), subdomain: String(link.subdomain ?? subdomain), port, authMode: 'token' },
        typeof made.json.relay_credential === 'string' ? made.json.relay_credential : undefined,
      );
      return { status: 200, body: { share: { ...view(live), token: live.token }, share_link_id: link.id, url: live.url } };
    } catch (error) {
      await cloud('DELETE', `/api/v1/share-links/${encodeURIComponent(String(link.id))}`).catch(() => undefined);
      const message = error instanceof Error ? error.message : String(error);
      if (error instanceof ShareTunnelError && error.code === 'port_denied') return { status: 403, body: { error: 'port_refused', message } };
      const g = gateFailure as { status: number; error?: string; message?: string } | undefined;
      if (g && [403, 409, 429].includes(g.status)) {
        return { status: g.status, body: { error: g.error ?? 'gate_refused', message: g.message ?? message } };
      }
      return { status: 502, body: { error: 'tunnel_failed', message } };
    }
  };

  const readBody = async (req: IncomingMessage): Promise<any> => {
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(chunk as Buffer);
    try {
      return chunks.length ? JSON.parse(Buffer.concat(chunks).toString('utf8')) : {};
    } catch {
      return {};
    }
  };
  const view = (live: LiveShare) => {
    const s = live.status();
    return { id: live.linkId, url: live.url, port: live.port, state: s.state, connections: s.connections };
  };

  const server: Server = createServer(async (req, res) => {
    const send = (status: number, body: unknown) => {
      res.writeHead(status, { 'content-type': 'application/json' });
      res.end(JSON.stringify(body));
    };
    try {
      // Loopback callers only: no browser Origin and a loopback Host (blocks DNS rebinding).
      const host = String(req.headers.host ?? '').replace(/:\d+$/, '');
      if (req.headers.origin || !['127.0.0.1', 'localhost'].includes(host)) return send(403, { error: 'loopback_only' });
      const path = new URL(req.url ?? '/', 'http://127.0.0.1').pathname.replace(/\/+$/, '');
      const m = path.match(/^\/share-host\/links(?:\/([^/]+)(?:\/(pause|stop))?)?$/);
      if (!m) return send(404, { error: 'not_found' });
      const [, id, action] = m;
      if (!id && req.method === 'GET') return send(200, { links: tunnel.list().map(view) });
      if (!id && req.method === 'POST') {
        const b = await readBody(req);
        if (!b.linkId || !b.subdomain || !Number.isInteger(b.port)) return send(400, { error: 'bad_request' });
        const live = await serve(
          { id: String(b.linkId), subdomain: String(b.subdomain), port: b.port, authMode: b.authMode === 'public' ? 'public' : 'token' },
          typeof b.admission === 'string' ? b.admission : undefined,
        );
        return send(201, { ...view(live), token: live.token });
      }
      const live = id ? tunnel.get(decodeURIComponent(id)) : undefined;
      if (!live) return send(404, { error: 'link_not_found' });
      if (action === 'pause' && req.method === 'POST') {
        const b = await readBody(req);
        await live.pause(b.paused !== false);
        return send(200, view(live));
      }
      if ((action === 'stop' && req.method === 'POST') || (!action && req.method === 'DELETE')) {
        await live.stop('stopped');
        forget(live.linkId);
        return send(200, { stopped: true, id: live.linkId });
      }
      return send(405, { error: 'method_not_allowed' });
    } catch (error) {
      const code = error instanceof ShareTunnelError ? error.code : 'error';
      send(code === 'port_denied' ? 403 : 502, { error: code, message: error instanceof Error ? error.message : String(error) });
    }
  });
  server.on('error', (error) => log(`control API unavailable: ${error.message}`));
  server.listen(options.controlPort, '127.0.0.1', () => log(`control API on 127.0.0.1:${options.controlPort}`));
  void restore();

  return {
    async stop() {
      shuttingDown = true;
      if (retry) clearTimeout(retry);
      server.close();
      await tunnel.stopAll('stopped');
    },
    startShare,
  };
}
