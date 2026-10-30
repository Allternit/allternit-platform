/**
 * The iCTRL runner Phone Link installs on an iPhone: download + verify the
 * unsigned build we publish, and sign it for the user's phone.
 *
 * Artifacts: built once from our iCTRL fork, packaged by
 * ictrl-fork/Scripts/package-phone-link.sh (unsigned runner + host .ipa and a
 * manifest.json with their SHA-256), published under
 * <base>/<version>/. The manifest's own SHA-256 is pinned here per version,
 * so a configurable base URL can't swap in another build.
 *
 * Signing: a `RunnerSigner` turns those into packages the phone accepts.
 *  - LocalIdentitySigner: an Apple Development identity + development profile
 *    already on this Mac (developer machines; `codesign`, same steps as
 *    ictrl-fork/Scripts/sign-ipa.sh).
 *  - SideloadSigner: the product path. The user's own Apple ID (free account
 *    works) through the `allternit-sideload` sidecar, a small Rust CLI over
 *    isideload (MIT). The password goes to the sidecar's stdin once and is
 *    never written, logged or put in argv/env; Apple's session tokens stay in
 *    the macOS Keychain (isideload's keyring storage).
 */

import { execFile, spawn as nodeSpawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { sha256Hex } from './phone-adb.js';
import type { FetchFn } from './ictrl-client.js';

// ─── Artifacts ───────────────────────────────────────────────────────────────

export const DEFAULT_RUNNER_BASE_URL = 'https://runtime.allternit.com/phone-link/ictrl';

/**
 * version → SHA-256 of its manifest.json. Empty until the first build is
 * published (Scripts/package-phone-link.sh prints the line to add here).
 */
export const PINNED_RUNNER_MANIFESTS: Record<string, string> = {
  '1.98.12-allternit.1': '02fc92eec1769db97afba4cdea78e158c1c48bc18c2b69958ecc2e9b5934ffcf',
  // .2: joins the user's mesh (config mesh_control_url / mesh_auth_key); only
  // local clients may use the one-shot pairing grant. Live 2026-10-17.
  '1.98.12-allternit.2': '97dd791451a01e88afc7d738173388857edc22fdc2f8a0fcd5a821208bcba053',
  // .3: live screen — ReplayKit broadcast extension (iCTRLBroadcast) + the
  // runner's GET /video and *_live_screen RPCs. Published 2026-10-18.
  '1.98.12-allternit.3': 'dfab5703b22f9fdfe21942d5332ba06a68ea350e98750dfe671a722c52eca141',
  // .4: the extension is listed as a broadcast target (process mode under
  // NSExtensionAttributes) and sends app audio (AAC, /video types 4/5).
  '1.98.12-allternit.4': '7758efc9bd6667505015aca1b181ac48557afd0fd9d8f3b83eabfa1d00e83bdb',
  // .5: start_live_screen reports another screen recording (otherCapture).
  '1.98.12-allternit.5': '1c7c2c3ee695474fa225280e541db2844ada869bf434dc6fb20e5deb9d2c6041',
  // .6: mesh_restart RPC + mesh watchdog (tsnet Health()).
  '1.98.12-allternit.6': '43a796f27a2b752a911b7c724db99b14a23340aa78ded88c287e2391cf67e5ba',
  // .7: presses the broadcast sheet's Start Sharing button (iOS 27 label).
  '1.98.12-allternit.7': '587a2de7bd4e3dc46d9ab3a9cf19156069022d07c962c1d1fc271b8b4f1a2f65',
};
export const RUNNER_VERSION: string | null = '1.98.12-allternit.7';

export interface RunnerPackage {
  file: string;
  sha256: string;
  size: number;
  bundleId: string;
}

export interface RunnerManifest {
  version: string;
  protocol: 1;
  /** Bundle-id prefix the unsigned build carries; signers may replace it. */
  bundlePrefix: string;
  runner: RunnerPackage & { xctestConfig: string; testToRun: string };
  host: RunnerPackage;
}

export interface RunnerArtifacts {
  manifest: RunnerManifest;
  runnerIpa: string;
  hostIpa: string;
}

export class RunnerUnavailableError extends Error {
  readonly code = 'runner_unavailable';
}

export function parseRunnerManifest(text: string): RunnerManifest {
  const m = JSON.parse(text) as RunnerManifest;
  const pkg = (p: RunnerPackage | undefined) =>
    p && /^[\w.-]+\.ipa$/.test(p.file) && /^[0-9a-f]{64}$/.test(p.sha256) && Number.isInteger(p.size) && /^[\w.-]+$/.test(p.bundleId);
  if (m.protocol !== 1 || !pkg(m.runner) || !pkg(m.host) || !m.runner.xctestConfig || !m.runner.testToRun) {
    throw new RunnerUnavailableError('The Phone Link helper manifest is malformed.');
  }
  return m;
}

export interface EnsureRunnerDeps {
  baseUrl?: string;
  version?: string | null;
  /** Overrides the pin (dev builds: ALLTERNIT_ICTRL_MANIFEST_SHA256). */
  manifestSha256?: string;
  fetchImpl?: FetchFn;
}

/** Download (once) and verify the unsigned runner + host packages. */
export async function ensureRunnerArtifacts(dataDir: string, deps: EnsureRunnerDeps = {}): Promise<RunnerArtifacts> {
  const version = deps.version ?? process.env.ALLTERNIT_ICTRL_VERSION ?? RUNNER_VERSION;
  const pin = deps.manifestSha256 ?? process.env.ALLTERNIT_ICTRL_MANIFEST_SHA256 ?? (version ? PINNED_RUNNER_MANIFESTS[version] : undefined);
  if (!version || !pin) throw new RunnerUnavailableError('The iPhone helper for this version of Allternit has not been published yet.');
  const base = (deps.baseUrl ?? process.env.ALLTERNIT_ICTRL_BASE_URL ?? DEFAULT_RUNNER_BASE_URL).replace(/\/+$/, '');
  if (!/^https:\/\//.test(base) && !/^http:\/\/127\.0\.0\.1[:/]/.test(base)) throw new RunnerUnavailableError('The helper download URL must be https.');
  const fetchImpl = deps.fetchImpl ?? fetch;
  const dir = path.join(dataDir, 'ictrl', version);
  fs.mkdirSync(dir, { recursive: true });

  const get = async (name: string): Promise<Buffer> => {
    const res = await fetchImpl(`${base}/${version}/${name}`);
    if (!res.ok) throw new RunnerUnavailableError(`Downloading ${name} failed: HTTP ${res.status}`);
    return Buffer.from(await res.arrayBuffer());
  };

  const manifestPath = path.join(dir, 'manifest.json');
  const cached = fs.existsSync(manifestPath) ? fs.readFileSync(manifestPath) : null;
  let manifestBytes: Buffer;
  if (cached && sha256Hex(cached) === pin) {
    manifestBytes = cached;
  } else {
    manifestBytes = await get('manifest.json');
    if (sha256Hex(manifestBytes) !== pin) throw new RunnerUnavailableError('The helper manifest did not match the pinned checksum; refusing to use it.');
    fs.writeFileSync(manifestPath, manifestBytes);
  }
  const manifest = parseRunnerManifest(manifestBytes.toString('utf8'));
  if (manifest.version !== version) throw new RunnerUnavailableError('The helper manifest is for another version.');

  const fetchPackage = async (pkg: RunnerPackage): Promise<string> => {
    const file = path.join(dir, pkg.file);
    if (fs.existsSync(file)) {
      const data = fs.readFileSync(file);
      if (data.length === pkg.size && sha256Hex(data) === pkg.sha256) return file;
    }
    const data = await get(pkg.file);
    if (data.length !== pkg.size || sha256Hex(data) !== pkg.sha256) {
      throw new RunnerUnavailableError(`${pkg.file} did not match its checksum; refusing to install it.`);
    }
    fs.writeFileSync(file, data);
    return file;
  };
  return { manifest, runnerIpa: await fetchPackage(manifest.runner), hostIpa: await fetchPackage(manifest.host) };
}

// ─── Signing ─────────────────────────────────────────────────────────────────

export interface AppleIdCredentials {
  appleId: string;
  password: string;
}

export interface SignRequest {
  udid: string;
  deviceName: string;
  artifacts: RunnerArtifacts;
  workDir: string;
  /** Asked only by signers with needsAppleId. */
  credentials?: AppleIdCredentials;
  /**
   * Resolves with the 6-digit code the user typed, or 'sms' to have Apple text
   * the code to `phone` instead; asked only when Apple wants one.
   */
  twoFactor?: (hint: { sms: boolean; phone?: string }) => Promise<string>;
  onProgress?: (fraction: number, step: string) => void;
}

export interface SignedRunner {
  runnerIpa: string;
  hostIpa: string;
  /** Bundle-id prefix after signing (App IDs are global, so the sidecar uses `<ours>.<team id lowercased>`). */
  bundlePrefix: string;
  runnerBundleId: string;
  hostBundleId: string;
  teamId: string;
  /** Profile expiry (free accounts: 7 days); Desktop re-signs before it. */
  expiresAt?: string;
}

export class SigningError extends Error {
  constructor(
    readonly code:
      | 'bad_credentials'
      | 'password_required'
      | 'two_factor_failed'
      | 'app_id_limit'
      | 'certificate_limit'
      | 'device_limit'
      | 'no_identity'
      | 'signer_unavailable'
      | 'anisette_unavailable'
      | 'signing_failed',
    message: string,
  ) {
    super(message);
    this.name = 'SigningError';
  }
}

export interface RunnerSigner {
  readonly kind: 'local-identity' | 'apple-id';
  readonly needsAppleId: boolean;
  sign(req: SignRequest): Promise<SignedRunner>;
}

export type Exec = (cmd: string, args: string[], opts?: { cwd?: string; input?: string }) => Promise<{ stdout: string; stderr: string; code: number }>;

export const execTool: Exec = (cmd, args, opts = {}) =>
  new Promise((resolve) => {
    const child = execFile(cmd, args, { cwd: opts.cwd, maxBuffer: 32 * 1024 * 1024 }, (error, stdout, stderr) => {
      const code = error ? (typeof (error as { code?: unknown }).code === 'number' ? (error as { code: number }).code : 1) : 0;
      resolve({ stdout: String(stdout), stderr: String(stderr), code });
    });
    if (opts.input !== undefined) child.stdin?.end(opts.input);
  });

export const PROFILE_DIRS = [
  path.join(os.homedir(), 'Library', 'Developer', 'Xcode', 'UserData', 'Provisioning Profiles'),
  path.join(os.homedir(), 'Library', 'MobileDevice', 'Provisioning Profiles'),
];

export interface DecodedProfile {
  path: string;
  name: string;
  teamId: string;
  appIdPattern: string;
  getTaskAllow: boolean;
  expiresAt: string;
  devices: string[];
  provisionsAll: boolean;
  /** SHA-1 (upper hex) of each certificate in the profile. */
  certSha1: string[];
}

/** Does `TEAM.pattern` cover `bundleId`? 2 exact, 1 prefix wildcard, 0 full wildcard, -1 no. */
export function profileCovers(appIdPattern: string, teamId: string, bundleId: string): number {
  if (appIdPattern === `${teamId}.${bundleId}`) return 2;
  if (appIdPattern === `${teamId}.*`) return 0;
  if (appIdPattern.startsWith(`${teamId}.`) && appIdPattern.endsWith('*')) {
    return bundleId.startsWith(appIdPattern.slice(teamId.length + 1, -1)) ? 1 : -1;
  }
  return -1;
}

/**
 * Developer machines: sign with an Apple Development identity and a
 * development profile already installed (Xcode's automatic signing leaves
 * both). Inside out like sign-ipa.sh, with the .xctest taking the runner's
 * bundle id. Never used for end users.
 */
export class LocalIdentitySigner implements RunnerSigner {
  readonly kind = 'local-identity' as const;
  readonly needsAppleId = false;

  constructor(
    private readonly deps: {
      exec?: Exec;
      profileDirs?: string[];
      now?: () => number;
      /** Restrict to one team. */
      teamId?: string;
    } = {},
  ) {}

  private get exec(): Exec {
    return this.deps.exec ?? execTool;
  }

  async decodeProfile(file: string, plistPath: string): Promise<DecodedProfile | null> {
    const decoded = await this.exec('security', ['cms', '-D', '-i', file]);
    if (decoded.code !== 0) return null;
    fs.writeFileSync(plistPath, decoded.stdout);
    const json = await this.exec('plutil', ['-convert', 'json', '-o', '-', plistPath]);
    if (json.code !== 0) return null;
    const p = JSON.parse(json.stdout) as {
      Name?: string;
      TeamIdentifier?: string[];
      Entitlements?: Record<string, unknown>;
      ExpirationDate?: string;
      ProvisionedDevices?: string[];
      ProvisionsAllDevices?: boolean;
      DeveloperCertificates?: string[];
    };
    return {
      path: file,
      name: p.Name ?? path.basename(file),
      teamId: p.TeamIdentifier?.[0] ?? '',
      appIdPattern: String(p.Entitlements?.['application-identifier'] ?? ''),
      getTaskAllow: p.Entitlements?.['get-task-allow'] === true,
      expiresAt: p.ExpirationDate ?? '',
      devices: p.ProvisionedDevices ?? [],
      provisionsAll: p.ProvisionsAllDevices === true,
      certSha1: (p.DeveloperCertificates ?? []).map((c) => createHash('sha1').update(Buffer.from(c, 'base64')).digest('hex').toUpperCase()),
    };
  }

  /** Best development profile covering both bundle ids and this phone, plus a keychain identity in it. */
  async pick(udid: string, bundleIds: string[], scratch: string): Promise<{ profile: DecodedProfile; identity: string }> {
    const now = (this.deps.now ?? Date.now)();
    const candidates: { profile: DecodedProfile; score: number }[] = [];
    for (const dir of this.deps.profileDirs ?? PROFILE_DIRS) {
      if (!fs.existsSync(dir)) continue;
      for (const name of fs.readdirSync(dir).filter((n) => n.endsWith('.mobileprovision'))) {
        const profile = await this.decodeProfile(path.join(dir, name), path.join(scratch, 'candidate.plist'));
        if (!profile || !profile.getTaskAllow) continue;
        if (this.deps.teamId && profile.teamId !== this.deps.teamId) continue;
        if (!(Date.parse(profile.expiresAt) > now)) continue;
        if (!profile.provisionsAll && !profile.devices.includes(udid)) continue;
        const scores = bundleIds.map((id) => profileCovers(profile.appIdPattern, profile.teamId, id));
        if (scores.some((s) => s < 0)) continue;
        candidates.push({ profile, score: Math.min(...scores) });
      }
    }
    candidates.sort((a, b) => b.score - a.score || b.profile.expiresAt.localeCompare(a.profile.expiresAt));
    const identities = (await this.exec('security', ['find-identity', '-v', '-p', 'codesigning'])).stdout
      .split('\n')
      .map((l) => /\)\s+([0-9A-F]{40})\s+"/.exec(l)?.[1])
      .filter((s): s is string => !!s);
    for (const { profile } of candidates) {
      const identity = identities.find((sha) => profile.certSha1.includes(sha));
      if (identity) return { profile, identity };
    }
    throw new SigningError(
      'no_identity',
      'No Apple Development identity and development profile on this Mac covers the helper and this iPhone. Open Xcode once with automatic signing and this phone connected.',
    );
  }

  async sign(req: SignRequest): Promise<SignedRunner> {
    const { manifest } = req.artifacts;
    fs.mkdirSync(req.workDir, { recursive: true });
    const scratch = fs.mkdtempSync(path.join(req.workDir, 'sign-'));
    try {
      req.onProgress?.(0.1, 'Finding your developer certificate');
      const { profile, identity } = await this.pick(req.udid, [manifest.runner.bundleId, manifest.host.bundleId], scratch);
      const team = profile.teamId;
      const out: Record<'runner' | 'host', string> = { runner: '', host: '' };
      for (const which of ['runner', 'host'] as const) {
        req.onProgress?.(which === 'runner' ? 0.3 : 0.7, which === 'runner' ? 'Signing the helper' : 'Signing the helper app');
        const pkg = manifest[which];
        const src = which === 'runner' ? req.artifacts.runnerIpa : req.artifacts.hostIpa;
        out[which] = await this.signIpa(src, path.join(req.workDir, `${which}-signed.ipa`), pkg.bundleId, team, profile.path, identity, path.join(scratch, which));
      }
      return {
        runnerIpa: out.runner,
        hostIpa: out.host,
        bundlePrefix: manifest.bundlePrefix,
        runnerBundleId: manifest.runner.bundleId,
        hostBundleId: manifest.host.bundleId,
        teamId: team,
        expiresAt: profile.expiresAt,
      };
    } finally {
      fs.rmSync(scratch, { recursive: true, force: true });
    }
  }

  private async signIpa(src: string, dest: string, bundleId: string, team: string, profilePath: string, identity: string, work: string): Promise<string> {
    const must = async (cmd: string, args: string[], cwd?: string) => {
      const r = await this.exec(cmd, args, { cwd });
      if (r.code !== 0) throw new SigningError('signing_failed', `${cmd} ${args[0] ?? ''} failed: ${(r.stderr || r.stdout).trim().slice(0, 400)}`);
      return r.stdout;
    };
    fs.mkdirSync(work, { recursive: true });
    await must('unzip', ['-q', '-o', src, '-d', work]);
    const payload = path.join(work, 'Payload');
    const appName = fs.readdirSync(payload).find((n) => n.endsWith('.app'));
    if (!appName) throw new SigningError('signing_failed', `No Payload/*.app in ${path.basename(src)}`);
    const app = path.join(payload, appName);
    fs.copyFileSync(profilePath, path.join(app, 'embedded.mobileprovision'));
    const ent = path.join(work, 'entitlements.plist');
    fs.writeFileSync(
      ent,
      `<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict>` +
        `<key>application-identifier</key><string>${team}.${bundleId}</string>` +
        `<key>com.apple.developer.team-identifier</key><string>${team}</string>` +
        `<key>get-task-allow</key><true/>` +
        `<key>keychain-access-groups</key><array><string>${team}.${bundleId}</string></array>` +
        `</dict></plist>\n`,
    );
    const sign = (target: string, extra: string[] = []) => must('codesign', ['--force', '--timestamp=none', '--sign', identity, ...extra, target]);
    const plugins = path.join(app, 'PlugIns');
    const pluginDirs = fs.existsSync(plugins) ? fs.readdirSync(plugins).map((n) => path.join(plugins, n)) : [];
    // 1. The .xctest takes the runner's bundle id, before anything is sealed.
    for (const p of pluginDirs.filter((d) => d.endsWith('.xctest'))) {
      await must('/usr/libexec/PlistBuddy', ['-c', `Set :CFBundleIdentifier ${bundleId}`, path.join(p, 'Info.plist')]);
    }
    // 2. Inside out: loose dylibs, frameworks (deepest first), plug-ins, then the app.
    const walk = (dir: string, acc: string[] = []): string[] => {
      for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, e.name);
        if (e.isDirectory()) {
          if (e.name.endsWith('.framework')) acc.push(full);
          walk(full, acc);
        } else if (e.name.endsWith('.dylib') && !full.includes('.framework/')) acc.push(full);
      }
      return acc;
    };
    const nested = walk(app).sort((a, b) => b.length - a.length);
    for (const f of nested.filter((f) => f.endsWith('.dylib'))) await sign(f);
    for (const f of nested.filter((f) => f.endsWith('.framework'))) await sign(f);
    for (const p of pluginDirs) await sign(p);
    await sign(app, ['--entitlements', ent]);
    await must('codesign', ['--verify', '--deep', '--strict', app]);
    fs.rmSync(dest, { force: true });
    await must('zip', ['-qry', dest, 'Payload'], work);
    return dest;
  }
}

