/**
 * AllternitOS control-plane API client for the platform console.
 *
 * Talks directly to an AllternitOS control-plane instance configured with
 * CORS origins that include `platform.allternit.com`. The base URL is set via
 * `VITE_ALLTERNITOS_URL`. When it is unset there is no default: `osBase()`
 * returns null and no request is made. For local development set it to
 * `http://127.0.0.1:8080` in `.env.local`.
 */

export class AllternitOSApiError extends Error {
  constructor(
    message: string,
    public statusCode: number,
    public code?: string
  ) {
    super(message);
    this.name = 'AllternitOSApiError';
  }
}

export function formatOSApiError(err: unknown, fallback: string): string {
  if (err instanceof AllternitOSApiError) return err.message;
  if (err instanceof Error) return err.message;
  return fallback;
}

function osBase(): string | null {
  const configured = String(import.meta.env.VITE_ALLTERNITOS_URL ?? '').trim();
  if (!configured) return null;
  return configured.replace(/\/+$/, '');
}

/** True when a control-plane URL is configured for this build. */
export function isOSConfigured(): boolean {
  return osBase() !== null;
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const base = osBase();
  if (base === null) {
    throw new AllternitOSApiError('AllternitOS control plane is not configured', 0, 'not_configured');
  }
  const normalizedPath = path.startsWith('/') ? path : `/${path}`;
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    Accept: 'application/json',
  };

  const controller = new AbortController();
  const timeoutMs = Number(import.meta.env.VITE_ALLTERNITOS_API_TIMEOUT_MS || 15000);
  const timeoutId = window.setTimeout(() => controller.abort(), timeoutMs);

  const response = await fetch(`${base}${normalizedPath}`, {
    method,
    headers,
    signal: controller.signal,
    ...(body && method !== 'GET' ? { body: JSON.stringify(body) } : {}),
  }).finally(() => window.clearTimeout(timeoutId));

  if (!response.ok) {
    const errorData = await response.json().catch(() => ({}));
    throw new AllternitOSApiError(
      errorData.error || `HTTP ${response.status}`,
      response.status,
      errorData.code
    );
  }

  if (response.status === 204) {
    return undefined as T;
  }

  return response.json();
}

export interface NodeRecord {
  node_id: string;
  wireguard_public_key: string;
  region: string | null;
  zone: string | null;
  rack: string | null;
  health: string | null;
  last_heartbeat_at: string | null;
  capability_record?: {
    workers?: {
      capabilities?: Array<{
        capability: string;
        actions: string[];
      }>;
    };
  };
}

export interface NodeListResult {
  nodes: NodeRecord[];
}

export interface LeaseRecord {
  lease_id: string;
  requester_principal_id: string;
  workload_id: string;
  capability: string;
  node_id: string | null;
  state: string;
  issued_at: string;
  not_after: string;
}

export interface LeaseListResult {
  leases: LeaseRecord[];
}

export interface ChargeEvent {
  id: string;
  usage_event_id: string;
  resource_id: string;
  event_type: string;
  quantity: number;
  unit: string;
  amount: number;
  currency: string;
  measured_at: string;
  acknowledged_at: string | null;
  idempotency_key: string;
}

export const osApi = {
  listNodes(): Promise<NodeListResult> {
    return request('GET', '/v1/directory/nodes');
  },

  listLeases(): Promise<LeaseListResult> {
    return request('GET', '/v1/leases');
  },

  queryChargeEvents(query: {
    event_type?: string;
    resource_id?: string;
    usage_event_id?: string;
  } = {}): Promise<ChargeEvent[]> {
    return request('POST', '/v1/charge-events/query', query);
  },

  acknowledgeChargeEvents(keys: string[]): Promise<unknown> {
    return request(
      'POST',
      '/v1/charge-events/acknowledge',
      keys.map((key) => ({
        idempotency_key: key,
        ingested_at: new Date().toISOString(),
        ledger_reference: 'platform-console',
      }))
    );
  },
};
