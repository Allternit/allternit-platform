/**
 * Live links hosted by this computer (Apps + MCP 2026, decision D5; plan v3 lane 39).
 *
 * A live link serves a local port at https://<name>.live.allternitusercontent.com:
 *   1. cloud-api `POST /api/v1/share-links` (kind live, this computer's runtime device) makes the link
 *      and, once lane 17b lands, returns `relay_credential`; until then the credential comes from
 *      `POST /api/v1/share-links/:id/relay-credential`.
 *   2. `@allternit/share-tunnel` opens the local share gate (allternit-api `POST /shares` on loopback),
 *      connects the OpenTunnel relay and heartbeats cloud-api. A heartbeat 410 (revoked/killed) stops it.
 * Visitors send the per-share token as `X-Allternit-Share-Token` (or `?allternit_share_token=` once).
 *
 * `TunnelManager` (Cloudflare Web Access) is a different feature and stays untouched.
 *
 * The package is TypeScript source in `platform/packages/share-tunnel`; Desktop main is plain `tsc` with
 * `rootDir` = src/main, so it cannot import it directly. `scripts/build-share-tunnel.cjs` bundles it with
 * esbuild into `dist/vendor/share-tunnel.mjs` (part of `npm run build`), loaded here with a dynamic import.
 * The types below mirror the package's public API (platform/packages/share-tunnel/src/types.ts).
 */

import log from 'electron-log';

// ── Mirror of @allternit/share-tunnel's public API ───────────────────────────

type AuthMode = 'token' | 'public';
type ShareSubject = 'app' | 'mcp';
type StopReason = 'stopped' | 'paused' | 'killed' | 'revoked' | 'not_found' | 'relay_closed' | 'error';

interface LiveShare {
  readonly linkId: string;
  readonly subdomain: string;
  readonly url: string;
  readonly port: number;
  readonly token: string;
  pause(paused?: boolean): Promise<void>;
  rotate(): Promise<string>;
  status(): { state: 'connected' | 'reconnecting' | 'stopped'; connections: number; lastHeartbeatAt?: number; lastError?: string; stopReason?: StopReason };
  stop(reason?: StopReason): Promise<StopReason>;
  readonly stopped: Promise<StopReason>;
}

type TunnelEvent =
  | { type: 'started'; linkId: string; url: string }
  | { type: 'reconnecting'; linkId: string; attempt: number }
  | { type: 'connected'; linkId: string }
  | { type: 'heartbeat'; linkId: string; ok: boolean; status?: number; error?: string }
  | { type: 'stopped'; linkId: string; reason: StopReason; error?: string };

interface ShareTunnel {
  start(input: {
    link: { id: string; subdomain: string };
    port: number;
    authMode?: AuthMode;
    subject?: ShareSubject;
    label?: string;
    admission?: string | (() => Promise<string>);
  }): Promise<LiveShare>;
  stopAll(reason?: StopReason): Promise<void>;
}

export interface ShareTunnelModule {
  DENIED_PORTS: readonly number[];
  createShareTunnel(options: {
    relayApi: string;
    cloudApi: string;
    getAuthToken: () => Promise<string>;
    gate: unknown;
    dataDir?: string;
    heartbeatMs?: number;
    extraDeniedPorts?: readonly number[];
    onEvent?: (event: TunnelEvent) => void;
  }): ShareTunnel;
  httpShareGate(options: {
    baseUrl: string;
    auth: { bearer: string | (() => Promise<string>) } | { userId: string; desktopAccessToken: string | (() => Promise<string>) };
  }): unknown;
}

// ── IPC shapes (window.allternit.shares) ─────────────────────────────────────

export interface ShareStartRequest {
  /** Local port to share. */
  port?: number;
  /** An installed app's slug; used as the label. A live link still needs the port the app runs on. */
  appSlug?: string;
  /** Default `app`. MCP shares are always token-protected. */
  subject?: ShareSubject;
  /** Default `token`: visitors need the link's token. `public` only for app shares. */
  authMode?: AuthMode;
  label?: string;
  /** The artifact this port serves; the cloud link is then `subject: artifact` (owner only). */
  artifactId?: string;
}

export type ShareState = 'connected' | 'reconnecting' | 'paused' | 'stopped';

