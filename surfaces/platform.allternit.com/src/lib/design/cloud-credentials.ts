// BYOC (Bring Your Own Cloud) credential client for platform.allternit.com.
//
// Talks to the org-scoped cloud-credentials API at /api/v1/cloud-credentials
// on the Allternit gateway. Enterprise customers connect their own AWS/GCP/Azure
// account so ACU sandboxes can provision into the customer's cloud instead of
// allternit's. Tokens are sent via the shared api client so Clerk session
// bearer auth and the gateway base URL are handled consistently.

import { api } from '@/lib/api-client';
import { getAcuGatewayBaseUrl } from '@/lib/acu-gateway';

export type CloudProvider = 'aws' | 'gcp' | 'azure';

export interface CloudCredential {
  id: string;
  provider: CloudProvider;
  label: string;
  region?: string | null;
  external_id?: string | null;
  status: 'active' | 'revoked' | 'error';
  last_validated_at?: string | null;
  created_at: string;
}

export interface CreateCloudCredentialInput {
  provider: CloudProvider;
  label: string;
  region?: string;
  external_id?: string;
  /** Provider-shaped secret payload -- e.g. { role_arn: "..." } for AWS,
   * { service_account_json: {...} } for GCP, { client_secret: "..." } for
   * Azure. Sealed server-side before storage; never persisted client-side. */
  secret: Record<string, unknown>;
}

const BASE = '/api/v1/cloud-credentials';

export async function listCloudCredentials(): Promise<CloudCredential[]> {
  const data = await api.get<{ cloud_credentials: CloudCredential[] }>(BASE);
  return data.cloud_credentials ?? [];
}

export async function createCloudCredential(input: CreateCloudCredentialInput): Promise<CloudCredential> {
  return api.post<CloudCredential>(BASE, input);
}

export async function revokeCloudCredential(id: string): Promise<void> {
  return api.delete(`${BASE}/${encodeURIComponent(id)}`);
}

export interface TestCloudCredentialInput {
  provider: CloudProvider;
  region?: string;
  external_id?: string;
  secret: Record<string, unknown>;
}

export interface TestCloudCredentialResult {
  success: boolean;
  message: string;
  identity?: Record<string, unknown> | null;
}

/** Live-validates a not-yet-saved credential directly against the real cloud
 * provider via the ACU gateway. Nothing is persisted by this call. */
export async function testCloudCredential(input: TestCloudCredentialInput): Promise<TestCloudCredentialResult> {
  const gatewayBase = getAcuGatewayBaseUrl();
  if (!gatewayBase) {
    throw new Error('Credential testing is not available: no gateway is configured.');
  }
  const res = await fetch(`${gatewayBase}/v1/cloud-credentials/test`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(input),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    throw new Error(data.message || data.error || `Credential test failed (${res.status})`);
  }
  return data as TestCloudCredentialResult;
}
