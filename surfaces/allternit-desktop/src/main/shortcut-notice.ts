/**
 * One notice per session when a global shortcut cannot be registered on Linux
 * (Wayland blocks them, or another app holds the key). Never spams: only the
 * first failure notifies; every failure is logged by the caller.
 */
export interface ShortcutNoticeDeps {
  platform: string;
  sessionType: string | undefined;
  notify: (title: string, body: string) => void;
}

export function shortcutNoticeBody(accelerator: string, sessionType: string | undefined): string {
  const base = `Allternit could not register the global shortcut ${accelerator}.`;
  return sessionType === 'wayland'
    ? `${base} On Wayland, global shortcuts may need to be set in your desktop's own keyboard settings.`
    : `${base} Another app may be using that key.`;
}

export function createShortcutNotifier(deps: ShortcutNoticeDeps): (accelerator: string, registered: boolean) => boolean {
  let shown = false;
  return (accelerator, registered) => {
    if (registered || deps.platform !== 'linux' || shown) return false;
    shown = true;
    deps.notify('Shortcut unavailable', shortcutNoticeBody(accelerator, deps.sessionType));
    return true;
  };
}

let session: ((accelerator: string, registered: boolean) => boolean) | null = null;

/** Session-wide notifier; call with each register() result. Electron loads lazily. */
export function noteShortcutResult(accelerator: string, registered: boolean): void {
  session ??= createShortcutNotifier({
    platform: process.platform,
    sessionType: process.env.XDG_SESSION_TYPE,
    notify: (title, body) => {
      void import('electron').then(({ Notification }) => {
        if (Notification.isSupported()) new Notification({ title, body }).show();
      }).catch(() => {});
    },
  });
  session(accelerator, registered);
}
