// Local proof of the pod's media override, runnable on a Mac:
//   npm run local-test            (headless)
//   npm run local-test -- --headed
// Serves index.html and a generated 440 Hz tone on 127.0.0.1, opens it in the
// system Google Chrome with the same init script join.ts injects, and checks:
//   - getUserMedia returns our tracks, both live
//   - the video is the 1280x720 camera canvas and delivers frames (the tile
//     picture itself is proven by local-test/tile.ts, which needs the tile page)
//   - the mic is silent until __gizziSpeak, then carries sound, then silent again
// Then the call-capture check (peer-capture.ts, what join.ts records with):
//   a tester page and a bot page in separate browser contexts connect over a
//   real RTCPeerConnection on 127.0.0.1. The tester plays a WAV into the call
//   (out/audio/tester1.wav by default), then adds a second audio track later
//   with a renegotiation, plays a second WAV and stops that track. The bot page
//   runs the same init scripts join.ts injects and sends its own mic (with a
//   tone on it), and never plays the remote audio itself. Node pulls the
//   recording into out/local-capture/call.webm and checks: two remote tracks
//   added, the second one ended, the bot's own track never recorded, the
//   recording lasts as long as the call, and it is not silent. It also writes
//   out/local-capture/audio/seg-000.wav (+ segments.csv, capture-start-ms,
//   speakers.jsonl) so `bash transcribe.sh out/local-capture` can transcribe it.
//
//   node dist/local-test/run.js [--headed] [--capture-only | --media-only]
//     [--wav1 out/audio/tester1.wav] [--wav2 out/audio/tester.wav]
// Exits 0 on pass, 1 on fail.

