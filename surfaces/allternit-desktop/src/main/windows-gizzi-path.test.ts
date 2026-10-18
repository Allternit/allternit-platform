import { describe, expect, it } from 'vitest';
import {
  appendPathEntry,
  gizziShimContents,
  installWindowsGizziOnPath,
  parseRegPath,
  SHIM_MARKER,
} from './windows-gizzi-path.js';

const RES = String.raw`C:\Users\eoj\AppData\Local\Programs\Allternit Desktop\resources`;
const EXE = String.raw`${RES}\bin\gizzi-code.exe`;
const ENV = { LOCALAPPDATA: String.raw`C:\Users\eoj\AppData\Local` };
const BIN = String.raw`C:\Users\eoj\AppData\Local\Allternit\bin`;

function fakeDeps(opts: { regOut?: string | Error; shim?: string | null } = {}) {
  const files = new Map<string, string>();
  if (opts.shim != null) files.set(`${BIN}\\gizzi.cmd`, opts.shim);
  const calls: Array<[string, string[]]> = [];
  return {
    files,
    calls,
    deps: {
      env: ENV,
      exists: (p: string) => p === EXE,
      readFile: (p: string) => files.get(p) ?? null,
      writeFile: (p: string, d: string) => void files.set(p, d),
      mkdirp: () => {},
      run: async (file: string, args: string[]) => {
        calls.push([file, args]);
        if (file === 'reg' && args[0] === 'query') {
          if (opts.regOut instanceof Error) throw opts.regOut;
          return opts.regOut ?? '';
        }
        return '';
      },
    },
  };
}

describe('windows gizzi PATH', () => {
  it('parses reg query output, keeping the value type', () => {
    const out = '\r\nHKEY_CURRENT_USER\\Environment\r\n    Path    REG_EXPAND_SZ    %USERPROFILE%\\bin;C:\\tools\r\n\r\n';
    expect(parseRegPath(out)).toEqual({ type: 'REG_EXPAND_SZ', value: '%USERPROFILE%\\bin;C:\\tools' });
    expect(parseRegPath('nothing here')).toBeNull();
  });

  it('appends once, matching case, trailing slashes and %VARS%', () => {
    expect(appendPathEntry('C:\\a;C:\\b', BIN)).toBe(`C:\\a;C:\\b;${BIN}`);
    expect(appendPathEntry('', BIN)).toBe(BIN);
    expect(appendPathEntry(`C:\\a;${BIN.toUpperCase()}\\`, BIN)).toBeNull();
    expect(appendPathEntry('%LOCALAPPDATA%\\Allternit\\bin', BIN, ENV)).toBeNull();
  });

  it('writes the shim and appends the bin dir without dropping existing entries', async () => {
    const long = Array.from({ length: 80 }, (_, i) => `C:\\tools\\very-long-folder-name-${i}`).join(';');
    const { files, calls, deps } = fakeDeps({ regOut: `    Path    REG_SZ    ${long}\r\n` });
    const r = await installWindowsGizziOnPath(RES, deps);
    expect(r).toMatchObject({ action: 'installed', pathUpdated: true });
    expect(files.get(`${BIN}\\gizzi.cmd`)).toBe(gizziShimContents(EXE));
    expect(gizziShimContents(EXE)).toContain(`"${EXE}" %*`);
    const add = calls.find(([f, a]) => f === 'reg' && a[0] === 'add')!;
    expect(add[1]).toEqual(['add', 'HKCU\\Environment', '/v', 'Path', '/t', 'REG_SZ', '/d', `${long};${BIN}`, '/f']);
    expect(long.length).toBeGreaterThan(1024);
    expect(calls.some(([f]) => f === 'powershell.exe')).toBe(true);
  });

  it('creates an expandable Path when the user has none', async () => {
    const { calls, deps } = fakeDeps({ regOut: new Error('exit 1') });
    await installWindowsGizziOnPath(RES, deps);
    const add = calls.find(([f, a]) => f === 'reg' && a[0] === 'add')!;
    expect(add[1]).toContain('REG_EXPAND_SZ');
    expect(add[1]).toContain(BIN);
  });

  it('leaves PATH alone when the entry is already there', async () => {
    const { calls, deps } = fakeDeps({ regOut: `    Path    REG_EXPAND_SZ    ${BIN}\r\n`, shim: gizziShimContents(EXE) });
    const r = await installWindowsGizziOnPath(RES, deps);
    expect(r).toMatchObject({ action: 'installed', pathUpdated: false });
    expect(calls.filter(([f, a]) => f === 'reg' && a[0] === 'add')).toHaveLength(0);
  });

  it("keeps a gizzi.cmd it didn't write", async () => {
    const { files, calls, deps } = fakeDeps({ shim: '@echo off\r\nnode gizzi.js %*\r\n' });
    const r = await installWindowsGizziOnPath(RES, deps);
    expect(r.action).toBe('kept');
    expect(files.get(`${BIN}\\gizzi.cmd`)).not.toContain(SHIM_MARKER);
    expect(calls).toHaveLength(0);
  });

  it('skips when the bundle has no gizzi-code.exe', async () => {
    const { deps } = fakeDeps();
    expect((await installWindowsGizziOnPath('C:\\elsewhere', deps)).action).toBe('skipped');
  });
});
