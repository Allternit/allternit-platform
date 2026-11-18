#!/usr/bin/env node
// Idle cost of every Desktop window, read over the Chrome DevTools Protocol.
// Read-only: it never clicks, navigates or focuses a window.
//
//   node scripts/cdp-idle.mjs [--port 9333] [--seconds 10] [--json]
//   node scripts/cdp-idle.mjs --profile /hud        top self-time functions
//
// Start Desktop with --remote-debugging-port=<port> first. Per window it
// prints: main-thread busy % (TaskDuration delta), script %, layouts and style
// recalcs, running animations, and requests grouped by host + path.
// Node 22+ (global fetch + WebSocket), no dependencies.

const args = process.argv.slice(2);
const opt = (name, def) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1] : def;
};
if (args.includes('--help') || args.includes('-h')) {
  console.log('usage: cdp-idle.mjs [--port 9333] [--seconds 10] [--json] [--profile <path>]');
  process.exit(0);
}
const port = Number(opt('--port', '9333'));
const seconds = Number(opt('--seconds', '10'));
const profilePath = opt('--profile', null);
const asJson = args.includes('--json');

function connect(wsUrl) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(wsUrl);
    let id = 0;
    const pending = new Map();
    const handlers = [];
    ws.onmessage = (e) => {
      const m = JSON.parse(e.data);
      if (m.id && pending.has(m.id)) {
        pending.get(m.id)(m.result ?? { error: m.error });
        pending.delete(m.id);
      } else if (m.method) handlers.forEach((h) => h(m));
    };
    ws.onerror = reject;
    ws.onopen = () =>
      resolve({
        on: (h) => handlers.push(h),
        send: (method, params = {}) =>
          new Promise((res) => {
            const i = ++id;
            pending.set(i, res);
            ws.send(JSON.stringify({ id: i, method, params }));
          }),
        close: () => ws.close(),
      });
  });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const metrics = (r) => Object.fromEntries((r.metrics ?? []).map((x) => [x.name, x.value]));

let targets;
try {
  targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
} catch {
  console.error(`No DevTools endpoint on port ${port}. Start Desktop with --remote-debugging-port=${port}.`);
  process.exit(1);
}
const pages = targets.filter((t) => t.type === 'page' && /^https?:/.test(t.url));
const shortUrl = (u) => {
  const p = new URL(u);
  return p.pathname + p.search;
};

if (profilePath) {
  const t = pages.find((p) => shortUrl(p.url) === profilePath) ?? pages.find((p) => shortUrl(p.url).startsWith(profilePath));
  if (!t) {
    console.error(`No page ${profilePath}. Pages: ${pages.map((p) => shortUrl(p.url)).join(', ')}`);
    process.exit(1);
  }
  const c = await connect(t.webSocketDebuggerUrl);
  await c.send('Profiler.enable');
  await c.send('Profiler.setSamplingInterval', { interval: 500 });
  await c.send('Profiler.start');
  await sleep(Math.min(seconds, 10) * 1000);
  const { profile } = await c.send('Profiler.stop');
  c.close();
  const byId = new Map(profile.nodes.map((n) => [n.id, n]));
  const self = new Map();
  for (const s of profile.samples) {
    const f = byId.get(s).callFrame;
    const k = `${f.functionName || '(anonymous)'} ${f.url.split('/').pop()}:${f.lineNumber + 1}:${f.columnNumber + 1}`;
    self.set(k, (self.get(k) ?? 0) + 1);
  }
  const total = profile.samples.length || 1;
  console.log(`${shortUrl(t.url)}: ${total} samples`);
  [...self].sort((a, b) => b[1] - a[1]).slice(0, 10).forEach(([k, v]) => console.log(`${((100 * v) / total).toFixed(1).padStart(5)}%  ${k}`));
  process.exit(0);
}

const runs = await Promise.all(
  pages.map(async (p) => {
    const c = await connect(p.webSocketDebuggerUrl);
    const requests = new Map();
    c.on((m) => {
      if (m.method !== 'Network.requestWillBeSent' || m.params.redirectResponse) return;
      try {
        const u = new URL(m.params.request.url);
        const key = `${m.params.request.method} ${u.host}${u.pathname.replace(/[0-9a-f]{8}-[0-9a-f-]{27,}|\b[a-z]{2,5}_[A-Za-z0-9]{12,}/g, ':id')}`;
        requests.set(key, (requests.get(key) ?? 0) + 1);
      } catch {}
    });
    await c.send('Performance.enable');
    await c.send('Network.enable');
    return { p, c, requests, before: metrics(await c.send('Performance.getMetrics')) };
  }),
);
await sleep(seconds * 1000);

const rows = [];
for (const { p, c, requests, before } of runs) {
  const after = metrics(await c.send('Performance.getMetrics'));
  const d = (k) => (after[k] ?? 0) - (before[k] ?? 0);
  const ev = await c.send('Runtime.evaluate', {
    expression: 'JSON.stringify({hidden: document.visibilityState, animations: document.getAnimations().filter(a => a.playState === "running").length})',
    returnByValue: true,
  });
  const page = JSON.parse(ev.result?.value ?? '{}');
  c.close();
  rows.push({
    window: shortUrl(p.url),
    busyPct: +((100 * d('TaskDuration')) / seconds).toFixed(1),
    scriptPct: +((100 * d('ScriptDuration')) / seconds).toFixed(1),
    layouts: d('LayoutCount'),
    styleRecalcs: d('RecalcStyleCount'),
    animations: page.animations,
    visibility: page.hidden,
    requests: [...requests.values()].reduce((a, b) => a + b, 0),
    topRequests: [...requests].sort((a, b) => b[1] - a[1]).slice(0, 5),
  });
}

if (asJson) {
  console.log(JSON.stringify({ at: new Date().toISOString(), port, seconds, windows: rows }, null, 2));
} else {
  console.log(`Idle cost over ${seconds} s (port ${port})`);
  for (const r of rows) {
    console.log(`\n${r.window}  [${r.visibility}]`);
    console.log(`  busy ${r.busyPct}%  script ${r.scriptPct}%  layouts ${r.layouts}  style ${r.styleRecalcs}  animations ${r.animations}  requests ${r.requests}`);
    for (const [k, v] of r.topRequests) console.log(`    ${String(v).padStart(5)}  ${k}`);
  }
}
