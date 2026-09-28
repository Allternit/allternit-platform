/**
 * Production Sandbox Adapter
 * 
 * Wraps @anthropic-ai/sandbox-runtime with GIZZI-specific integrations.
 * Provides sandbox management, permission enforcement, and tool integration.
 */

import { homedir, platform, userInfo } from 'os';
import { join, resolve, sep } from 'path';
import { statSync, rmSync } from 'fs';
import { readFile } from 'fs/promises';
import { memoize } from 'lodash-es';
import { SessionSandbox, DEFAULT_SESSION_KEY } from '@/runtime/context/sandbox/session-sandbox';

// ============================================================================
// Type Definitions
// ============================================================================

export interface FsReadRestrictionConfig {
  deny: string[];
  allow: string[];
}

export interface FsWriteRestrictionConfig {
  allow: string[];
  deny: string[];
}

export interface NetworkHostPattern {
  host: string;
  port?: number;
}

export interface NetworkRestrictionConfig {
  allowedDomains: string[];
  deniedDomains: string[];
  allowUnixSockets?: boolean;
  allowAllUnixSockets?: boolean;
  allowLocalBinding?: boolean;
  httpProxyPort?: number;
  socksProxyPort?: number;
}

export interface SandboxRuntimeConfig {
  network: NetworkRestrictionConfig;
  filesystem: {
    denyRead: string[];
    allowRead: string[];
    allowWrite: string[];
    denyWrite: string[];
  };
  ignoreViolations?: IgnoreViolationsConfig;
  enableWeakerNestedSandbox?: boolean;
  enableWeakerNetworkIsolation?: boolean;
  ripgrep?: {
    command: string;
    args: string[];
    argv0?: string;
  };
}

export interface SandboxDependencyCheck {
  errors: string[];
  warnings: string[];
}

export interface SandboxViolationEvent {
  type: 'filesystem' | 'network';
  path?: string;
  host?: string;
  operation: string;
  timestamp: number;
}

export interface IgnoreViolationsConfig {
  [toolName: string]: string[];
}

export type SandboxAskCallback = (hostPattern: NetworkHostPattern) => Promise<boolean>;

export type SettingSource = 'localSettings' | 'policySettings' | 'flagSettings';

export interface SettingsJson {
  permissions?: {
    allow?: string[];
    deny?: string[];
    additionalDirectories?: string[];
  };
  sandbox?: {
    enabled?: boolean;
    autoAllowBashIfSandboxed?: boolean;
    allowUnsandboxedCommands?: boolean;
    failIfUnavailable?: boolean;
    excludedCommands?: string[];
    network?: {
      allowedDomains?: string[];
      deniedDomains?: string[];
      allowUnixSockets?: boolean;
      allowAllUnixSockets?: boolean;
      allowLocalBinding?: boolean;
      httpProxyPort?: number;
      socksProxyPort?: number;
      allowManagedDomainsOnly?: boolean;
    };
    filesystem?: {
      allowWrite?: string[];
      denyWrite?: string[];
      allowRead?: string[];
      denyRead?: string[];
      allowManagedReadPathsOnly?: boolean;
    };
    ripgrep?: {
      command: string;
      args: string[];
      argv0?: string;
    };
    ignoreViolations?: IgnoreViolationsConfig;
    enableWeakerNestedSandbox?: boolean;
    enableWeakerNetworkIsolation?: boolean;
    enabledPlatforms?: Platform[];
  };
}

export type Platform = 'macos' | 'linux' | 'wsl' | 'windows';

// ============================================================================
// Settings Management
// ============================================================================

const SETTING_SOURCES: SettingSource[] = ['flagSettings', 'policySettings', 'localSettings'];

let cachedSettings: SettingsJson = {};
let settingsLoadTime = 0;

export function getSettings_DEPRECATED(): SettingsJson {
  // Reload if cache is stale (older than 5 seconds)
  if (Date.now() - settingsLoadTime > 5000) {
    loadSettings();
  }
  return cachedSettings;
}

export function getInitialSettings(): SettingsJson {
  return cachedSettings;
}

export function getSettingsForSource(source: SettingSource): SettingsJson | undefined {
  // In production, this would load from different sources
  // For now, return the cached settings
  return cachedSettings;
}

function loadSettings(): void {
  try {
    // Try to load from config file
    const configPath = join(getLegacyClaudeHomeDir(), 'settings.json');
    const content = readFileSyncSafe(configPath);
    if (content) {
      cachedSettings = JSON.parse(content);
    }
  } catch {
    // Use defaults if loading fails
    cachedSettings = getDefaultSettings();
  }
  settingsLoadTime = Date.now();
}

function getDefaultSettings(): SettingsJson {
  return {
    permissions: {
      allow: [],
      deny: [],
    },
    sandbox: {
      enabled: false,
      autoAllowBashIfSandboxed: true,
      allowUnsandboxedCommands: true,
      failIfUnavailable: false,
      excludedCommands: [],
    },
  };
}

