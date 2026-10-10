/**
 * Allternit Browser Module
 * 
 * Complete browser automation and canvas hosting system.
 * Ported from OpenClaw architecture.
 * 
 * @module @allternit/browser
 */

// Types
export * from './types/index.js';

// Browser Server
export {
  startBrowserServer,
  stopBrowserServer,
  getBrowserState,
} from './browser/server.js';
export { createBrowserRouteContext } from './browser/server-context.js';

// Browser Routes
export { registerBrowserRoutes } from './browser/routes/index.js';
export { registerBrowserBasicRoutes } from './browser/routes/basic.js';
export { registerBrowserTabRoutes } from './browser/routes/tabs.js';
export { registerBrowserAgentRoutes } from './browser/routes/agent.js';
export { registerBrowserProtocolRoutes } from './browser/routes/protocol.js';

// CDP (Chrome DevTools Protocol)
export { CDPClient } from './browser/cdp/client.js';
export { CDPBridge } from './cdp-bridge.js';
export {
  getTabs,
  openTab,
  focusTab,
  closeTab,
  getTabAction,
} from './browser/cdp/tabs.js';
export {
  captureScreenshot,
  normalizeScreenshot,
} from './browser/cdp/screenshot.js';
export {
  snapshotAria,
  snapshotAi,
  snapshotRole,
} from './browser/cdp/snapshot.js';

// Playwright Integration
export {
  launchBrowser,
  connectViaCDP,
} from './browser/playwright/launcher.js';
export {
  clickViaPlaywright,
  typeViaPlaywright,
  navigateViaPlaywright,
  takeScreenshotViaPlaywright,
  hoverViaPlaywright,
  pressKeyViaPlaywright,
  evaluateViaPlaywright,
  waitForViaPlaywright,
} from './browser/playwright/actions.js';
export {
  snapshotAiViaPlaywright,
  snapshotRoleViaPlaywright,
  snapshotAriaViaPlaywright,
} from './browser/playwright/snapshot.js';
export { pdfViaPlaywright } from './browser/playwright/pdf.js';
export { LocalPlaywrightProvider } from './protocol/local-provider.js';
export type {
  LocalBrowserSessionBinding,
  StartRecordedSessionInput,
  StartedRecordedSession,
} from './protocol/local-provider.js';
export {
  defaultRecordingsRoot,
  finalizeVideoRecording,
  prepareVideoRecording,
  recordVideoOptions,
  recordingArtifactPath,
  recordingStagingDir,
} from './protocol/video-recorder.js';
export type { FinalizedVideo, VideoRecordingStart } from './protocol/video-recorder.js';
export { BrowserRunController } from './protocol/run-controller.js';
export type {
  BrowserRunControllerOptions,
  ExecuteBrowserActionInput,
  ExecuteBrowserActionResult,
  StartBrowserRunInput,
} from './protocol/run-controller.js';
export {
  ExtensionTabProvider,
  RemoteBrowserProvider,
  createStagehandProvider,
  providerKind,
} from './protocol/remote-provider.js';
export type {
  ExtensionTabProviderMessage,
  ExtensionTabProviderOptions,
  FetchLike,
  RemoteBrowserProviderOptions,
} from './protocol/remote-provider.js';
export { compileBrowserTrajectoryToSkill } from './protocol/skill-factory.js';
export type {
  BrowserSkillPackage,
  CompileBrowserSkillOptions,
} from './protocol/skill-factory.js';
export {
  acuRecordingJsonlToTrajectory,
  loadAcuRecordingToTrajectory,
  mapAcuActionKind,
  normalizeIsoTimestamp,
  resolveRecordingPath,
} from './protocol/recording-to-trajectory.js';
export type {
  AcuRecordedFrame,
  AcuRecordingManifest,
  AcuRecordingToTrajectoryOptions,
} from './protocol/recording-to-trajectory.js';

// Canvas Host
export {
  A2UI_PATH,
  CANVAS_HOST_PATH,
  CANVAS_WS_PATH,
  handleA2uiHttpRequest,
  injectCanvasLiveReload,
} from './canvas-host/a2ui.js';
export {
  createCanvasHostHandler,
  startCanvasHost,
} from './canvas-host/server.js';

// Version
export const VERSION = '0.1.0';
