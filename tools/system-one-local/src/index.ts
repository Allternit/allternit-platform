// Library entry.
//
//   import { SystemOne } from "allternit-system-one";
//   const res = await new SystemOne().evaluate({ model: "jev-latest", state, questions });
export { SystemOne, configFromEnv, type EngineConfig } from "./engine.ts";
export { LocalBackend, LOCAL_ALIASES } from "./backends/local.ts";
export { OpenAICompatRuntime, type ChatRuntime, type CompletionRequest, type CompletionResult } from "./backends/runtime.ts";
export { TypeSafeBackend } from "./backends/typesafe.ts";
export { routeModel, RouteModelRefused } from "./route-model.ts";
export { createHandler, serve, HOST, DEFAULT_PORT } from "./server.ts";
export { validateRequest } from "./validate.ts";
export { confidence, weightedScore, labelDistribution, voteDistribution } from "./math.ts";
export * from "./types.ts";
