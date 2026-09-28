/**
 * Config Utilities
 */

import { saveGlobalConfig as saveRealGlobalConfig } from '../shared/utils/config.js'
import type { GlobalConfig as FullGlobalConfig } from '../shared/utils/config.js'

export interface Config {
  [key: string]: unknown
}

export function getConfig(): Config {
  return {}
}

export function setConfig(key: string, value: unknown): void {
  // Implementation
}

// Global config for pet/companion
export interface GlobalConfig {
  theme?: string
  notifications?: boolean
  autoUpdate?: boolean
  telemetry?: boolean
  debug?: boolean
  [key: string]: unknown
}

// Delegates to the real config store; the merge-by-re-export below provides
// the canonical getGlobalConfig/saveGlobalConfig from the complete counterpart.
export function setGlobalConfig(config: GlobalConfig): void {
  saveRealGlobalConfig((current: FullGlobalConfig) => ({ ...current, ...config } as FullGlobalConfig))
}

// Merge-by-re-export: complete counterpart (local exports win on conflict)
export { CONFIG_WRITE_DISPLAY_THRESHOLD, DEFAULT_GLOBAL_CONFIG, EDITOR_MODES, GLOBAL_CONFIG_KEYS, NOTIFICATION_CHANNELS, PROJECT_CONFIG_KEYS, _getConfigForTesting, _setGlobalConfigCacheForTesting, _wouldLoseAuthStateForTesting, checkHasTrustDialogAccepted, enableConfigs, formatAutoUpdaterDisabledReason, getAutoUpdaterDisabledReason, getCurrentProjectConfig, getCustomApiKeyStatus, getGlobalConfig, getGlobalConfigWriteCount, getManagedGizziRulesDir, getMemoryPath, getOrCreateUserID, getProjectPathForConfig, getRemoteControlAtStartup, getUserGizziRulesDir, isAutoUpdaterDisabled, isGlobalConfigKey, isPathTrusted, isProjectConfigKey, recordFirstStartTime, resetTrustDialogAcceptedCacheForTesting, saveCurrentProjectConfig, saveGlobalConfig, shouldSkipPluginAutoupdate } from "../shared/utils/config.js";
export type { AccountInfo, AutoUpdaterDisabledReason, DiffTool, EditorMode, GlobalConfigKey, HistoryEntry, InstallMethod, NotificationChannel, OutputStyle, PastedContent, ProjectConfig, ProjectConfigKey, ReleaseChannel, SerializedStructuredHistoryEntry } from "../shared/utils/config.js";
