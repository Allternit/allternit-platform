/**
 * TypeScript type declarations for Electron APIs exposed via contextBridge.
 *
 * This declaration matches the API surface exposed by src/preload/index.ts
 * as `window.allternit` and `window.allternitSidecar`.
 */

export interface BackendStatus {
  installed: boolean;
  running: boolean;
  version?: string;
  url: string;
}

export interface SidecarStatus {
  status: 'stopped' | 'starting' | 'running' | 'error' | 'crashed';
}

export interface ConnectionState {
  mode: 'bundled' | 'remote' | 'development';
  url: string;
  status?: string;
  lastError?: string;
}

export interface BackendConfig {
  mode: 'bundled' | 'remote';
  remoteUrl?: string;
}

export interface ConnectionAPI {
  test(): Promise<ConnectionState>;
  getBackend(): Promise<{ mode: 'bundled' | 'remote' | 'development'; url: string }>;
  setBackend(config: BackendConfig): Promise<void>;
  onStateChange(callback: (state: unknown) => void): () => void;
}

export interface SidecarAPI {
  start(): Promise<boolean>;
  stop(): Promise<boolean>;
  restart(): Promise<boolean>;
  getStatus(): Promise<SidecarStatus['status']>;
  getApiUrl(): Promise<string | undefined>;
  onStatusChanged(handler: (status: string) => void): () => void;
}

export interface BackendAPI {
  getStatus(): Promise<BackendStatus>;
  restart(): Promise<string>;
  sidecar: SidecarAPI;
  onDownloadProgress(handler: (progress: { stage: string; percent: number }) => void): () => void;
}

export interface BonsaiStatus {
  installed: boolean;
  running: boolean;
  installing: boolean;
  url: string;
  revisions?: { source?: string; model?: string; mlxWheel?: string };
  installDir: string;
  error?: string;
}

export interface BonsaiAPI {
  getStatus(): Promise<BonsaiStatus>;
  install(): Promise<void>;
  cancelInstall(): Promise<boolean>;
  start(): Promise<void>;
  stop(): Promise<boolean>;
  remove(): Promise<void>;
  onProgress(handler: (progress: { stage: string; message: string }) => void): () => void;
}

export type S1Backend = 'laya_bundled' | 'system_one_local' | 'auto';

export interface LayaCheckpoint {
  source: 'pinned' | 'revision' | 'path';
  revision?: string;
  path?: string;
}

export interface SystemOneStatus {
  layaSupported: boolean;
  systemOne: { available: boolean; running: boolean; url: string };
  laya: {
    installed: boolean;
    installing: boolean;
    running: boolean;
    url: string;
    version: string;
    checkpoint: LayaCheckpoint;
    needsUv: boolean;
    uvHint?: string;
    installDir: string;
  };
  backend: S1Backend;
  apiBackend?: S1Backend;
  shadowDir: string | null;
  shadowState: boolean;
  liveMode: boolean;
  embed: { running: boolean; url: string };
  error?: string;
}

export interface SystemOneProgress {
  stage: 'starting' | 'installing' | 'ready' | 'error' | 'cancelled' | 'needs-uv';
  message: string;
}

export interface SystemOneAPI {
  getStatus(): Promise<SystemOneStatus>;
  install(): Promise<void>;
  cancelInstall(): Promise<boolean>;
  repair(): Promise<void>;
  remove(): Promise<void>;
  start(): Promise<void>;
  stop(): Promise<boolean>;
  setCheckpoint(checkpoint: { revision?: string; path?: string } | null): Promise<LayaCheckpoint>;
  setShadowState(enabled: boolean): Promise<boolean>;
  setLiveMode(enabled: boolean): Promise<boolean>;
  onProgress(handler: (progress: SystemOneProgress) => void): () => void;
}

