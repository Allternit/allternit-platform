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

function collectMdxFiles(dir) {
  const results = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
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
const orphanPages = [...referenced].filter((page) => page.includes('/') && !existingMdx.has(page));
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
      if (mdxPath.endsWith('guides/platform-workflows.mdx')) continue;
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
