/**
 * Self-hosted Allternit Agents Server client. Import from
 * `@allternit/platform/selfhost`, or use `SelfHostClient` from the package root.
 */
export { SelfHostClient, SelfHostSessions, SELFHOST_DEFAULT_BASE_URL, TURN_END_STATUSES, isTurnEnd } from "./client.ts";
export type { SelfHostClientOptions, StreamEventsOptions } from "./client.ts";
export { runCustomToolWorker, truncateUtf8, MAX_TOOL_RESULT_BYTES, RENEW_EVERY_MS } from "./worker.ts";
export type { CustomToolWorkerOptions, ToolCallContext, ToolHandler } from "./worker.ts";
export * from "./operations.ts";
export type * from "./types.ts";
export { SelfHostClient as default } from "./client.ts";