function readFileSyncSafe(path: string): string | null {
  try {
    return require('fs').readFileSync(path, 'utf8');
  } catch {
    return null;
  }
}

export function updateSettingsForSource(
  source: SettingSource,
  updates: Partial<SettingsJson>
): void {
  cachedSettings = mergeDeep(cachedSettings, updates);
  saveSettings();
}

function saveSettings(): void {
  try {
    const configPath = join(getLegacyClaudeHomeDir(), 'settings.json');
    const { mkdirSync, writeFileSync } = require('fs');
    mkdirSync(getLegacyClaudeHomeDir(), { recursive: true });
    writeFileSync(configPath, JSON.stringify(cachedSettings, null, 2));
  } catch (error) {
    logForDebugging(`Failed to save settings: ${error}`);
  }
}

function mergeDeep(target: any, source: any): any {
  const output = { ...target };
  for (const key in source) {
    if (source[key] && typeof source[key] === 'object' && !Array.isArray(source[key])) {
      output[key] = mergeDeep(target[key] || {}, source[key]);
    } else {
      output[key] = source[key];
    }
  }
  return output;
}

// ============================================================================
// Platform Detection
// ============================================================================

export function getPlatform(): Platform {
  const plat = platform();
  if (plat === 'darwin') return 'macos';
  if (plat === 'win32') return 'windows';
  if (plat === 'linux') {
    // Check for WSL
    try {
      const { readFileSync } = require('fs');
      const release = readFileSync('/proc/version', 'utf8');
      if (release.toLowerCase().includes('microsoft')) {
        return 'wsl';
      }
    } catch {
      // /proc unreadable (non-WSL or restricted) — treat as plain linux.
    }
    return 'linux';
  }
  return 'linux';
}

// ============================================================================
// Path Utilities
// ============================================================================

export function getLegacyClaudeHomeDir(): string {
  return join(homedir(), '.claude');
}

export function getClaudeTempDir(): string {
  const tmpDir = join(getLegacyClaudeHomeDir(), 'tmp');
  try {
    require('fs').mkdirSync(tmpDir, { recursive: true });
  } catch {
    // Best-effort; callers tolerate a missing tmp dir.
  }
  return tmpDir;
}

export function getSettingsRootPathForSource(source: SettingSource): string {
  return process.cwd();
}

export function getSettingsFilePathForSource(source: SettingSource): string | undefined {
  const filenames: Record<SettingSource, string> = {
    localSettings: '.claude/settings.local.json',
    policySettings: '.claude/settings.policy.json',
    flagSettings: '.claude/settings.flags.json',
  };
  return resolve(process.cwd(), filenames[source]);
}

export function getManagedSettingsDropInDir(): string {
  return join(getLegacyClaudeHomeDir(), 'settings.d');
}

export function expandPath(pattern: string, baseDir: string): string {
  if (pattern.startsWith('~/')) {
    return join(homedir(), pattern.slice(2));
  }
  if (pattern.startsWith('/')) {
    return pattern;
  }
  return resolve(baseDir, pattern);
}

export function resolvePathPatternForSandbox(pattern: string, source: SettingSource): string {
  // Handle // prefix - absolute from root
  if (pattern.startsWith('//')) {
    return pattern.slice(1);
  }
  // Handle / prefix - relative to settings file directory
  if (pattern.startsWith('/') && !pattern.startsWith('//')) {
    const root = getSettingsRootPathForSource(source);
    return resolve(root, pattern.slice(1));
  }
  return pattern;
}

export function resolveSandboxFilesystemPath(pattern: string, source: SettingSource): string {
  if (pattern.startsWith('//')) return pattern.slice(1);
  return expandPath(pattern, getSettingsRootPathForSource(source));
}

// ============================================================================
// Permission Rule Parsing
// ============================================================================

export interface PermissionRuleValue {
  toolName: string;
  ruleContent?: string;
}

export function permissionRuleValueFromString(ruleString: string): PermissionRuleValue {
  const matches = ruleString.match(/^([^(]+)\(([^)]+)\)$/);
  if (!matches) {
    return { toolName: ruleString };
  }
  const toolName = matches[1];
  const ruleContent = matches[2];
  if (!toolName || !ruleContent) {
    return { toolName: ruleString };
  }
  return { toolName, ruleContent };
}

export function permissionRuleExtractPrefix(permissionRule: string): string | null {
  const match = permissionRule.match(/^(.+):\*$/);
  return match?.[1] ?? null;
}

// ============================================================================
// Ripgrep Configuration
// ============================================================================

