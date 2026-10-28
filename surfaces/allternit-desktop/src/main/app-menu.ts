/**
 * Allternit Desktop — application menu (the items at the top left of the
 * macOS menu bar: Allternit Desktop, File, Edit, View, Window, Help).
 *
 * Commands that belong to the workspace UI (New Session, the modes, the
 * sidebar, Settings) are sent to the main window as `menu-command` window
 * events; the renderer handles them (allternit-ai src/shell/desktop-menu.ts).
 * Shortcuts the renderer already binds itself (Find, Bot Activity, HUD) are
 * shown without registering them, so the renderer's own handler and any key
 * the user rebound in Settings keep working.
 */
import { app, Menu, type MenuItemConstructorOptions } from 'electron';

export type AppMenuCommand =
  | 'new-session'
  | 'settings'
  | 'toggle-sidebar'
  | 'find'
  | 'activity'
  | 'mode-chat'
  | 'mode-bot'
  | 'mode-code'
  | 'mode-browser';

export interface AppMenuHandlers {
  /** Show the main window and send it a workspace command. */
  sendCommand: (command: AppMenuCommand) => void;
  quickChat: () => void;
  toggleHud: () => void;
  checkForUpdates: () => void;
  openUrl: (url: string) => void;
  quickChatAccelerator: string;
  hudAccelerator: string;
}

export function buildAppMenu(h: AppMenuHandlers): Menu {
  const isMac = process.platform === 'darwin';
  const cmd = (command: AppMenuCommand) => () => h.sendCommand(command);
  // Shown in the menu, but the key press stays with the renderer.
  const shown = { registerAccelerator: false } as const;

  const template: MenuItemConstructorOptions[] = [
    ...(isMac
      ? [{
          label: app.name,
          submenu: [
            { role: 'about' },
            { label: 'Check for Updates…', click: h.checkForUpdates },
            { type: 'separator' },
            { label: 'Settings…', accelerator: 'CommandOrControl+,', click: cmd('settings') },
            { type: 'separator' },
            { role: 'services' },
            { type: 'separator' },
            { role: 'hide' },
            { role: 'hideOthers' },
            { role: 'unhide' },
            { type: 'separator' },
            { role: 'quit' },
          ],
        } satisfies MenuItemConstructorOptions]
      : []),
    {
      label: 'File',
      submenu: [
        { label: 'New Session', accelerator: 'CommandOrControl+N', click: cmd('new-session') },
        { label: 'Quick Chat', accelerator: h.quickChatAccelerator, ...shown, click: h.quickChat },
        { label: 'Toggle HUD', accelerator: h.hudAccelerator, ...shown, click: h.toggleHud },
        { type: 'separator' },
        ...(isMac
          ? [{ role: 'close' } as const]
          : [
              { label: 'Settings…', accelerator: 'CommandOrControl+,', click: cmd('settings') },
              { type: 'separator' } as const,
              { role: 'quit' } as const,
            ]),
      ],
    },
    {
      label: 'Edit',
      submenu: [
        { role: 'undo' },
        { role: 'redo' },
        { type: 'separator' },
        { role: 'cut' },
        { role: 'copy' },
        { role: 'paste' },
        ...(isMac ? [{ role: 'pasteAndMatchStyle' } as const] : []),
        { role: 'delete' },
        { role: 'selectAll' },
        { type: 'separator' },
        { label: 'Find…', accelerator: 'CommandOrControl+F', ...shown, click: cmd('find') },
      ],
    },
    {
      label: 'View',
      submenu: [
        { label: 'Home', accelerator: 'CommandOrControl+1', click: cmd('mode-chat') },
        { label: 'Bots', accelerator: 'CommandOrControl+2', click: cmd('mode-bot') },
        { label: 'Code', accelerator: 'CommandOrControl+3', click: cmd('mode-code') },
        { label: 'ACI', accelerator: 'CommandOrControl+4', click: cmd('mode-browser') },
        { type: 'separator' },
        { label: 'Toggle Sidebar', accelerator: 'CommandOrControl+\\', click: cmd('toggle-sidebar') },
        { label: 'Bot Activity', accelerator: 'CommandOrControl+Shift+M', ...shown, click: cmd('activity') },
        { type: 'separator' },
        { role: 'resetZoom' },
        { role: 'zoomIn' },
        { role: 'zoomOut' },
        { type: 'separator' },
        { role: 'togglefullscreen' },
        ...(app.isPackaged ? [] : [{ type: 'separator' } as const, { role: 'reload' } as const, { role: 'toggleDevTools' } as const]),
      ],
    },
    { role: 'windowMenu' },
    {
      role: 'help',
      submenu: [
        { label: 'Allternit Docs', click: () => h.openUrl('https://docs.allternit.com') },
        { label: 'Keyboard Shortcuts', click: () => h.openUrl('https://docs.allternit.com/surfaces/desktop#keyboard-shortcuts') },
      ],
    },
  ];

  return Menu.buildFromTemplate(template);
}