export interface ShareView {
  /** cloud-api share link id (`shl_...`): the id for pause/resume/stop/rotate. */
  id: string;
  subdomain: string;
  /** Bare link, e.g. https://<name>.live.allternitusercontent.com. */
  url: string;
  /** The link to hand out: carries the token once (`?allternit_share_token=`) for token links. */
  shareUrl: string;
  /** Per-share token visitors send as `X-Allternit-Share-Token`; null while stopped. */
  token: string | null;
  port: number;
  subject: ShareSubject;
  authMode: AuthMode;
  label: string | null;
  state: ShareState;
  connections: number;
  lastHeartbeatAt: number | null;
  lastError: string | null;
  stopReason: StopReason | null;
}

export type ShareErrorCode =
  | 'disabled'
  | 'bad_request'
  | 'port_denied'
  | 'already_shared'
  | 'not_paired'
  | 'signed_out'
  | 'cap_reached'
  | 'killed'
  | 'not_found'
  | 'create_failed'
  | 'admission_refused'
  | 'gate_failed'
  | 'relay_failed';

export type ShareResult<T> = ({ ok: true } & T) | { ok: false; error: ShareErrorCode; message: string; details?: unknown };

export type ShareEvent =
  | { type: 'share'; share: ShareView }
  | { type: 'removed'; id: string };

export interface ShareManagerDeps {
  isEnabled(): boolean;
  /** cloud-api origin, e.g. https://api.allternit.com. */
  cloudApi(): string;
  /** OpenTunnel relay API. */
  relayApi(): string;
  /** Clerk session token for cloud-api, or null when signed out. */
  getClerkToken(): Promise<string | null>;
  /** This computer's paired runtime device (cloud-api `runtime_devices.id`), or null when unpaired. */
  getRuntimeDeviceId(): string | null;
  /** Loopback allternit-api, the share gate (D2). */
  gateBaseUrl(): string;
  gateUserId(): string;
  getDesktopAccessToken(): string | null;
  /** Ports refused on top of the package's own list (D7: every Desktop service port). */
  extraDeniedPorts: readonly number[];
  dataDir?: string;
  emit(event: ShareEvent): void;
  loadModule?(): Promise<ShareTunnelModule>;
  fetch?: typeof fetch;
}

interface Entry {
  link: { id: string; subdomain: string; url: string | null };
  port: number;
  subject: ShareSubject;
  authMode: AuthMode;
  label: string | null;
  share: LiveShare | null;
  /** `paused` is ours: the gate answers 503 and cloud-api says paused; the heartbeat then stops the tunnel. */
  paused: boolean;
  stopReason: StopReason | null;
  lastError: string | null;
}

export const DEFAULT_RELAY_API = 'https://api.live.allternitusercontent.com';
const CLOUD_TIMEOUT_MS = 15_000;
const QUIT_REVOKE_TIMEOUT_MS = 3_000;

const HOST_STATUS: Record<ShareErrorCode, number> = {
  disabled: 403,
  port_denied: 403,
  already_shared: 409,
  bad_request: 400,
  not_paired: 409,
  signed_out: 409,
  cap_reached: 429,
  killed: 410,
  not_found: 404,
  create_failed: 502,
  admission_refused: 502,
  gate_failed: 502,
  relay_failed: 502,
};

class CloudError extends Error {
  constructor(readonly status: number, readonly code: string, message: string, readonly body: unknown) {
    super(message);
  }
}

export class ShareManager {
  private readonly entries = new Map<string, Entry>();
  private tunnel: ShareTunnel | null = null;
  private module: ShareTunnelModule | null = null;

  constructor(private readonly deps: ShareManagerDeps) {}

  async start(request: ShareStartRequest): Promise<ShareResult<{ share: ShareView }>> {
    if (!this.deps.isEnabled()) return fail('disabled', 'Live links are off on this computer.');
    const subject: ShareSubject = request?.subject === 'mcp' ? 'mcp' : 'app';
    const authMode: AuthMode = request?.authMode === 'public' ? 'public' : 'token';
    if (subject === 'mcp' && authMode === 'public') return fail('bad_request', 'MCP links always need the link token.');
    const port = Number(request?.port);
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      return fail(
        'bad_request',
        request?.appSlug
          ? 'An app live link needs the port the app runs on. Installed apps share through their hosted link.'
          : 'Pick a port from 1 to 65535.',
      );
    }
    for (const entry of this.entries.values()) {
      if (entry.port === port && entry.share) return fail('already_shared', `Port ${port} already has a live link.`);
    }