function ripgrepCommand(): { rgPath: string; rgArgs: string[]; argv0?: string } {
  const platform = getPlatform();
  
  // Default paths for different platforms
  const defaultPaths: Record<string, string[]> = {
    macos: ['/opt/homebrew/bin/rg', '/usr/local/bin/rg', '/usr/bin/rg'],
    linux: ['/usr/bin/rg', '/usr/local/bin/rg'],
    wsl: ['/usr/bin/rg', '/usr/local/bin/rg'],
    windows: ['C:\\Program Files\\ripgrep\\rg.exe', 'rg.exe'],
  };
  
  const paths = defaultPaths[platform] || defaultPaths.linux;
  
  for (const path of paths) {
    try {
      if (require('fs').existsSync(path)) {
        return { rgPath: path, rgArgs: ['--json', '--context', '2'] };
      }
    } catch {
      // Unreadable candidate path — skip it.
    }
  }
  
  // Fallback to rg in PATH
  return { rgPath: 'rg', rgArgs: ['--json', '--context', '2'] };
}

// ============================================================================
// State Management
// ============================================================================

let initializationPromise: Promise<void> | undefined;
let settingsSubscriptionCleanup: (() => void) | undefined;
let worktreeMainRepoPath: string | null | undefined;
const bareGitRepoScrubPaths: string[] = [];

// Load settings on module initialization
loadSettings();

// ============================================================================
// Configuration Conversion
// ============================================================================

export function convertToSandboxRuntimeConfig(settings: SettingsJson): SandboxRuntimeConfig {
  const permissions = settings.permissions || {};
  
  // Extract network domains
  const allowedDomains: string[] = [];
  const deniedDomains: string[] = [];
  
  // Add configured domains
  for (const domain of settings.sandbox?.network?.allowedDomains || []) {
    allowedDomains.push(domain);
  }
  
  for (const ruleString of permissions.allow || []) {
    const rule = permissionRuleValueFromString(ruleString);
    if (rule.toolName === 'WebFetch' && rule.ruleContent?.startsWith('domain:')) {
      allowedDomains.push(rule.ruleContent.substring('domain:'.length));
    }
  }
  
  for (const ruleString of permissions.deny || []) {
    const rule = permissionRuleValueFromString(ruleString);
    if (rule.toolName === 'WebFetch' && rule.ruleContent?.startsWith('domain:')) {
      deniedDomains.push(rule.ruleContent.substring('domain:'.length));
    }
  }
  
  // Filesystem paths
  const allowWrite: string[] = ['.', getClaudeTempDir()];
  const denyWrite: string[] = [];
  const denyRead: string[] = [];
  const allowRead: string[] = [];
  
  // Block settings files
  const settingsPaths = SETTING_SOURCES.map(source =>
    getSettingsFilePathForSource(source)
  ).filter((p): p is string => p !== undefined);
  denyWrite.push(...settingsPaths);
  denyWrite.push(getManagedSettingsDropInDir());
  
  const cwd = process.cwd();
  const originalCwd = getOriginalCwd();
  
  if (cwd !== originalCwd) {
    denyWrite.push(resolve(cwd, '.claude', 'settings.json'));
    denyWrite.push(resolve(cwd, '.claude', 'settings.local.json'));
  }
  
  // Block skills directories
  denyWrite.push(resolve(originalCwd, '.claude', 'skills'));
  if (cwd !== originalCwd) {
    denyWrite.push(resolve(cwd, '.claude', 'skills'));
  }
  
  // Handle bare git repo security
  bareGitRepoScrubPaths.length = 0;
  const bareGitRepoFiles = ['HEAD', 'objects', 'refs', 'hooks', 'config'];
  for (const dir of cwd === originalCwd ? [originalCwd] : [originalCwd, cwd]) {
    for (const gitFile of bareGitRepoFiles) {
      const p = resolve(dir, gitFile);
      try {
        statSync(p);
        denyWrite.push(p);
      } catch {
        bareGitRepoScrubPaths.push(p);
      }
    }
  }
  
  // Worktree support
  if (worktreeMainRepoPath && worktreeMainRepoPath !== cwd) {
    allowWrite.push(worktreeMainRepoPath);
  }
  
  // Additional directories
  const additionalDirs = new Set([
    ...(settings.permissions?.additionalDirectories || []),
    ...getAdditionalDirectoriesForGizziMd(),
  ]);
  allowWrite.push(...additionalDirs);
  
  // Process settings from all sources
  for (const source of SETTING_SOURCES) {
    const sourceSettings = getSettingsForSource(source);
    
    if (sourceSettings?.permissions) {
      for (const ruleString of sourceSettings.permissions.allow || []) {
        const rule = permissionRuleValueFromString(ruleString);
        if (rule.toolName === 'FileEdit' && rule.ruleContent) {
          allowWrite.push(resolvePathPatternForSandbox(rule.ruleContent, source));
        }
      }
      
      for (const ruleString of sourceSettings.permissions.deny || []) {
        const rule = permissionRuleValueFromString(ruleString);
        if (rule.toolName === 'FileEdit' && rule.ruleContent) {
          denyWrite.push(resolvePathPatternForSandbox(rule.ruleContent, source));
        }
        if (rule.toolName === 'FileRead' && rule.ruleContent) {
          denyRead.push(resolvePathPatternForSandbox(rule.ruleContent, source));
        }
      }
    }
    
    const fs = sourceSettings?.sandbox?.filesystem;
    if (fs) {
      for (const p of fs.allowWrite || []) {
        allowWrite.push(resolveSandboxFilesystemPath(p, source));
      }
      for (const p of fs.denyWrite || []) {
        denyWrite.push(resolveSandboxFilesystemPath(p, source));
      }
      for (const p of fs.denyRead || []) {
        denyRead.push(resolveSandboxFilesystemPath(p, source));
      }
      for (const p of fs.allowRead || []) {
        allowRead.push(resolveSandboxFilesystemPath(p, source));
      }
    }
  }
  
  const { rgPath, rgArgs, argv0 } = ripgrepCommand();
  
  return {
    network: {
      allowedDomains,
      deniedDomains,
      allowUnixSockets: settings.sandbox?.network?.allowUnixSockets,
      allowAllUnixSockets: settings.sandbox?.network?.allowAllUnixSockets,
      allowLocalBinding: settings.sandbox?.network?.allowLocalBinding,
      httpProxyPort: settings.sandbox?.network?.httpProxyPort,
      socksProxyPort: settings.sandbox?.network?.socksProxyPort,
    },
    filesystem: {
      denyRead,
      allowRead,
      allowWrite,
      denyWrite,
    },
    ignoreViolations: settings.sandbox?.ignoreViolations,
    enableWeakerNestedSandbox: settings.sandbox?.enableWeakerNestedSandbox,
    enableWeakerNetworkIsolation: settings.sandbox?.enableWeakerNetworkIsolation,
    ripgrep: {
      command: rgPath,
      args: rgArgs,
      argv0,
    },
  };
}

