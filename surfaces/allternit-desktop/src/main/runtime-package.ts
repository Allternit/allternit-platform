/**
 * Runtime packages: the parts of Allternit that change often (allternit-api,
 * gizzi-code and the platform screens) update on their own, without a new
 * Desktop app build.
 *
 * Layout under <userData>/runtime:
 *   versions/<version>/   manifest.json, manifest.sig, bin/…, platform/…
 *   state.json            { active, previous, pending, trial, bad[] }
 *
 * A package is used only when its manifest is signed by RUNTIME_PUBLIC_KEY,
 * targets this platform, needs no newer shell API than SHELL_API, and is newer
 * than the runtime bundled in the app (resources/runtime.json). A new package
 * is staged as `pending` and promoted on the next launch as a `trial`; if the
 * backend fails to start on it, it is marked bad and the previous one returns.
 */
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { pipeline } from 'node:stream/promises';
import { Readable, Transform } from 'node:stream';
import zlib from 'node:zlib';

/** Bump when the preload/IPC contract the screens rely on changes. */
export const SHELL_API = 1;

/** Ed25519 key that signs runtime packages (private half stays with the release operator). */
export const RUNTIME_PUBLIC_KEY = `-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAfFzE4gMEyDUK/Tb40TsLV5hXAYcD0Lro6gqWijht7ZU=
-----END PUBLIC KEY-----`;

export const DEFAULT_RUNTIME_FEED = 'https://runtime.allternit.com';

export interface RuntimeManifest {
  version: string;
  /** Monotonic build number (commit time, unix seconds). Higher wins. */
  build: number;
  platform: string;
  shellApi: number;
  files: Record<string, { sha256: string; size: number }>;
}

export interface FeedEntry {
  /** 2 = per-file delta feed (manifest + content-addressed objects); absent = one tarball. */
  format?: number;
  version: string;
  build: number;
  shellApi: number;
  /** format 1: the tarball. */
  url?: string;
  sha256?: string;
  size?: number;
  /** format 2: the signed manifest, relative to the platform feed dir. */
  manifest?: string;
  manifestSha256?: string;
}

/** What one update had to download, for the log. */
export interface StageStats {
  reused: number;
  downloaded: number;
  downloadedBytes: number;
}

interface RuntimeState {
  active?: string;
  previous?: string;
  pending?: string;
  /** Version promoted this launch and not yet confirmed healthy. */
  trial?: string;
  bad: string[];
}

export function platformId(platform = process.platform, arch = process.arch): string {
  return `${platform}-${arch}`;
}

export function verifySignature(data: Buffer, signatureB64: string, publicKey = RUNTIME_PUBLIC_KEY): boolean {
  try {
    return crypto.verify(null, data, publicKey, Buffer.from(signatureB64.trim(), 'base64'));
  } catch {
    return false;
  }
}

/** No bytes for this long aborts a download (then it is retried). */
export const STALL_MS = 60_000;
const DOWNLOAD_TRIES = 3;

/**
 * Fetch `url` into `dest` (gunzipped when `gunzip`). A transfer that sends no
 * bytes for `stallMs` is aborted and retried, up to DOWNLOAD_TRIES times: a
 * fetch without a timeout can hang forever on a dead connection and freeze the
 * whole update with no error. Resolves to the bytes received.
 */
