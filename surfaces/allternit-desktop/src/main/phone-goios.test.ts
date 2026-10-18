import { afterEach, describe, expect, it } from 'vitest';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import type { ChildProcess } from 'node:child_process';
import { sha256Hex } from './phone-adb.js';
import {
  DDI_NAME,
  ensureDdi,
  ensureGoIos,
  firstJson,
  GoIos,
  lastGoIosError,
  parseAppsList,
  parseJsonLines,
  wrapChild,
  type ExecResult,
  type LongRunning,
} from './phone-goios.js';

const tmp: string[] = [];
afterEach(() => {
  for (const d of tmp.splice(0)) fs.rmSync(d, { recursive: true, force: true });
});
const tmpDir = () => {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), 'goios-'));
  tmp.push(d);
  return d;
};
const response = (data: Buffer, status = 200) => ({ ok: status === 200, status, arrayBuffer: async () => data }) as unknown as Response;

describe('downloads', () => {
  it('refuses a go-ios archive that does not match the pin', async () => {
    const prev = process.env.ALLTERNIT_GOIOS_PATH;
    delete process.env.ALLTERNIT_GOIOS_PATH;
    try {
      await expect(ensureGoIos(tmpDir(), { platform: 'darwin', fetchImpl: async () => response(Buffer.from('evil')) })).rejects.toThrow(/pinned checksum/);
    } finally {
      if (prev !== undefined) process.env.ALLTERNIT_GOIOS_PATH = prev;
    }
  });

  it('go-ios is macOS-only for now', async () => {
    const prev = process.env.ALLTERNIT_GOIOS_PATH;
    delete process.env.ALLTERNIT_GOIOS_PATH;
    try {
      await expect(ensureGoIos(tmpDir(), { platform: 'linux' })).rejects.toThrow(/Mac/);
    } finally {
      if (prev !== undefined) process.env.ALLTERNIT_GOIOS_PATH = prev;
    }
  });

  it('places a verified DDI, falls back to the mirror, and clears extracted copies', async () => {
    const dir = tmpDir();
    const good = Buffer.from('ddi bytes');
    const archive = { url: 'https://runtime.example/ddi.zip', sha256: sha256Hex(good), size: good.length };
    fs.mkdirSync(path.join(dir, DDI_NAME, 'Restore'), { recursive: true });
    const urls: string[] = [];
    await ensureDdi(dir, {
      archive,
      fallbackUrls: ['https://mirror.example/ddi.zip'],
      fetchImpl: (async (url: string) => {
        urls.push(url);
        return url.includes('runtime') ? response(Buffer.alloc(0), 404) : response(good);
      }) as unknown as typeof fetch,
    });
    expect(urls).toEqual(['https://runtime.example/ddi.zip', 'https://mirror.example/ddi.zip']);
    expect(fs.readdirSync(dir)).toEqual([`${DDI_NAME}.zip`]);
    // second call: already there, no download
    await ensureDdi(dir, { archive, fetchImpl: (async () => { throw new Error('no network'); }) as unknown as typeof fetch });
  });

  it('a mirror serving other bytes is refused', async () => {
    const good = Buffer.from('ddi bytes');
    const archive = { url: 'https://a/ddi.zip', sha256: sha256Hex(good), size: good.length };
    await expect(ensureDdi(tmpDir(), { archive, fallbackUrls: [], fetchImpl: (async () => response(Buffer.from('xxxxxxxxx'))) as unknown as typeof fetch })).rejects.toThrow(/pinned checksum/);
  });
});

