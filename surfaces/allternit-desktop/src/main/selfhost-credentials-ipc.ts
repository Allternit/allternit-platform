/**
 * Keychain bridge for the self-hosted server credential
 * (window.allternit.selfhost.credentials). The web app asks for a named secret
 * (it uses "selfhost-server-token"); the value is sealed with secure-store.ts
 * (Electron safeStorage / macOS Keychain). The value is never logged.
 */

import { deleteSecret, readSecret, writeSecret } from './secure-store.js';

type Guarded = (
  channel: string,
  fn: (event: Electron.IpcMainInvokeEvent, ...args: any[]) => unknown,
) => void;

const NAME_RE = /^selfhost-[a-z0-9-]{1,64}$/;
const MAX_VALUE_LENGTH = 512;

function storeKey(name: unknown): string {
  if (typeof name !== 'string' || !NAME_RE.test(name)) {
    throw new Error('Invalid self-host credential name');
  }
  return `selfhost-credential-${name}`;
}

export function registerSelfHostCredentialsIpc(handleGuarded: Guarded): void {
  handleGuarded('selfhost:credentials:get', (_event, name: unknown) => readSecret(storeKey(name)));

  handleGuarded('selfhost:credentials:set', (_event, name: unknown, value: unknown) => {
    const key = storeKey(name);
    if (typeof value !== 'string' || value.length === 0 || value.length >= MAX_VALUE_LENGTH) {
      return { success: false, error: 'Invalid credential value' };
    }
    try {
      writeSecret(key, value);
      return { success: true };
    } catch {
      return { success: false, error: 'Could not store the credential securely' };
    }
  });

  handleGuarded('selfhost:credentials:delete', (_event, name: unknown) => {
    deleteSecret(storeKey(name));
    return { success: true };
  });
}
