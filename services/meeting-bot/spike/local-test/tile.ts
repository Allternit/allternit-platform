// Local proof that the bot's camera is our real bot (allternit-ai /meeting-tile),
// runnable on a Mac. Needs the tile page served (allternit-ai dev server):
//   npx tsc -p tsconfig.json && node dist/local-test/tile.js \
//     [--tile-url http://127.0.0.1:3013/meeting-tile] [--headed] [--cpu 60]
// Starts Chrome the way join.ts does (a normal browser driven over its
// DevTools port), installs the same media override, starts the same tile feed,
// and opens a stand-in meeting page that calls getUserMedia like Meet. Then,
// for each tile state, it grabs 3 frames from the page's camera track and
// checks: 1280x720, frames change, and each frame matches a screenshot of the
// tile window taken at the same moment (so it is the tile, not a text card).
// Writes out/tile/camera-frames.png. --cpu N samples node + Chrome CPU time
// for N seconds with the screencast running.
import { chromium, type Page } from "playwright-core";
import { spawn, execFileSync } from "node:child_process";
import { createServer } from "node:http";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { AddressInfo } from "node:net";
import { installGizziMedia } from "../gizzi-media.js";
import { startTileFeed, type TileState } from "../tile-feed.js";

const argv = process.argv.slice(2);
const get = (f: string) => { const i = argv.indexOf(f); return i >= 0 ? argv[i + 1] : undefined; };
const TILE_URL = get("--tile-url") ?? process.env.TILE_URL ?? "http://127.0.0.1:3013/meeting-tile";
const CPU_SECONDS = Number(get("--cpu") ?? "0");
const here = dirname(fileURLToPath(import.meta.url));
const OUT = join(here, "..", "..", "out", "tile");
mkdirSync(OUT, { recursive: true });
const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

// The stand-in meeting page: asks for the camera like Meet and can hand back frames.
const MEETING_HTML = `<!doctype html><meta charset="utf-8"><title>Stand-in meeting</title>
<body style="margin:0;background:#111"><video id="v" autoplay muted playsinline style="width:640px"></video>
<script>
window.__cam = (async () => {
  const s = await navigator.mediaDevices.getUserMedia({ video: true, audio: true });
  const v = document.getElementById("v");
  v.srcObject = s; await v.play();
  return { label: s.getVideoTracks()[0].label, deviceId: s.getVideoTracks()[0].getSettings().deviceId };
})();
window.__grab = () => {
  const v = document.getElementById("v");
  const c = document.createElement("canvas"); c.width = v.videoWidth; c.height = v.videoHeight;
  c.getContext("2d").drawImage(v, 0, 0);
  return { w: v.videoWidth, h: v.videoHeight, png: c.toDataURL("image/png").split(",")[1] };
};
</script>`;

const server = createServer((_req, res) => res.writeHead(200, { "content-type": "text/html; charset=utf-8" }).end(MEETING_HTML));
await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
const meetingUrl = `http://127.0.0.1:${(server.address() as AddressInfo).port}/`;

