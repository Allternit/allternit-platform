#!/usr/bin/env node
// Tell IndexNow (Bing, Copilot, Yandex, Seznam, Naver...) which pages a site has,
// right after the site deploys. ChatGPT search and Copilot read Bing's index,
// so this is how a new or changed page gets there without waiting for a crawl.
//
//   node scripts/indexnow.mjs allternit.com labs.allternit.com ...
//
// For each host it reads https://<host>/sitemap.xml (and one level of sitemap
// index), checks that https://<host>/<key>.txt is live with the key in it, then
// posts the URLs to api.indexnow.org. The key file sits in each site's public
// folder; the key is public by design (IndexNow proves ownership with the file).
//
// Copy of allternit-websites scripts/indexnow.mjs (same key, scripts/indexnow.key).
// It never fails the deploy: every problem is printed and the exit code is 0.
// Set INDEXNOW_DRY_RUN=1 to print what would be sent without sending it.

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const KEY = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'indexnow.key'), 'utf8').trim();
const ENDPOINT = 'https://api.indexnow.org/indexnow';
const DRY = process.env.INDEXNOW_DRY_RUN === '1';
const UA = 'allternit-indexnow/1.0';

const sleep = ms => new Promise(r => setTimeout(r, ms));

async function get(url) {
  const res = await fetch(url, { headers: { 'user-agent': UA }, redirect: 'follow' });
  return { status: res.status, type: res.headers.get('content-type') || '', text: await res.text() };
}

const locs = xml => [...xml.matchAll(/<loc>\s*([^<\s]+)\s*<\/loc>/g)].map(m => m[1].replace(/&amp;/g, '&'));

async function sitemapUrls(host) {
  const top = await get(`https://${host}/sitemap.xml`);
  if (top.status !== 200 || !/xml/.test(top.type)) {
    throw new Error(`sitemap.xml gave ${top.status} ${top.type || 'no type'}`);
  }
  if (!/<sitemapindex/.test(top.text)) return locs(top.text);
  const out = [];
  for (const child of locs(top.text)) {
    const sm = await get(child);
    if (sm.status === 200) out.push(...locs(sm.text));
    else console.warn(`  ${host}: child sitemap ${child} gave ${sm.status}`);
  }
  return out;
}

// The key file goes live with the deploy; give Pages a moment if it is not there yet.
async function keyLive(host) {
  for (let i = 0; i < 4; i++) {
    try {
      const r = await get(`https://${host}/${KEY}.txt`);
      if (r.status === 200 && r.text.trim() === KEY) return true;
    } catch {}
    await sleep(10_000);
  }
  return false;
}

async function submit(host) {
  const all = await sitemapUrls(host);
  const urlList = [...new Set(all.filter(u => {
    try { return new URL(u).host === host; } catch { return false; }
  }))];
  const dropped = all.length - urlList.length;
  if (!urlList.length) return `no URLs for ${host} in its sitemap (${all.length} found, all other hosts)`;
  if (!(await keyLive(host))) return `key file https://${host}/${KEY}.txt is not live; skipped`;

  const body = { host, key: KEY, keyLocation: `https://${host}/${KEY}.txt`, urlList };
  if (DRY) return `dry run: would send ${urlList.length} URLs${dropped ? ` (${dropped} other-host URLs left out)` : ''}`;

  const res = await fetch(ENDPOINT, {
    method: 'POST',
    headers: { 'content-type': 'application/json; charset=utf-8', 'user-agent': UA },
    body: JSON.stringify(body),
  });
  // 200 = received, 202 = received and key check pending. Anything else is a problem.
  const note = res.ok ? 'accepted' : `REJECTED: ${(await res.text()).slice(0, 200)}`;
  return `${res.status} ${note}, ${urlList.length} URLs${dropped ? ` (${dropped} other-host URLs left out)` : ''}`;
}

const hosts = process.argv.slice(2).flatMap(a => a.split(/[\s,]+/)).filter(Boolean);
if (!hosts.length) {
  console.log('indexnow: no hosts deployed, nothing to send');
  process.exit(0);
}
for (const host of hosts) {
  try {
    console.log(`indexnow ${host}: ${await submit(host)}`);
  } catch (e) {
    console.log(`indexnow ${host}: failed: ${e.message}`);
  }
}
