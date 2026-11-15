/* SPIKE S1 (Android bridge) page-side client. Debug/test only.
 *
 * Loaded by s1.html (mode A, served offline by the app on http://localhost:8765),
 * or pasted into the DevTools console of the real https://m.allternit.com page in
 * the TWA (mode B; then send the adb REQUEST_CHANNEL broadcast).
 *
 * Candidate 1: Chrome posts an empty message with ports[0] (a MessagePort) to the
 * page when the app's requestPostMessageChannel() succeeds. Everything after that
 * goes over the port. Candidate 2: fetch() to the app's loopback server.
 */
(function () {
  if (window.S1 && window.S1.installed) return;
  // Chrome sends the port from 'android-app://<verified host>/<package>'; a MessageEvent
  // origin has no path, so the page sees 'android-app://<its own hostname>'. Only apps
  // listed with use_as_origin in this host's assetlinks.json can open that channel.
  const APP_ORIGIN = 'android-app://' + location.hostname;
  const C2_BASE = location.origin === 'http://localhost:8765' ? '' : 'http://127.0.0.1:8765';
  const log = (line) => {
    console.log('[S1] ' + line);
    const out = document.getElementById('out'); // looked up per line: the client loads in <head>
    if (out) { out.textContent += line + '\n'; }
  };
  // Run counter per app launch (the activity adds ?launch=<n>), so a reused tab starts over.
  const launchKey = 's1:' + (new URLSearchParams(location.search).get('launch') || 'manual') + ':';
  const run = Number(sessionStorage.getItem(launchKey + 'run') || '0') + 1;
  sessionStorage.setItem(launchKey + 'run', String(run));

  let port = null;
  let ports = 0;
  let suiteStarted = false;
  const waiters = new Map(); // key -> {resolve, ...}
  let streamSeq = 0;
  const streams = new Map();

  function send(obj) { port.postMessage(JSON.stringify(obj)); }
  let sendHello = () => {};

  function onPortMessage(ev) {
    let m;
    try { m = JSON.parse(ev.data); } catch (e) { log('bad message ' + String(ev.data).slice(0, 60)); return; }
    const now = performance.now();
    if (m.t === 'tok') {
      const st = streams.get(m.s);
      if (!st) return;
      if (st.count === 0) st.first = now;
      if (m.i !== st.next) st.outOfOrder++;
      st.next = m.i + 1;
      st.count++;
      st.last = now;
    } else if (m.t === 'end') {
      const st = streams.get(m.s);
      if (st) { st.end = m; st.resolve(); }
    } else if (m.t === 'pong') {
      const w = waiters.get('pong' + m.id); if (w) { waiters.delete('pong' + m.id); w(now); }
    } else if (m.t === 'welcome') {
      const w = waiters.get('welcome'); if (w) { waiters.delete('welcome'); w(m); }
    } else if (m.t === 'ready') {
      // The app's side of the channel is connected: page -> app messages arrive from now on.
      if (waiters.has('welcome')) sendHello();
    }
  }

  window.addEventListener('message', (ev) => {
    if (!ev.ports || !ev.ports[0]) return;
    if (ev.origin !== APP_ORIGIN) {
      log('ignored port from origin ' + ev.origin);
      return;
    }
    ports++;
    port = ev.ports[0];
    port.onmessage = onPortMessage;
    log('PORT #' + ports + ' received, origin=' + ev.origin + ', run=' + run + ', online=' + navigator.onLine);
    if (!suiteStarted && !window.S1_NO_AUTORUN) { suiteStarted = true; suite(); }
  });

  function wait(key, ms) {
    return new Promise((resolve, reject) => {
      const t = setTimeout(() => { waiters.delete(key); reject(new Error('timeout ' + key)); }, ms);
      waiters.set(key, (v) => { clearTimeout(t); resolve(v); });
    });
  }

  const pct = (arr, p) => { const a = [...arr].sort((x, y) => x - y); return a[Math.min(a.length - 1, Math.floor(p * a.length))]; };
  const r1 = (x) => Math.round(x * 10) / 10;

  async function c1Ping(n) {
    const rtts = [];
    for (let i = 0; i < n; i++) {
      const t0 = performance.now();
      const p = wait('pong' + i, 5000);
      send({ t: 'ping', id: i });
      rtts.push((await p) - t0);
    }
    return { n, p50: r1(pct(rtts, 0.5)), p95: r1(pct(rtts, 0.95)), max: r1(Math.max(...rtts)) };
  }

  function c1Stream(n, bytes) {
    const s = ++streamSeq;
    return new Promise((resolve) => {
      const st = { count: 0, next: 0, outOfOrder: 0, first: 0, last: 0, end: null };
      const t = setTimeout(() => st.resolve(), 60000);
      st.resolve = () => {
        clearTimeout(t);
        streams.delete(s);
        const total = st.last - t0;
        resolve({
          n, bytes, received: st.count, outOfOrder: st.outOfOrder,
          nativeFails: st.end ? st.end.fail : 'no end', nativePostMs: st.end ? r1(st.end.postMs) : null,
          firstMs: r1(st.first - t0), totalMs: r1(total),
          msgsPerSec: st.count > 1 ? Math.round(st.count / (total / 1000)) : 0,
          pass: st.count === n && st.outOfOrder === 0,
        });
      };
      streams.set(s, st);
      const t0 = performance.now();
      send({ t: 'stream', s, n, bytes });
    });
  }

  async function c2Stream(n, bytes) {
    const t0 = performance.now();
    const res = await fetch(C2_BASE + '/c2/stream?n=' + n + '&bytes=' + bytes,
      C2_BASE ? { targetAddressSpace: 'loopback' } : {});
    const reader = res.body.getReader();
    const dec = new TextDecoder();
    let buf = '', count = 0, next = 0, outOfOrder = 0, first = 0, last = 0;
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      let idx;
      while ((idx = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, idx); buf = buf.slice(idx + 1);
        const m = JSON.parse(line);
        const now = performance.now();
        if (count === 0) first = now;
        if (m.i !== next) outOfOrder++;
        next = m.i + 1; count++; last = now;
      }
    }
    const total = last - t0;
    return { n, bytes, received: count, outOfOrder, firstMs: r1(first - t0), totalMs: r1(total),
      msgsPerSec: count > 1 ? Math.round(count / (total / 1000)) : 0, pass: count === n && outOfOrder === 0 };
  }

  async function c2Ping(n) {
    const rtts = [];
    for (let i = 0; i < n; i++) {
      const t0 = performance.now();
      await (await fetch(C2_BASE + '/c2/ping', C2_BASE ? { targetAddressSpace: 'loopback' } : {})).text();
      rtts.push(performance.now() - t0);
    }
    return { n, p50: r1(pct(rtts, 0.5)), p95: r1(pct(rtts, 0.95)), max: r1(Math.max(...rtts)) };
  }

  async function suite() {
    const result = { t: 'result', run, href: location.href, online: navigator.onLine, ua: navigator.userAgent, ports };
    try {
      // Handshake: hello until welcome. A hello sent before the app's PostMessageService
      // binding is up is dropped by Chrome, so retry, and resend at once on 'ready'.
      const welcomeP = wait('welcome', 5000);
      let hellos = 0;
      sendHello = () => { hellos++; send({ t: 'hello', run, href: location.href, online: navigator.onLine }); };
      sendHello();
      const retry = setInterval(() => { if (waiters.has('welcome')) sendHello(); else clearInterval(retry); }, 250);
      try { result.welcome = await welcomeP; } finally { clearInterval(retry); }
      result.hellos = hellos;
      log('welcome ' + JSON.stringify(result.welcome) + ' after ' + result.hellos + ' hello(s)');
      result.c1_ping = await c1Ping(100); log('C1 ping ' + JSON.stringify(result.c1_ping));
      result.c1_500x16 = await c1Stream(500, 16); log('C1 500x16B ' + JSON.stringify(result.c1_500x16));
      result.c1_5000x16 = await c1Stream(5000, 16); log('C1 5000x16B ' + JSON.stringify(result.c1_5000x16));
      result.c1_500x1k = await c1Stream(500, 1024); log('C1 500x1KB ' + JSON.stringify(result.c1_500x1k));
      if (!C2_BASE) {
        // Same-origin here (page is on localhost), so this measures the transport only,
        // not the Local Network Access prompt. See README for the cross-origin probe.
        result.c2_ping = await c2Ping(50); log('C2 ping ' + JSON.stringify(result.c2_ping));
        result.c2_500x16 = await c2Stream(500, 16); log('C2 500x16B ' + JSON.stringify(result.c2_500x16));
        result.c2_5000x16 = await c2Stream(5000, 16); log('C2 5000x16B ' + JSON.stringify(result.c2_5000x16));
      }
      result.pass = result.c1_500x16.pass;
    } catch (e) {
      result.error = String(e);
      result.pass = false;
      log('ERROR ' + e);
    }
    try { send(result); } catch (e) { log('could not send result: ' + e); }
    log('RUN ' + run + ' ' + (result.pass ? 'PASS' : 'FAIL') + ' (500 streamed messages over the TWA port)');
    sessionStorage.setItem(launchKey + 'result' + run, JSON.stringify({ pass: result.pass, rate: result.c1_500x16 && result.c1_500x16.msgsPerSec }));
    if (run === 1 && !window.S1_NO_AUTORELOAD) {
      log('reloading in 1.5 s to test the channel after a reload ...');
      setTimeout(() => location.reload(), 1500);
    } else if (run >= 2) {
      const prev = JSON.parse(sessionStorage.getItem(launchKey + 'result' + (run - 1)) || '{}');
      const verdict = prev.pass && result.pass ? 'PASS' : 'FAIL';
      log('RELOAD TEST ' + verdict + ': before reload ' + JSON.stringify(prev) + ', after reload pass=' + result.pass);
      try { send({ t: 'result', reloadTest: verdict, before: prev, afterPass: result.pass, run }); } catch (e) {}
    }
  }

  /** Mode B helper: cross-origin loopback probe from the real https page (LNA). */
  async function c2probe() {
    try {
      const t0 = performance.now();
      const r = await fetch('http://127.0.0.1:8765/c2/ping', { targetAddressSpace: 'loopback' });
      log('C2 probe ' + r.status + ' ' + (await r.text()) + ' in ' + r1(performance.now() - t0) + ' ms');
    } catch (e) { log('C2 probe FAILED: ' + e); }
    try {
      const q = await navigator.permissions.query({ name: 'loopback-network' });
      log('permission loopback-network = ' + q.state);
    } catch (e) { log('permissions.query(loopback-network) not supported: ' + e); }
  }

  window.S1 = { installed: true, suite, c2probe, state: () => ({ ports, run, hasPort: !!port }), reset() { sessionStorage.removeItem(launchKey + 'run'); } };
  log('S1 client ready (run ' + run + '), waiting for the app to open the channel. ' + new Date().toISOString());
})();
