// §1 config — env (SUBS_GATEWAY_*) + optional flat-key policy file.
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { defaultAdaptersDir } from "./adapters/registry.js";
import type { KeychainBackendKind } from "./security/keychain.js";

export interface TcpConfig {
  enabled: boolean;
  host: string;
  port: number;
}

export interface Config {
  stateDir: string;
  dbPath: string;
  artifactsDir: string;
  udsPath: string;
  // Adapter package dir (manifest.yaml per adapter); default resolved from the
  // package layout, overridable for tests.
  adaptersDir: string;
  // D3/D15 secret-store backend: macOS Keychain (default) or a 0600 file
  // under stateDir on Sessions machines. Env: SUBS_GATEWAY_KEYCHAIN.
  keychainBackend: KeychainBackendKind;
  tcp: TcpConfig;
  policyPath: string;
  policy: Record<string, string>;
  // §A8 — per-capability stall watchdog timeouts (seconds); defaults 90, with
  // research.deep at 1200. Policy keys: stall_timeout_s, stall_timeout_s.<cap>.
  stallTimeouts: { defaultS: number; byCapability: Record<string, number> };
  // Base URL of the local allternit-api (CommRails peer messages, D12).
  apiBase: string;
  // Login mode browser (plain, non-automated Google Chrome; Firefox still
  // works when set explicitly). null → auto-detect at boot; unset and
  // undetected → POST /v1/accounts/:id/login answers 501.
  // Env: SUBS_GATEWAY_LOGIN_BROWSER.
  loginBrowser: string | null;
  // Image-chat history policy: image tasks run in this provider project
  // (null = no project, plain new chats) and reuse one chat per account until
  // it holds `max` images. Env: SUBS_GATEWAY_IMAGE_PROJECT (default
  // "Allternit"; empty disables), SUBS_GATEWAY_IMAGE_CHAT_MAX (default 20).
  imageChats: { project: string | null; max: number };
  // AAI host (/aai/*). disabled = per-adapter kill switch (env SUBS_GATEWAY_AAI_DISABLED, csv of adapterIds);
  // loopback = the Allternit-bot provider reaching allternit-api (base default `${apiBase}/api/v1`,
  // SUBS_GATEWAY_AAI_LOOPBACK_BASE / _BOTS csv; bearer via SUBS_GATEWAY_AAI_LOOPBACK_TOKEN).
  aai: { disabled: string[]; loopbackBaseUrl: string; loopbackBots: string[] };
  // Close a subscription's Chrome after this many idle minutes (no task in
  // flight for the account); the next task relaunches it. 0 keeps Chrome
  // resident (the default). Env: SUBS_GATEWAY_LANE_IDLE_MIN.
  laneIdleCloseMin: number;
}

const ENV_PREFIX = "SUBS_GATEWAY_";

// Google Chrome first (Sessions machines ship it for the adapter; Google
// sign-in and Cloudflare accept a plain Chrome window), then Firefox.
function detectLoginBrowser(): string | null {
  const candidates = [
    "/usr/bin/google-chrome-stable",
    "/usr/bin/google-chrome",
    "/opt/google/chrome/chrome",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/opt/firefox/firefox",
    "/usr/bin/firefox",
    "/Applications/Firefox.app/Contents/MacOS/firefox",
  ];
  return candidates.find((c) => existsSync(c)) ?? null;
}

function expandHome(p: string): string {
  if (p === "~") return homedir();
  if (p.startsWith("~/") || p.startsWith("~\\")) return join(homedir(), p.slice(2));
  return p;
}

// Content-addressed artifact location: <artifactsDir>/<sha256[0:2]>/<sha256>.
export function artifactPath(config: Config, sha256: string): string {
  return join(config.artifactsDir, sha256.slice(0, 2), sha256);
}