const chromePath = process.env.CHROME_PATH ?? (process.platform === "darwin" ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" : "/usr/bin/google-chrome");
const profile = mkdtempSync(join(tmpdir(), "gizzi-tile-test-"));
const chrome = spawn(chromePath, [
  "--autoplay-policy=no-user-gesture-required",
  "--use-fake-ui-for-media-stream",
  "--disable-background-timer-throttling",
  "--disable-renderer-backgrounding",
  "--disable-backgrounding-occluded-windows",
  "--force-color-profile=srgb",
  "--window-size=1280,800",
  `--user-data-dir=${profile}`,
  "--remote-debugging-port=0",
  "--no-first-run",
  "--no-default-browser-check",
  ...(argv.includes("--headed") ? [] : ["--headless=new"]),
  "about:blank",
], { stdio: "ignore" });
const portFile = join(profile, "DevToolsActivePort");
for (let i = 0; i < 100 && !existsSync(portFile); i++) await sleep(100);
const browser = await chromium.connectOverCDP(`http://127.0.0.1:${readFileSync(portFile, "utf8").split("\n")[0].trim()}`);

// Mean absolute RGB difference between two PNGs (base64), computed in a page.
async function compare(page: Page, a: string, b: string): Promise<number> {
  return page.evaluate(async ([x, y]) => {
    const load = (s: string) => new Promise<HTMLImageElement>((r) => { const im = new Image(); im.onload = () => r(im); im.src = "data:image/png;base64," + s; });
    const [ia, ib] = await Promise.all([load(x), load(y)]);
    const px = (im: HTMLImageElement) => { const c = document.createElement("canvas"); c.width = 320; c.height = 180; const g = c.getContext("2d")!; g.drawImage(im, 0, 0, 320, 180); return g.getImageData(0, 0, 320, 180).data; };
    const da = px(ia), db = px(ib);
    let sum = 0;
    for (let i = 0; i < da.length; i += 4) sum += Math.abs(da[i] - db[i]) + Math.abs(da[i + 1] - db[i + 1]) + Math.abs(da[i + 2] - db[i + 2]);
    return +(sum / (da.length / 4) / 3).toFixed(2);
  }, [a, b] as const);
}

interface CpuSample { node: number; chrome: number; procs: number }
function cpuTimes(rootPid: number): CpuSample {
  const rows = execFileSync("ps", ["-A", "-o", "pid=,ppid=,time="], { encoding: "utf8" }).trim().split("\n").map((l) => l.trim().split(/\s+/));
  const secs = (t: string) => t.split(/[:]/).reduce((acc, part) => acc * 60 + Number(part.replace("-", "")), 0);
  const kids = new Set([rootPid]);
  for (let changed = true; changed;) {
    changed = false;
    for (const [pid, ppid] of rows) if (kids.has(Number(ppid)) && !kids.has(Number(pid))) { kids.add(Number(pid)); changed = true; }
  }
  let chromeT = 0, nodeT = 0;
  for (const [pid, , time] of rows) {
    if (kids.has(Number(pid))) chromeT += secs(time);
    if (Number(pid) === process.pid) nodeT += secs(time);
  }
  return { node: nodeT, chrome: chromeT, procs: kids.size };
}

let failed = false;
const report: Record<string, unknown> = {};
try {
  const context = browser.contexts()[0];
  await context.addInitScript(installGizziMedia, { name: "Gizzi (Allternit)", state: "Joining" });
  const page = context.pages()[0] ?? (await context.newPage());
  const tile = await startTileFeed(browser, page, { url: TILE_URL, name: "Gizzi (Allternit)", state: "joining", onEvent: (k, d) => console.log(`[tile] ${k} ${d ? JSON.stringify(d) : ""}`) });
  await page.goto(meetingUrl);
  report.camera = await page.evaluate(() => (window as unknown as { __cam: Promise<unknown> }).__cam);
  await sleep(1500);

  const states: TileState[] = ["joining", "listening", "thinking", "speaking", "paused"];
  const rows: Array<{ state: TileState; frames: string[] }> = [];
  const checks: Array<[string, boolean]> = [];
  for (const st of states) {
    await tile.setState(st);
    let t = 0;
    const pump = st === "speaking" ? setInterval(() => { t += 0.08; tile.setLevel(Math.abs(Math.sin(t * 6)) * 0.8); }, 80) : null;
    await sleep(1200);
    const frames: string[] = [];
    const vsTile: number[] = [];
    for (let i = 0; i < 3; i++) {
      if (i) await sleep(450);
      const [cam, shot] = await Promise.all([
        page.evaluate(() => (window as unknown as { __grab: () => { w: number; h: number; png: string } }).__grab()),
        tile.tilePage.screenshot({ type: "png" }),
      ]);
      if (i === 0) checks.push([`${st}: camera track is 1280x720`, cam.w === 1280 && cam.h === 720]);
      frames.push(cam.png);
      vsTile.push(await compare(page, cam.png, shot.toString("base64")));
    }
    if (pump) clearInterval(pump);
    const change = [await compare(page, frames[0], frames[1]), await compare(page, frames[1], frames[2])];
    report[st] = { cameraVsTileScreenshot: vsTile, frameToFrame: change };
    checks.push([`${st}: camera frames match the tile window (mean diff < 8 of 255)`, vsTile.every((d) => d < 8)]);
    checks.push([`${st}: camera frames change over time`, change.some((d) => d > 0.15)]);
    rows.push({ state: st, frames });
  }
  // Negative control: the camera's empty frame (no tile) against the tile window.
  const blank = await page.evaluate(() => { const c = document.createElement("canvas"); c.width = 1280; c.height = 720; const g = c.getContext("2d")!; g.fillStyle = "#1f1f1e"; g.fillRect(0, 0, 1280, 720); return c.toDataURL("image/png").split(",")[1]; });
  const blankVsTile = await compare(page, blank, (await tile.tilePage.screenshot({ type: "png" })).toString("base64"));
  report.blankVsTile = blankVsTile;
  checks.push(["control: an empty camera frame does NOT match the tile window (diff >= 8)", blankVsTile >= 8]);
  report.feed = tile.stats();
  report.media = await page.evaluate(() => window.__gizziMedia);

  // camera-frames.png: one row per state, 3 frames each.
  const sheet = await context.newPage();
  await sheet.setViewportSize({ width: 3 * 426 + 140, height: rows.length * 240 });
  await sheet.setContent(`<body style="margin:0;background:#111;color:#ddd;font:14px system-ui">${rows
    .map((r) => `<div style="display:flex;align-items:center;height:240px"><div style="width:140px;padding-left:10px">${r.state}</div>${r.frames.map((f) => `<img src="data:image/png;base64,${f}" style="width:426px;height:240px">`).join("")}</div>`)
    .join("")}</body>`);
  await sheet.screenshot({ path: join(OUT, "camera-frames.png") });
  await sheet.close();

  if (CPU_SECONDS > 0) {
    await tile.setState("listening");
    const a = cpuTimes(chrome.pid!);
    const t0 = Date.now();
    await sleep(CPU_SECONDS * 1000);
    const b = cpuTimes(chrome.pid!);
    const wall = (Date.now() - t0) / 1000;
    report.cpu = {
      seconds: wall,
      nodePct: +(((b.node - a.node) / wall) * 100).toFixed(1),
      chromePct: +(((b.chrome - a.chrome) / wall) * 100).toFixed(1),
      chromeProcesses: b.procs,
      feed: tile.stats(),
      note: "% of one core, from ps cumulative CPU time; Chrome = the spawned Chrome and all its children",
    };
  }

  await tile.stop();
  console.log(JSON.stringify(report, null, 1));
  for (const [name, ok] of checks) {
    console.log(`${ok ? "PASS" : "FAIL"}  ${name}`);
    if (!ok) failed = true;
  }
} catch (e) {
  console.log("ERROR", e);
  failed = true;
} finally {
  await browser.close().catch(() => undefined);
  chrome.kill("SIGTERM");
  server.close();
  await sleep(500);
  rmSync(profile, { recursive: true, force: true });
}
writeFileSync(join(OUT, "tile-test.json"), JSON.stringify(report, null, 1));
console.log(failed ? "tile-test: FAILED" : "tile-test: PASSED");
process.exit(failed ? 1 : 0);
