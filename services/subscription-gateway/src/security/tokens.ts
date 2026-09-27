// §A6.2 — per-caller scoped bearer tokens. Plaintext is shown once at
// issuance; only the sha256 hash is stored (§A6.3 moves the hash to the
// Keychain later — the tokens table is the Phase 1 backing store).
import { createHash, randomBytes, randomUUID, timingSafeEqual } from "node:crypto";
import type { Db } from "../store/db.js";
import { findTokenByHash, type TokenRow } from "../store/queries.js";

export const TOKEN_SCOPES = [
  "tasks:submit",
  "tasks:read",
  "artifacts:read",
  "accounts:manage",
  "approve:external_publish",
] as const;
export type TokenScope = (typeof TOKEN_SCOPES)[number];

const TOKEN_PREFIX = "sgw_";

export interface IssuedToken {
  token: string; // plaintext — returned once, never stored
  record: TokenRow;
}

export interface VerifiedCaller {
  caller_id: string;
  scopes: TokenScope[];
}

function hashToken(plaintext: string): string {
  return createHash("sha256").update(plaintext, "utf8").digest("hex");
}

export function issueToken(
  db: Db,
  callerId: string,
  name: string,
  scopes: TokenScope[]
): IssuedToken {
  const token = TOKEN_PREFIX + randomBytes(32).toString("base64url");
  const record: TokenRow = {
    token_id: randomUUID(),
    caller_id: callerId,
    name,
    token_hash: hashToken(token),
    scopes: [...scopes],
    created_at: new Date().toISOString(),
    revoked_at: null,
  };
  db.prepare(
    `INSERT INTO tokens (token_id, caller_id, name, token_hash, scopes, created_at, revoked_at)
     VALUES (?, ?, ?, ?, ?, ?, NULL)`
  ).run(
    record.token_id,
    record.caller_id,
    record.name,
    record.token_hash,
    JSON.stringify(record.scopes),
    record.created_at
  );
  return { token, record };
}

export function verifyToken(db: Db, presented: string): VerifiedCaller | null {
  const presentedHash = hashToken(presented);
  const row = findTokenByHash(db, presentedHash);
  if (!row || row.revoked_at !== null) return null;
  const a = Buffer.from(presentedHash, "utf8");
  const b = Buffer.from(row.token_hash, "utf8");
  if (a.length !== b.length || !timingSafeEqual(a, b)) return null;
  return { caller_id: row.caller_id, scopes: row.scopes as TokenScope[] };
}

export function revokeToken(db: Db, tokenId: string): void {
  db.prepare(
    "UPDATE tokens SET revoked_at = ? WHERE token_id = ? AND revoked_at IS NULL"
  ).run(new Date().toISOString(), tokenId);
}

// CLI bootstrap (§A6.2): the `allternit` CLI authenticates with a `cli` caller
// token the gateway issues at first boot and stores in the keychain under
// account `cli-token`; the CLI reads it back with `security
// find-generic-password`. Re-issued when the keychain entry is missing or no
// longer verifies (fresh DB).
export const CLI_TOKEN_ACCOUNT = "cli-token";

export function ensureCliToken(
  db: Db,
  keychain: { get(account: string): string | null; set(account: string, value: string): void }
): { issued: boolean } {
  const existing = keychain.get(CLI_TOKEN_ACCOUNT);
  if (existing !== null && verifyToken(db, existing)) return { issued: false };
  const { token } = issueToken(db, "cli", CLI_TOKEN_ACCOUNT, [
    "tasks:submit",
    "tasks:read",
    "artifacts:read",
    "accounts:manage",
  ]);
  keychain.set(CLI_TOKEN_ACCOUNT, token);
  return { issued: true };
}
