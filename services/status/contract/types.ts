// Allternit status contract v1. Mirror of status.schema.json. Keep both in sync.

export type Status = "operational" | "degraded" | "down" | "maintenance" | "unknown";
export type ComponentGroup =
  | "website" | "app" | "mobile" | "signin" | "api" | "cloud-computers" | "phone-push"
  | "mail" | "console" | "docs" | "services" | "downloads" | "other";
export type IncidentStatus = "investigating" | "identified" | "monitoring" | "resolved";
export type IncidentImpact = "degraded" | "down" | "maintenance";
export type VantageId = "cloudflare" | "allternit-standby";

export interface Component {
  id: string;
  name: string;
  status: Status;
  /** Fraction 0..1, null = no data. */
  uptime_90d: number | null;
  last_change: string | null;
  critical?: boolean;
}

export interface IncidentUpdate {
  id?: number;
  ts: string;
  status: IncidentStatus;
  body: string;
}

export interface Incident {
  id: string;
  title: string;
  state: "open" | "resolved";
  impact: IncidentImpact;
  component_ids: string[];
  started_at: string;
  resolved_at: string | null;
  /** false once a human edits; automation then only appends "Resolved". */
  auto: boolean;
  updates: IncidentUpdate[];
}

/** GET /api/v1/status.json (open incidents only). */
export interface StatusDoc {
  schema: "allternit.status/v1";
  last_checked_at: string;
  overall: Status;
  components: Component[];
  incidents: Incident[];
}

export interface HistoryDay {
  date: string; // YYYY-MM-DD (UTC)
  status: Status | null; // null = no data
  uptime: number | null; // fraction 0..1, null = no data
}

/** GET /api/v1/history.json?days=90 */
export interface HistoryDoc {
  schema: "allternit.history/v1";
  days: number;
  generated_at: string;
  components: { id: string; days: HistoryDay[] }[];
}

/** GET /api/v1/incidents.json (open and resolved, newest first). */
export interface IncidentsDoc {
  schema: "allternit.incidents/v1";
  incidents: Incident[];
}

/** GET /api/health */
export interface HealthDoc {
  status: "ok" | "stale";
  last_checked_at: string | null;
  age_seconds: number | null;
}

export interface TargetComponent {
  id: string;
  name: string;
  group: ComponentGroup;
  critical: boolean;
  /** "degraded" for Other sites: they never turn the page red. */
  max_status?: "degraded" | "down";
}

export interface Target {
  id: string;
  component_id: string;
  name: string;
  url: string;
  method: "GET" | "HEAD";
  expect_status: number;
  follow_redirects?: boolean;
  /** Substring that must appear in the body; null = status code only. */
  body_marker: string | null;
  critical: boolean;
  role?: "live" | "ready" | "page" | "proxy";
  vantages: VantageId[];
  timeout_ms?: number;
}

/** GET /api/v1/targets.json */
export interface TargetsDoc {
  schema: "allternit.targets/v1";
  version: number;
  components: TargetComponent[];
  targets: Target[];
}

export interface ProbeResult {
  target_id: string;
  ok: boolean;
  status_code: number | null;
  latency_ms: number | null;
  marker_ok?: boolean | null;
  error?: string | null;
}

/** POST /api/v1/ingest. Header `X-Allternit-Signature: sha256=<hex HMAC-SHA256 of raw body>`. */
export interface IngestPayload {
  schema: "allternit.ingest/v1";
  vantage: "allternit-standby";
  ts: string;
  targets_version?: number;
  results: ProbeResult[];
}
export const INGEST_SIGNATURE_HEADER = "X-Allternit-Signature";

/** POST /api/admin/incidents (bearer). */
export interface AdminIncidentCreate {
  schema: "allternit.admin.incident.create/v1";
  title: string;
  impact: IncidentImpact;
  component_ids: string[];
  body: string;
  status?: "investigating" | "identified" | "monitoring";
}

/** PATCH /api/admin/incidents/:id (bearer). Any edit sets auto=false. */
export interface AdminIncidentPatch {
  schema: "allternit.admin.incident.patch/v1";
  title?: string;
  status?: IncidentStatus;
  body?: string;
  component_ids?: string[];
}
