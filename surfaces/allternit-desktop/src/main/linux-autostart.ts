/**
 * XDG autostart for Linux "Start at login".
 *
 * Electron's app.setLoginItemSettings is a no-op on Linux, so the toggle writes
 * or removes ~/.config/autostart/allternit.desktop (honouring XDG_CONFIG_HOME).
 * The Mac path passes no hidden flag from unified-main, so none is added here.
 * Pure helpers are separate from the fs calls so they can be unit tested.
 */
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

export const AUTOSTART_FILE = 'allternit.desktop';

export function autostartPath(env: NodeJS.ProcessEnv = process.env, home: string = os.homedir()): string {
  const base = env.XDG_CONFIG_HOME && path.isAbsolute(env.XDG_CONFIG_HOME)
    ? env.XDG_CONFIG_HOME
    : path.join(home, '.config');
  return path.join(base, 'autostart', AUTOSTART_FILE);
}

/** Quote one Exec argument per the Desktop Entry spec (spaces, quotes, $, backticks, backslashes). */
export function quoteExecArg(arg: string): string {
  if (!/[\s"'`$\\<>~|&;*?#()]/.test(arg)) return arg;
  return `"${arg.replace(/(["`$\\])/g, '\\$1').replace(/%/g, '%%')}"`;
}

export function autostartContents(execPath: string): string {
  return [
    '[Desktop Entry]',
    'Type=Application',
    'Name=Allternit',
    `Exec=${quoteExecArg(execPath)}`,
    'Terminal=false',
    'X-GNOME-Autostart-enabled=true',
    'Comment=Allternit desktop app',
    '',
  ].join('\n');
}

export function isLinuxAutostartEnabled(file: string = autostartPath()): boolean {
  try {
    return fs.existsSync(file);
  } catch {
    return false;
  }
}

export function setLinuxAutostart(
  enabled: boolean,
  file: string = autostartPath(),
  execPath: string = process.execPath,
): void {
  if (enabled) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, autostartContents(execPath), { mode: 0o644 });
  } else {
    fs.rmSync(file, { force: true });
  }
}
