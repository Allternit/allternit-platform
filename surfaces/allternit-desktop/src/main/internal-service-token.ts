/**
 * ALLTERNIT_INTERNAL_SERVICE_TOKEN for Desktop-hosted runtimes.
 *
 * allternit-api gates its `/internal/*` routes on this shared token
 * (cmd/allternit-api/src/internal_auth.rs `require_internal_token`; with no
 * token and no ALLTERNIT_LOCAL_DEV_BYPASS every call answers 401). gizzi
 * registers its bundled `allternit-connectors` and `allternit-tools` MCP
 * servers only when it holds the same token
 * (cmd/gizzi-code/src/runtime/tools/mcp/bundled.ts). Without it, agents on
 * Desktop and on every cloud computer (the same app, headless) have no phone,
 * mail, knowledge or channel tools.
 *
 * Desktop main makes one token, keeps it encrypted at 0600 and reuses it on
 * every launch, so an adopted (already running) API or gizzi still matches
 * the freshly spawned one. Both processes get the SAME value.
 */
import * as fs from 'node:fs';
import * as path from 'node:path';
import { randomBytes } from 'node:crypto';

const TOKEN_FORMAT = /^[a-f0-9]{64}$/i;

export interface InternalTokenStore {
  filePath: string;
  encode(value: string): Buffer;
  decode(raw: Buffer): string;
  /** Moves an unreadable file aside before a new token replaces it. */
  quarantine(filePath: string): void;
  warn?(message: string, error?: unknown): void;
}

/** Reads the persisted token, or makes and saves a new one. */
export function loadOrCreateInternalServiceToken(store: InternalTokenStore): string {
  if (fs.existsSync(store.filePath)) {
    try {
      const token = store.decode(fs.readFileSync(store.filePath)).trim();
      if (TOKEN_FORMAT.test(token)) return token;
      throw new Error('Internal service token has an invalid format');
    } catch (error) {
      store.warn?.('[Auth] Internal service token is unreadable; replacing it:', error);
      store.quarantine(store.filePath);
    }
  }
  const token = randomBytes(32).toString('hex');
  fs.mkdirSync(path.dirname(store.filePath), { recursive: true });
  fs.writeFileSync(store.filePath, store.encode(token), { mode: 0o600 });
  return token;
}

/**
 * Launch env for the two processes that must agree on the token.
 *
 * `ownerId` is the runtime's paired owner (the signed-in user's id on a
 * personal Desktop, `platform:<project_id>` on a Platform API project
 * computer). gizzi sends it as `x-allternit-user-id` on the internal MCP
 * routes; allternit-api lists `phone_text`/`phone_call` only for an owner
 * with a number and resolves Platform agents against that owner, so it must
 * be the real owner, never gizzi's `local-dev-user` fallback.
 */
export function internalServiceLaunchEnvironment(
  token: string,
  ownerId: string | null | undefined,
): { api: Record<string, string>; gizzi: Record<string, string> } {
  const owner = ownerId?.trim();
  return {
    api: { ALLTERNIT_INTERNAL_SERVICE_TOKEN: token },
    gizzi: {
      ALLTERNIT_INTERNAL_SERVICE_TOKEN: token,
      ...(owner ? { ALLTERNIT_USER_ID: owner } : {}),
    },
  };
}