// ─── Product signer: the allternit-sideload sidecar (isideload) ──────────────

/**
 * Which signer this build uses: developer machines (unpackaged, or
 * ALLTERNIT_PHONE_LINK_LOCAL_SIGNING=1) sign with the Mac's own Xcode
 * identity; shipped builds use the Apple ID sidecar next to the app
 * (`resources/bin/allternit-sideload`, required in macOS packaging via
 * `npm run prepare:sideload`). Shared by Phone Link setup (phone-link-ipc.ts)
 * and the automatic renewal (phone-service.ts).
 */
export function pickSigner(packaged: boolean, env: NodeJS.ProcessEnv = process.env): RunnerSigner {
  if (!packaged || env.ALLTERNIT_PHONE_LINK_LOCAL_SIGNING === '1') return new LocalIdentitySigner();
  const bin = typeof process.resourcesPath === 'string' ? path.join(process.resourcesPath, 'bin', 'allternit-sideload') : null;
  return new SideloadSigner({
    // The sidecar binary is a hard requirement of macOS packaging; a missing
    // one here means the build is broken, and signing reports it.
    binary: bin && fs.existsSync(bin) ? bin : null,
    // Allternit-hosted anisette v3 server (anisette.allternit.com, live since 2026-10-17).
    anisetteUrl: env.ALLTERNIT_ANISETTE_URL ?? 'https://anisette.allternit.com',
  });
}