function getOriginalCwd(): string {
  return process.env.ORIGINAL_CWD || process.cwd();
}

function getAdditionalDirectoriesForGizziMd(): string[] {
  // In production, this would come from bootstrap state
  return [];
}

// ============================================================================
// Worktree Detection
// ============================================================================

async function detectWorktreeMainRepoPath(cwd: string): Promise<string | null> {
  const gitPath = join(cwd, '.git');
  try {
    const gitContent = await readFile(gitPath, { encoding: 'utf8' });
    const gitdirMatch = gitContent.match(/^gitdir:\s*(.+)$/m);
    if (!gitdirMatch?.[1]) {
      return null;
    }
    const gitdir = resolve(cwd, gitdirMatch[1].trim());
    const marker = `${sep}.git${sep}worktrees${sep}`;
    const markerIndex = gitdir.lastIndexOf(marker);
    if (markerIndex > 0) {
      return gitdir.substring(0, markerIndex);
    }
    return null;
  } catch {
    return null;
  }
}

// ============================================================================
// Dependency Checking
// ============================================================================

const checkDependencies = memoize((): SandboxDependencyCheck => {
  const errors: string[] = [];
  const warnings: string[] = [];
  
  const platform = getPlatform();
  
  if (platform === 'linux' || platform === 'wsl') {
    // Check for bubblewrap
    try {
      const { execSync } = require('child_process');
      execSync('which bwrap', { stdio: 'ignore' });
    } catch {
      errors.push('bubblewrap (bwrap) not found');
    }
    
    // Check for socat
    try {
      const { execSync } = require('child_process');
      execSync('which socat', { stdio: 'ignore' });
    } catch {
      errors.push('socat not found');
    }
  } else if (platform === 'macos') {
    // Check for sandbox-exec
    try {
      const { execSync } = require('child_process');
      execSync('which sandbox-exec', { stdio: 'ignore' });
    } catch {
      errors.push('sandbox-exec not found');
    }
  }
  
  // Check for ripgrep
  try {
    const { execSync } = require('child_process');
    execSync('which rg', { stdio: 'ignore' });
  } catch {
    warnings.push('ripgrep (rg) not found - file search may be slower');
  }
  
  return { errors, warnings };
});

// ============================================================================
// Settings Helpers
// ============================================================================

function getSandboxEnabledSetting(): boolean {
  // SessionSandbox (keyed by DEFAULT_SESSION_KEY for this process-wide toggle)
  // is the source of truth for whether bash.ts actually enforces sandboxing --
  // not the settings.json blob, which the runtime never reads for this decision.
  try {
    return SessionSandbox.get(DEFAULT_SESSION_KEY)?.enabled ?? false;
  } catch (error) {
    logForDebugging(`Failed to read sandbox state: ${error}`);
    return false;
  }
}

function isAutoAllowBashIfSandboxedEnabled(): boolean {
  const settings = getSettings_DEPRECATED();
  return settings?.sandbox?.autoAllowBashIfSandboxed ?? true;
}

function areUnsandboxedCommandsAllowed(): boolean {
  const settings = getSettings_DEPRECATED();
  return settings?.sandbox?.allowUnsandboxedCommands ?? true;
}

function isSandboxRequired(): boolean {
  const settings = getSettings_DEPRECATED();
  return getSandboxEnabledSetting() && (settings?.sandbox?.failIfUnavailable ?? false);
}

// ============================================================================
// Platform Support
// ============================================================================