export interface VmSetupAPI {
  checkConnectivity(): Promise<{
    internet: boolean;
    github: boolean;
    allternitServices: boolean;
  }>;
  downloadImages(onProgress: (progress: {
    stage: 'downloading' | 'verifying' | 'extracting' | 'complete';
    fileName: string;
    bytesDownloaded: number;
    totalBytes: number;
    speed: number;
    eta: number;
  }) => void): Promise<boolean>;
  initializeVm(onProgress: (progress: {
    stage: 'verifying' | 'booting' | 'connecting' | 'ready';
    message: string;
    progress: number;
  }) => void): Promise<boolean>;
  checkImagesExist(): Promise<boolean>;
  getStatus(): Promise<'stopped' | 'starting' | 'running' | 'error'>;
}

export interface WindowBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface WindowState {
  maximized: boolean;
  minimized: boolean;
  fullscreen: boolean;
  focused: boolean;
  bounds: WindowBounds;
}

export interface WindowAPI {
  minimize(): Promise<void>;
  maximize(): Promise<{ maximized: boolean }>;
  close(): Promise<void>;
  isMaximized(): Promise<boolean>;
  fullscreen(enabled?: boolean): Promise<{ fullscreen: boolean }>;
  setAlwaysOnTop(enabled: boolean): Promise<void>;
  getState(): Promise<WindowState>;
  getBounds(): Promise<WindowBounds>;
  setBounds(bounds: Partial<WindowBounds>): Promise<void>;
  center(): Promise<void>;
  hide(): Promise<void>;
  show(): Promise<void>;
  minimizeToTray(): Promise<void>;
  onEvent(event: string, handler: (payload: unknown) => void): () => void;
}

export interface StoreAPI {
  get<T>(key: string): Promise<T>;
  set<T>(key: string, value: T): Promise<void>;
}

export interface AppInfo {
  version: string;
  platform: string;
  isPackaged: boolean;
  manifest: unknown;
}

export type UpdateStatus =
  | { state: 'checking' }
  | { state: 'available' }
  | { state: 'up-to-date' }
  | { state: 'downloaded'; version?: string; releaseNotes?: string; updateURL?: string }
  | { state: 'error'; message: string };

export interface AppAPI {
  getInfo(): Promise<AppInfo>;
  isFirstLaunch(): Promise<boolean>;
  completeOnboarding(): Promise<boolean>;
  checkForUpdates(): Promise<{ ok: boolean; reason?: string; message?: string }>;
  installUpdate(): Promise<void>;
  onUpdateStatus(handler: (status: UpdateStatus) => void): () => void;
  /** Record this computer as a trusted Cowork device. */
  registerDevice(): Promise<boolean>;
  /** This computer in Settings → Cowork devices; null for an untrusted frame. */
  deviceInfo: { id: string; name: string; platform: string; kind: 'desktop' } | null;
}

export interface LinksAPI {
  setOpenInApp(enabled: boolean): Promise<void>;
  onOpenInApp(callback: (url: string) => void): () => void;
}

export interface AuthAccount {
  userId: string;
  userEmail: string;
  expiresAt: number;
  runtimeId: string;
  organizationId?: string;
  capabilities: string[];
}

export interface AuthAPI {
  startLogin(): Promise<void>;
  getSession(): Promise<AuthAccount | null>;
  listAccounts(): Promise<Array<{
    userId: string;
    userEmail: string;
    clientId: string;
    lastSignedInAt: string;
    lastSeenAt: string;
    current: boolean;
    backend?: {
      mode: 'bundled' | 'remote' | 'development';
      remoteUrl?: string;
    };
  }>>;
  forgetAccount(userId: string): Promise<void>;
  signOut(): Promise<void>;
  hardSignOut(): Promise<void>;
  getClerkToken(): Promise<string | null>;
  onSessionUpdated?(handler: (session: { userId: string; userEmail: string }) => void): () => void;
}

export interface PairingInfo {
  pairingId: string;
  userCode: string;
  name: string;
  runtimeType: string;
  hostname?: string;
  platform?: string;
  publicKeyFingerprint: string;
  capabilities: string[];
  status: string;
  expiresAt: string;
}

export interface RuntimeDevice {
  id: string;
  name: string;
  runtimeType: string;
  hostname?: string;
  platform?: string;
  version?: string;
  capabilities: string[];
  publicKeyFingerprint: string;
  status: string;
  lastSeenAt?: string;
  createdAt: string;
  credentialExpiresAt: string;
}

