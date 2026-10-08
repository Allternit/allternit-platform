#!/usr/bin/env node
/**
 * Docs linter for the Allternit Mintlify docs site.
 *
 * - Ensures docs.json navigation references only existing .mdx files.
 * - Ensures every .mdx file under surfaces/docs is referenced in docs.json.
 * - Checks local Markdown links inside surfaces/docs.
 * - Runs the Mintlify build validator.
 * - Flags competitor names that should not appear in public docs.
 */

const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const ROOT = path.resolve(__dirname, '..');
const DOCS_DIR = path.join(ROOT, 'surfaces', 'docs');
const DOCS_JSON = path.join(DOCS_DIR, 'docs.json');
const BUILD_SCRIPT = path.join(DOCS_DIR, 'scripts', 'build.cjs');

const COMPETITOR_NAMES = /\b(anthropic|claude|openai|codex|chatgpt|gpt-4|gpt-4o|kimi|moonshot)\b/i;

let failed = false;

function fail(message) {
  failed = true;
  process.stderr.write(`FAIL: ${message}\n`);
}

function pass(message) {
  process.stdout.write(`PASS: ${message}\n`);
}

function collectStrings(obj, out = new Set()) {
  if (Array.isArray(obj)) {
    obj.forEach((item) => collectStrings(item, out));
  } else if (obj && typeof obj === 'object') {
    Object.values(obj).forEach((value) => collectStrings(value, out));
  } else if (typeof obj === 'string') {
    out.add(obj);
  }
  return out;
}

// Folders `.mintignore` keeps out of the build (unpublished drafts) are not
// public docs: no navigation entry, no public-docs wording rules.
const MINT_IGNORED = (() => {
  try {
    return fs
      .readFileSync(path.join(DOCS_DIR, '.mintignore'), 'utf8')
      .split('\n')
      .map((l) => l.trim())
      .filter((l) => l && !l.startsWith('#'))
      .map((l) => path.join(DOCS_DIR, l.replace(/\/$/, '')));
  } catch {
    return [];
  }
})();

function collectMdxFiles(dir) {
  const results = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (MINT_IGNORED.includes(full)) continue;
    if (entry.isDirectory()) {
      results.push(...collectMdxFiles(full));
    } else if (entry.name.endsWith('.mdx')) {
      results.push(full);
    }
  }
  return results;
}

// 1. docs.json is valid JSON.
let docs;
try {
  docs = JSON.parse(fs.readFileSync(DOCS_JSON, 'utf8'));
  pass('docs.json is valid JSON');
} catch (error) {
  fail(`docs.json parse error: ${error.message}`);
  process.exit(1);
}

// 2. Navigation references map to existing .mdx files.
const navStrings = [...collectStrings(docs.navigation)];
const existingMdx = new Set(
  collectMdxFiles(DOCS_DIR).map((f) => path.relative(DOCS_DIR, f).replace(/\.mdx$/, ''))
);
const referenced = new Set(
  navStrings.filter((s) => s && !s.startsWith('http') && !s.startsWith('mailto:'))
);

