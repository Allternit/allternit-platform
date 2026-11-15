/**
 * macOS permission help for the "Messages on your Mac" tool.
 * The runtime (allternit-api) is a child of this app, so macOS attributes
 * Full Disk Access and Automation to this app bundle.
 */
export type PrivacyPane = 'fullDisk' | 'automation';

export const PRIVACY_PANE_URLS: Record<PrivacyPane, string> = {
  fullDisk: 'x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles',
  automation: 'x-apple.systempreferences:com.apple.preference.security?Privacy_Automation',
};

export function isPrivacyPane(value: unknown): value is PrivacyPane {
  return value === 'fullDisk' || value === 'automation';
}

export type OpenPrivacyResult = { supported: false } | { supported: true; opened: boolean };

export async function openPrivacySettings(
  pane: unknown,
  deps: { platform: string; openExternal: (url: string) => Promise<void> },
): Promise<OpenPrivacyResult> {
  if (deps.platform !== 'darwin') return { supported: false };
  if (!isPrivacyPane(pane)) throw new Error('Invalid privacy pane');
  await deps.openExternal(PRIVACY_PANE_URLS[pane]);
  return { supported: true, opened: true };
}

/** Trim an executable path to its enclosing .app bundle ("" when not in one). */
export function bundlePathFromExe(exePath: string): string {
  const i = exePath.indexOf('.app/');
  return i === -1 ? '' : exePath.slice(0, i + 4);
}

export type MessagesAccessHint =
  | { supported: false }
  | { supported: true; appName: string; appPath: string };

export function messagesAccessHint(deps: {
  platform: string;
  exePath: string;
  appName: string;
}): MessagesAccessHint {
  if (deps.platform !== 'darwin') return { supported: false };
  return { supported: true, appName: deps.appName, appPath: bundlePathFromExe(deps.exePath) };
}
