import memoize from 'lodash-es/memoize.js'
import { homedir } from 'os'
import { join } from 'path'

// Memoized: 150+ callers, many on hot paths. Keyed off GIZZI_CONFIG_DIR so
// tests that change the env var get a fresh value without explicit cache.clear.
export const getGizziConfigHomeDir = memoize(
  (): string => {
    return (
      process.env.GIZZI_CONFIG_DIR ?? join(homedir(), '.gizzi')
    ).normalize('NFC')
  },
  () => process.env.GIZZI_CONFIG_DIR,
)

// Read-only legacy home used only as a migration source from Claude Code.
export const getLegacyClaudeHomeDir = memoize(
  (): string => {
    return (
      process.env.CLAUDE_CONFIG_DIR ?? join(homedir(), '.claude')
    ).normalize('NFC')
  },
  () => process.env.CLAUDE_CONFIG_DIR,
)

export function getTeamsDir(): string {
  return join(getLegacyClaudeHomeDir(), 'teams')
}

/**
 * Check if NODE_OPTIONS contains a specific flag.
 * Splits on whitespace and checks for exact match to avoid false positives.
 */
export function hasNodeOption(flag: string): boolean {
  const nodeOptions = process.env.NODE_OPTIONS
  if (!nodeOptions) {
    return false
  }
  return nodeOptions.split(/\s+/).includes(flag)
}

export function isEnvTruthy(envVar: string | boolean | undefined): boolean {
  if (!envVar) return false
  if (typeof envVar === 'boolean') return envVar
  const normalizedValue = envVar.toLowerCase().trim()
  return ['1', 'true', 'yes', 'on'].includes(normalizedValue)
}

export function isEnvDefinedFalsy(
  envVar: string | boolean | undefined,
): boolean {
  if (envVar === undefined) return false
  if (typeof envVar === 'boolean') return !envVar
  if (!envVar) return false
  const normalizedValue = envVar.toLowerCase().trim()
  return ['0', 'false', 'no', 'off'].includes(normalizedValue)
}

/**
 * --bare / GIZZI_SIMPLE — skip hooks, LSP, plugin sync, skill dir-walk,
 * attribution, background prefetches, and ALL keychain/credential reads.
 * Auth is strictly ANTHROPIC_API_KEY env or apiKeyHelper from --settings.
 * Explicit CLI flags (--plugin-dir, --add-dir, --mcp-config) still honored.
 * ~30 gates across the codebase.
 *
 * Checks argv directly (in addition to the env var) because several gates
 * run before main.tsx's action handler sets GIZZI_SIMPLE=1 from --bare
 * — notably startKeychainPrefetch() at main.tsx top-level.
 */
export function isBareMode(): boolean {
  return (
    isEnvTruthy(process.env.GIZZI_SIMPLE) ||
    process.argv.includes('--bare')
  )
}

/**
 * Parses an array of environment variable strings into a key-value object
 * @param envVars Array of strings in KEY=VALUE format
 * @returns Object with key-value pairs
 */
export function parseEnvVars(
  rawEnvArgs: string[] | undefined,
): Record<string, string> {
  const parsedEnv: Record<string, string> = {}

  // Parse individual env vars
  if (rawEnvArgs) {
    for (const envStr of rawEnvArgs) {
      const [key, ...valueParts] = envStr.split('=')
      if (!key || valueParts.length === 0) {
        throw new Error(
          `Invalid environment variable format: ${envStr}, environment variables should be added as: -e KEY1=value1 -e KEY2=value2`,
        )
      }
      parsedEnv[key] = valueParts.join('=')
    }
  }
  return parsedEnv
}

/**
 * Get the AWS region with fallback to default
 * Matches the Anthropic Bedrock SDK's region behavior
 */
export function getAWSRegion(): string {
  return process.env.AWS_REGION || process.env.AWS_DEFAULT_REGION || 'us-east-1'
}

/**
 * Get the default Vertex AI region
 */
export function getDefaultVertexRegion(): string {
  return process.env.CLOUD_ML_REGION || 'us-east5'
}

/**
 * Check if bash commands should maintain project working directory (reset to original after each command)
 * @returns true if GIZZI_BASH_MAINTAIN_PROJECT_WORKING_DIR is set to a truthy value
 */
export function shouldMaintainProjectWorkingDir(): boolean {
  return isEnvTruthy(process.env.GIZZI_BASH_MAINTAIN_PROJECT_WORKING_DIR)
}

