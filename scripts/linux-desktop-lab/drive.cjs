// MANUAL lab step (not CI): drive Allternit Desktop over CDP, click Get started,
// sign in with a one-time Clerk ticket read from a FILE (never printed), watch pairing.
// Usage: node drive.cjs <playwright-core path> <port> <outdir> [ticketFile]
const fs = require('fs');
const { chromium } = require(process.argv[2]);
const [port, out, ticketFile] = process.argv.slice(3);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
fs.mkdirSync(out, { recursive: true });

async function shotAll(browser, tag) {
  const pages = browser.contexts().flatMap((c) => c.pages());
  let i = 0;
  for (const p of pages) {
    let title = ''; try { title = await p.title(); } catch {}
    console.log(`[${tag}] page ${i}: ${title} | ${p.url().slice(0, 110)}`);
    try { await p.screenshot({ path: `${out}/${tag}-${i}.png`, timeout: 8000 }); } catch (e) { console.log('  shot failed', e.message.split('\n')[0]); }
    i++;
  }
  return pages;
}

(async () => {
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  let pages = await shotAll(browser, '1-start');
  const startup = pages.find((p) => p.url().startsWith('data:text/html'));
  if (startup) {
    const btn = startup.locator('#btn-get-started');
    if (await btn.count()) { console.log('click Get started'); await btn.click(); await sleep(8000); }
  }
  let auth;
  for (let w = 0; w < 8 && !auth; w++) {
    pages = browser.contexts().flatMap((c) => c.pages());
    auth = pages.find((p) => p.url().includes('__desktop_auth__'));
    if (!auth) await sleep(5000);
  }
  pages = await shotAll(browser, '2-after-start');
  if (auth && ticketFile && fs.existsSync(ticketFile)) {
    const ticket = fs.readFileSync(ticketFile, 'utf8').trim();
    console.log('signing in with one-time ticket through Clerk JS in the page');
    await auth.waitForFunction(() => window.Clerk && window.Clerk.loaded, null, { timeout: 30000 }).catch(() => console.log('Clerk not loaded'));
    const r = await auth.evaluate(async (t) => {
      try {
        const si = await window.Clerk.client.signIn.create({ strategy: 'ticket', ticket: t });
        await window.Clerk.setActive({ session: si.createdSessionId });
        return 'status=' + si.status;
      } catch (e) { return 'error=' + (e && (e.errors?.[0]?.longMessage || e.message)); }
    }, ticket);
    console.log('sign-in result:', r);
    for (let t = 0; t < 12; t++) {
      await sleep(5000);
      pages = await shotAll(browser, `3-pair-${t}`);
      if (!pages.some((p) => p.url().includes('__desktop_auth__')) && t > 2) break;
    }
  } else {
    console.log('auth window not found or no ticket; auth found =', Boolean(auth));
  }
  await sleep(15000);
  await shotAll(browser, '4-end');
  await browser.close();
})().catch((e) => { console.error('driver error', e.message); process.exit(1); });
