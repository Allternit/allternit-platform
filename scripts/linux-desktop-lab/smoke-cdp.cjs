// CI step 4 part 2: connect to the running app over CDP and prove the startup
// window rendered. Usage: node smoke-cdp.cjs <playwright-core path> <port> <outdir> [main.log path]
// Passes only when a page has #btn-get-started and main.log has no init failure.
// No sign-in here: that needs a Clerk ticket and stays a manual step (see README).
const fs = require('fs');
const { chromium } = require(process.argv[2]);
const [port, out, mainLog] = process.argv.slice(3);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const BAD = [/Initialization Error/i, /Failed to initialize bundled mode/i, /Failed to start Allternit Backend/i];

async function pagesOf(browser) { return browser.contexts().flatMap((c) => c.pages()); }

function checkLog() {
  const log = mainLog && fs.existsSync(mainLog) ? fs.readFileSync(mainLog, 'utf8') : '';
  return BAD.filter((re) => re.test(log));
}

(async () => {
  let browser;
  const deadline = Date.now() + 60000; // the app quits by itself when the engine fails
  while (Date.now() < deadline && !browser) {
    try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 3000 }); } catch { await sleep(2000); }
  }
  if (!browser) { console.log('FAIL: CDP never came up on', port, '(app did not start or quit early)'); process.exit(1); }
  fs.mkdirSync(out, { recursive: true });
  let found = false;
  for (let i = 0; i < 40 && !found; i++) {
    for (const p of await pagesOf(browser)) {
      if (await p.locator('#btn-get-started').count().catch(() => 0)) found = true;
    }
    if (!found) await sleep(2000);
  }
  let n = 0;
  for (const p of await pagesOf(browser)) {
    let title = ''; try { title = await p.title(); } catch {}
    console.log(`window ${n}: "${title}" ${p.url().slice(0, 80)}`);
    try { await p.screenshot({ path: `${out}/window-${n}.png`, timeout: 8000 }); } catch (e) { console.log('  screenshot failed:', e.message.split('\n')[0]); }
    n++;
  }
  await browser.close().catch(() => {});
  const bad = checkLog();
  if (bad.length) { console.log('FAIL: main.log has an init failure:', bad.map(String).join(', ')); process.exit(1); }
  if (!found) { console.log('FAIL: no window with #btn-get-started rendered'); process.exit(1); }
  console.log('PASS: startup window rendered, main.log clean');
})().catch((e) => { console.error('FAIL: driver error', e.message); process.exit(1); });