/**
 * JSON lines over stdio; the sidecar side is cmd/allternit-sideload/src/protocol.rs
 * (keep both in sync). One sign per process. Desktop → sidecar (stdin):
 *   {"op":"sign","apple_id","password"?,"udid","device_name","runner_ipa","host_ipa","out_dir","anisette_url"?,"bundle_prefix"?}
 *   {"op":"two_factor","code":"123456"}
 *   {"op":"two_factor_sms"}   (text the code to the trusted number instead)
 *   {"op":"cancel"}                      (closing stdin does the same)
 * Sidecar → Desktop (stdout):
 *   {"event":"progress","fraction":0.4,"step":"Registering this iPhone"}
 *   {"event":"two_factor_required","sms":false,"phone":"•••• 12","retry_reason"?}
 *   {"event":"done","runner_ipa","host_ipa","bundle_prefix","runner_bundle_id","host_bundle_id","team_id","expires_at","session_reused"}
 *   {"event":"error","code":"bad_credentials|password_required|two_factor_failed|app_id_limit|certificate_limit|device_limit|anisette_unavailable|invalid_request|signing_failed","message"}
 * `password` may be left out when the Apple session saved in the Keychain
 * (service com.allternit.sideload) is still valid; otherwise the answer is
 * `password_required`. The sidecar only signs; Desktop installs with go-ios.
 */
