/**
 * Centralized desktop port and URL configuration.
 *
 * All hard-coded ports should live here so the desktop, backend, gizzi,
 * and tunnel managers agree on a single source of truth.
 */

export const PORTS = {
  /** Rust operator API (allternit-api) — production port; only the packaged desktop may bind it */
  API: 8013,
  /** Rust operator API dev default — what an allternit-api with no explicit env binds */
  API_DEV: 18013,
  /** Gizzi Code AI runtime terminal server */
  GIZZI: 4096,
  /** Local development Vite UI server */
  DEV_UI: 3014,
  /** Rails backend (when running locally) */
  RAILS: 3021,
  /** Chrome extension native-messaging bridge */
  EXTENSION_BRIDGE: 3011,
  /** ACU (Python assistant) extension relay */
  ACU_RELAY: 3012,
  /** ACU computer-use FastAPI gateway (`/v1/computer-use`, `/health`) */
  ACU: 8760,
  /** Open Notebook research backend */
  NOTEBOOK: 5055,
  /** Local speech-to-text and text-to-speech service */
  VOICE: 8001,
  /** Dedicated inbound webhook receiver (bot-owned triggers) */
  WEBHOOK_RECEIVER: 8080,
  /** open-connector connector sidecar (see services/open-connector) */
  CONNECTOR_SIDECAR: 8014,
  /** Office engine sidecar (services/office-engine) backing /api/office/* */
  OFFICE_ENGINE: 8099,
  /** Local model engine (services/local-engine) serving Model Lab telemetry */
  LOCAL_ENGINE: 3015,
  /**
   * Allternit Factory engine (`allternit-factory serve`). Not the engine's own
   * default 3011: EXTENSION_BRIDGE owns 3011 in Desktop. allternit-api reaches
   * it through ALLTERNIT_FACTORY_URL.
   */
  FACTORY: 3018,
} as const;

export const HOSTS = {
  LOOPBACK: '127.0.0.1',
  LOCALHOST: 'localhost',
} as const;

export const URLS = {
  API: `http://${HOSTS.LOOPBACK}:${PORTS.API}`,
  GIZZI: `http://${HOSTS.LOOPBACK}:${PORTS.GIZZI}`,
  DEV_UI: `http://${HOSTS.LOCALHOST}:${PORTS.DEV_UI}`,
  RAILS: `http://${HOSTS.LOOPBACK}:${PORTS.RAILS}`,
  EXTENSION_BRIDGE: `http://${HOSTS.LOOPBACK}:${PORTS.EXTENSION_BRIDGE}`,
  ACU_RELAY: `http://${HOSTS.LOOPBACK}:${PORTS.ACU_RELAY}`,
  ACU: `http://${HOSTS.LOOPBACK}:${PORTS.ACU}`,
  NOTEBOOK: `http://${HOSTS.LOOPBACK}:${PORTS.NOTEBOOK}`,
  VOICE: `http://${HOSTS.LOOPBACK}:${PORTS.VOICE}`,
  WEBHOOK_RECEIVER: `http://${HOSTS.LOOPBACK}:${PORTS.WEBHOOK_RECEIVER}`,
  CONNECTOR_SIDECAR: `http://${HOSTS.LOOPBACK}:${PORTS.CONNECTOR_SIDECAR}`,
  OFFICE_ENGINE: `http://${HOSTS.LOOPBACK}:${PORTS.OFFICE_ENGINE}`,
  LOCAL_ENGINE: `http://${HOSTS.LOOPBACK}:${PORTS.LOCAL_ENGINE}`,
  /** Canonical Allternit control plane. Human Clerk sessions approve runtime pairing here. */
  CLOUD_API: 'https://api.allternit.com',
  /** Canonical browser experience. Pairing must never follow a local static UI URL. */
  PLATFORM: 'https://ai.allternit.com',
  PRODUCTION_UI: 'https://ai.allternit.com',
  /** Local static platform UI fallback served by the Rust API at the root path. */
  PLATFORM_STATIC: `http://${HOSTS.LOOPBACK}:${PORTS.API}`,
} as const;

/** Build a URL for the local API with an optional path. */
export function apiUrl(path?: string): string {
  return path ? `${URLS.API}${path}` : URLS.API;
}

/** Build a URL for the local Gizzi runtime with an optional path. */
export function gizziUrl(path?: string): string {
  return path ? `${URLS.GIZZI}${path}` : URLS.GIZZI;
}

/** Build a URL for the local static platform UI fallback served by the Rust API. */
export function staticUiUrl(path?: string): string {
  return path ? `${URLS.PLATFORM_STATIC}${path}` : URLS.PLATFORM_STATIC;
}

/** Build a URL for the dedicated inbound webhook receiver. */
export function webhookReceiverUrl(path?: string): string {
  return path ? `${URLS.WEBHOOK_RECEIVER}${path}` : URLS.WEBHOOK_RECEIVER;
}

/** Build a URL for the development UI server with an optional path. */
export function devUiUrl(path?: string): string {
  return path ? `${URLS.DEV_UI}${path}` : URLS.DEV_UI;
}

/** Build a URL for the Chrome extension bridge with an optional path. */
export function extensionBridgeUrl(path?: string): string {
  return path ? `${URLS.EXTENSION_BRIDGE}${path}` : URLS.EXTENSION_BRIDGE;
}

/** Build a URL for the ACU extension relay with an optional path. */
export function acuRelayUrl(path?: string): string {
  return path ? `${URLS.ACU_RELAY}${path}` : URLS.ACU_RELAY;
}

/** Build a URL for the research notebook backend with an optional path. */
export function notebookUrl(path?: string): string {
  return path ? `${URLS.NOTEBOOK}${path}` : URLS.NOTEBOOK;
}

/** Base URL of the Factory engine Desktop runs (factory-engine-manager). */
export function factoryEngineUrl(port: number = PORTS.FACTORY): string {
  return `http://${HOSTS.LOOPBACK}:${port}`;
}

/** Build a URL for the office-engine sidecar with an optional path and port override. */
export function officeEngineUrl(path?: string, port?: number): string {
  const base = port ? `http://${HOSTS.LOOPBACK}:${port}` : URLS.OFFICE_ENGINE;
  return path ? `${base}${path}` : base;
}
