import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { describe, expect, it } from 'vitest';
import { autostartContents, autostartPath, isLinuxAutostartEnabled, launchedHidden, quoteExecArg, setLinuxAutostart } from './linux-autostart.js';

describe('linux autostart', () => {
  it('uses ~/.config by default and respects XDG_CONFIG_HOME', () => {
    expect(autostartPath({}, '/home/t')).toBe('/home/t/.config/autostart/allternit.desktop');
    expect(autostartPath({ XDG_CONFIG_HOME: '/x/cfg' }, '/home/t')).toBe('/x/cfg/autostart/allternit.desktop');
    expect(autostartPath({ XDG_CONFIG_HOME: 'relative' }, '/home/t')).toBe('/home/t/.config/autostart/allternit.desktop');
  });
  it('quotes an Exec path with spaces', () => {
    expect(quoteExecArg('/opt/Allternit Desktop/allternit')).toBe('"/opt/Allternit Desktop/allternit"');
    expect(quoteExecArg('/usr/bin/allternit')).toBe('/usr/bin/allternit');
    const c = autostartContents('/opt/Allternit Desktop/allternit');
    expect(c).toContain('Exec="/opt/Allternit Desktop/allternit"\n');
    expect(c.startsWith('[Desktop Entry]\nType=Application\n')).toBe(true);
  });
  it('background mode starts hidden', () => {
    expect(autostartContents('/usr/bin/allternit', true)).toContain('Exec=/usr/bin/allternit --hidden\n');
    expect(autostartContents('/usr/bin/allternit')).toContain('Exec=/usr/bin/allternit\n');
    expect(launchedHidden(['/usr/bin/allternit', '--hidden'])).toBe(true);
    expect(launchedHidden(['/usr/bin/allternit'])).toBe(false);
  });
  it('enable writes, disable removes, read reflects the file', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'autostart-'));
    const file = path.join(dir, 'autostart', 'allternit.desktop');
    expect(isLinuxAutostartEnabled(file)).toBe(false);
    setLinuxAutostart(true, file, '/opt/a b/allternit');
    expect(isLinuxAutostartEnabled(file)).toBe(true);
    expect(fs.readFileSync(file, 'utf8')).toContain('Exec="/opt/a b/allternit"');
    setLinuxAutostart(false, file);
    expect(isLinuxAutostartEnabled(file)).toBe(false);
    setLinuxAutostart(false, file);
    setLinuxAutostart(true, file, '/usr/bin/allternit', true);
    expect(fs.readFileSync(file, 'utf8')).toContain('Exec=/usr/bin/allternit --hidden\n');
    setLinuxAutostart(false, file);
    fs.rmSync(dir, { recursive: true, force: true });
  });
});
