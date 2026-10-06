import { readdir } from 'fs/promises'
import { join } from 'path'
import { coerce as semverCoerce } from 'semver'
import { getSessionId } from '@/bootstrap/state.js'
import { getCwd } from './cwd.js'
import { logForDebugging } from './debug.js'
import { execFileNoThrow } from './execFileNoThrow.js'
import { pathExists } from './file.js'
import { gte as semverGte } from './semver.js'

const MIN_DESKTOP_VERSION = '1.1.2396'

function isDevMode(): boolean {
  if ((process.env.NODE_ENV as string) === 'development') {
    return true
  }

  // Local builds from build directories are dev mode even with NODE_ENV=production
  const pathsToCheck = [process.argv[1] || '', process.execPath || '']
  const buildDirs = [
    '/build-ant/',
    '/build-ant-native/',
    '/build-external/',
    '/build-external-native/',
  ]

  return pathsToCheck.some(p => buildDirs.some(dir => p.includes(dir)))
}

/**
 * Builds a deep link URL for Allternit Desktop to resume a CLI session.
 * Format: allternit://resume?session={sessionId}&cwd={cwd}
 * In dev mode: allternit-dev://resume?session={sessionId}&cwd={cwd}
 */
function buildDesktopDeepLink(sessionId: string): string {
  const protocol = isDevMode() ? 'allternit-dev' : 'allternit'
  const url = new URL(`${protocol}://resume`)
  url.searchParams.set('session', sessionId)
  url.searchParams.set('cwd', getCwd())
  return url.toString()
}

/**
 * Check if Allternit Desktop app is installed.
 * On macOS, checks for /Applications/Allternit Desktop.app or /Applications/Allternit.app.
 * On Linux, checks if xdg-open can handle allternit:// protocol.
 * On Windows, checks if the protocol handler exists.
 * In dev mode, always returns true (assumes dev Desktop is running).
 */
async function isDesktopInstalled(): Promise<boolean> {
  // In dev mode, assume the dev Desktop app is running
  if (isDevMode()) {
    return true
  }

  const platform = process.platform

  if (platform === 'darwin') {
    // Check for Allternit Desktop.app or Allternit.app in /Applications
    return (
      (await pathExists('/Applications/Allternit Desktop.app')) ||
      (await pathExists('/Applications/Allternit.app'))
    )
  } else if (platform === 'linux') {
    // Check if xdg-mime can find a handler for allternit://
    // Note: xdg-mime returns exit code 0 even with no handler, so check stdout too
    const { code, stdout } = await execFileNoThrow('xdg-mime', [
      'query',
      'default',
      'x-scheme-handler/allternit',
    ])
    return code === 0 && stdout.trim().length > 0
  } else if (platform === 'win32') {
    // On Windows, try to query the registry for the protocol handler
    const { code } = await execFileNoThrow('reg', [
      'query',
      'HKEY_CLASSES_ROOT\\allternit',
      '/ve',
    ])
    return code === 0
  }

  return false
}

/**
 * Detect the installed Allternit Desktop version.
 * On macOS, reads CFBundleShortVersionString from the app plist.
 * On Windows, finds the highest app-X.Y.Z directory in the Squirrel install.
 * Returns null if version cannot be determined.
 */
async function getDesktopVersion(): Promise<string | null> {
  const platform = process.platform

  if (platform === 'darwin') {
    let appPath = '/Applications/Allternit Desktop.app'
    if (!(await pathExists(appPath))) {
      appPath = '/Applications/Allternit.app'
    }
    const { code, stdout } = await execFileNoThrow('defaults', [
      'read',
      `${appPath}/Contents/Info.plist`,
      'CFBundleShortVersionString',
    ])
    if (code !== 0) {
      return null
    }
    const version = stdout.trim()
    return version.length > 0 ? version : null
  } else if (platform === 'win32') {
    const localAppData = process.env.LOCALAPPDATA
    if (!localAppData) {
      return null
    }
    const installDir = join(localAppData, 'Allternit')
    try {
      const entries = await readdir(installDir)
      const versions = entries
        .filter(e => e.startsWith('app-'))
        .map(e => e.slice(4))
        .filter(v => semverCoerce(v) !== null)
        .sort((a, b) => {
          const ca = semverCoerce(a)!
          const cb = semverCoerce(b)!
          return ca.compare(cb)
        })
      return versions.length > 0 ? versions[versions.length - 1]! : null
    } catch {
      return null
    }
  }

  return null
}