/**
 * Check if running on Homespace (ant-internal cloud environment)
 */
export function isRunningOnHomespace(): boolean {
  return (
    process.env.USER_TYPE === 'ant' &&
    isEnvTruthy(process.env.COO_RUNNING_ON_HOMESPACE)
  )
}

/**
 * Conservative check for whether Gizzi is running inside a protected
 * (privileged or ASL3+) COO namespace or cluster.
 *
 * Conservative means: when signals are ambiguous, assume protected. We would
 * rather over-report protected usage than miss it. Unprotected environments
 * are homespace, namespaces on the open allowlist, and no k8s/COO signals
 * at all (laptop/local dev).
 *
 * Used for telemetry to measure auto-mode usage in sensitive environments.
 */
export function isInProtectedNamespace(): boolean {
  // USER_TYPE is build-time --define'd; in external builds this block is
  // DCE'd so the require() and namespace allowlist never appear in the bundle.
  if (process.env.USER_TYPE === 'ant') {
    return false; // Stubbed for external distribution
  }
  return false
}

// @[MODEL LAUNCH]: Add a Vertex region override env var for the new model.
/**
 * Model prefix → env var for Vertex region overrides.
 * Order matters: more specific prefixes must come before less specific ones
 * (e.g., 'claude-opus-4-1' before 'claude-opus-4').
 */
const VERTEX_REGION_OVERRIDES: ReadonlyArray<[string, string]> = [
  ['claude-haiku-4-5', 'VERTEX_REGION_GIZZI_HAIKU_4_5'],
  ['claude-3-5-haiku', 'VERTEX_REGION_GIZZI_3_5_HAIKU'],
  ['claude-3-5-sonnet', 'VERTEX_REGION_GIZZI_3_5_SONNET'],
  ['claude-3-7-sonnet', 'VERTEX_REGION_GIZZI_3_7_SONNET'],
  ['claude-opus-4-1', 'VERTEX_REGION_GIZZI_4_1_OPUS'],
  ['claude-opus-4', 'VERTEX_REGION_GIZZI_4_0_OPUS'],
  ['claude-sonnet-4-6', 'VERTEX_REGION_GIZZI_4_6_SONNET'],
  ['claude-sonnet-4-5', 'VERTEX_REGION_GIZZI_4_5_SONNET'],
  ['claude-sonnet-4', 'VERTEX_REGION_GIZZI_4_0_SONNET'],
]

/**
 * Get the Vertex AI region for a specific model.
 * Different models may be available in different regions.
 */
export function getVertexRegionForModel(
  model: string | undefined,
): string | undefined {
  if (model) {
    const match = VERTEX_REGION_OVERRIDES.find(([prefix]) =>
      model.startsWith(prefix),
    )
    if (match) {
      return process.env[match[1]] || getDefaultVertexRegion()
    }
  }
  return getDefaultVertexRegion()
}

// Rebuilt default export (merge-rot repair): real named exports only.
export default {
  getGizziConfigHomeDir,
  getLegacyClaudeHomeDir,
  getTeamsDir,
  hasNodeOption,
  isEnvTruthy,
  isEnvDefinedFalsy,
  isBareMode,
  parseEnvVars,
  getAWSRegion,
  getDefaultVertexRegion,
  shouldMaintainProjectWorkingDir,
  isRunningOnHomespace,
  isInProtectedNamespace,
  getVertexRegionForModel,
}

let warnedOldPeerVar = false

/**
 * Factory peer registration is default-on; `ALLTERNIT_FACTORY_PEER=0` opts
 * out. `GIZZI_ENABLE_RAILS_PEER` is the name shells set before the Factory,
 * still read (with a one-time notice) so an opt-out keeps working.
 */
export function isFactoryPeerDisabled(env: NodeJS.ProcessEnv = process.env): boolean {
  if (env.ALLTERNIT_FACTORY_PEER !== undefined) return isEnvDefinedFalsy(env.ALLTERNIT_FACTORY_PEER)
  const old = env.GIZZI_ENABLE_RAILS_PEER // old-names: keep (opt-out set before the Factory)
  if (old === undefined) return false
  if (!warnedOldPeerVar) {
    warnedOldPeerVar = true
    process.stderr.write('gizzi: GIZZI_ENABLE_RAILS_PEER is deprecated; set ALLTERNIT_FACTORY_PEER instead.\n') // old-names: keep (the deprecation notice)
  }
  return isEnvDefinedFalsy(old)
}
