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
import { Readable } from 'node:stream';

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
  version: string;
  build: number;
  shellApi: number;
  url: string;
  sha256: string;
  size: number;
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
  log?: { info: (...a: unknown[]) => void; warn: (...a: unknown[]) => void };
}

export class RuntimePackages {
  private readonly opts: Required<Omit<RuntimePackagesOptions, 'log'>> & Pick<RuntimePackagesOptions, 'log'>;
  private activeRoot: string | null | undefined;

  constructor(options: RuntimePackagesOptions) {
    this.opts = {
      publicKey: RUNTIME_PUBLIC_KEY,
      shellApi: SHELL_API,
      platform: platformId(),
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
  async checkForUpdate(feedUrl: string, fetchImpl: typeof fetch = fetch): Promise<string | null> {
    const base = `${feedUrl.replace(/\/$/, '')}/stable/${this.opts.platform}`;
    const res = await fetchImpl(`${base}/latest.json`, { cache: 'no-store' });
    if (!res.ok) throw new Error(`runtime feed ${res.status}`);
    const raw = Buffer.from(await res.arrayBuffer());
    const sigRes = await fetchImpl(`${base}/latest.json.sig`, { cache: 'no-store' });
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
      const dl = await fetchImpl(new URL(entry.url, `${base}/`).toString());
      if (!dl.ok || !dl.body) throw new Error(`runtime download ${dl.status}`);
      await pipeline(Readable.fromWeb(dl.body as never), fs.createWriteStream(archive));
      if ((await sha256File(archive)) !== entry.sha256) throw new Error('runtime archive checksum mismatch');
      fs.mkdirSync(staging, { recursive: true });
      await new Promise<void>((resolve, reject) => {
        const tar = spawn('tar', ['-xzf', archive, '-C', staging], { stdio: 'ignore' });
        tar.on('error', reject);
        tar.on('exit', (code) => (code === 0 ? resolve() : reject(new Error(`tar exited ${code}`))));
      });
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

  hasPendingUpdate(): boolean {
    return Boolean(this.readState().pending);
  }
}
