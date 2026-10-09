/**
 * Motion export on Desktop: the app draws each frame by seeking (the same
 * `renderFrame` the preview uses) and sends it here as a JPEG. This module
 * keeps the frames in a private temp folder, encodes them with the local
 * ffmpeg into an H.264 MP4 and asks the person where to save it.
 *
 * Nothing here runs code from the artifact: the frames are pictures. If
 * ffmpeg isn't installed `check` says so and the app encodes in the window
 * instead.
 */

import { execFile as nodeExecFile } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import * as fs from 'node:fs';
import * as os from 'node:os';
import { join } from 'node:path';

export const MAX_FRAMES = 12_000;
export const MAX_FRAME_BYTES = 24 * 1024 * 1024;
const FFMPEG_CANDIDATES = ['ffmpeg', '/opt/homebrew/bin/ffmpeg', '/usr/local/bin/ffmpeg', '/usr/bin/ffmpeg'];
const ENCODE_TIMEOUT_MS = 10 * 60_000;

type ExecFile = (file: string, args: string[], opts: { timeout: number }, cb: (err: Error | null, stdout: string, stderr: string) => void) => unknown;

export interface MotionRenderDeps {
  execFile?: ExecFile;
  tmpdir?: () => string;
  /** Ask where to save; resolve to a path, or null when cancelled. */
  chooseSavePath(defaultName: string): Promise<string | null>;
}

interface Session {
  dir: string;
  fps: number;
  frames: number;
  received: number;
}

export interface BeginOptions { fps: number; width: number; height: number; frames: number }
export interface FinishResult { success: boolean; savedPath?: string; error?: string; cancelled?: boolean }

const ID = /^[a-f0-9-]{36}$/;

function safeName(title: string): string {
  const base = title.replace(/[\\/:*?"<>|\u0000-\u001f]+/g, ' ').replace(/\s+/g, ' ').trim().slice(0, 80);
  return base || 'motion';
}

export function ffmpegArgs(dir: string, out: string, o: { fps: number }): string[] {
  return [
    '-y', '-hide_banner', '-loglevel', 'error',
    '-framerate', String(o.fps),
    '-i', join(dir, 'f%06d.jpg'),
    '-c:v', 'libx264', '-preset', 'medium', '-crf', '18',
    '-pix_fmt', 'yuv420p', '-movflags', '+faststart',
    out,
  ];
}

export function createMotionRender(deps: MotionRenderDeps) {
  const exec: ExecFile = deps.execFile ?? ((file, args, opts, cb) => nodeExecFile(file, args, opts, cb));
  const tmp = deps.tmpdir ?? os.tmpdir;
  const sessions = new Map<string, Session>();
  let ffmpegPath: string | null | undefined;

  const run = (file: string, args: string[], timeout: number) =>
    new Promise<{ ok: boolean; error?: string }>((resolve) => {
      exec(file, args, { timeout }, (err, _out, stderr) => resolve(err ? { ok: false, error: (stderr || err.message).trim().slice(-400) } : { ok: true }));
    });

  async function findFfmpeg(): Promise<string | null> {
    if (ffmpegPath !== undefined) return ffmpegPath;
    for (const candidate of FFMPEG_CANDIDATES) {
      if ((await run(candidate, ['-version'], 5000)).ok) return (ffmpegPath = candidate);
    }
    // A failed lookup is not cached for good: the person may install ffmpeg while the app is open.
    return null;
  }

  function drop(id: string) {
    const s = sessions.get(id);
    if (!s) return;
    sessions.delete(id);
    try { fs.rmSync(s.dir, { recursive: true, force: true }); } catch { /* ignore */ }
  }

  return {
    async check(): Promise<{ ffmpeg: boolean }> {
      return { ffmpeg: Boolean(await findFfmpeg()) };
    },

    async begin(o: BeginOptions): Promise<{ id: string }> {
      const fps = Math.round(Number(o?.fps));
      const frames = Math.round(Number(o?.frames));
      if (!(fps >= 1 && fps <= 120) || !(frames >= 1 && frames <= MAX_FRAMES)) throw new Error('Invalid render size.');
      const id = randomUUID();
      const dir = join(tmp(), `allternit-motion-${id}`);
      fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
      sessions.set(id, { dir, fps, frames, received: 0 });
      return { id };
    },

    async frame(id: string, index: number, jpeg: ArrayBuffer | Uint8Array): Promise<void> {
      const s = ID.test(String(id)) ? sessions.get(id) : undefined;
      if (!s) throw new Error('No render in progress.');
      if (!Number.isInteger(index) || index < 0 || index >= s.frames) throw new Error('Frame out of range.');
      const bytes = jpeg instanceof Uint8Array ? jpeg : new Uint8Array(jpeg);
      if (bytes.byteLength === 0 || bytes.byteLength > MAX_FRAME_BYTES) throw new Error('Bad frame.');
      fs.writeFileSync(join(s.dir, `f${String(index).padStart(6, '0')}.jpg`), bytes);
      s.received += 1;
    },

    async finish(id: string, opts: { title: string }): Promise<FinishResult> {
      const s = ID.test(String(id)) ? sessions.get(id) : undefined;
      if (!s) return { success: false, error: 'No render in progress.' };
      try {
        if (s.received !== s.frames) return { success: false, error: `Only ${s.received} of ${s.frames} frames arrived.` };
        const ffmpeg = await findFfmpeg();
        if (!ffmpeg) return { success: false, error: 'ffmpeg isn’t installed.' };
        const out = join(s.dir, 'out.mp4');
        const done = await run(ffmpeg, ffmpegArgs(s.dir, out, { fps: s.fps }), ENCODE_TIMEOUT_MS);
        if (!done.ok) return { success: false, error: `ffmpeg failed: ${done.error ?? 'unknown error'}` };
        if (!fs.existsSync(out)) return { success: false, error: 'ffmpeg produced no file.' };
        const target = await deps.chooseSavePath(`${safeName(opts?.title ?? '')}.mp4`);
        if (!target) return { success: false, cancelled: true };
        fs.copyFileSync(out, target);
        return { success: true, savedPath: target };
      } catch (e) {
        return { success: false, error: (e as Error).message };
      } finally {
        drop(id);
      }
    },

    async abort(id: string): Promise<void> {
      if (ID.test(String(id))) drop(id);
    },

    /** Test hook. */
    _sessions: sessions,
  };
}
