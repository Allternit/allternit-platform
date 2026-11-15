/**
 * Governance client and helpers for the console: key expiry status, CIDR
 * allowlist validation, the audit log and the organization policy.
 *
 * Routes (Platform API, Clerk session):
 *   GET  /api/v1/platform/audit-log?project_id&action&from&to&after&format
 *   GET|PUT /api/v1/platform/org/policy
 */

import { api } from "@/lib/api-client";

// ── Key status ──────────────────────────────────────────────────────────────

export type KeyStatusState = "active" | "expiring" | "expired";

export interface KeyStatus {
  state: KeyStatusState;
  /** Whole days left (rounded up). Null when the key has no expiry. */
  daysLeft: number | null;
  /** "Active", "Expires in N days" or "Expired". */
  label: string;
}

/** A key that expires within this many days is flagged as "expiring". */
export const EXPIRING_SOON_DAYS = 14;
const DAY_MS = 24 * 60 * 60 * 1000;

export function keyStatus(
  key: { expires_at?: string | null; revoked_at?: string | null },
  now: Date = new Date(),
): KeyStatus {
  const raw = key.expires_at;
  if (!raw) return { state: "active", daysLeft: null, label: "Active" };
  const at = new Date(raw).getTime();
  if (Number.isNaN(at)) return { state: "active", daysLeft: null, label: "Active" };
  const diff = at - now.getTime();
  if (diff <= 0) return { state: "expired", daysLeft: 0, label: "Expired" };
  const daysLeft = Math.max(1, Math.ceil(diff / DAY_MS));
  return {
    state: daysLeft <= EXPIRING_SOON_DAYS ? "expiring" : "active",
    daysLeft,
    label: `Expires in ${daysLeft} ${daysLeft === 1 ? "day" : "days"}`,
  };
}

// ── CIDR validation ─────────────────────────────────────────────────────────

function isIPv4(s: string): boolean {
  const parts = s.split(".");
  if (parts.length !== 4) return false;
  return parts.every((p) => /^\d{1,3}$/.test(p) && Number(p) <= 255 && (p === "0" || !p.startsWith("0")));
}

function isIPv6(s: string): boolean {
  if (!s.includes(":") || /[^0-9a-fA-F:]/.test(s)) return false;
  const doubles = s.split("::").length - 1;
  if (doubles > 1 || s.includes(":::")) return false;
  const groupsOk = (part: string) => part === "" || part.split(":").every((g) => /^[0-9a-fA-F]{1,4}$/.test(g));
  if (doubles === 1) {
    const [head, tail] = s.split("::");
    if (!groupsOk(head) || !groupsOk(tail)) return false;
    const count = (head ? head.split(":").length : 0) + (tail ? tail.split(":").length : 0);
    return count <= 7;
  }
  const groups = s.split(":");
  return groups.length === 8 && groupsOk(s);
}

/** True for `1.2.3.4`, `10.0.0.0/8`, `2001:db8::/32` and bare addresses. */
export function isValidCidr(input: string): boolean {
  const value = input.trim();
  if (!value) return false;
  const slash = value.indexOf("/");
  const addr = slash === -1 ? value : value.slice(0, slash);
  const prefix = slash === -1 ? null : value.slice(slash + 1);
  const v4 = isIPv4(addr);
  const v6 = !v4 && isIPv6(addr);
  if (!v4 && !v6) return false;
  if (prefix === null) return true;
  if (!/^\d{1,3}$/.test(prefix)) return false;
  return Number(prefix) <= (v4 ? 32 : 128);
}

/** Bare addresses become /32 (IPv4) or /128 (IPv6). */
export function normalizeCidr(input: string): string {
  const value = input.trim();
  if (value.includes("/")) return value;
  return value.includes(":") ? `${value}/128` : `${value}/32`;
}

/** Splits free text (commas, spaces or new lines) into valid, normalized CIDRs and the entries that failed. */
export function parseCidrList(text: string): { cidrs: string[]; invalid: string[] } {
  const entries = text
    .split(/[\s,;]+/)
    .map((s) => s.trim())
    .filter(Boolean);
  const cidrs: string[] = [];
  const invalid: string[] = [];
  for (const entry of entries) {
    if (isValidCidr(entry)) {
      const normal = normalizeCidr(entry);
      if (!cidrs.includes(normal)) cidrs.push(normal);
    } else {
      invalid.push(entry);
    }
  }
  return { cidrs, invalid };
}

// ── Expiry helpers ──────────────────────────────────────────────────────────

export const EXPIRY_PRESET_DAYS = [7, 30, 90] as const;

export function expiryFromDays(days: number, now: Date = new Date()): string {
  return new Date(now.getTime() + days * DAY_MS).toISOString();
}

/** A custom date (YYYY-MM-DD) expires at the end of that day, local time. */
export function expiryFromDate(date: string): string | null {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date)) return null;
  const d = new Date(`${date}T23:59:59`);
  return Number.isNaN(d.getTime()) ? null : d.toISOString();
}