export interface SidecarProcess {
  write(line: string): void;
  onLine(handler: (line: string) => void): void;
  exited: Promise<number | null>;
  kill(): void;
}

export type SidecarSpawn = (bin: string) => SidecarProcess;

export const spawnSidecarProcess: SidecarSpawn = (bin) => {
  // Plain spawn, not the lifeline shim: it exits when its stdin closes, and
  // nothing secret goes in argv or env.
  const child = nodeSpawn(bin, ['--json'], { stdio: ['pipe', 'pipe', 'pipe'], env: { PATH: process.env.PATH ?? '', HOME: os.homedir() } });
  const handlers: ((line: string) => void)[] = [];
  let pending = '';
  child.stdout.on('data', (chunk: Buffer) => {
    pending += chunk.toString('utf8');
    const lines = pending.split('\n');
    pending = lines.pop() ?? '';
    for (const line of lines) if (line.trim()) handlers.forEach((h) => h(line));
  });
  child.stderr.resume();
  return {
    write: (line) => void child.stdin.write(`${line}\n`),
    onLine: (h) => void handlers.push(h),
    exited: new Promise((resolve) => child.once('exit', (code) => resolve(code))),
    kill: () => child.kill('SIGTERM'),
  };
};

const SIDECAR_CODES = new Set([
  'bad_credentials',
  'password_required',
  'two_factor_failed',
  'app_id_limit',
  'certificate_limit',
  'device_limit',
  'anisette_unavailable',
  'signing_failed',
]);