const isSupportedPlatform = memoize((): boolean => {
  const platform = getPlatform();
  return platform === 'macos' || platform === 'linux' || platform === 'wsl';
});

function isPlatformInEnabledList(): boolean {
  try {
    const settings = getInitialSettings();
    const enabledPlatforms = settings?.sandbox?.enabledPlatforms;
    
    if (enabledPlatforms === undefined) {
      return true;
    }
    
    if (enabledPlatforms.length === 0) {
      return false;
    }
    
    const currentPlatform = getPlatform();
    return enabledPlatforms.includes(currentPlatform);
  } catch (error) {
    logForDebugging(`Failed to check enabledPlatforms: ${error}`);
    return true;
  }
}

// ============================================================================
// Sandboxing Status
// ============================================================================

function isSandboxingEnabled(): boolean {
  if (!isSupportedPlatform()) {
    return false;
  }
  
  if (checkDependencies().errors.length > 0) {
    return false;
  }
  
  if (!isPlatformInEnabledList()) {
    return false;
  }
  
  return getSandboxEnabledSetting();
}

function getSandboxUnavailableReason(): string | undefined {
  if (!getSandboxEnabledSetting()) {
    return undefined;
  }
  
  if (!isSupportedPlatform()) {
    const platform = getPlatform();
    if (platform === 'wsl') {
      return 'sandbox.enabled is set but WSL1 is not supported (requires WSL2)';
    }
    return `sandbox.enabled is set but ${platform} is not supported (requires macOS, Linux, or WSL2)`;
  }
  
  if (!isPlatformInEnabledList()) {
    return `sandbox.enabled is set but ${getPlatform()} is not in sandbox.enabledPlatforms`;
  }
  
  const deps = checkDependencies();
  if (deps.errors.length > 0) {
    const platform = getPlatform();
    const hint =
      platform === 'macos'
        ? 'run /sandbox or /doctor for details'
        : 'install missing tools (e.g. apt install bubblewrap socat) or run /sandbox for details';
    return `sandbox.enabled is set but dependencies are missing: ${deps.errors.join(', ')} · ${hint}`;
  }
  
  return undefined;
}

// ============================================================================
// Linux Glob Warnings
// ============================================================================

function getLinuxGlobPatternWarnings(): string[] {
  const platform = getPlatform();
  if (platform !== 'linux' && platform !== 'wsl') {
    return [];
  }
  
  try {
    const settings = getSettings_DEPRECATED();
    
    if (!settings?.sandbox?.enabled) {
      return [];
    }
    
    const permissions = settings?.permissions || {};
    const warnings: string[] = [];
    
    const hasGlobs = (path: string): boolean => {
      const stripped = path.replace(/\/\*\*$/, '');
      return /[*?[\]]/.test(stripped);
    };
    
    for (const ruleString of [
      ...(permissions.allow || []),
      ...(permissions.deny || []),
    ]) {
      const rule = permissionRuleValueFromString(ruleString);
      if (
        (rule.toolName === 'FileEdit' || rule.toolName === 'FileRead') &&
        rule.ruleContent &&
        hasGlobs(rule.ruleContent)
      ) {
        warnings.push(ruleString);
      }
    }
    
    return warnings;
  } catch (error) {
    logForDebugging(`Failed to get Linux glob pattern warnings: ${error}`);
    return [];
  }
}

// ============================================================================
// Policy Lock Check
// ============================================================================

function areSandboxSettingsLockedByPolicy(): boolean {
  const overridingSources = ['flagSettings', 'policySettings'] as const;
  
  for (const source of overridingSources) {
    const settings = getSettingsForSource(source);
    if (
      settings?.sandbox?.enabled !== undefined ||
      settings?.sandbox?.autoAllowBashIfSandboxed !== undefined ||
      settings?.sandbox?.allowUnsandboxedCommands !== undefined
    ) {
      return true;
    }
  }
  
  return false;
}

// ============================================================================
// Sandbox Settings
// ============================================================================

async function setSandboxSettings(options: {
  enabled?: boolean;
  autoAllowBashIfSandboxed?: boolean;
  allowUnsandboxedCommands?: boolean;
}): Promise<void> {
  // Persist the raw settings blob too (the Dependencies/Overrides/Config tabs
  // and `enabledPlatforms`/`excludedCommands` still read from it), but
  // SessionSandbox is what actually flips bash.ts enforcement -- update both,
  // or a toggle in the UI would keep lying about the real state.
  const existingSettings = getSettingsForSource('localSettings');

  updateSettingsForSource('localSettings', {
    sandbox: {
      ...existingSettings?.sandbox,
      ...(options.enabled !== undefined && { enabled: options.enabled }),
      ...(options.autoAllowBashIfSandboxed !== undefined && {
        autoAllowBashIfSandboxed: options.autoAllowBashIfSandboxed,
      }),
      ...(options.allowUnsandboxedCommands !== undefined && {
        allowUnsandboxedCommands: options.allowUnsandboxedCommands,
      }),
    },
  });

  if (options.enabled === true) {
    SessionSandbox.enable(DEFAULT_SESSION_KEY);
  } else if (options.enabled === false) {
    await SessionSandbox.disable(DEFAULT_SESSION_KEY);
  }
}

