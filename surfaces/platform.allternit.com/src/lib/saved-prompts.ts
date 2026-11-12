/**
 * Saved Playground prompts for the selected project, and share links.
 * Routes (Clerk session): /api/v1/platform/projects/:id/prompts[...] and
 * /api/v1/platform/shared-prompts/:token (signed-in only).
 */

import { api } from "@/lib/api-client";
import type { ChatCompletionMessage } from "@/lib/console-gateway";
import type { PlaygroundParams } from "@/lib/playground-request";

/** What a saved prompt stores in `body` (images are never stored). */
export interface PromptBody {
  model: string;
  systemPrompt: string;
  messages: ChatCompletionMessage[];
  params: PlaygroundParams;
}

export interface SavedPrompt {
  id: string;
  name: string;
  body: PromptBody;
  share_url?: string | null;
  updated_at: string;
}

export interface SharedPrompt {
  name: string;
  body: PromptBody;
}

function base(projectId: string): string {
  return `/api/v1/platform/projects/${encodeURIComponent(projectId)}/prompts`;
}

/** Accepts a bare array or `{data: [...]}`. */
function unwrapList<T>(res: unknown): T[] {
  if (Array.isArray(res)) return res as T[];
  const data = (res as { data?: unknown } | null)?.data;
  return Array.isArray(data) ? (data as T[]) : [];
}

export async function listPrompts(projectId: string): Promise<SavedPrompt[]> {
  return unwrapList<SavedPrompt>(await api.get(base(projectId)));
}

export function savePrompt(projectId: string, name: string, body: PromptBody): Promise<SavedPrompt> {
  return api.post<SavedPrompt>(base(projectId), { name, body });
}

export function updatePrompt(
  projectId: string,
  id: string,
  patch: { name?: string; body?: PromptBody }
): Promise<SavedPrompt> {
  return api.patch<SavedPrompt>(`${base(projectId)}/${encodeURIComponent(id)}`, patch);
}

export async function deletePrompt(projectId: string, id: string): Promise<void> {
  await api.delete(`${base(projectId)}/${encodeURIComponent(id)}`);
}

export function sharePrompt(projectId: string, id: string): Promise<{ share_url: string }> {
  return api.post<{ share_url: string }>(`${base(projectId)}/${encodeURIComponent(id)}/share`);
}

export async function unsharePrompt(projectId: string, id: string): Promise<void> {
  await api.delete(`${base(projectId)}/${encodeURIComponent(id)}/share`);
}

export function getSharedPrompt(token: string): Promise<SharedPrompt> {
  return api.get<SharedPrompt>(`/api/v1/platform/shared-prompts/${encodeURIComponent(token)}`);
}

/** A past request loaded for replay (Wave 3 Logs). Null when no body was captured. */
export async function fetchReplayRequest(requestId: string): Promise<Record<string, unknown> | null> {
  const detail = await api.get<{ request?: Record<string, unknown> | null }>(
    `/api/v1/me/usage/requests/${encodeURIComponent(requestId)}`
  );
  return detail.request && typeof detail.request === "object" ? detail.request : null;
}