export interface DevicePairingAPI {
  lookup(code: string): Promise<PairingInfo>;
  approve(code: string): Promise<{ status: string; pairingId?: string; runtimeName?: string }>;
  deny(code: string): Promise<{ status: string }>;
  listDevices(): Promise<RuntimeDevice[]>;
}

export type MeshState = 'stopped' | 'starting' | 'running' | 'error';

export interface MeshStatus {
  state: MeshState;
  meshIp?: string;
  error?: string;
  proxies: Array<{ target: string; url: string }>;
}

export interface MeshAPI {
  start(): Promise<MeshStatus>;
  stop(): Promise<MeshStatus>;
  status(): Promise<MeshStatus>;
  /** Resolve a 100.64.0.0/10 instance URL to a loopback URL bridged into the tailnet. */
  proxyFor(instanceUrl: string): Promise<string>;
}

export interface ShellAPI {
  openExternal(url: string): Promise<void>;
  openRemoteControl(): Promise<void>;
  openFabricSession(): Promise<void>;
  getOfficeHostStatus(): Promise<Record<'word' | 'excel' | 'powerpoint', {
    installed: boolean;
    running: boolean;
    bundlePath: string | null;
  }>>;
  showSave(options: unknown): Promise<unknown>;
  showOpen(options: unknown): Promise<unknown>;
}

export type OfficeProductId = 'word' | 'excel' | 'powerpoint';
export interface OfficeAddinProductStatus {
  product: OfficeProductId;
  hostInstalled: boolean;
  hostRunning: boolean;
  health: 'not-installed' | 'installed' | 'update-available' | 'needs-repair' | 'unsupported';
  installedVersion: string | null;
  availableVersion: string | null;
  manifestPath: string | null;
  installMethod: 'macos-wef' | 'windows-developer' | 'web-guided' | 'unsupported';
  detail: string;
}
export interface OfficeAddinActionResult {
  ok: boolean;
  requiresHostRestart?: boolean;
  requiresUserConfirmation?: boolean;
  manifestPath?: string;
  detail: string;
}
export interface OfficeAddinsAPI {
  getStatus(): Promise<Record<OfficeProductId, OfficeAddinProductStatus>>;
  install(product: OfficeProductId): Promise<OfficeAddinActionResult>;
  repair(product: OfficeProductId): Promise<OfficeAddinActionResult>;
  remove(product: OfficeProductId): Promise<OfficeAddinActionResult>;
}

export interface ThemeAPI {
  get(): Promise<'light' | 'dark'>;
  set(theme: 'light' | 'dark' | 'system'): Promise<void>;
  onChanged(handler: (dark: boolean) => void): () => void;
}

export interface ExtensionAPI {
  getStatus(): Promise<{ connected: boolean }>;
  send(message: unknown): Promise<boolean>;
  onMessage(handler: (message: unknown) => void): () => void;
  onStatusChange(handler: (status: { connected: boolean }) => void): () => void;
}

export type TunnelStatus = 'stopped' | 'starting' | 'running' | 'error';
export interface TunnelState {
  status: TunnelStatus;
  url?: string;
  error?: string;
}

export interface TunnelAPI {
  enable(): Promise<{ success: boolean; url?: string; error?: string }>;
  start(): Promise<{ success: boolean; url?: string; token?: string; error?: string }>;
  disable(): Promise<{ success: boolean }>;
  getUrl(): Promise<string | null>;
  getState(): Promise<TunnelState>;
  onStateChange(handler: (state: TunnelState) => void): () => void;
}

export type PermissionPanel = 'accessibility' | 'screen-recording';
export interface PermissionStatus {
  accessibility: 'granted' | 'denied' | 'unknown' | 'not-applicable';
  screenRecording: 'granted' | 'denied' | 'unknown' | 'not-applicable';
}

