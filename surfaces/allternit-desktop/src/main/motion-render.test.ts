import { describe, expect, it } from 'vitest';
import * as fs from 'node:fs';
import * as os from 'node:os';
import { join } from 'node:path';
import { createMotionRender, ffmpegArgs, MAX_FRAMES } from './motion-render.js';

const jpeg = new Uint8Array([0xff, 0xd8, 0xff, 0xd9]);
const root = fs.mkdtempSync(join(os.tmpdir(), 'motion-render-test-'));

function make(over: { ffmpeg?: boolean; save?: string | null; fail?: boolean } = {}) {
  const calls: string[][] = [];
  const r = createMotionRender({
    tmpdir: () => root,
    execFile: (file, args, _o, cb) => {
      calls.push([file, ...args]);
      if (args[0] === '-version') return over.ffmpeg === false ? cb(new Error('not found'), '', '') : cb(null, 'ffmpeg 7', '');
      if (over.fail) return cb(new Error('boom'), '', 'bad codec');
      fs.writeFileSync(args[args.length - 1], 'mp4');
      cb(null, '', '');
    },
    chooseSavePath: async () => (over.save === undefined ? join(root, 'saved.mp4') : over.save),
  });
  return { r, calls };
}

describe('motion render (Desktop)', () => {
  it('reports ffmpeg presence', async () => {
    expect(await make().r.check()).toEqual({ ffmpeg: true });
    expect(await make({ ffmpeg: false }).r.check()).toEqual({ ffmpeg: false });
  });

  it('writes frames, encodes, saves and cleans up', async () => {
    const { r, calls } = make();
    const { id } = await r.begin({ fps: 30, width: 1920, height: 1080, frames: 3 });
    for (let i = 0; i < 3; i++) await r.frame(id, i, jpeg.buffer.slice(0) as ArrayBuffer);
    const res = await r.finish(id, { title: 'Q3 / results' });
    expect(res).toEqual({ success: true, savedPath: join(root, 'saved.mp4') });
    expect(fs.readFileSync(join(root, 'saved.mp4'), 'utf8')).toBe('mp4');
    expect(r._sessions.size).toBe(0);
    const enc = calls.find((c) => c.includes('libx264'))!;
    expect(enc).toContain('-framerate');
    expect(enc[enc.indexOf('-framerate') + 1]).toBe('30');
  });

  it('keeps the save dialog default name safe', async () => {
    let name = '';
    const r = createMotionRender({
      tmpdir: () => root,
      execFile: (_f, a, _o, cb) => { if (a[0] !== '-version') fs.writeFileSync(a[a.length - 1], 'x'); cb(null, '', ''); },
      chooseSavePath: async (n) => { name = n; return null; },
    });
    const { id } = await r.begin({ fps: 24, width: 10, height: 10, frames: 1 });
    await r.frame(id, 0, jpeg);
    expect(await r.finish(id, { title: '../../etc/passwd: a*b' })).toEqual({ success: false, cancelled: true });
    expect(name).toBe('.. .. etc passwd a b.mp4');
    expect(name).not.toMatch(/[\\/:*]/);
  });

  it('refuses bad input', async () => {
    const { r } = make();
    await expect(r.begin({ fps: 0, width: 1, height: 1, frames: 1 })).rejects.toThrow();
    await expect(r.begin({ fps: 30, width: 1, height: 1, frames: MAX_FRAMES + 1 })).rejects.toThrow();
    const { id } = await r.begin({ fps: 30, width: 1, height: 1, frames: 2 });
    await expect(r.frame(id, 2, jpeg)).rejects.toThrow(/range/);
    await expect(r.frame(id, 0, new Uint8Array(0))).rejects.toThrow(/Bad frame/);
    await expect(r.frame('../../x', 0, jpeg)).rejects.toThrow(/No render/);
    expect((await r.finish(id, { title: 't' })).error).toMatch(/Only 0 of 2/);
    expect((await r.finish('nope', { title: 't' })).success).toBe(false);
  });

  it('reports ffmpeg failures and cleans up on abort', async () => {
    const { r } = make({ fail: true });
    const { id } = await r.begin({ fps: 30, width: 1, height: 1, frames: 1 });
    await r.frame(id, 0, jpeg);
    expect((await r.finish(id, { title: 't' })).error).toMatch(/ffmpeg failed: bad codec/);
    const b = await r.begin({ fps: 30, width: 1, height: 1, frames: 1 });
    await r.abort(b.id);
    expect(r._sessions.size).toBe(0);
  });

  it('builds the encode command', () => {
    expect(ffmpegArgs('/d', '/d/out.mp4', { fps: 24 })).toEqual(expect.arrayContaining(['-pix_fmt', 'yuv420p', '/d/out.mp4']));
  });
});