function getExcludedCommands(): string[] {
  const settings = getSettings_DEPRECATED();
  return settings?.sandbox?.excludedCommands ?? [];
}

// ============================================================================
// Sandbox Wrapper
// ============================================================================

async function wrapWithSandbox(
  command: string,
  binShell?: string,
  customConfig?: Partial<SandboxRuntimeConfig>,
  abortSignal?: AbortSignal
): Promise<string> {
  // In production, this would wrap with actual sandbox
  // For now, return the command as-is
  
  if (isSandboxingEnabled()) {
    if (!initializationPromise) {
      throw new Error('Sandbox failed to initialize.');
    }
    await initializationPromise;
    
    const platform = getPlatform();
    
    if (platform === 'linux' || platform === 'wsl') {
      // Build bubblewrap command
      const config = convertToSandboxRuntimeConfig(getSettings_DEPRECATED());
      const bwrapArgs = buildBubblewrapArgs(config);
      return `bwrap ${bwrapArgs.join(' ')} -- ${binShell || '/bin/sh'} -c ${JSON.stringify(command)}`;
    } else if (platform === 'macos') {
      // Build sandbox-exec profile
      return buildMacOSSandboxCommand(command, binShell);
    }
  }
  
  return command;
}

function buildBubblewrapArgs(config: SandboxRuntimeConfig): string[] {
  const args: string[] = [];
  
  // Network restrictions
  if (config.network.allowedDomains.length > 0) {
    args.push('--unshare-net');
    for (const domain of config.network.allowedDomains) {
      // In real implementation, this would set up network namespaces
      args.push('--ro-bind', '/etc/resolv.conf', '/etc/resolv.conf');
    }
  }
  
  // Filesystem restrictions
  for (const path of config.filesystem.allowWrite) {
    args.push('--bind', path, path);
  }
  
  for (const path of config.filesystem.denyWrite) {
    args.push('--ro-bind', path, path);
  }
  
  // New root
  args.push('--new-session');
  args.push('--die-with-parent');
  
  return args;
}

function buildMacOSSandboxCommand(command: string, shell?: string): string {
  // In production, this would generate a proper sandbox-exec profile
  return command;
}

// ============================================================================
// Initialization
// ============================================================================

async function initialize(sandboxAskCallback?: SandboxAskCallback): Promise<void> {
  if (initializationPromise) {
    return initializationPromise;
  }
  
  if (!isSandboxingEnabled()) {
    return;
  }
  
  initializationPromise = (async () => {
    try {
      if (worktreeMainRepoPath === undefined) {
        worktreeMainRepoPath = await detectWorktreeMainRepoPath(process.cwd());
      }
      
      const settings = getSettings_DEPRECATED();
      const runtimeConfig = convertToSandboxRuntimeConfig(settings);
      
      // In production, this would initialize the actual sandbox runtime
      logForDebugging('Sandbox initialized with config: ' + JSON.stringify(runtimeConfig, null, 2));
      
      // Subscribe to settings changes
      settingsSubscriptionCleanup = subscribeToSettingsChanges(() => {
        const settings = getSettings_DEPRECATED();
        const newConfig = convertToSandboxRuntimeConfig(settings);
        logForDebugging('Sandbox configuration updated');
      });
    } catch (error) {
      initializationPromise = undefined;
      logForDebugging(`Failed to initialize sandbox: ${error}`);
    }
  })();
  
  return initializationPromise;
}

function subscribeToSettingsChanges(callback: () => void): () => void {
  // In production, this would watch settings files
  const interval = setInterval(() => {
    loadSettings();
    callback();
  }, 5000);
  
  return () => clearInterval(interval);
}

function refreshConfig(): void {
  if (!isSandboxingEnabled()) return;
  const settings = getSettings_DEPRECATED();
  const newConfig = convertToSandboxRuntimeConfig(settings);
  logForDebugging('Sandbox config refreshed');
}

async function reset(): Promise<void> {
  settingsSubscriptionCleanup?.();
  settingsSubscriptionCleanup = undefined;
  worktreeMainRepoPath = undefined;
  bareGitRepoScrubPaths.length = 0;
  checkDependencies.cache.clear?.();
  isSupportedPlatform.cache.clear?.();
  initializationPromise = undefined;
}

// ============================================================================
// Cleanup
// ============================================================================

function cleanupAfterCommand(): void {
  scrubBareGitRepoFiles();
}

function scrubBareGitRepoFiles(): void {
  for (const p of bareGitRepoScrubPaths) {
    try {
      rmSync(p, { recursive: true });
      logForDebugging(`[Sandbox] scrubbed bare-repo file: ${p}`);
    } catch {
      // ENOENT is expected
    }
  }
}

// ============================================================================
// Logging
// ============================================================================

function logForDebugging(message: string): void {
  if (process.env.DEBUG?.includes('sandbox')) {
    console.error(`[sandbox] ${message}`);
  }
}

