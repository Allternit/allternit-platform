// @allternit/subscription-adapter-sdk — public surface (Phase 1 primitives).
export {
  SelectorNotFoundError,
  SelectorPack,
  createResolver,
} from "./selectors";
export type {
  DriftSignal,
  ResolverOptions,
  SdkSelectorResolver,
  SelectorKeyDef,
  SelectorStrategy,
} from "./selectors";

export {
  CompletionTimeout,
  awaitCompletion,
  createCompletionTracker,
} from "./completion";
export type {
  CompletionKeys,
  CompletionOptions,
  CompletionResult,
  CompletionSignals,
  CompletionTracker,
} from "./completion";

export {
  PROGRESS_KEYS,
  createHeartbeat,
  extractCounterBadge,
  extractPartialArtifacts,
  extractStepList,
  watchStreamingGrowth,
} from "./progress";
export type {
  HeartbeatHandle,
  PartialArtifactEvent,
  ProgressEmit,
  ProgressEvent,
  ProgressHeartbeat,
  StreamingGrowthWatcher,
} from "./progress";

export { attachFiles, fillComposer, submit, waitForSendReady } from "./composer";
export type { AttachFile, SendReadyOptions, SubmitOptions } from "./composer";

export { extractLastAssistantTurn } from "./extract";

export { createBannerClassifier } from "./banners";
export type { BannerClassifier, BannerMatch, BannerPattern, QuotaSignalKind } from "./banners";

export { detectAuthState, threadIdFromUrl } from "./auth";
export type { AuthState, DetectAuthOptions } from "./auth";

export { PacingCapExceeded, createPacer } from "./pacing";
export type { PacerOptions, PacingCap } from "./pacing";

// §A1 — runtime boundary impls
export {
  createExecutionContext,
  createPageLease,
  createRedactingLogger,
  redactExcerpt,
  redactText,
} from "./runtime";
export type { ExecutionContextInput, SdkPageLease } from "./runtime";

// §A3.1 + §A6.6 — download/image capture
export { CaptureError, captureDownload, captureImages, sniffMime } from "./download";
export type { CaptureImagesOptions, CaptureImagesResult, SniffedMime } from "./download";

// §A3.5 — fixture recorder/loader
export { loadFixture, recordFixture } from "./fixtures";

// §A3.4 — probe
export { probe } from "./probe";
export type { ProbeInput } from "./probe";

// §A3.3 — declarative chat adapter
export { DeclarativeChatAdapter, stalledError, timeoutError } from "./declarative";
export type { DeclarativeChatConfig, SdkAdapterRuntime } from "./declarative";

// §A3.5 — conformance suite runner
export {
  CANONICAL_FIXTURES,
  ConformanceError,
  DEFAULT_EXPECTATIONS,
  runConformance,
} from "./conformance";
export type {
  ConformanceAdapterFactory,
  ConformanceCheck,
  ConformanceOptions,
  ConformanceReport,
  ConformanceTarget,
  FixtureExpectation,
} from "./conformance";
