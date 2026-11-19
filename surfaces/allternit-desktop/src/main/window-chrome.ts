/**
 * Pure helpers for window chrome and HUD gating. No electron imports so they
 * are unit-testable.
 */

/** Every Allternit window reads "Allternit"; the app `name` stays "@allternit/desktop" (it names userData). */
export const APP_WINDOW_TITLE = 'Allternit';

/** Linux and Windows draw an Electron menu bar in the window; macOS uses the global app menu. */
export function hidesMenuBarBeforePairing(platform: NodeJS.Platform): boolean {
  return platform === 'linux' || platform === 'win32';
}

/** The Connect page and pairing error screens are loaded from the packaged static folder. */
export function isPrePairingPage(url: string): boolean {
  return url.startsWith('file:') && /\/connect\.html(?:[?#]|$)/.test(url);
}

/** The HUD and quick chat open only once the device holds a saved paired identity. */
export function mayOpenHud(hasPairedIdentity: boolean): boolean {
  return hasPairedIdentity;
}

/** Per-OS install command for uv (Laya's Python runner). */
export function uvInstallCommand(platform: NodeJS.Platform): string {
  if (platform === 'darwin') return 'brew install uv';
  if (platform === 'win32') return 'powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex"';
  return 'curl -LsSf https://astral.sh/uv/install.sh | sh';
}

export function uvInstallHint(platform: NodeJS.Platform): string {
  return `Laya needs uv. Install it with '${uvInstallCommand(platform)}', then choose Repair.`;
}