// 2. Navigation references map to existing .mdx files.
// Nav icons are asset paths (e.g. /icons/play.svg), not pages: check the file exists instead.
const isAsset = (s) => /\.(svg|png|jpe?g|webp|gif)$/i.test(s);
for (const asset of [...referenced].filter(isAsset)) {
  if (!fs.existsSync(path.join(DOCS_DIR, asset.replace(/^\//, '')))) fail(`docs.json references missing asset: ${asset}`);
}
const orphanPages = [...referenced].filter((page) => page.includes('/') && !isAsset(page) && !existingMdx.has(page));
for (const page of orphanPages) {
  fail(`docs.json references missing page: ${page}`);
}
if (orphanPages.length === 0) {
  pass('all docs.json navigation pages exist');
}

// 3. Every .mdx file is referenced in docs.json.
for (const file of existingMdx) {
  if (!referenced.has(file)) {
    fail(`MDX file not in docs.json navigation: ${file}.mdx`);
  }
}
if ([...existingMdx].every((f) => referenced.has(f))) {
  pass('all MDX files are referenced in docs.json');
}

// 4. Check local markdown links.
const linkPattern = /\[([^\]]+)\]\(([^)]+)\)/g;
for (const mdxPath of collectMdxFiles(DOCS_DIR)) {
  const text = fs.readFileSync(mdxPath, 'utf8');
  let match;
  while ((match = linkPattern.exec(text)) !== null) {
    const raw = match[2];
    if (
      raw.startsWith('http://') ||
      raw.startsWith('https://') ||
      raw.startsWith('mailto:') ||
      raw.startsWith('#') ||
      raw.startsWith('/')
    ) {
      continue;
    }
    const target = raw.split('#')[0];
    if (!target) continue;
    const resolved = path.resolve(path.dirname(mdxPath), target);
    if (!fs.existsSync(resolved)) {
      fail(`broken link in ${path.relative(ROOT, mdxPath)}: ${raw}`);
    }
  }
}
pass('all local markdown links resolve');

// 5. Flag competitor names in public docs.
for (const mdxPath of collectMdxFiles(DOCS_DIR)) {
  const text = fs.readFileSync(mdxPath, 'utf8');
  const lines = text.split('\n');
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (COMPETITOR_NAMES.test(line)) {
      // Allow the OpenAI migration guide to mention OpenAI in the title/body.
      if (mdxPath.endsWith('guides/openai-migration.mdx')) continue;
      // Native session pickup names the other CLIs; that page is the catalog.
      if (mdxPath.endsWith('cli/native-sessions.mdx')) continue;
      if (mdxPath.endsWith('cli/session.mdx')) continue;
      // Subscription setup and its workflow index must name the supported providers.
      if (mdxPath.endsWith('surfaces/subscriptions.mdx')) continue;
      if (mdxPath.endsWith('guides/subscriptions.mdx')) continue;
      // The subscription gateway API reference names supported providers and routing IDs.
      if (mdxPath.endsWith('api/subscription-gateway.mdx')) continue;
      // Vendor-bot pages must name the vendors whose bots and connectors they cover.
      if (['guides/bot-phone.mdx', 'guides/keep-vendor-bots-online.mdx', 'guides/vendor-bots-on-the-phone.mdx', 'guides/vendor-bots-use-your-phone.mdx', 'guides/how-vendor-bots-connect.mdx', 'guides/allternit-bot-cli.mdx'].some((p) => mdxPath.endsWith(p))) continue;
      if (mdxPath.endsWith('guides/platform-workflows.mdx')) continue;
      // Vendor bots connect to named vendor accounts; photo avatars run on the
      // named subscription/key lanes and quote their on-screen button labels.
      if (mdxPath.endsWith('guides/vendor-bots.mdx')) continue;
      if (mdxPath.endsWith('guides/bot-avatars.mdx')) continue;
      // Porting an MCP App from another host has to name that host and its globals.
      if (mdxPath.endsWith('plugins/guides/porting.mdx')) continue;
      // Connecting AI apps (approvals, MCP Events subscriptions) has to name the apps people connect.
      if (['guides/connected-apps.mdx', 'guides/mcp-events.mdx'].some((p) => mdxPath.endsWith(p))) continue;
      // The Allternit Factory runs bots on named harnesses and vendor accounts; these pages
      // and the generated contract reference have to name which ones (binding badges, harness ids).
      if (['factory/agents.mdx', 'factory/overview.mdx', 'factory/api-reference.mdx'].some((p) => mdxPath.endsWith(p))) continue;
      // Memory Drive is shared with other agents (install steps for Claude Code, Codex, their
      // session ids), and hosted agents name provider/model ids; these pages have to name them.
      if (['guides/memory-drive.mdx', 'core/memory-drive.mdx', 'cli/memory.mdx', 'api/platform/agents.mdx', 'api/memory-drive.mdx'].some((p) => mdxPath.endsWith(p))) continue;
      // A project's own model key is set per provider (`PUT /v1/model_keys/anthropic`); the
      // conversations billing note and the SDK examples have to name that provider id.
      if (['api/platform/conversations.mdx', 'api/platform/sdks/python.mdx', 'api/platform/sdks/typescript.mdx'].some((p) => mdxPath.endsWith(p))) continue;
      // The computer toolset absorbs each model family's native computer-use tool and the hosted
      // driver plugs into those SDKs, so these pages have to name the providers and SDK classes.
      if (['api/platform/hosted-computers.mdx', 'tools/computer-toolset.mdx', 'tools/computer-use.mdx', 'core/computer-use-engine.mdx'].some((p) => mdxPath.endsWith(p))) continue;
      // Release notes describe those same import/share features by the apps' names.
      if (mdxPath.endsWith('release-notes.mdx')) continue;
      // Provider env vars (OPENAI_API_KEY etc.) are configuration, not endorsement.
      if (mdxPath.endsWith('tools/open-notebook.mdx')) continue;
      fail(
        `competitor mention in ${path.relative(ROOT, mdxPath)}:${i + 1}: ${line.trim()}`
      );
    }
  }
}
pass('no competitor mentions outside provider integration references');

// 6. Run Mintlify build validator.
try {
  execSync(`node ${JSON.stringify(BUILD_SCRIPT)}`, { cwd: ROOT, stdio: 'inherit' });
  pass('Mintlify build validation passed');
} catch (error) {
  fail('Mintlify build validation failed');
}

if (failed) {
  process.exit(1);
}
process.stdout.write('\nDocs lint passed.\n');
