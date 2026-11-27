// @ts-nocheck
/**
 * Leftovers of the ported Claude Code auto-updater that live code still uses.
 *
 * Gizzi updates through the signed release feed now
 * (src/shared/installation/updater.ts, PLAN.md D2). The GCS bucket, the
 * GrowthBook version floor / kill switch and the npm self-install that used
 * to live here are gone; the prompt footer reads Updater status through
 * components/AutoUpdaterWrapper.tsx.
 */
import { constants as fsConstants } from 'fs'
import { access } from 'fs/promises'
import { homedir } from 'os'
import { env } from './env.js'
import { ClaudeError } from './errors.js'
import { execFileNoThrowWithCwd } from './execFileNoThrow.js'
import { logError } from './log.js'

class AutoUpdaterError extends ClaudeError {}

export type InstallStatus =
  | 'success'
  | 'no_permissions'
  | 'install_failed'
  | 'in_progress'

/** Still threaded through REPL → PromptInput → footer props; nothing sets it any more. */
export type AutoUpdaterResult = {
  version: string | null
  status: InstallStatus
  notifications?: string[]
}

export type NpmDistTags = {
  latest: string | null
  stable: string | null
}

async function getInstallationPrefix(): Promise<string | null> {
  // Run from home directory to avoid reading project-level .npmrc/.bunfig.toml
  const isBun = env.isRunningWithBun()
  let prefixResult = null
  if (isBun) {
    prefixResult = await execFileNoThrowWithCwd('bun', ['pm', 'bin', '-g'], {
      cwd: homedir(),
    })
  } else {
    prefixResult = await execFileNoThrowWithCwd(
      'npm',
      ['-g', 'config', 'get', 'prefix'],
      { cwd: homedir() },
    )
  }
  if (prefixResult.code !== 0) {
    logError(new Error(`Failed to check ${isBun ? 'bun' : 'npm'} permissions`))
    return null
  }
  return prefixResult.stdout.trim()
}

/** Used by doctorDiagnostic.ts to warn when the global npm prefix is not writable. */
export async function checkGlobalInstallPermissions(): Promise<{
  hasPermissions: boolean
  npmPrefix: string | null
}> {
  try {
    const prefix = await getInstallationPrefix()
    if (!prefix) {
      return { hasPermissions: false, npmPrefix: null }
    }

    try {
      await access(prefix, fsConstants.W_OK)
      return { hasPermissions: true, npmPrefix: prefix }
    } catch {
      logError(
        new AutoUpdaterError(
          'Insufficient permissions for global npm install.',
        ),
      )
      return { hasPermissions: false, npmPrefix: prefix }
    }
  } catch (error) {
    logError(error as Error)
    return { hasPermissions: false, npmPrefix: null }
  }
}