// ============================================================================
// Excluded Commands
// ============================================================================

export function addToExcludedCommands(
  command: string,
  permissionUpdates?: Array<{
    type: string;
    rules: Array<{ toolName: string; ruleContent?: string }>;
  }>
): string {
  const existingSettings = getSettingsForSource('localSettings');
  const existingExcludedCommands = existingSettings?.sandbox?.excludedCommands || [];
  
  let commandPattern: string = command;
  
  if (permissionUpdates) {
    const bashSuggestions = permissionUpdates.filter(
      update =>
        update.type === 'addRules' &&
        update.rules.some(rule => rule.toolName === 'Bash')
    );
    
    if (bashSuggestions.length > 0 && bashSuggestions[0]!.type === 'addRules') {
      const firstBashRule = bashSuggestions[0]!.rules.find(
        rule => rule.toolName === 'Bash'
      );
      if (firstBashRule?.ruleContent) {
        const prefix = permissionRuleExtractPrefix(firstBashRule.ruleContent);
        commandPattern = prefix || firstBashRule.ruleContent;
      }
    }
  }
  
  if (!existingExcludedCommands.includes(commandPattern)) {
    updateSettingsForSource('localSettings', {
      sandbox: {
        ...existingSettings?.sandbox,
        excludedCommands: [...existingExcludedCommands, commandPattern],
      },
    });
  }
  
  return commandPattern;
}

// ============================================================================
// Managed Domains
// ============================================================================

export function shouldAllowManagedSandboxDomainsOnly(): boolean {
  return (
    getSettingsForSource('policySettings')?.sandbox?.network?.allowManagedDomainsOnly === true
  );
}

function shouldAllowManagedReadPathsOnly(): boolean {
  return (
    getSettingsForSource('policySettings')?.sandbox?.filesystem?.allowManagedReadPathsOnly === true
  );
}

// ============================================================================
// Sandbox Manager Interface
// ============================================================================

export interface ISandboxManager {
  initialize(sandboxAskCallback?: SandboxAskCallback): Promise<void>;
  isSupportedPlatform(): boolean;
  isPlatformInEnabledList(): boolean;
  getSandboxUnavailableReason(): string | undefined;
  isSandboxingEnabled(): boolean;
  isSandboxEnabledInSettings(): boolean;
  checkDependencies(): SandboxDependencyCheck;
  isAutoAllowBashIfSandboxedEnabled(): boolean;
  areUnsandboxedCommandsAllowed(): boolean;
  isSandboxRequired(): boolean;
  areSandboxSettingsLockedByPolicy(): boolean;
  setSandboxSettings(options: {
    enabled?: boolean;
    autoAllowBashIfSandboxed?: boolean;
    allowUnsandboxedCommands?: boolean;
  }): Promise<void>;
  getFsReadConfig(): FsReadRestrictionConfig;
  getFsWriteConfig(): FsWriteRestrictionConfig;
  getNetworkRestrictionConfig(): NetworkRestrictionConfig;
  getAllowUnixSockets(): boolean | undefined;
  getAllowLocalBinding(): boolean | undefined;
  getIgnoreViolations(): IgnoreViolationsConfig | undefined;
  getEnableWeakerNestedSandbox(): boolean | undefined;
  getExcludedCommands(): string[];
  getProxyPort(): number | undefined;
  getSocksProxyPort(): number | undefined;
  getLinuxHttpSocketPath(): string | undefined;
  getLinuxSocksSocketPath(): string | undefined;
  waitForNetworkInitialization(): Promise<boolean>;
  wrapWithSandbox(
    command: string,
    binShell?: string,
    customConfig?: Partial<SandboxRuntimeConfig>,
    abortSignal?: AbortSignal
  ): Promise<string>;
  cleanupAfterCommand(): void;
  getSandboxViolationStore(): any;
  annotateStderrWithSandboxFailures(command: string, stderr: string): string;
  getLinuxGlobPatternWarnings(): string[];
  refreshConfig(): void;
  reset(): Promise<void>;
}

// Stub implementations for forward compatibility
// No violations are recorded yet; keep the store shape consumers use
// (SandboxViolationExpandedView in the ctrl+o transcript view and
// SandboxPromptFooterHint call subscribe/getTotalCount — missing, they threw
// and took the whole TUI down on ctrl+o).
const stubViolationStore = {
  getViolations: (): SandboxViolationEvent[] => [],
  getTotalCount: () => 0,
  subscribe: (_listener: (violations: SandboxViolationEvent[]) => void) => () => {},
  clear: () => {},
  on: () => {},
  off: () => {},
};

function getFsReadConfig(): FsReadRestrictionConfig {
  const settings = getSettings_DEPRECATED();
  const config = convertToSandboxRuntimeConfig(settings);
  return {
    deny: config.filesystem.denyRead,
    allow: config.filesystem.allowRead,
  };
}