    let mod: ShareTunnelModule;
    try {
      mod = await this.loadModule();
    } catch (error) {
      log.error('[Shares] share-tunnel bundle failed to load', error);
      return fail('relay_failed', 'Live links are not available in this build.');
    }
    if (mod.DENIED_PORTS.includes(port) || this.deps.extraDeniedPorts.includes(port)) {
      return fail('port_denied', `Port ${port} runs Allternit itself and cannot be shared.`);
    }
    const runtimeDeviceId = this.deps.getRuntimeDeviceId();
    if (!runtimeDeviceId) return fail('not_paired', 'Pair this computer with your Allternit account first.');
    if (!(await this.deps.getClerkToken())) return fail('signed_out', 'Sign in to Allternit to make a live link.');

    const label = (request?.label ?? request?.appSlug ?? '').trim().slice(0, 120) || null;
    const artifactId = typeof request?.artifactId === 'string' && request.artifactId ? request.artifactId : null;
    let link: { id: string; subdomain: string; url?: string | null; relay_credential?: string };
    try {
      link = await this.cloud('POST', '/api/v1/share-links', {
        // Two vocabularies: cloud subject port|artifact|mcp; the gate's is app|mcp (port/artifact -> app).
        kind: 'live',
        subject: subject === 'mcp' ? 'mcp' : artifactId ? 'artifact' : 'port',
        ...(artifactId && subject !== 'mcp' ? { artifact_id: artifactId } : {}),
        target_port: port,
        runtime_device_id: runtimeDeviceId,
        // cloud-api's signed_in/org links are token links at the gate.
        auth_mode: authMode === 'public' ? 'public' : 'signed_in',
        ...(label ? { label } : {}),
      });
    } catch (error) {
      return cloudFailure(error, 'create_failed');
    }