describe('output parsing', () => {
  it('reads JSON log lines and the last error', () => {
    const text = '{"level":"INFO","msg":"hi"}\nnot json\n{"level":"ERROR","msg":"failed connecting to zipconduit","err":"dev image?"}\n';
    expect(parseJsonLines(text)).toHaveLength(2);
    expect(lastGoIosError(text)).toBe('failed connecting to zipconduit: dev image?');
  });

  it('reads results and app lists', () => {
    expect(firstJson<{ deviceList: string[] }>('{"level":"INFO","msg":"no udid"}\n{"deviceList":["A"]}')).toEqual({ deviceList: ['A'] });
    expect(parseAppsList('com.openai.chat ChatGPT 1.2\nai.x.GrokApp Grok 1.0\n')).toEqual(['com.openai.chat', 'ai.x.GrokApp']);
  });

  it('streams lines from a long-running child', async () => {
    const child = new EventEmitter() as ChildProcess & EventEmitter;
    const stdout = new PassThrough();
    const stderr = new PassThrough();
    Object.assign(child, { stdout, stderr, kill: () => child.emit('exit', null, 'SIGTERM') });
    const lr = wrapChild(child);
    const seen: (string | undefined)[] = [];
    lr.onLine((_line, parsed) => seen.push(parsed?.msg));
    stderr.write('{"level":"INFO","msg":"tunnel');
    stderr.write(' up"}\n{"msg":"b"}\n');
    await new Promise((r) => setImmediate(r));
    expect(seen).toEqual(['tunnel up', 'b']);
    lr.stop();
    expect(await lr.exited).toEqual({ code: null, signal: 'SIGTERM' });
    expect(lr.isRunning()).toBe(false);
  });
});

describe('GoIos commands', () => {
  function make(results: Record<string, ExecResult>) {
    const calls: string[][] = [];
    const spawned: string[][] = [];
    const goios = new GoIos(
      async (args) => {
        calls.push(args);
        return results[args[0]] ?? { stdout: '', stderr: '', code: 0 };
      },
      (args) => {
        spawned.push(args);
        return { onLine: () => () => {}, exited: new Promise(() => {}), isRunning: () => true, stop: () => {} } as LongRunning;
      },
      '/data/tunnel',
    );
    return { goios, calls, spawned };
  }

  it('list, devmode, launch errors', async () => {
    const { goios } = make({
      list: { stdout: '{"deviceList":["UDID-1"]}\n', stderr: '', code: 0 },
      devmode: { stdout: '{"DeveloperModeEnabled":true}\n', stderr: '', code: 0 },
      launch: {
        stdout: '',
        stderr: '{"level":"ERROR","msg":"launch failed","err":"Unable to launch because its profile has not been explicitly trusted by the user"}\n',
        code: 1,
      },
    });
    expect(await goios.list()).toEqual(['UDID-1']);
    expect(await goios.devmodeEnabled('UDID-1')).toBe(true);
    expect(await goios.launch('UDID-1', 'x')).toMatchObject({ ok: false, untrusted: true, notInstalled: false });
  });

  it('iOS 26: processcontrol error code 2 on launch means the developer is not trusted yet', async () => {
    const { goios } = make({
      launch: {
        stdout: '',
        stderr: '{"level":"ERROR","msg":"launch app command failed","err":"failed invoking method \'launchSuspendedProcessWithDevicePath:bundleIdentifier:environment:arguments:options:\' with error: Request to launch x failed. (Error code: 2, Domain: com.apple.dt.deviceprocesscontrolservice)"}\n',
        code: 1,
      },
    });
    expect(await goios.launch('UDID-1', 'x')).toMatchObject({ ok: false, untrusted: true });
  });

  it('runtest passes the auto-approve env only when asked; tunnel uses our identity dir', () => {
    const { goios, spawned } = make({});
    goios.runtest({ udid: 'U', runnerBundleId: 'p.ictrl.uitests.xctrunner', xctestConfig: 'iCTRLUITests.xctest', testToRun: 'iCTRLUITests.iCTRLUITests/testStartServer', env: { ICTRL_ALLOW_AUTH_ONCE: '1' } });
    goios.runtest({ udid: 'U', runnerBundleId: 'r', xctestConfig: 'c', testToRun: 't' });
    goios.tunnelStart('U');
    expect(spawned[0]).toContain('--env=ICTRL_ALLOW_AUTH_ONCE=1');
    expect(spawned[0]).toContain('--test-runner-bundle-id=p.ictrl.uitests.xctrunner');
    expect(spawned[1].some((a) => a.startsWith('--env'))).toBe(false);
    expect(spawned[2]).toEqual(['tunnel', 'start', '--userspace', '--pair-record-path=/data/tunnel', '--udid=U']);
  });

  it('install failure surfaces go-ios error text', async () => {
    const { goios } = make({ install: { stdout: '', stderr: '{"level":"ERROR","msg":"failed writing","err":"ApplicationVerificationFailed"}', code: 1 } });
    await expect(goios.install('U', '/x.ipa')).rejects.toThrow('failed writing: ApplicationVerificationFailed');
  });
});
