/**
 * Teleport API Utilities
 */

export interface TeleportApiConfig {
  baseUrl: string
  token?: string
}

/**
 * One Teleport call through the local allternit-api relay to a connected
 * computer: `teleportRequest("<targetId>", "teleports/tp_1")`.
 */
export async function teleportRequest<T>(
  endpoint: string,
  config?: TeleportApiConfig & { targetId?: string; method?: string; body?: unknown }
): Promise<T> {
  const { platformRequest } = await import("../../runtime/bots/platform-api.js")
  const path = config?.targetId
    ? `/api/v1/teleports/to/${encodeURIComponent(config.targetId)}/${endpoint.replace(/^\//, "")}`
    : `/api/v1/${endpoint.replace(/^\//, "")}`
  return platformRequest<T>((config?.method ?? "GET") as "GET", path, config?.body)
}

// Merge-by-re-export: complete counterpart (local exports win on conflict)
export { CCR_BYOC_BETA, CodeSessionSchema, axiosGetWithRetry, fetchCodeSessionsFromSessionsAPI, fetchSession, getBranchFromSession, getOAuthHeaders, isTransientNetworkError, prepareApiRequest, sendEventToRemoteSession, updateSessionTitle } from "../../shared/utils/teleport/api.js";
export type { CodeSession, GitRepositoryOutcome, GitSource, KnowledgeBaseSource, ListSessionsResponse, Outcome, OutcomeGitInfo, RemoteMessageContent, SessionContext, SessionContextSource, SessionResource, SessionStatus } from "../../shared/utils/teleport/api.js";