    const entry: Entry = {
      link: { id: link.id, subdomain: link.subdomain, url: link.url ?? null },
      port,
      subject,
      authMode,
      label,
      share: null,
      paused: false,
      stopReason: null,
      lastError: null,
    };
    this.entries.set(link.id, entry);
    try {
      await this.serve(entry, link.relay_credential);
    } catch (error) {
      this.entries.delete(link.id);
      // Do not leave a dead link counting against the live-link cap.
      await this.cloud('DELETE', `/api/v1/share-links/${encodeURIComponent(link.id)}`).catch(() => undefined);
      return tunnelFailure(error);
    }
    return { ok: true, share: this.view(entry) };
  }

  /**
   * `POST /host/shares/start` relayed from the web (lane 40, platform #2971 / ai #1307): the owner's Clerk
   * session already passed cloud-api's runtime proxy. Body `{port, label?, artifact_id?}` -> 201
   * `{share, share_link_id, url}`; errors `{error, message}` with 400/403/409/410/429/502.
   * `url` is the link to hand out (it carries the token once for token links).
   */
  async handleHostStart(rawBody: string | undefined): Promise<{ status: number; payload: unknown }> {
    let body: { port?: unknown; label?: unknown; artifact_id?: unknown } = {};
    try {
      body = rawBody ? JSON.parse(rawBody) : {};
    } catch {
      return { status: 400, payload: { error: 'invalid_body', message: 'Send {port, label?, artifact_id?} as JSON.' } };
    }
    const result = await this.start({
      port: typeof body.port === 'number' ? body.port : Number.NaN,
      ...(typeof body.label === 'string' ? { label: body.label } : {}),
      ...(typeof body.artifact_id === 'string' ? { artifactId: body.artifact_id } : {}),
    });
    if (result.ok) {
      return { status: 201, payload: { share: result.share, share_link_id: result.share.id, url: result.share.shareUrl } };
    }
    return { status: HOST_STATUS[result.error], payload: { error: result.error, message: result.message } };
  }

  /**
   * `POST /host/shares/:share_link_id/{resume,pause,stop}` relayed from the web (platform #2971 / ai #1307):
   * the web only flips cloud state, so Resume must come here to re-open the tunnel. 200 `{share, share_link_id}`
   * (stop: `{stopped: true, share_link_id}`); errors `{error, message}` (404 not_found when this computer is not
   * serving that link, 403 disabled, 410 killed, 502 tunnel/cloud failure).
   */
  async handleHostCommand(action: string, id: string): Promise<{ status: number; payload: unknown }> {
    const result =
      action === 'resume' ? await this.resume(id) : action === 'pause' ? await this.pause(id) : action === 'stop' ? await this.stop(id) : null;
    if (!result) return { status: 404, payload: { error: 'not_found', message: 'Unknown live link command.' } };
    if (result.ok) {
      return { status: 200, payload: { ...('share' in result ? { share: result.share } : { stopped: true }), share_link_id: id } };
    }
    return { status: HOST_STATUS[result.error], payload: { error: result.error, message: result.message } };
  }

  list(): ShareView[] {
    return [...this.entries.values()].map((entry) => this.view(entry));
  }

  /** Pauses: the gate answers 503 at once and cloud-api marks the link paused (its heartbeat then stops the tunnel). */
  async pause(id: string): Promise<ShareResult<{ share: ShareView }>> {
    const entry = this.entries.get(id);
    if (!entry) return fail('not_found', 'That live link is not running on this computer.');
    if (!entry.paused) {
      try {
        await this.cloud('POST', `/api/v1/share-links/${encodeURIComponent(id)}/pause`, { paused: true });
      } catch (error) {
        return cloudFailure(error, 'gate_failed');
      }
      entry.paused = true;
      await entry.share?.pause(true).catch((error) => log.warn('[Shares] gate pause failed', error));
      this.publish(entry);
    }
    return { ok: true, share: this.view(entry) };
  }

  /** Resumes a paused link; restarts the tunnel when the heartbeat already stopped it (the token changes then). */
  async resume(id: string): Promise<ShareResult<{ share: ShareView }>> {
    const entry = this.entries.get(id);
    if (!entry) return fail('not_found', 'That live link is not running on this computer.');
    if (!this.deps.isEnabled()) return fail('disabled', 'Live links are off on this computer.');
    try {
      await this.cloud('POST', `/api/v1/share-links/${encodeURIComponent(id)}/pause`, { paused: false });
    } catch (error) {
      return cloudFailure(error, 'gate_failed');
    }
    entry.paused = false;
    entry.stopReason = null;
    entry.lastError = null;
    // Pausing made cloud-api drop the relay tunnel, so the old handle cannot carry traffic again:
    // close it (this also drops its saved identity) and open a fresh tunnel with a fresh relay credential.
    const previous = entry.share;
    entry.share = null;
    if (previous) await previous.stop('paused').catch(() => undefined);
    try {
      await this.serve(entry);
    } catch (error) {
      entry.lastError = error instanceof Error ? error.message : String(error);
      this.publish(entry);
      return tunnelFailure(error);
    }
    this.publish(entry);
    return { ok: true, share: this.view(entry) };
  }

  /** Stops serving and revokes the link in cloud-api, so it stops counting against the cap. */
  async stop(id: string): Promise<ShareResult<{ stopped: true }>> {
    const entry = this.entries.get(id);
    if (!entry) return fail('not_found', 'That live link is not running on this computer.');
    this.entries.delete(id);
    await entry.share?.stop('stopped').catch(() => undefined);
    this.deps.emit({ type: 'removed', id });
    try {
      await this.cloud('DELETE', `/api/v1/share-links/${encodeURIComponent(id)}`);
    } catch (error) {
      // 410: already revoked or killed: nothing left to do.
      if (!(error instanceof CloudError && (error.status === 410 || error.status === 404))) {
        log.warn('[Shares] revoke after stop failed', error);
      }
    }
    return { ok: true, stopped: true };
  }

  /** New gate token; the old token and any link carrying it stop working. */
  async rotate(id: string): Promise<ShareResult<{ share: ShareView }>> {
    const entry = this.entries.get(id);
    if (!entry?.share) return fail('not_found', 'That live link is not running on this computer.');
    try {
      await entry.share.rotate();
    } catch (error) {
      return tunnelFailure(error);
    }
    this.publish(entry);
    return { ok: true, share: this.view(entry) };
  }

  /**
   * App quit: every share stops and its link is revoked (best effort, short deadline), so the phone list
   * and the cap never show a link nobody serves. Must run before allternit-api (the gate) stops.
   */
  async stopAll(): Promise<void> {
    const entries = [...this.entries.values()];
    this.entries.clear();
    if (entries.length === 0) return;
    await this.tunnel?.stopAll('stopped').catch(() => undefined);
    await Promise.race([
      Promise.all(
        entries.map((entry) =>
          this.cloud('DELETE', `/api/v1/share-links/${encodeURIComponent(entry.link.id)}`).catch(() => undefined),
        ),
      ),
      new Promise((resolve) => setTimeout(resolve, QUIT_REVOKE_TIMEOUT_MS)),
    ]);
  }

  /** Flag turned off: stop everything now. */
  async disable(): Promise<void> {
    const ids = [...this.entries.keys()];
    await Promise.all(ids.map((id) => this.stop(id)));
  }

  // ── internals ──────────────────────────────────────────────────────────────

  private async serve(entry: Entry, credential?: string): Promise<void> {
    const tunnel = await this.ensureTunnel();
    const linkId = entry.link.id;
    const share = await tunnel.start({
      link: { id: linkId, subdomain: entry.link.subdomain },
      port: entry.port,
      authMode: entry.authMode,
      subject: entry.subject,
      ...(entry.label ? { label: entry.label } : {}),
      // Needed only the first time this link is served here; later starts reuse the saved identity.
      admission: credential
        ? credential
        : async () => {
            const minted = await this.cloud<{ credential?: string; relay_credential?: string }>(
              'POST',
              `/api/v1/share-links/${encodeURIComponent(linkId)}/relay-credential`,
            );
            const value = minted.credential ?? minted.relay_credential;
            if (!value) throw new Error('cloud-api returned no relay credential');
            return value;
          },
    });
    entry.share = share;
    void share.stopped.then((reason) => {
      if (this.entries.get(linkId) !== entry || entry.share !== share) return;
      entry.share = null;
      entry.stopReason = reason;
      if (reason === 'paused') entry.paused = true;
      if (reason === 'killed' || reason === 'revoked' || reason === 'not_found') {
        // Gone in cloud-api: drop it here too.
        this.entries.delete(linkId);
        this.deps.emit({ type: 'share', share: this.view(entry) });
        this.deps.emit({ type: 'removed', id: linkId });
        return;
      }
      this.publish(entry);
    });
  }

  private async ensureTunnel(): Promise<ShareTunnel> {
    if (this.tunnel) return this.tunnel;
    const mod = await this.loadModule();
    const desktopAccessToken = async () => {
      const token = this.deps.getDesktopAccessToken();
      if (!token) throw new Error('allternit-api is not running');
      return token;
    };
    this.tunnel = mod.createShareTunnel({
      relayApi: this.deps.relayApi(),
      cloudApi: this.deps.cloudApi(),
      getAuthToken: async () => {
        const token = await this.deps.getClerkToken();
        if (!token) throw new Error('signed out');
        return token;
      },
      gate: mod.httpShareGate({
        baseUrl: this.deps.gateBaseUrl(),
        auth: { userId: this.deps.gateUserId(), desktopAccessToken },
      }),
      extraDeniedPorts: this.deps.extraDeniedPorts,
      ...(this.deps.dataDir ? { dataDir: this.deps.dataDir } : {}),
      onEvent: (event) => this.onTunnelEvent(event),
    });
    return this.tunnel;
  }

  private onTunnelEvent(event: TunnelEvent): void {
    if (event.type === 'heartbeat' && !event.ok) log.warn('[Shares] heartbeat failed', event.linkId, event.status ?? event.error);
    if (event.type === 'stopped') log.info('[Shares] stopped', event.linkId, event.reason, event.error ?? '');
    const entry = this.entries.get(event.linkId);
    if (!entry) return;
    if (event.type === 'stopped' && event.error) entry.lastError = event.error;
    if (event.type === 'heartbeat' && !event.ok && event.error) entry.lastError = event.error;
    if (event.type === 'heartbeat' && event.ok) entry.lastError = null;
    // `stopped` is published from share.stopped, after the entry is updated.
    if (event.type !== 'stopped') this.publish(entry);
  }

  private async loadModule(): Promise<ShareTunnelModule> {
    if (this.module) return this.module;
    this.module = this.deps.loadModule
      ? await this.deps.loadModule()
      : ((await import(new URL('../vendor/share-tunnel.mjs', import.meta.url).href)) as ShareTunnelModule);
    return this.module;
  }

  private publish(entry: Entry): void {
    if (this.entries.get(entry.link.id) !== entry) return;
    this.deps.emit({ type: 'share', share: this.view(entry) });
  }

  private view(entry: Entry): ShareView {
    const status = entry.share?.status();
    const url = entry.share?.url ?? entry.link.url ?? `https://${entry.link.subdomain}.live.allternitusercontent.com`;
    const token = entry.share?.token ?? null;
    const state: ShareState = entry.paused
      ? 'paused'
      : !entry.share || status?.state === 'stopped'
        ? 'stopped'
        : status?.state === 'reconnecting'
          ? 'reconnecting'
          : 'connected';
    return {
      id: entry.link.id,
      subdomain: entry.link.subdomain,
      url,
      shareUrl: entry.authMode === 'token' && token ? `${url}/?allternit_share_token=${encodeURIComponent(token)}` : url,
      token,
      port: entry.port,
      subject: entry.subject,
      authMode: entry.authMode,
      label: entry.label,
      state,
      connections: status?.connections ?? 0,
      lastHeartbeatAt: status?.lastHeartbeatAt ?? null,
      lastError: entry.lastError ?? status?.lastError ?? null,
      stopReason: entry.stopReason ?? status?.stopReason ?? null,
    };
  }

  private async cloud<T = any>(method: string, path: string, body?: unknown): Promise<T> {
    const token = await this.deps.getClerkToken();
    if (!token) throw new CloudError(401, 'signed_out', 'Sign in to Allternit to manage live links.', null);
    const doFetch = this.deps.fetch ?? fetch;
    const response = await doFetch(`${this.deps.cloudApi().replace(/\/+$/, '')}${path}`, {
      method,
      headers: { authorization: `Bearer ${token}`, ...(body === undefined ? {} : { 'content-type': 'application/json' }) },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      signal: AbortSignal.timeout(CLOUD_TIMEOUT_MS),
    });
    const parsed = (await response.json().catch(() => null)) as any;
    if (!response.ok) {
      throw new CloudError(response.status, parsed?.error ?? 'http_' + response.status, parsed?.message ?? `cloud-api answered ${response.status}`, parsed);
    }
    return parsed as T;
  }
}

