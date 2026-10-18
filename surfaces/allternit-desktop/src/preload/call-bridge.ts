/**
 * Preload for the hidden call bridge window (main/phone-call-bridge-window.ts).
 * Exposes only the command channel with main: no other Desktop API.
 */

import { contextBridge, ipcRenderer, type IpcRendererEvent } from 'electron';

export interface CallBridgeCommand {
  id: string;
  op: 'devices' | 'join' | 'leave';
  args: unknown;
}

contextBridge.exposeInMainWorld('callBridgeHost', {
  onCommand: (cb: (cmd: CallBridgeCommand) => void): void => {
    ipcRenderer.on('call-bridge:cmd', (_e: IpcRendererEvent, cmd: CallBridgeCommand) => cb(cmd));
  },
  reply: (msg: { id: string; ok: boolean; value?: unknown; error?: string }): void => ipcRenderer.send('call-bridge:reply', msg),
  event: (msg: { type: 'disconnected'; serial: string; reason: string } | { type: 'log'; message: string }): void =>
    ipcRenderer.send('call-bridge:event', msg),
  ready: (): void => ipcRenderer.send('call-bridge:ready'),
});
