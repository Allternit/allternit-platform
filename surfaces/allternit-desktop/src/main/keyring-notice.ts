/**
 * Linux without a keyring: Electron safeStorage reports the 'basic_text'
 * backend, so sign-in tokens get only weak encryption. Warn once, never block.
 */
export const KEYRING_MESSAGE =
  'No system keyring found: install gnome-keyring or KWallet so Allternit can store your sign-in securely.';

export function shouldWarnNoKeyring(platform: string, backend: string | null | undefined, alreadyWarned: boolean): boolean {
  return platform === 'linux' && backend === 'basic_text' && !alreadyWarned;
}

let warned = false;

/** Call once after app ready. */
export async function warnIfNoKeyring(log: { warn: (...a: unknown[]) => void }): Promise<void> {
  if (process.platform !== 'linux') return;
  try {
    const { safeStorage, Notification } = await import('electron');
    const backend = safeStorage.getSelectedStorageBackend?.();
    if (!shouldWarnNoKeyring(process.platform, backend, warned)) return;
    warned = true;
    log.warn(`[Keyring] safeStorage backend is basic_text. ${KEYRING_MESSAGE}`);
    if (Notification.isSupported()) new Notification({ title: 'No system keyring', body: KEYRING_MESSAGE }).show();
  } catch (err) {
    log.warn('[Keyring] backend check failed', err);
  }
}