function fail(error: ShareErrorCode, message: string, details?: unknown): { ok: false; error: ShareErrorCode; message: string; details?: unknown } {
  return details === undefined ? { ok: false, error, message } : { ok: false, error, message, details };
}

function cloudFailure(error: unknown, fallback: ShareErrorCode) {
  if (error instanceof CloudError) {
    if (error.status === 401) return fail('signed_out', error.message);
    if (error.status === 429) return fail('cap_reached', error.message, error.body);
    if (error.status === 410) return fail('killed', error.message, error.body);
    if (error.status === 404) return fail('not_found', error.message);
    if (error.status === 400 && error.code === 'device_required') return fail('not_paired', error.message);
    return fail(fallback, error.message, error.body);
  }
  return fail(fallback, error instanceof Error ? error.message : String(error));
}

function tunnelFailure(error: unknown) {
  const code = (error as { code?: string } | null)?.code;
  const message = error instanceof Error ? error.message : String(error);
  if (code === 'port_denied' || code === 'bad_port') return fail('port_denied', message);
  if (code === 'admission_required' || code === 'admission_refused' || code === 'name_mismatch') return fail('admission_refused', message);
  if (code === 'gate_failed') return fail('gate_failed', message);
  if (error instanceof CloudError) return cloudFailure(error, 'admission_refused');
  return fail('relay_failed', message);
}