async function fetchToFile(fetchImpl: typeof fetch, url: string, dest: string, gunzip: boolean, stallMs: number): Promise<number> {
  for (let attempt = 1; ; attempt++) {
    const ac = new AbortController();
    let timer: NodeJS.Timeout | undefined;
    const arm = () => {
      clearTimeout(timer);
      timer = setTimeout(() => ac.abort(new Error(`download stalled for ${stallMs / 1000}s`)), stallMs);
    };
    try {
      arm();
      const res = await fetchImpl(url, { signal: ac.signal });
      if (!res.ok || !res.body) throw Object.assign(new Error(`${res.status}`), { status: res.status });
      let bytes = 0;
      const watch = new Transform({
        transform(chunk: Buffer, _enc, cb) {
          bytes += chunk.length;
          arm();
          cb(null, chunk);
        },
      });
      const body = Readable.fromWeb(res.body as never);
      if (gunzip) await pipeline(body, watch, zlib.createGunzip(), fs.createWriteStream(dest), { signal: ac.signal });
      else await pipeline(body, watch, fs.createWriteStream(dest), { signal: ac.signal });
      return bytes;
    } catch (err) {
      const status = (err as { status?: number }).status;
      const reason = ac.signal.aborted ? (ac.signal.reason as Error) : (err as Error);
      if (attempt >= DOWNLOAD_TRIES || (status !== undefined && status < 500)) {
        throw new Error(`runtime download ${path.basename(dest)}: ${reason?.message ?? reason}`);
      }
      await new Promise((r) => setTimeout(r, 250 * attempt));
    } finally {
      clearTimeout(timer);
    }
  }
}

function sha256File(file: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const hash = crypto.createHash('sha256');
    fs.createReadStream(file).on('error', reject).on('data', (c) => hash.update(c)).on('end', () => resolve(hash.digest('hex')));
  });
}

export interface RuntimePackagesOptions {
  /** <userData>/runtime */
  root: string;
  /** process.resourcesPath (bundled fallback) */
  resourcesPath: string;
  publicKey?: string;
  shellApi?: number;
  platform?: string;
  /** Abort a download after this long with no bytes (default STALL_MS). */
  stallMs?: number;
  log?: { info: (...a: unknown[]) => void; warn: (...a: unknown[]) => void };
}

export class RuntimePackages {
  private readonly opts: Required<Omit<RuntimePackagesOptions, 'log'>> & Pick<RuntimePackagesOptions, 'log'>;
  private activeRoot: string | null | undefined;
  private checking: Promise<string | null> | null = null;

  constructor(options: RuntimePackagesOptions) {
    this.opts = {
      publicKey: RUNTIME_PUBLIC_KEY,
      shellApi: SHELL_API,
      platform: platformId(),
      stallMs: STALL_MS,
      ...options,
    };
  }

  private get versionsDir() { return path.join(this.opts.root, 'versions'); }
  private get statePath() { return path.join(this.opts.root, 'state.json'); }

  readState(): RuntimeState {
    try {
      const s = JSON.parse(fs.readFileSync(this.statePath, 'utf8'));
      return { ...s, bad: Array.isArray(s.bad) ? s.bad : [] };
    } catch {
      return { bad: [] };
    }
  }

  private writeState(state: RuntimeState) {
    fs.mkdirSync(this.opts.root, { recursive: true });
    const tmp = `${this.statePath}.tmp`;
    fs.writeFileSync(tmp, JSON.stringify(state, null, 2));
    fs.renameSync(tmp, this.statePath);
  }

  /** Build number of the runtime bundled inside the app (0 when the app predates runtime packages). */
  bundledBuild(): number {
    try {
      return Number(JSON.parse(fs.readFileSync(path.join(this.opts.resourcesPath, 'runtime.json'), 'utf8')).build) || 0;
    } catch {
      return 0;
    }
  }

  /** Signed, compatible manifest of an installed version; cheap size check unless `deep`. */
  async readVerified(version: string, deep = false): Promise<RuntimeManifest | null> {
    const dir = path.join(this.versionsDir, version);
    try {
      const raw = fs.readFileSync(path.join(dir, 'manifest.json'));
      const sig = fs.readFileSync(path.join(dir, 'manifest.sig'), 'utf8');
      if (!verifySignature(raw, sig, this.opts.publicKey)) return null;
      const m = JSON.parse(raw.toString('utf8')) as RuntimeManifest;
      if (m.version !== version || m.platform !== this.opts.platform || m.shellApi > this.opts.shellApi) return null;
      for (const [rel, f] of Object.entries(m.files)) {
        const abs = path.join(dir, rel);
        if (!abs.startsWith(dir + path.sep)) return null;
        const st = fs.statSync(abs);
        if (st.size !== f.size) return null;
        if (deep && (await sha256File(abs)) !== f.sha256) return null;
      }
      return m;
    } catch {
      return null;
    }
  }