export interface PermissionGuideAPI {
  check(): Promise<PermissionStatus>;
  requestCheck(): Promise<PermissionStatus>;
  readyForCheck(): Promise<PermissionStatus>;
  present(panel: PermissionPanel): Promise<{ success: boolean; alreadyGranted?: boolean; error?: string }>;
  dismiss(): Promise<{ success: boolean; error?: string }>;
  getStatus(): Promise<{ active: boolean }>;
  getDriverStatus(): Promise<{
    available: boolean;
    running: boolean;
    embedded: boolean;
    executable?: string;
    socket?: string;
    error?: string;
  }>;
  onStatusChanged(handler: (status: PermissionStatus) => void): () => void;
}

export interface FeatureFlagsAPI {
  get(key?: string): Promise<unknown>;
  set(key: string, value: unknown): Promise<boolean>;
  onChanged(handler: (key: string, value: unknown) => void): () => void;
}

export type SiwcState =
  | 'disabled' | 'signed_out' | 'signing_in' | 'signed_in'
  | 'plan_usage_disabled' | 'needs_reauth' | 'error';

/** Sign in with ChatGPT status. Never contains tokens. */
export interface SiwcStatus {
  enabled: boolean;
  state: SiwcState;
  email?: string;
  expiresAt?: string;
  detail?: string;
}

export interface SiwcAPI {
  status(): Promise<SiwcStatus>;
  signIn(opts?: { enablePlanUsage?: boolean }): Promise<{ ok: true; status: SiwcStatus } | { ok: false; error: string; status: SiwcStatus }>;
  cancel(): Promise<SiwcStatus>;
  signOut(): Promise<SiwcStatus & { revocationConfirmed: boolean }>;
  onStatusChanged(handler: (status: SiwcStatus) => void): () => void;
}

export interface PersistedStateAPI {
  get(key: string): Promise<unknown>;
  set(key: string, value: unknown): Promise<boolean>;
  patch(key: string, partial: unknown): Promise<boolean>;
  onChanged(handler: (key: string, value: unknown) => void): () => void;
}

export interface FindInPageResult {
  requestId: number;
  activeMatchOrdinal: number;
  matches: number;
  finalUpdate: boolean;
}

export interface FindInPageAPI {
  search(text: string, options?: { forward?: boolean; matchCase?: boolean }): Promise<void>;
  next(): Promise<void>;
  previous(): Promise<void>;
  stop(keepSelection?: boolean): Promise<void>;
  onResult(handler: (result: FindInPageResult) => void): () => void;
}

export interface LocaleAPI {
  get(): Promise<string>;
  set(locale: string): Promise<boolean>;
  onChanged(handler: (locale: string) => void): () => void;
}

export interface MenuBarAPI {
  getMode(): Promise<boolean>;
  setMode(enabled: boolean): Promise<boolean>;
  onModeChanged(handler: (enabled: boolean) => void): () => void;
}

export interface StartupAPI {
  getOnLogin(): Promise<boolean>;
  setOnLogin(enabled: boolean): Promise<boolean>;
}

export interface ResearchAPI {
  getStatus(): Promise<{ running: boolean; ready: boolean }>;
  start(): Promise<boolean>;
  stop(): Promise<void>;
}

export interface McpAPI {
  listServers(): Promise<unknown[]>;
  listTools(serverId?: string): Promise<unknown[]>;
  callTool(serverId: string, toolName: string, args: unknown): Promise<{ success: boolean; result?: unknown; error?: string }>;
  addServer(id: string, config: unknown): Promise<{ success: boolean; error?: string }>;
  removeServer(id: string): Promise<{ success: boolean; error?: string }>;
  onServerReady(handler: (data: { serverId: string; tools: unknown[] }) => void): () => void;
  onServerError(handler: (data: { serverId: string; error: string }) => void): () => void;
}

export interface WorkerAPI {
  send(workerName: string, message: unknown): Promise<{ success: boolean; result?: unknown; error?: string }>;
  list(): Promise<string[]>;
}

export interface HyperframesAPI {
  check(): Promise<{ available: boolean; version?: string }>;
  render(
    html: string,
    options?: { format?: 'mp4' | 'mov' | 'webm'; fps?: number; width?: number; height?: number }
  ): Promise<{ success: boolean; savedPath?: string; error?: string }>;
  onProgress(handler: (message: string) => void): () => void;
}

