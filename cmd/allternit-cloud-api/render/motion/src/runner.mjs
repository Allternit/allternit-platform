// Motion cloud render entry. Bundled with the app's own renderer by
// scripts/build-motion-render.mjs into dist/render.mjs; the cloud-api host
// runs it as `node dist/render.mjs --in composition.json --out out.mp4`.
//
// One renderer: `renderFrame` below is src/lib/artifacts/motion/render.ts from
// allternit-ai, unchanged. Frames are drawn into @napi-rs/canvas and piped to
// ffmpeg as raw RGBA. Progress goes to stdout as `progress <0..1>` lines;
// everything else (errors) goes to stderr. Exit code 0 means the MP4 exists.

import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createCanvas, GlobalFonts, loadImage } from '@napi-rs/canvas';
import { parseMotion, totalDuration } from 'allternit-ai/lib/artifacts/motion/schema';
import { compositionImages, renderFrame } from 'allternit-ai/lib/artifacts/motion/render';

/** Fonts shipped next to the bundle, registered under the first family of each theme stack. */
const FONTS = [
  ['Inter', 'Inter_500Medium.ttf'],
  ['Inter', 'Inter_600SemiBold.ttf'],
  ['Inter', 'Inter_700Bold.ttf'],
  ['Inter', 'Inter_800ExtraBold.ttf'],
  // The serif and mono stacks start with Georgia and ui-monospace/Menlo,
  // which a Linux host doesn't have; these stand in.
  ['Georgia', 'SourceSerif4_500Medium.ttf'],
  ['Georgia', 'SourceSerif4_700Bold.ttf'],
  ['Menlo', 'JetBrainsMono_500Medium.ttf'],
  ['Menlo', 'JetBrainsMono_700Bold.ttf'],
  ['ui-monospace', 'JetBrainsMono_500Medium.ttf'],
  ['ui-monospace', 'JetBrainsMono_700Bold.ttf'],
];

function registerFonts() {
  const dir = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'fonts');
  for (const [family, file] of FONTS) {
    const p = path.join(dir, file);
    if (existsSync(p)) GlobalFonts.registerFromPath(p, family);
  }
}

function args(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 2) {
    const k = argv[i];
    if (!k?.startsWith('--') || argv[i + 1] === undefined) throw new Error(`Bad arguments near ${k ?? '(end)'}`);
    out[k.slice(2)] = argv[i + 1];
  }
  for (const k of ['in', 'out']) if (!out[k]) throw new Error(`Missing --${k}`);
  return { input: out.in, output: out.out, ffmpeg: out.ffmpeg || 'ffmpeg' };
}

/** Only `data:image/...` logos are decoded; remote URLs are never fetched from the server. */
async function loadAssets(comp) {
  const map = new Map();
  await Promise.all(
    compositionImages(comp).map(async (src) => {
      if (!/^data:image\//i.test(src)) return;
      try {
        const img = await loadImage(src);
        map.set(src, { source: img, width: img.width || 512, height: img.height || 512 });
      } catch {
        // The wordmark draws instead.
      }
    }),
  );
  return map;
}

function startFfmpeg(bin, { width, height, fps }, output) {
  const proc = spawn(
    bin,
    [
      '-hide_banner', '-loglevel', 'error', '-y',
      '-f', 'rawvideo', '-pix_fmt', 'rgba', '-s', `${width}x${height}`, '-framerate', String(fps), '-i', 'pipe:0',
      '-an', '-c:v', 'libx264', '-preset', 'veryfast', '-crf', '18',
      '-pix_fmt', 'yuv420p', '-r', String(fps), '-movflags', '+faststart', output,
    ],
    { stdio: ['pipe', 'ignore', 'pipe'] },
  );
  let stderr = '';
  proc.stderr.on('data', (d) => {
    stderr = (stderr + d).slice(-2000);
  });
  let failed = null;
  proc.stdin.on('error', (e) => {
    failed = e;
  });
  const exited = new Promise((resolve) => proc.on('close', (code) => resolve(code)));
  return { proc, exited, error: () => failed, stderr: () => stderr };
}

async function write(stream, buf, ff) {
  if (ff.error()) throw ff.error();
  if (stream.write(buf)) return;
  await new Promise((resolve, reject) => {
    const onDrain = () => { stream.off('error', onError); resolve(); };
    const onError = (e) => { stream.off('drain', onDrain); reject(e); };
    stream.once('drain', onDrain);
    stream.once('error', onError);
  });
}

async function main() {
  const { input, output, ffmpeg } = args(process.argv.slice(2));
  const parsed = parseMotion(await readFile(input, 'utf8'));
  if (!parsed.ok) throw new Error(`Invalid composition: ${parsed.error}`);
  const comp = parsed.comp;
  registerFonts();
  const assets = await loadAssets(comp);

  // H.264 needs even sizes; the drawing is scaled from the composition's own units.
  const size = { width: comp.width + (comp.width % 2), height: comp.height + (comp.height % 2), fps: comp.fps };
  const frames = Math.max(1, Math.round(totalDuration(comp) * comp.fps));
  const canvas = createCanvas(size.width, size.height);
  const ctx = canvas.getContext('2d');
  const ff = startFfmpeg(ffmpeg, size, output);

  try {
    for (let i = 0; i < frames; i += 1) {
      ctx.setTransform(size.width / comp.width, 0, 0, size.height / comp.height, 0, 0);
      renderFrame(ctx, comp, i / comp.fps, assets);
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      await write(ff.proc.stdin, canvas.data(), ff);
      if (i % 10 === 9 || i === frames - 1) console.log(`progress ${((i + 1) / frames).toFixed(4)}`);
    }
    ff.proc.stdin.end();
  } catch (e) {
    ff.proc.kill('SIGKILL');
    await ff.exited;
    throw new Error(`${e instanceof Error ? e.message : e}\n${ff.stderr()}`);
  }
  const code = await ff.exited;
  if (code !== 0) throw new Error(`ffmpeg exited with ${code}\n${ff.stderr()}`);
}

main().catch((e) => {
  console.error(e instanceof Error ? e.stack || e.message : String(e));
  process.exit(1);
});
