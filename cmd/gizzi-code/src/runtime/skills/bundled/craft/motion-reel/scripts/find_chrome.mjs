// Locate an installed Chrome/Chromium. $CHROME wins; then the usual macOS, Linux and Windows locations.
import fs from 'fs';
import { execSync } from 'child_process';
export function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  const files = [
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/Applications/Chromium.app/Contents/MacOS/Chromium',
    `${process.env.HOME}/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`,
    'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
  ];
  for (const f of files) if (fs.existsSync(f)) return f;
  for (const b of ['google-chrome', 'google-chrome-stable', 'chromium', 'chromium-browser']) {
    try { const p = execSync(`command -v ${b}`, { stdio: ['ignore', 'pipe', 'ignore'] }).toString().trim(); if (p) return p; } catch {}
  }
  console.error('Chrome/Chromium not found. Install Google Chrome or set CHROME=/path/to/chrome');
  process.exit(1);
}