export interface MiniAppInstallProgress {
  id: string;
  line: string;
  type: 'stdout' | 'stderr' | 'info';
}

export interface MiniAppStatus {
  managed: boolean;
  running: boolean;
  port: number | null;
}

export interface MiniAppsAPI {
  install(id: string): Promise<{ success: boolean; error?: string }>;
  start(id: string): Promise<{ success: boolean; error?: string }>;
  stop(id: string): Promise<{ success: boolean }>;
  getStatus(id: string): Promise<MiniAppStatus>;
  launchDesktop(id: string): Promise<{ success: boolean; error?: string }>;
  getApproval(id: string, registration?: MiniAppRuntimeRegistration): Promise<{ approved: boolean; fingerprint?: string; approvedAt?: string }>;
  reviewAndApprove(registration: MiniAppRuntimeRegistration): Promise<{ success: boolean; approved: boolean; fingerprint?: string; error?: string }>;
  revokeApproval(id: string): Promise<{ success: boolean }>;
  setSecret(id: string, name: string, value: string): Promise<{ success: boolean; error?: string }>;
  listSecrets(id: string): Promise<string[]>;
  deleteSecret(id: string, name: string): Promise<{ success: boolean }>;
  removeRuntime(id: string): Promise<{ success: boolean; error?: string }>;
  rollbackRuntime(id: string): Promise<{ success: boolean; error?: string }>;
  installRelease(options: MiniAppReleaseInstallOptions): Promise<MiniAppReleaseInstallResult>;
  rollbackRelease(id: string, registryUrl?: string): Promise<{ success: boolean; error?: string; currentVersion?: string }>;
  removeRelease(id: string, registryUrl?: string): Promise<{ success: boolean; error?: string }>;
  listReleaseInstalls(): Promise<MiniAppReleaseInstallInfo[]>;
  getReleaseInstall(id: string): Promise<MiniAppReleaseInstallInfo | null>;
  oauthStart(appId: string, providerId: string, provider: MiniAppOAuthProvider, accountId: string): Promise<{ flowId?: string; error?: string }>;
  oauthCancel(flowId: string): Promise<{ success: boolean }>;
  oauthAccounts(appId: string): Promise<MiniAppOAuthAccountMetadata[]>;
  oauthDisconnect(appId: string, providerId: string, accountId: string): Promise<{ success: boolean; error?: string }>;
  onOAuthComplete(handler: (result: MiniAppOAuthFlowResult) => void): () => void;
  onProgress(handler: (p: MiniAppInstallProgress) => void): () => void;
}

export interface MiniAppReleaseInstallOptions {
  registryUrl: string;
  id: string;
  version?: string;
}

export interface MiniAppReleaseInstallResult {
  success: boolean;
  error?: string;
  id: string;
  version?: string;
  previousVersion?: string;
  rolledBack?: boolean;
}

export interface MiniAppReleaseInstallInfo {
  id: string;
  currentVersion?: string;
  previousVersion?: string;
  healthy: boolean;
  releases: Record<string, { installedAt: string; sha256: string; signature: string; publisherKey: string; healthy: boolean }>;
}

export interface MiniAppOAuthProvider {
  authorizationUrl: string;
  tokenUrl: string;
  revocationUrl?: string;
  clientId: string;
  scopes: string[];
  additionalAuthParams?: Record<string, string>;
}

export interface MiniAppOAuthAccountMetadata {
  appId: string;
  providerId: string;
  accountId: string;
  scopes: string[];
  expiresAt?: string;
  createdAt: string;
  lastRefreshedAt?: string;
  needsReauth: boolean;
}

export interface MiniAppOAuthFlowResult {
  flowId: string;
  success: boolean;
  error?: string;
  scopes?: string[];
  expiresAt?: string;
  appId: string;
  providerId: string;
  accountId: string;
}