/** Returns an error message, or null when the expiry satisfies the policy. */
export function validateExpiry(
  expiresAt: string | null,
  policy: Pick<OrgPolicy, "require_key_expiry" | "max_key_ttl_days"> | null,
  now: Date = new Date(),
): string | null {
  if (!expiresAt) {
    return policy?.require_key_expiry ? "Your organization requires keys to expire. Pick an expiry." : null;
  }
  const at = new Date(expiresAt).getTime();
  if (Number.isNaN(at)) return "That expiry date is not valid.";
  if (at <= now.getTime()) return "The expiry must be in the future.";
  const max = policy?.max_key_ttl_days;
  if (max && at - now.getTime() > max * DAY_MS) {
    return `Your organization limits keys to ${max} days.`;
  }
  return null;
}

// ── Organization policy ─────────────────────────────────────────────────────

export interface OrgPolicy {
  require_key_expiry: boolean;
  max_key_ttl_days: number | null;
  live_keys_admin_only: boolean;
  ip_allowlist: string[];
  spend_alert_email: string | null;
  updated_at?: string | null;
}

export const DEFAULT_ORG_POLICY: OrgPolicy = {
  require_key_expiry: false,
  max_key_ttl_days: null,
  live_keys_admin_only: false,
  ip_allowlist: [],
  spend_alert_email: null,
};

export async function getOrgPolicy(): Promise<OrgPolicy> {
  const row = await api.get<Partial<OrgPolicy>>("/api/v1/platform/org/policy");
  return { ...DEFAULT_ORG_POLICY, ...row, ip_allowlist: row.ip_allowlist ?? [] };
}

export async function putOrgPolicy(input: OrgPolicy): Promise<OrgPolicy> {
  const row = await api.put<Partial<OrgPolicy>>("/api/v1/platform/org/policy", {
    require_key_expiry: input.require_key_expiry,
    max_key_ttl_days: input.max_key_ttl_days,
    live_keys_admin_only: input.live_keys_admin_only,
    ip_allowlist: input.ip_allowlist,
    spend_alert_email: input.spend_alert_email,
  });
  return { ...DEFAULT_ORG_POLICY, ...row, ip_allowlist: row.ip_allowlist ?? [] };
}

// ── Audit log ───────────────────────────────────────────────────────────────

export interface AuditEvent {
  id: string;
  action: string;
  actor_email?: string | null;
  resource?: string | null;
  ip?: string | null;
  created_at: string;
}

export interface AuditFilters {
  /** Omit for every project in the organization. */
  project_id?: string;
  action?: string;
  /** YYYY-MM-DD */
  from?: string;
  /** YYYY-MM-DD */
  to?: string;
}

export interface AuditPage {
  data: AuditEvent[];
  /** Cursor for the next page, or null at the end. */
  next: string | null;
}

function auditQuery(filters: AuditFilters, extra: Record<string, string | undefined> = {}): string {
  const params = new URLSearchParams();
  const all: Record<string, string | undefined> = { ...filters, ...extra };
  for (const [k, v] of Object.entries(all)) if (v) params.set(k, v);
  const q = params.toString();
  return q ? `?${q}` : "";
}

export async function listAuditLog(filters: AuditFilters, after?: string): Promise<AuditPage> {
  const res = await api.get<{ data: AuditEvent[]; has_more?: boolean; next_cursor?: string | null }>(
    `/api/v1/platform/audit-log${auditQuery(filters, { after, limit: "50" })}`,
  );
  const data = res.data ?? [];
  const next = res.next_cursor ?? (res.has_more && data.length > 0 ? data[data.length - 1].id : null);
  return { data, next };
}

/** Downloads the filtered log as CSV through the browser. */
export async function downloadAuditCsv(filters: AuditFilters): Promise<void> {
  const res = await api.raw(`/api/v1/platform/audit-log${auditQuery(filters, { format: "csv" })}`, {
    headers: { Accept: "text/csv" },
  });
  if (!res.ok) {
    const body = (await res.json().catch(() => ({}))) as { error?: string; message?: string };
    throw new Error(body.error || body.message || `Download failed (HTTP ${res.status})`);
  }
  const blob = await res.blob();
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = "audit-log.csv";
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}

// ── Roles ───────────────────────────────────────────────────────────────────

/** Console role names mapped to Clerk organization roles. */
export const ORG_ROLES: { value: string; clerk: string; label: string; hint: string }[] = [
  { value: "admin", clerk: "org:admin", label: "Admin", hint: "Manages members, policy and live keys" },
  { value: "developer", clerk: "org:developer", label: "Developer", hint: "Creates sandbox keys and works in projects" },
  { value: "billing", clerk: "org:billing", label: "Billing", hint: "Manages payment methods and spend" },
  { value: "viewer", clerk: "org:viewer", label: "Viewer", hint: "Read-only" },
];