  /**
   * Call once at launch, before any runtime binary starts. Promotes a pending
   * package to a trial, and returns the root new runtime files come from.
   */
  async prepareForLaunch(): Promise<string | null> {
    const state = this.readState();
    if (state.trial) {
      // Last launch promoted a trial and never confirmed it: treat as failed.
      this.opts.log?.warn('[Runtime] unconfirmed trial', state.trial, '— rolling back');
      state.bad = [...new Set([...state.bad, state.trial])];
      state.active = state.previous;
      state.previous = undefined;
      state.trial = undefined;
    }
    if (state.pending && !state.bad.includes(state.pending)) {
      if (await this.readVerified(state.pending, true)) {
        state.previous = state.active;
        state.active = state.pending;
        state.trial = state.pending;
      }
    }
    state.pending = undefined;
    let root: string | null = null;
    if (state.active && !state.bad.includes(state.active)) {
      const m = await this.readVerified(state.active);
      if (m && m.build > this.bundledBuild()) {
        root = path.join(this.versionsDir, state.active);
      } else {
        // The app was updated past it, or it is damaged: the bundled runtime wins.
        state.active = undefined;
        state.trial = undefined;
      }
    } else {
      state.active = undefined;
      state.trial = undefined;
    }
    this.writeState(state);
    this.prune(state);
    this.activeRoot = root;
    this.opts.log?.info('[Runtime] launching on', root ?? 'bundled runtime');
    return root;
  }

  /** The backend came up healthy on the trial runtime. */
  confirmHealthy() {
    const state = this.readState();
    if (state.trial) {
      state.trial = undefined;
      this.writeState(state);
    }
  }

  /** The backend failed on the trial runtime: mark it bad. Returns true if a restart should retry on the previous one. */
  failTrial(): boolean {
    const state = this.readState();
    if (!state.trial) return false;
    state.bad = [...new Set([...state.bad, state.trial])];
    state.active = state.previous;
    state.previous = undefined;
    state.trial = undefined;
    this.writeState(state);
    return true;
  }

  /** Version the app is running on, for display. */
  currentVersion(): string {
    return this.activeRoot ? path.basename(this.activeRoot) : 'bundled';
  }

  /** Path to a runtime file: the active package's copy when it has one, else the bundled copy. */
  file(...segments: string[]): string {
    if (this.activeRoot) {
      const candidate = path.join(this.activeRoot, ...segments);
      if (fs.existsSync(candidate)) return candidate;
    }
    return path.join(this.opts.resourcesPath, ...segments);
  }

  private prune(state: RuntimeState) {
    const keep = new Set([state.active, state.previous, state.pending].filter(Boolean) as string[]);
    let entries: string[] = [];
    try { entries = fs.readdirSync(this.versionsDir); } catch { return; }
    for (const name of entries) {
      if (!keep.has(name)) fs.rmSync(path.join(this.versionsDir, name), { recursive: true, force: true });
    }
  }

  /**
   * Check the feed and stage a newer package as pending. Resolves to the staged
   * version, or null when there is nothing newer.
   */
  checkForUpdate(feedUrl: string, fetchImpl: typeof fetch = fetch): Promise<string | null> {
    // One check at a time: two would share (and delete) the same staging dir.
    this.checking ??= this.runCheck(feedUrl, fetchImpl).finally(() => {
      this.checking = null;
    });
    return this.checking;
  }

  /** A small feed file (latest.json, a manifest, a signature), bounded by the stall timeout. */
  private fetchSmall(fetchImpl: typeof fetch, url: string) {
    // undici's RequestInit has no `cache`; the runtime accepts the DOM shape.
    return fetchImpl(url, { cache: 'no-store', signal: AbortSignal.timeout(this.opts.stallMs) } as RequestInit);
  }