// Minimal hand-rolled YAML subset: flat `key: value` pairs only. `#` comment
// lines and blank lines are skipped; values may be single- or double-quoted.
// No nesting, lists, anchors, or multi-line values — by design, so the daemon
// carries no YAML dependency. Anything more complex belongs in env vars.
export function parsePolicyFile(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (line === "" || line.startsWith("#")) continue;
    const m = /^([A-Za-z0-9_.-]+)\s*:\s*(.*)$/.exec(line);
    if (!m) continue;
    let value = m[2].trim();
    const quoted = /^(['"])(.*)\1$/.exec(value);
    if (quoted) value = quoted[2];
    out[m[1]] = value;
  }
  return out;
}

// §A8 watchdog timeouts from policy: stall_timeout_s (default) and
// stall_timeout_s.<capability> (per-capability override).
export function stallTimeoutsFromPolicy(
  policy: Record<string, string>
): Config["stallTimeouts"] {
  const byCapability: Record<string, number> = {};
  let defaultS = 90;
  for (const [key, value] of Object.entries(policy)) {
    const n = Number(value);
    if (!Number.isFinite(n) || n <= 0) continue;
    if (key === "stall_timeout_s") defaultS = n;
    else if (key.startsWith("stall_timeout_s.")) byCapability[key.slice("stall_timeout_s.".length)] = n;
  }
  return { defaultS, byCapability };
}

export function stallTimeoutFor(config: Config, capability: string): number {
  return (
    config.stallTimeouts.byCapability[capability] ??
    (capability === "research.deep" ? 1200 : config.stallTimeouts.defaultS)
  );
}

function positiveInt(raw: string | undefined, fallback: number, name: string): number {
  if (raw === undefined || raw.trim() === "") return fallback;
  const n = Number(raw);
  if (!Number.isInteger(n) || n < 1) throw new Error(`${name} must be a positive integer, got ${JSON.stringify(raw)}`);
  return n;
}

function nonNegativeInt(raw: string | undefined, fallback: number, name: string): number {
  if (raw === undefined || raw.trim() === "") return fallback;
  const n = Number(raw);
  if (!Number.isInteger(n) || n < 0) throw new Error(`${name} must be a whole number of minutes, got ${JSON.stringify(raw)}`);
  return n;
}

function csvList(v: string | undefined): string[] {
  return (v ?? "").split(",").map((x) => x.trim()).filter(Boolean);
}

export function loadConfig(env: NodeJS.ProcessEnv = process.env): Config {
  const stateDir = expandHome(
    env[`${ENV_PREFIX}STATE_DIR`] ?? "~/.allternit/subscriptions/"
  );
  const policyPath = join(stateDir, "policy.yaml");
  const policy = existsSync(policyPath)
    ? parsePolicyFile(readFileSync(policyPath, "utf8"))
    : {};
  const keychainEnv = env[`${ENV_PREFIX}KEYCHAIN`];
  if (keychainEnv !== undefined && keychainEnv !== "file" && keychainEnv !== "keychain") {
    throw new Error(
      `${ENV_PREFIX}KEYCHAIN must be "file" or "keychain", got ${JSON.stringify(keychainEnv)}`
    );
  }
  return {
    stateDir,
    dbPath: join(stateDir, "state.db"),
    artifactsDir: join(stateDir, "artifacts"),
    udsPath: join(stateDir, "gateway.sock"),
    adaptersDir: env[`${ENV_PREFIX}ADAPTERS_DIR`] ?? defaultAdaptersDir(),
    keychainBackend: keychainEnv ?? "keychain",
    tcp: {
      enabled: env[`${ENV_PREFIX}TCP`] === "1",
      host: env[`${ENV_PREFIX}TCP_HOST`] ?? "127.0.0.1",
      port: Number(env[`${ENV_PREFIX}TCP_PORT`] ?? "7788"),
    },
    policyPath,
    policy,
    stallTimeouts: stallTimeoutsFromPolicy(policy),
    apiBase: env[`${ENV_PREFIX}API_BASE`] ?? "http://127.0.0.1:18013",
    loginBrowser: env[`${ENV_PREFIX}LOGIN_BROWSER`] ?? detectLoginBrowser(),
    imageChats: {
      project: (env[`${ENV_PREFIX}IMAGE_PROJECT`] ?? "Allternit").trim() || null,
      max: positiveInt(env[`${ENV_PREFIX}IMAGE_CHAT_MAX`], 20, `${ENV_PREFIX}IMAGE_CHAT_MAX`),
    },
    aai: {
      disabled: csvList(env[`${ENV_PREFIX}AAI_DISABLED`]),
      loopbackBaseUrl: env[`${ENV_PREFIX}AAI_LOOPBACK_BASE`] ?? `${env[`${ENV_PREFIX}API_BASE`] ?? "http://127.0.0.1:18013"}/api/v1`,
      loopbackBots: csvList(env[`${ENV_PREFIX}AAI_LOOPBACK_BOTS`]),
    },
    laneIdleCloseMin: nonNegativeInt(env[`${ENV_PREFIX}LANE_IDLE_MIN`], 0, `${ENV_PREFIX}LANE_IDLE_MIN`),
  };
}