export class SideloadSigner implements RunnerSigner {
  readonly kind = 'apple-id' as const;
  readonly needsAppleId = true;

  constructor(
    private readonly deps: {
      /** Path to allternit-sideload, or null when this build doesn't ship it. */
      binary: string | null;
      spawn?: SidecarSpawn;
      /** Allternit-hosted anisette v3 server (isideload's default is a third party's). */
      anisetteUrl?: string;
    },
  ) {}

  sign(req: SignRequest): Promise<SignedRunner> {
    const bin = this.deps.binary;
    if (!bin) return Promise.reject(new SigningError('signer_unavailable', 'Apple ID signing is not in this build yet.'));
    if (!req.credentials?.appleId) return Promise.reject(new SigningError('bad_credentials', 'Apple ID is required.'));
    const proc = (this.deps.spawn ?? spawnSidecarProcess)(bin);
    return new Promise<SignedRunner>((resolve, reject) => {
      let settled = false;
      const finish = (fn: () => void) => {
        if (settled) return;
        settled = true;
        fn();
        proc.kill();
      };
      proc.onLine((line) => {
        let msg: Record<string, unknown>;
        try {
          msg = JSON.parse(line);
        } catch {
          return;
        }
        switch (msg.event) {
          case 'progress':
            req.onProgress?.(Number(msg.fraction) || 0, String(msg.step ?? ''));
            break;
          case 'two_factor_required': {
            if (!req.twoFactor) return finish(() => reject(new SigningError('two_factor_failed', 'A verification code is required.')));
            req.twoFactor({ sms: msg.sms === true, phone: typeof msg.phone === 'string' ? msg.phone : undefined }).then(
              (code) => proc.write(JSON.stringify(code === 'sms' ? { op: 'two_factor_sms' } : { op: 'two_factor', code })),
              (error) => finish(() => reject(error)),
            );
            break;
          }
          case 'done':
            finish(() =>
              resolve({
                runnerIpa: String(msg.runner_ipa),
                hostIpa: String(msg.host_ipa),
                bundlePrefix: String(msg.bundle_prefix),
                runnerBundleId: String(msg.runner_bundle_id),
                hostBundleId: String(msg.host_bundle_id),
                teamId: String(msg.team_id),
                expiresAt: typeof msg.expires_at === 'string' ? msg.expires_at : undefined,
              }),
            );
            break;
          case 'error': {
            const code = SIDECAR_CODES.has(String(msg.code)) ? (String(msg.code) as SigningError['code']) : 'signing_failed';
            finish(() => reject(new SigningError(code, String(msg.message ?? 'Signing failed.'))));
            break;
          }
        }
      });
      proc.exited.then(() => finish(() => reject(new SigningError('signing_failed', 'The signing helper stopped unexpectedly.'))));
      fs.mkdirSync(req.workDir, { recursive: true });
      proc.write(
        JSON.stringify({
          op: 'sign',
          apple_id: req.credentials!.appleId,
          // Empty password: re-sign with the saved session (password_required if it lapsed).
          password: req.credentials!.password || undefined,
          udid: req.udid,
          device_name: req.deviceName,
          runner_ipa: req.artifacts.runnerIpa,
          host_ipa: req.artifacts.hostIpa,
          out_dir: req.workDir,
          anisette_url: this.deps.anisetteUrl,
          bundle_prefix: req.artifacts.manifest?.bundlePrefix,
        }),
      );
    });
  }
}