export interface MiniAppRuntimeRegistration {
  id: string;
  name: string;
  version?: string;
  installCommand?: string;
  startCommand?: string;
  stopCommand?: string;
  healthUrl?: string;
  permissions?: { network?: string[]; filesystem?: string[]; secrets?: string[]; processes?: boolean };
  /** Manifest OAuth declaration; covered by the approval fingerprint. */
  oauth?: Record<string, unknown>;
}

export interface VoiceAPI {
  isAvailable(): Promise<boolean>;
  startDictation(): Promise<{ success: boolean; error?: string }>;
  stopDictation(): Promise<void>;
  transcribe(wav: ArrayBuffer): Promise<{ text?: string; error?: string; status?: 'downloading' }>;
  onTranscript(handler: (event: { text: string; isFinal: boolean }) => void): () => void;
}

export type PhoneConnState = 'connecting' | 'online' | 'unauthorized' | 'lost' | 'reconnecting';

export interface PhoneSimInfo { subscriptionId: number | null; slot: number | null; carrier: string }

export interface PhoneInfoData {
  serial: string;
  id: string;
  name: string;
  model: string | null;
  androidVersion: string | null;
  battery: number | null;
  sims: PhoneSimInfo[];
  state: PhoneConnState;
  host: string;
  port: number;
  streaming: boolean;
}

export interface PhonePairingStatus {
  phase: 'idle' | 'waiting_for_scan' | 'pairing' | 'connecting' | 'done' | 'failed';
  qrPayload?: string;
  serial?: string;
  error?: string;
}

export type PhoneStatusSnapshot =
  | { ready: true; phones: PhoneInfoData[]; pairing: PhonePairingStatus; scrcpy: boolean; artemis: { state: 'not_installed' | 'installing' | 'ready' | 'error'; error?: string } | null }
  | { ready: false; error: string; phones: PhoneInfoData[]; artemis: { state: 'not_installed' | 'installing' | 'ready' | 'error'; error?: string } | null };

export interface PhonesAPI {
  status(): Promise<PhoneStatusSnapshot>;
  startQrPairing(): Promise<{ payload: string }>;
  pairWithCode(hostPort: string, code: string): Promise<PhonePairingStatus>;
  cancelPairing(): Promise<void>;
  forget(id: string): Promise<void>;
  startScrcpy(serial: string): Promise<{ ok: boolean; error?: string }>;
  stopScrcpy(serial: string): Promise<void>;
  tryOpenApp(serial: string, app: string): Promise<{ ok: boolean; message?: string }>;
  screenshot(serial: string): Promise<{ ok: boolean; dataBase64?: string; message?: string }>;
  installArtemis(): Promise<unknown>;
  onDevices(handler: (phones: PhoneInfoData[]) => void): () => void;
  onPairing(handler: (status: PhonePairingStatus) => void): () => void;
}

export interface AllternitDesktopAPI {
  sdk: { getBackendUrl(): Promise<string> };
  connection: ConnectionAPI;
  backend: BackendAPI;
  bonsai: BonsaiAPI;
  systemOne: SystemOneAPI;
  vm: VmSetupAPI;
  window: WindowAPI;
  store: StoreAPI;
  state: PersistedStateAPI;
  app: AppAPI;
  links: LinksAPI;
  auth: AuthAPI;
  devicePairing: DevicePairingAPI;
  mesh: MeshAPI;
  shell: ShellAPI;
  officeAddins: OfficeAddinsAPI;
  theme: ThemeAPI;
  extension: ExtensionAPI;
  tunnel: TunnelAPI;
  permissionGuide: PermissionGuideAPI;
  phones: PhonesAPI;
  featureFlags: FeatureFlagsAPI;
  siwc: SiwcAPI;
  findInPage: FindInPageAPI;
  locale: LocaleAPI;
  menuBar: MenuBarAPI;
  startup: StartupAPI;
  mcp: McpAPI;
  research: ResearchAPI;
  worker: WorkerAPI;
  hyperframes: HyperframesAPI;
  miniApps: MiniAppsAPI;
  voice: VoiceAPI;
}

declare global {
  interface Window {
    allternit: AllternitDesktopAPI;
    allternitSidecar: SidecarAPI;
  }
}

export {};