  private async runCheck(feedUrl: string, fetchImpl: typeof fetch): Promise<string | null> {
    const base = `${feedUrl.replace(/\/$/, '')}/stable/${this.opts.platform}`;
    const res = await this.fetchSmall(fetchImpl, `${base}/latest.json`);
    if (!res.ok) throw new Error(`runtime feed ${res.status}`);
    const raw = Buffer.from(await res.arrayBuffer());
    const sigRes = await this.fetchSmall(fetchImpl, `${base}/latest.json.sig`);
    if (!sigRes.ok || !verifySignature(raw, await sigRes.text(), this.opts.publicKey)) {
      throw new Error('runtime feed signature invalid');
    }
    const entry = JSON.parse(raw.toString('utf8')) as FeedEntry;
    const state = this.readState();
    const currentBuild = Math.max(
      this.bundledBuild(),
      state.active ? (await this.readVerified(state.active))?.build ?? 0 : 0,
      state.pending ? (await this.readVerified(state.pending))?.build ?? 0 : 0,
    );
    if (entry.build <= currentBuild || entry.shellApi > this.opts.shellApi || state.bad.includes(entry.version)) return null;
    if (!/^[A-Za-z0-9._-]+$/.test(entry.version)) throw new Error('bad runtime version');

    fs.mkdirSync(this.versionsDir, { recursive: true });
    const archive = path.join(this.versionsDir, `${entry.version}.download.tar.gz`);
    const staging = path.join(this.versionsDir, `${entry.version}.partial`);
    const finalDir = path.join(this.versionsDir, entry.version);
    fs.rmSync(staging, { recursive: true, force: true });
    try {
      fs.mkdirSync(staging, { recursive: true });
      if (entry.format === 2) {
        const feedRoot = feedUrl.replace(/\/$/, '');
        const stats = await this.stageDelta(entry, base, feedRoot, staging, fetchImpl);
        this.opts.log?.info('[Runtime] update', entry.version, `reused ${stats.reused} files, downloaded ${stats.downloaded} (${(stats.downloadedBytes / 1e6).toFixed(1)} MB)`);
      } else {
        if (!entry.url || !entry.sha256) throw new Error('runtime feed entry has no package');
        await fetchToFile(fetchImpl, new URL(entry.url, `${base}/`).toString(), archive, false, this.opts.stallMs);
        if ((await sha256File(archive)) !== entry.sha256) throw new Error('runtime archive checksum mismatch');
        await new Promise<void>((resolve, reject) => {
          const tar = spawn('tar', ['-xzf', archive, '-C', staging], { stdio: 'ignore' });
          tar.on('error', reject);
          tar.on('exit', (code) => (code === 0 ? resolve() : reject(new Error(`tar exited ${code}`))));
        });
      }
      fs.rmSync(finalDir, { recursive: true, force: true });
      fs.renameSync(staging, finalDir);
      const manifest = await this.readVerified(entry.version, true);
      if (!manifest || manifest.build !== entry.build) {
        fs.rmSync(finalDir, { recursive: true, force: true });
        throw new Error('runtime package failed verification');
      }
      for (const rel of Object.keys(manifest.files)) {
        if (rel.startsWith('bin/')) fs.chmodSync(path.join(finalDir, rel), 0o755);
      }
      const next = this.readState();
      next.pending = entry.version;
      this.writeState(next);
      this.prune(next);
      this.opts.log?.info('[Runtime] staged', entry.version, 'for next launch');
      return entry.version;
    } finally {
      fs.rmSync(archive, { force: true });
      fs.rmSync(staging, { recursive: true, force: true });
    }
  }

  /**
   * Local copies by content hash: the active package (its signed manifest) and
   * the runtime bundled in the app (resources/runtime.json `files`, written at
   * build time). Only entries whose size still matches are offered.
   */
  private localIndex(): Map<string, string> {
    const index = new Map<string, string>();
    const add = (root: string, files: Record<string, { sha256: string; size: number }> | undefined) => {
      for (const [rel, f] of Object.entries(files ?? {})) {
        const abs = path.join(root, rel);
        try {
          if (!index.has(f.sha256) && fs.statSync(abs).size === f.size) index.set(f.sha256, abs);
        } catch {
          // missing locally
        }
      }
    };
    const active = this.readState().active;
    if (active) {
      try {
        const dir = path.join(this.versionsDir, active);
        add(dir, JSON.parse(fs.readFileSync(path.join(dir, 'manifest.json'), 'utf8')).files);
      } catch {
        // no usable active manifest
      }
    }
    try {
      add(this.opts.resourcesPath, JSON.parse(fs.readFileSync(path.join(this.opts.resourcesPath, 'runtime.json'), 'utf8')).files);
    } catch {
      // app predates the bundled file list
    }
    return index;
  }