import { chromium, type Browser, type Page } from "playwright-core";
import { createServer } from "node:http";
import { spawnSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { AddressInfo } from "node:net";
import { installGizziMedia } from "../gizzi-media.js";
import { drainPeerCapture, installPeerCapture, type PeerCaptureEvent } from "../peer-capture.js";

const here = dirname(fileURLToPath(import.meta.url));
// Compiled to dist/local-test/run.js; the HTML stays in the source folder.
const srcDir = join(here, "..", "..", "local-test");
const spikeDir = join(srcDir, "..");
const argv = process.argv.slice(2);
const flag = (f: string): string | undefined => {
  const i = argv.indexOf(f);
  return i >= 0 ? argv[i + 1] : undefined;
};
const wav1Path = resolve(flag("--wav1") ?? join(spikeDir, "out", "audio", "tester1.wav"));
const wav2Path = resolve(flag("--wav2") ?? join(spikeDir, "out", "audio", "tester.wav"));

function toneWav(seconds: number, hz: number, rate = 48000): Buffer {
  const n = Math.floor(seconds * rate);
  const buf = Buffer.alloc(44 + n * 2);
  buf.write("RIFF", 0);
  buf.writeUInt32LE(36 + n * 2, 4);
  buf.write("WAVEfmt ", 8);
  buf.writeUInt32LE(16, 16);
  buf.writeUInt16LE(1, 20); // PCM
  buf.writeUInt16LE(1, 22); // mono
  buf.writeUInt32LE(rate, 24);
  buf.writeUInt32LE(rate * 2, 28);
  buf.writeUInt16LE(2, 32);
  buf.writeUInt16LE(16, 34);
  buf.write("data", 36);
  buf.writeUInt32LE(n * 2, 40);
  for (let i = 0; i < n; i++) buf.writeInt16LE(Math.round(Math.sin((2 * Math.PI * hz * i) / rate) * 0.5 * 32767), 44 + i * 2);
  return buf;
}

const html = readFileSync(join(srcDir, "index.html"));
const tone = toneWav(1.5, 440);
const server = createServer((req, res) => {
  if (req.url === "/" || req.url === "/index.html") {
    res.writeHead(200, { "content-type": "text/html; charset=utf-8" }).end(html);
  } else if (req.url === "/tone.wav") {
    res.writeHead(200, { "content-type": "audio/wav" }).end(tone);
  } else if (req.url === "/call.html") {
    res.writeHead(200, { "content-type": "text/html; charset=utf-8" }).end("<!doctype html><title>call</title><body>call</body>");
  } else if ((req.url === "/wav1" || req.url === "/wav2") && existsSync(req.url === "/wav1" ? wav1Path : wav2Path)) {
    res.writeHead(200, { "content-type": "audio/wav" }).end(readFileSync(req.url === "/wav1" ? wav1Path : wav2Path));
  } else {
    res.writeHead(404).end();
  }
});
await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
const port = (server.address() as AddressInfo).port;

const chrome =
  process.env.CHROME_PATH ??
  (process.platform === "darwin" ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" : "/usr/bin/google-chrome");

const browser = await chromium.launch({
  executablePath: chrome,
  headless: !argv.includes("--headed"),
  // The mDNS flag: host candidates stay plain 127.0.0.1 addresses, so the two
  // peers connect without an mDNS responder (headless, CI).
  args: ["--autoplay-policy=no-user-gesture-required", "--disable-features=WebRtcHideLocalIpsWithMdns"],
});

function report(checks: Array<[string, boolean]>): boolean {
  let ok = true;
  for (const [name, pass] of checks) {
    console.log(`${pass ? "PASS" : "FAIL"}  ${name}`);
    if (!pass) ok = false;
  }
  return ok;
}

async function mediaCheck(browser: Browser): Promise<boolean> {
  const context = await browser.newContext();
  await context.addInitScript(installGizziMedia, { name: "Gizzi (Allternit)", state: "Listening" });
  const page = await context.newPage();
  page.on("pageerror", (e) => console.log("pageerror:", String(e)));
  await page.goto(`http://127.0.0.1:${port}/`);
  await page.waitForFunction(() => (window as unknown as { __result?: unknown }).__result, null, { timeout: 30_000 });
  const r = (await page.evaluate(() => (window as unknown as { __result: Record<string, unknown> }).__result)) as {
    error?: string;
    overrideInstalled: boolean;
    devices: string[];
    videoTrack: { readyState: string; deviceId: string };
    audioTrack: { readyState: string; deviceId: string };
    videoSize: string;
    distinctFrames: number;
    framesDrawn: number;
    silentRms: number;
    speakingRms: number;
    afterRms: number;
    speak: { durationSec: number };
  };
  await page.screenshot({ path: join(srcDir, "..", "dist", "local-test", "tile.png") });
  console.log(JSON.stringify(r, null, 2));

  const checks: Array<[string, boolean]> = r.error
    ? [[`page error: ${r.error}`, false]]
    : [
        ["init script installed before page scripts", r.overrideInstalled],
        ["enumerateDevices lists the tile and the voice", r.devices.some((d) => d.startsWith("videoinput:")) && r.devices.some((d) => d.startsWith("audioinput:"))],
        ["video track is live", r.videoTrack.readyState === "live"],
        ["audio track is live", r.audioTrack.readyState === "live"],
        ["video is 1280x720", r.videoSize === "1280x720"],
        ["camera canvas keeps drawing (framesDrawn > 10)", r.framesDrawn > 10],
        ["mic silent before speaking (rms < 0.001)", r.silentRms < 0.001],
        ["__gizziSpeak puts sound on the mic (rms > 0.05)", r.speakingRms > 0.05],
        ["mic silent again after speaking (rms < 0.001)", r.afterRms < 0.001],
      ];
  await context.close();
  return report(checks);
}

// ---- call capture check ------------------------------------------------------

type CallWin = Window & {
  __pc?: RTCPeerConnection;
  __ac?: AudioContext;
  __dests?: MediaStreamAudioDestinationNode[];
  __second?: RTCRtpTransceiver;
};

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/** One offer/answer round, non-trickle (each side waits for ICE gathering). */
async function negotiate(offerer: Page, answerer: Page): Promise<void> {
  const offer = await offerer.evaluate(async () => {
    const pc = (window as CallWin).__pc as RTCPeerConnection;
    await pc.setLocalDescription(await pc.createOffer());
    await new Promise<void>((r) => {
      if (pc.iceGatheringState === "complete") return r();
      pc.addEventListener("icegatheringstatechange", () => pc.iceGatheringState === "complete" && r());
      setTimeout(r, 3000);
    });
    return (pc.localDescription as RTCSessionDescription).toJSON() as RTCSessionDescriptionInit;
  });
  const answer = await answerer.evaluate(async (o) => {
    const pc = (window as CallWin).__pc as RTCPeerConnection;
    await pc.setRemoteDescription(o);
    await pc.setLocalDescription(await pc.createAnswer());
    await new Promise<void>((r) => {
      if (pc.iceGatheringState === "complete") return r();
      pc.addEventListener("icegatheringstatechange", () => pc.iceGatheringState === "complete" && r());
      setTimeout(r, 3000);
    });
    return (pc.localDescription as RTCSessionDescription).toJSON() as RTCSessionDescriptionInit;
  }, offer);
  await offerer.evaluate(async (a) => {
    await ((window as CallWin).__pc as RTCPeerConnection).setRemoteDescription(a);
  }, answer);
}

function ffmpeg(args: string[]): { code: number; err: string } {
  const r = spawnSync("ffmpeg", ["-hide_banner", "-nostats", ...args], { encoding: "utf8" });
  return { code: r.status ?? -1, err: r.stderr ?? String(r.error ?? "") };
}

function wavSeconds(file: string): number {
  const r = spawnSync("ffprobe", ["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", file], { encoding: "utf8" });
  return Number((r.stdout ?? "").trim());
}

async function captureCheck(browser: Browser): Promise<boolean> {
  if (!existsSync(wav1Path) || !existsSync(wav2Path)) {
    return report([[`test WAVs present (${wav1Path}, ${wav2Path})`, false]]);
  }
  if (spawnSync("ffmpeg", ["-version"]).status !== 0) return report([["ffmpeg on PATH (decodes the recording)", false]]);

  const out = join(spikeDir, "out", "local-capture");
  rmSync(out, { recursive: true, force: true });
  mkdirSync(join(out, "audio"), { recursive: true });
  const events: PeerCaptureEvent[] = [];
  const onEvent = (e: PeerCaptureEvent) => {
    events.push(e);
    appendFileSync(join(out, "events.jsonl"), JSON.stringify(e) + "\n");
    console.log(`  [bot] ${e.event} ${JSON.stringify(e.detail ?? {})}`);
  };
  const speakers = (name: string, from: number, to: number) => {
    for (let t = from; t <= to; t += 250) appendFileSync(join(out, "speakers.jsonl"), JSON.stringify({ t, name }) + "\n");
  };

  // The bot: the same init scripts join.ts injects. The tester: a plain page.
  const botCtx = await browser.newContext();
  await botCtx.addInitScript(installGizziMedia, { name: "Gizzi (Allternit)", state: "Listening" });
  await botCtx.addInitScript(installPeerCapture, { timesliceMs: 1000 });
  const testerCtx = await browser.newContext();
  const bot = await botCtx.newPage();
  const tester = await testerCtx.newPage();
  bot.on("pageerror", (e) => console.log("bot pageerror:", String(e)));
  tester.on("pageerror", (e) => console.log("tester pageerror:", String(e)));
  await bot.goto(`http://127.0.0.1:${port}/call.html`);
  await tester.goto(`http://127.0.0.1:${port}/call.html`);

  await tester.evaluate(async () => {
    const w = window as CallWin;
    w.__pc = new RTCPeerConnection();
    w.__ac = new AudioContext({ sampleRate: 48000 });
    await w.__ac.resume();
    const d = w.__ac.createMediaStreamDestination();
    w.__dests = [d];
    w.__pc.addTrack(d.stream.getAudioTracks()[0], d.stream);
  });
  // The bot sends its own mic (gizzi-media.ts) on the same connection; it must
  // never end up in the recording.
  const botSide = await bot.evaluate(async () => {
    const w = window as CallWin;
    w.__pc = new RTCPeerConnection();
    const s = await navigator.mediaDevices.getUserMedia({ audio: true });
    const t = s.getAudioTracks()[0];
    w.__pc.addTrack(t, s);
    return { capture: Boolean(window.__gizziPeerCap), media: Boolean(window.__gizziMedia), localTrack: t.id };
  });
  console.log("  bot:", JSON.stringify(botSide));

  let chain: Promise<unknown> = Promise.resolve();
  let bytes = 0;
  const pull = (flush = false) => {
    chain = chain.then(async () => {
      bytes += await drainPeerCapture(bot, out, onEvent, { flush });
    });
    return chain;
  };
  const puller = setInterval(() => void pull(), 1000);

  let connected = false;
  let d1 = 0;
  let d2 = 0;
  let botSpoke = false;
  try {
    await negotiate(tester, bot);
    connected = await bot
      .waitForFunction(() => (window as CallWin).__pc?.connectionState === "connected", null, { timeout: 15_000 })
      .then(() => true, () => false);
    console.log(`  connected: ${connected}`);

    const play = (i: number, url: string) =>
      tester.evaluate(
        async ({ i, url }) => {
          const w = window as CallWin;
          const ac = w.__ac as AudioContext;
          const buf = await ac.decodeAudioData(await (await fetch(url)).arrayBuffer());
          const src = ac.createBufferSource();
          src.buffer = buf;
          src.connect((w.__dests as MediaStreamAudioDestinationNode[])[i]);
          await new Promise<void>((r) => {
            src.onended = () => r();
            src.start();
          });
          return buf.duration;
        },
        { i, url },
      );

    await sleep(1000);
    // A tone on the bot's own mic while the tester talks: it goes out to the
    // tester, and must not come back into the bot's recording.
    const tone = bot.evaluate(() => window.__gizziSpeak?.("/tone.wav")).then(() => (botSpoke = true), () => undefined);
    let t0 = Date.now();
    d1 = await play(0, "/wav1");
    speakers("Tester (track 1)", t0, Date.now());
    await tone;

    // A second remote track arrives later (renegotiation), talks, then stops.
    await tester.evaluate(() => {
      const w = window as CallWin;
      const d = (w.__ac as AudioContext).createMediaStreamDestination();
      (w.__dests as MediaStreamAudioDestinationNode[]).push(d);
      w.__second = (w.__pc as RTCPeerConnection).addTransceiver(d.stream.getAudioTracks()[0], { direction: "sendonly", streams: [d.stream] });
    });
    await negotiate(tester, bot);
    await sleep(800);
    t0 = Date.now();
    d2 = await play(1, "/wav2");
    speakers("Tester (track 2)", t0, Date.now());
    await sleep(500);
    await tester.evaluate(() => (window as CallWin).__second?.stop());
    await negotiate(tester, bot);
    await sleep(2000);
  } finally {
    clearInterval(puller);
    await pull(true);
  }
  const status = await bot.evaluate(() => window.__gizziPeerCap?.status() ?? null);
  const stoppedAt = Date.now();
  await testerCtx.close();
  await botCtx.close();

  const added = events.filter((e) => e.event === "peer_track_added");
  const ended = events.filter((e) => e.event === "peer_track_ended");
  const started = events.find((e) => e.event === "peer_recorder_started");
  const addedIds = added.map((e) => String(e.detail?.id));
  const expected = started ? (stoppedAt - started.t) / 1000 : 0;

  const webm = join(out, "call.webm");
  const wav = join(out, "audio", "seg-000.wav");
  let seconds = 0;
  let meanDb = -Infinity;
  let maxDb = -Infinity;
  if (existsSync(webm)) {
    const vd = ffmpeg(["-i", webm, "-af", "volumedetect", "-f", "null", "-"]);
    meanDb = Number(/mean_volume: (-?[\d.]+) dB/.exec(vd.err)?.[1] ?? "-Infinity");
    maxDb = Number(/max_volume: (-?[\d.]+) dB/.exec(vd.err)?.[1] ?? "-Infinity");
    ffmpeg(["-y", "-i", webm, "-ac", "1", "-ar", "16000", wav]);
    seconds = wavSeconds(wav);
    // The transcribe.sh / merge-transcript.ts inputs.
    writeFileSync(join(out, "audio", "segments.csv"), `seg-000.wav,0,${seconds.toFixed(3)}\n`);
    writeFileSync(join(out, "audio", "capture-start-ms"), `${started?.t ?? 0}\n`);
  }
  const summary = { bytes, status, tracksAdded: addedIds, tracksEnded: ended.map((e) => e.detail), botSpoke, d1, d2, expected, seconds, meanDb, maxDb };
  writeFileSync(join(out, "capture-test.json"), JSON.stringify({ botSide, summary }, null, 2));
  console.log("  " + JSON.stringify(summary));

  return report([
    ["bot page has the media override, and its RTCPeerConnection went through the capture wrapper", botSide.capture && botSide.media && events.some((e) => e.event === "peer_connection_created")],
    ["tester and bot connected over WebRTC", connected],
    ["two remote audio tracks added (the second after a renegotiation)", added.length === 2],
    ["the stopped second track logged as ended", added.length === 2 && ended.some((e) => e.detail?.id === addedIds[1])],
    ["the bot's own mic track was never recorded", !addedIds.includes(botSide.localTrack) && botSpoke],
    [`recording length ${seconds.toFixed(1)} s matches the call (${expected.toFixed(1)} s, within 2.5 s)`, Math.abs(seconds - expected) < 2.5],
    [`recording covers both talkers (${seconds.toFixed(1)} s >= ${(d1 + d2).toFixed(1)} s)`, seconds >= d1 + d2],
    [`recording is not silent (mean ${meanDb} dB > -50 dB)`, meanDb > -50],
  ]);
}

let failed = false;
try {
  if (!argv.includes("--capture-only")) {
    console.log("== media override check");
    if (!(await mediaCheck(browser))) failed = true;
  }
  if (!argv.includes("--media-only")) {
    console.log("== call capture check (peer-capture.ts)");
    if (!(await captureCheck(browser))) failed = true;
  }
} finally {
  await browser.close();
  server.close();
}
console.log(failed ? "local-test: FAILED" : "local-test: PASSED");
process.exit(failed ? 1 : 0);
