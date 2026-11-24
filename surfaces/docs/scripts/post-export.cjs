#!/usr/bin/env node
// Runs after `mint export` is unzipped into dist/. The export has no
// robots.txt or sitemap.xml, so Cloudflare Pages answered both with the home
// page HTML and crawlers (Google, Bing, Brave) got no list of docs pages.
// This writes:
//   dist/robots.txt   allow all, points at the sitemap
//   dist/sitemap.xml  every page in docs.json navigation that was exported
//   dist/<key>.txt    the IndexNow key file (scripts/indexnow.mjs pings after deploy)
//
//   node scripts/post-export.cjs [distDir]
const fs = require('fs');
const path = require('path');

const SITE = 'https://docs.allternit.com';
const root = path.join(__dirname, '..');
const dist = path.resolve(process.argv[2] || path.join(root, 'dist'));
if (!fs.existsSync(path.join(dist, 'index.html'))) {
  console.error(`post-export: ${dist}/index.html not found; run the export first`);
  process.exit(1);
}

function collect(node, out) {
  if (typeof node === 'string') return void out.push(node);
  if (Array.isArray(node)) return void node.forEach(n => collect(n, out));
  if (node && typeof node === 'object') {
    for (const k of ['pages', 'groups', 'tabs', 'anchors', 'dropdowns', 'languages', 'versions', 'items']) {
      if (node[k] !== undefined) collect(node[k], out);
    }
    if (typeof node.page === 'string') out.push(node.page);
  }
}

const docs = JSON.parse(fs.readFileSync(path.join(root, 'docs.json'), 'utf8'));
const pages = [];
collect(docs.navigation, pages);

// The first page is the site home ("/"); its own path is the same page again.
const home = pages.find(p => !/^[a-z]+:|\s/i.test(p));
const urls = new Set([`${SITE}/`]);
let missing = 0;
for (const raw of pages) {
  if (/^[a-z]+:|\s/i.test(raw)) continue; // external links and OpenAPI "GET /x" entries
  const p = raw.split('#')[0].replace(/^\/+/, '').replace(/\.(mdx?|html)$/, '').replace(/\/index$/, '');
  if (!p || p === home) continue;
  const exported = fs.existsSync(path.join(dist, p, 'index.html')) || fs.existsSync(path.join(dist, `${p}.html`));
  if (!exported) { missing++; continue; }
  urls.add(`${SITE}/${p}/`); // live pages answer on the trailing-slash form (others 308 to it)
}

const today = new Date().toISOString().slice(0, 10);
fs.writeFileSync(
  path.join(dist, 'sitemap.xml'),
  `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n` +
    [...urls].map(u => `  <url><loc>${u}</loc><lastmod>${today}</lastmod></url>`).join('\n') +
    `\n</urlset>\n`
);
fs.writeFileSync(path.join(dist, 'robots.txt'), `User-agent: *\nAllow: /\n\nSitemap: ${SITE}/sitemap.xml\n`);
const key = fs.readFileSync(path.join(__dirname, 'indexnow.key'), 'utf8').trim();
fs.writeFileSync(path.join(dist, `${key}.txt`), key);

console.log(`post-export: sitemap.xml ${urls.size} URLs (${missing} nav pages not in the export), robots.txt, IndexNow key file`);