  /**
   * Format 2: fetch the signed manifest, copy every file already on this
   * machine (same content hash), and download only the rest from the
   * content-addressed `objects/<sha256>.gz` store. The caller verifies every
   * file against the manifest afterwards.
   */
  private async stageDelta(entry: FeedEntry, base: string, feedRoot: string, staging: string, fetchImpl: typeof fetch): Promise<StageStats> {
    if (!entry.manifest || !entry.manifestSha256) throw new Error('runtime feed entry has no manifest');
    const manifestUrl = new URL(entry.manifest, `${base}/`).toString();
    const manRes = await this.fetchSmall(fetchImpl, manifestUrl);
    if (!manRes.ok) throw new Error(`runtime manifest ${manRes.status}`);
    const raw = Buffer.from(await manRes.arrayBuffer());
    if (crypto.createHash('sha256').update(raw).digest('hex') !== entry.manifestSha256) throw new Error('runtime manifest checksum mismatch');
    const sigRes = await this.fetchSmall(fetchImpl, `${manifestUrl}.sig`);
    const sig = sigRes.ok ? await sigRes.text() : '';
    if (!verifySignature(raw, sig, this.opts.publicKey)) throw new Error('runtime manifest signature invalid');
    const manifest = JSON.parse(raw.toString('utf8')) as RuntimeManifest;
    if (manifest.version !== entry.version || manifest.build !== entry.build || manifest.platform !== this.opts.platform) {
      throw new Error('runtime manifest does not match the feed');
    }
    fs.writeFileSync(path.join(staging, 'manifest.json'), raw);
    fs.writeFileSync(path.join(staging, 'manifest.sig'), sig);

    const index = this.localIndex();
    const stats: StageStats = { reused: 0, downloaded: 0, downloadedBytes: 0 };
    const pending: Array<[string, { sha256: string; size: number }]> = [];
    for (const [rel, f] of Object.entries(manifest.files)) {
      const dst = path.join(staging, rel);
      if (!dst.startsWith(staging + path.sep) || !/^[0-9a-f]{64}$/.test(f.sha256)) throw new Error('bad runtime manifest entry');
      fs.mkdirSync(path.dirname(dst), { recursive: true });
      let src = index.get(f.sha256);
      // Confirm the content: packaging can re-sign binaries inside the app, so a listed copy may differ.
      if (src && (await sha256File(src)) !== f.sha256) src = undefined;
      if (!src) {
        // An older app without a bundled file list: check the bundled copy at the same path.
        const same = path.join(this.opts.resourcesPath, rel);
        try {
          if (fs.statSync(same).size === f.size && (await sha256File(same)) === f.sha256) src = same;
        } catch {
          // not present
        }
      }
      if (src) {
        fs.copyFileSync(src, dst, fs.constants.COPYFILE_FICLONE);
        stats.reused += 1;
      } else {
        pending.push([rel, f]);
      }
    }
    const download = async ([rel, f]: [string, { sha256: string; size: number }]) => {
      const bytes = await fetchToFile(fetchImpl, `${feedRoot}/objects/${f.sha256}.gz`, path.join(staging, rel), true, this.opts.stallMs);
      stats.downloaded += 1;
      stats.downloadedBytes += bytes;
    };
    for (let i = 0; i < pending.length; i += 8) {
      await Promise.all(pending.slice(i, i + 8).map(download));
    }
    return stats;
  }

  hasPendingUpdate(): boolean {
    return Boolean(this.readState().pending);
  }
}
