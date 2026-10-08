/**
 * Platform API console client: projects and their project keys.
 *
 * Talks to the Allternit Cloud API at /api/v1/platform/* with the signed-in
 * session. Project keys (`alt_test_…` for sandbox, `alt_live_…` for live) are
 * shown in full only once, at creation. While the Platform API is switched off
 * for a deployment these routes answer 404 `platform_api_disabled`.
 */

import { api, AllternitApiError } from "@/lib/api-client";

export type ProjectEnv = "sandbox" | "live";

export interface PlatformProject {
  id: string;
  name: string;
  env: ProjectEnv;
  plan: string;
  spend_cap_cents: number;
  created_at: string;
  archived_at?: string | null;
}

export interface ProjectKey {
  id: string;
  project_id: string;
  account_id?: string | null;
  env: ProjectEnv;
  name: string;
  prefix: string;
  scopes: string[];
  last_used_at?: string | null;
  created_at: string;
}

export interface CreatedProjectKey extends ProjectKey {
  token: string;
}

interface Page<T> {
  data: T[];
  has_more: boolean;
  next_cursor: string | null;
}

/** Scopes a project key can carry (matches the cloud API's PLATFORM_SCOPES). */
export const PROJECT_KEY_SCOPES: { value: string; label: string; hint: string }[] = [
  { value: "agents", label: "Agents", hint: "Agents and conversations" },
  { value: "voice", label: "Voice", hint: "Calls and realtime sessions" },
  { value: "messaging", label: "Messaging", hint: "Texts" },
  { value: "numbers", label: "Numbers", hint: "Phone numbers and carrier registration" },
  { value: "channels", label: "Channels", hint: "Channel connections" },
  { value: "twin", label: "Twin", hint: "People, inbox, approvals, autonomy, memory" },
  { value: "webhooks", label: "Webhooks", hint: "Webhook endpoints" },
  { value: "usage", label: "Usage", hint: "Usage reports" },
  { value: "inference", label: "Inference", hint: "/v1/chat/completions" },
  { value: "computers", label: "Computers", hint: "Hosted computers and the computer/browser toolsets" },
];

/** True when the deployment has the Platform API switched off. */
export function isPlatformDisabled(err: unknown): boolean {
  return err instanceof AllternitApiError && err.statusCode === 404 && err.code === "platform_api_disabled";
}

export async function listProjects(): Promise<PlatformProject[]> {
  const page = await api.get<Page<PlatformProject>>("/api/v1/platform/projects?limit=100");
  return page.data.filter((p) => !p.archived_at);
}

export async function createProject(input: { name: string; env: ProjectEnv }): Promise<PlatformProject> {
  return api.post<PlatformProject>("/api/v1/platform/projects", input);
}

export async function listProjectKeys(projectId: string): Promise<ProjectKey[]> {
  const page = await api.get<Page<ProjectKey>>(`/api/v1/platform/projects/${encodeURIComponent(projectId)}/keys`);
  return page.data;
}

export async function createProjectKey(
  projectId: string,
  input: { name: string; scopes: string[]; account_id?: string },
): Promise<CreatedProjectKey> {
  return api.post<CreatedProjectKey>(`/api/v1/platform/projects/${encodeURIComponent(projectId)}/keys`, input);
}

export async function revokeProjectKey(projectId: string, keyId: string): Promise<void> {
  await api.delete(`/api/v1/platform/projects/${encodeURIComponent(projectId)}/keys/${encodeURIComponent(keyId)}`);
}

export async function updateProject(
  projectId: string,
  input: { name?: string; spend_cap_cents?: number; archived?: boolean },
): Promise<PlatformProject> {
  return api.patch<PlatformProject>(`/api/v1/platform/projects/${encodeURIComponent(projectId)}`, input);
}
