export { AllternitPlatform, Conversations, ConversationStream } from "./client.ts";
export type { ConversationStreamEvent, MessageDeltaEvent, MessageCompletedEvent } from "./client.ts";
export { HttpTransport, parseSSE, DEFAULT_BASE_URL } from "./core.ts";
export type { ClientOptions, RequestOptions, Page, SSEEvent, Transport, RequestSpec } from "./core.ts";
export * from "./errors.ts";
export * from "./generated/operations.ts";
export type * from "./generated/types.ts";
// Self-hosted Agents Server (allternit-agents-server). Its generated resources
// share names with the Platform ones, so they live under the `selfhost` namespace.
export { SelfHostClient, runCustomToolWorker, isTurnEnd } from "./selfhost/index.ts";
export type { SelfHostClientOptions, CustomToolWorkerOptions, ToolHandler, ToolCallContext, StreamEventsOptions } from "./selfhost/index.ts";
export * as selfhost from "./selfhost/index.ts";
export { AllternitPlatform as default } from "./client.ts";
