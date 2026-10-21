/**
 * Allternit Desktop — Startup Window Preload
 *
 * Minimal bridge for the onboarding/loading splash. The startup window loads
 * a data: URL document, so it historically ran with nodeIntegration on and
 * contextIsolation off just to reach ipcRenderer. This preload exposes
 * exactly the channels the splash uses, letting the window run with
 * contextIsolation + sandbox enabled.
 */

import { contextBridge, ipcRenderer, type IpcRendererEvent } from 'electron';

function subscribe<T>(channel: string, callback: (payload: T) => void): () => void {
  const listener = (_event: IpcRendererEvent, payload: T) => callback(payload);
  ipcRenderer.on(channel, listener);
  return () => ipcRenderer.removeListener(channel, listener);
}

contextBridge.exposeInMainWorld('startup', {
  /** Welcome step: the user clicked "Get started" — begin sign-in. */
  startLogin: (): void => {
    ipcRenderer.send('auth:start-login');
  },
  /** Main-process push: backend service states for the loading step. */
  onServices: (callback: (services: unknown) => void) => subscribe('services', callback),
  /** Main-process push: status line text. */
  onStatus: (callback: (message: string) => void) => subscribe('status', callback),
  /** Main-process push: startup progress percent. */
  onProgress: (callback: (percent: number) => void) => subscribe('progress', callback),
  /** Main-process push: local backend connected. */
  onComplete: (callback: () => void) => subscribe<unknown>('complete', callback),
  /** Main-process push: startup failure message. */
  onError: (callback: (message: string) => void) => subscribe('error', callback),
  /** Main-process push: show the folder-grant step (consumer Cowork P1). */
  onFoldersShow: (callback: () => void) => subscribe<unknown>('folders:show', callback),
  /** Main-process push: leave the folder-grant step. */
  onFoldersHide: (callback: () => void) => subscribe<unknown>('folders:hide', callback),
  /** Folder-grant step: native directory picker; null when cancelled. */
  pickFolder: (): Promise<string | null> => ipcRenderer.invoke('startup:pick-folder'),
  /** Folder-grant step: persist trusted_folders via /cowork-preferences. */
  saveFolders: (folders: string[]): Promise<{ saved: number }> =>
    ipcRenderer.invoke('startup:save-folders', folders),
});