export type DesktopInstallStatus =
  | { status: 'not-installed' }
  | { status: 'version-too-old'; version: string }
  | { status: 'ready'; version: string }

/**
 * Check Desktop install status including version compatibility.
 */
export async function getDesktopInstallStatus(): Promise<DesktopInstallStatus> {
  const installed = await isDesktopInstalled()
  if (!installed) {
    return { status: 'not-installed' }
  }

  let version: string | null
  try {
    version = await getDesktopVersion()
  } catch {
    // Best effort — proceed with handoff if version detection fails
    return { status: 'ready', version: 'unknown' }
  }

  if (!version) {
    // Can't determine version — assume it's ready (dev mode or unknown install)
    return { status: 'ready', version: 'unknown' }
  }

  const coerced = semverCoerce(version)
  if (!coerced || !semverGte(coerced.version, MIN_DESKTOP_VERSION)) {
    return { status: 'version-too-old', version }
  }

  return { status: 'ready', version }
}

/**
 * Opens a deep link URL using the platform-specific mechanism.
 * Returns true if the command succeeded, false otherwise.
 */
async function openDeepLink(deepLinkUrl: string): Promise<boolean> {
  const platform = process.platform
  logForDebugging(`Opening deep link: ${deepLinkUrl}`)

  if (platform === 'darwin') {
    if (isDevMode()) {
      // In dev mode, `open` launches a bare Electron binary (without app code)
      // because setAsDefaultProtocolClient registers just the Electron executable.
      // Use AppleScript to route the URL to the already-running Electron app.
      const { code } = await execFileNoThrow('osascript', [
        '-e',
        `tell application "Electron" to open location "${deepLinkUrl}"`,
      ])
      return code === 0
    }
    const { code } = await execFileNoThrow('open', [deepLinkUrl])
    return code === 0
  } else if (platform === 'linux') {
    const { code } = await execFileNoThrow('xdg-open', [deepLinkUrl])
    return code === 0
  } else if (platform === 'win32') {
    // On Windows, use cmd /c start to open URLs
    const { code } = await execFileNoThrow('cmd', [
      '/c',
      'start',
      '',
      deepLinkUrl,
    ])
    return code === 0
  }

  return false
}

/**
 * Build and open a deep link to resume the current session in Allternit Desktop.
 * Returns an object with success status and any error message.
 */
export async function openCurrentSessionInDesktop(): Promise<{
  success: boolean
  error?: string
  deepLinkUrl?: string
}> {
  const sessionId = getSessionId()

  // Check if Desktop is installed
  const installed = await isDesktopInstalled()
  if (!installed) {
    return {
      success: false,
      error:
        'Allternit Desktop is not installed. Install it from https://github.com/Allternit/desktop/releases',
    }
  }

  // Build and open the deep link
  const deepLinkUrl = buildDesktopDeepLink(sessionId)
  const opened = await openDeepLink(deepLinkUrl)

  if (!opened) {
    return {
      success: false,
      error: 'Failed to open Allternit Desktop. Please try opening it manually.',
      deepLinkUrl,
    }
  }

  return { success: true, deepLinkUrl }
}

/**
 * Build the A:// Studio (Design mode) deep link.
 * Format: allternit://design?prompt=<optional prompt>
 * In dev mode: allternit-dev://design?prompt=...
 */
export function buildDesignDeepLink(prompt?: string): string {
  const protocol = isDevMode() ? 'allternit-dev' : 'allternit'
  const url = new URL(`${protocol}://design`)
  const trimmed = prompt?.trim()
  if (trimmed) url.searchParams.set('prompt', trimmed)
  return url.toString()
}

/**
 * Open A:// Studio (the design mode) in Allternit Desktop, carrying an
 * optional prompt into the studio composer. Returns the deep link in all
 * cases so callers can surface it when the open itself fails.
 */
export async function openDesignStudioInDesktop(prompt?: string): Promise<{
  success: boolean
  error?: string
  deepLinkUrl: string
  notInstalled?: boolean
}> {
  const deepLinkUrl = buildDesignDeepLink(prompt)
  const installed = await isDesktopInstalled()
  if (!installed) {
    return {
      success: false,
      notInstalled: true,
      error:
        'Allternit Desktop is not installed. Install it from https://github.com/Allternit/desktop/releases',
      deepLinkUrl,
    }
  }
  const opened = await openDeepLink(deepLinkUrl)
  if (!opened) {
    return {
      success: false,
      error: 'Failed to open Allternit Desktop. Please try opening it manually.',
      deepLinkUrl,
    }
  }
  return { success: true, deepLinkUrl }
}