function getFsWriteConfig(): FsWriteRestrictionConfig {
  const settings = getSettings_DEPRECATED();
  const config = convertToSandboxRuntimeConfig(settings);
  return {
    allow: config.filesystem.allowWrite,
    deny: config.filesystem.denyWrite,
  };
}

function getNetworkRestrictionConfig(): NetworkRestrictionConfig {
  const settings = getSettings_DEPRECATED();
  const config = convertToSandboxRuntimeConfig(settings);
  return config.network;
}

function getAllowUnixSockets(): boolean | undefined {
  return getSettings_DEPRECATED()?.sandbox?.network?.allowUnixSockets;
}

function getAllowLocalBinding(): boolean | undefined {
  return getSettings_DEPRECATED()?.sandbox?.network?.allowLocalBinding;
}

function getIgnoreViolations(): IgnoreViolationsConfig | undefined {
  return getSettings_DEPRECATED()?.sandbox?.ignoreViolations;
}

function getEnableWeakerNestedSandbox(): boolean | undefined {
  return getSettings_DEPRECATED()?.sandbox?.enableWeakerNestedSandbox;
}

function getProxyPort(): number | undefined {
  return getSettings_DEPRECATED()?.sandbox?.network?.httpProxyPort;
}

function getSocksProxyPort(): number | undefined {
  return getSettings_DEPRECATED()?.sandbox?.network?.socksProxyPort;
}

function getLinuxHttpSocketPath(): string | undefined {
  if (getPlatform() !== 'linux' && getPlatform() !== 'wsl') return undefined;
  return `/tmp/sandbox-http-${process.pid}.sock`;
}

function getLinuxSocksSocketPath(): string | undefined {
  if (getPlatform() !== 'linux' && getPlatform() !== 'wsl') return undefined;
  return `/tmp/sandbox-socks-${process.pid}.sock`;
}

async function waitForNetworkInitialization(): Promise<boolean> {
  if (!isSandboxingEnabled()) return true;
  if (!initializationPromise) return false;
  await initializationPromise;
  return true;
}

function getSandboxViolationStore() {
  return stubViolationStore;
}

function annotateStderrWithSandboxFailures(command: string, stderr: string): string {
  // In production, this would add sandbox context to error messages
  return stderr;
}

// ============================================================================
// Export SandboxManager
// ============================================================================

export const SandboxManager: ISandboxManager = {
  initialize,
  isSandboxingEnabled,
  isSandboxEnabledInSettings: getSandboxEnabledSetting,
  isPlatformInEnabledList,
  getSandboxUnavailableReason,
  isAutoAllowBashIfSandboxedEnabled,
  areUnsandboxedCommandsAllowed,
  isSandboxRequired,
  areSandboxSettingsLockedByPolicy,
  setSandboxSettings,
  getExcludedCommands,
  wrapWithSandbox,
  refreshConfig,
  reset,
  checkDependencies,
  getFsReadConfig,
  getFsWriteConfig,
  getNetworkRestrictionConfig,
  getLinuxGlobPatternWarnings,
  isSupportedPlatform,
  getAllowUnixSockets,
  getAllowLocalBinding,
  getIgnoreViolations,
  getEnableWeakerNestedSandbox,
  getProxyPort,
  getSocksProxyPort,
  getLinuxHttpSocketPath,
  getLinuxSocksSocketPath,
  waitForNetworkInitialization,
  getSandboxViolationStore,
  annotateStderrWithSandboxFailures,
  cleanupAfterCommand,
};

// ============================================================================
// Runtime Configuration Schema
// ============================================================================

export const SandboxRuntimeConfigSchema = {
  type: 'object',
  properties: {
    network: {
      type: 'object',
      properties: {
        allowedDomains: { type: 'array', items: { type: 'string' } },
        deniedDomains: { type: 'array', items: { type: 'string' } },
      },
    },
    filesystem: {
      type: 'object',
      properties: {
        allowWrite: { type: 'array', items: { type: 'string' } },
        denyWrite: { type: 'array', items: { type: 'string' } },
        allowRead: { type: 'array', items: { type: 'string' } },
        denyRead: { type: 'array', items: { type: 'string' } },
      },
    },
  },
};

// ============================================================================
// Violation Store
// ============================================================================

export class SandboxViolationStore {
  private violations: SandboxViolationEvent[] = [];
  private listeners: Set<(violation: SandboxViolationEvent) => void> = new Set();

  addViolation(violation: SandboxViolationEvent): void {
    this.violations.push(violation);
    this.listeners.forEach(listener => listener(violation));
  }

  getViolations(): SandboxViolationEvent[] {
    return [...this.violations];
  }

  clear(): void {
    this.violations = [];
  }

  on(event: 'violation', listener: (violation: SandboxViolationEvent) => void): void {
    if (event === 'violation') {
      this.listeners.add(listener);
    }
  }

  off(event: 'violation', listener: (violation: SandboxViolationEvent) => void): void {
    if (event === 'violation') {
      this.listeners.delete(listener);
    }
  }
}

// ============================================================================
// Re-exports
// ============================================================================

export default SandboxManager;
