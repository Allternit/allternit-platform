#!/usr/bin/env node
/** Resolve private Allternit/allternit-ai (workspace UI). Never the console. */
const fs = require('fs');
const path = require('path');

function resolveWorkspaceUi() {
  const envPath = process.env.ALLTERNIT_AI_PATH;
  if (envPath && fs.existsSync(path.join(envPath, 'package.json'))) {
    return path.resolve(envPath);
  }
  const repoRoot = path.resolve(__dirname, '../../..');
  const candidates = [
    path.join(repoRoot, '.hosted-ui'),
    path.resolve(repoRoot, '..', 'allternit-ai'),
  ];
  for (const c of candidates) {
    if (fs.existsSync(path.join(c, 'package.json'))) return c;
  }
  throw new Error(
    'Workspace UI not found. Clone Allternit/allternit-ai next to this repo or set ALLTERNIT_AI_PATH. Do not write discovery artifacts into this public repo.',
  );
}

module.exports = { resolveWorkspaceUi };
