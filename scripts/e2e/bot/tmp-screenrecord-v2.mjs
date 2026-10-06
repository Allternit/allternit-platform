#!/usr/bin/env node
import { chromium } from 'playwright';
import fs from 'fs';
import path from 'path';

const OUT_DIR = '/tmp/omb-preview-v2';
fs.mkdirSync(OUT_DIR, { recursive: true });

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({
  viewport: { width: 1280, height: 720 },
  recordVideo: { dir: OUT_DIR, size: { width: 1280, height: 720 } },
});
const page = await context.newPage();

page.on('console', (msg) => console.log('PAGE CONSOLE:', msg.type(), msg.text()));
page.on('pageerror', (err) => console.error('PAGE ERROR:', err.message, err.stack));

const wait = (ms) => new Promise((r) => setTimeout(r, ms));

async function screenshot(name) {
  await page.screenshot({ path: path.join(OUT_DIR, `${name}.png`) });
}

async function clickFirstVisible(selector, opts = {}) {
  const loc = page.locator(selector).first();
  await loc.waitFor({ state: 'visible', timeout: opts.timeout ?? 10000 });
  await loc.click(opts);
  return loc;
}

async function clickByTitle(title, opts = {}) {
  return clickFirstVisible(`button[title="${title}"]`, opts);
}

try {
  await page.goto('http://localhost:3014/', { waitUntil: 'domcontentloaded', timeout: 30000 });
  await wait(4000);
  await screenshot('01-initial');

  // Collapse the rail to show the agents mascot on the collapsed strip.
  const collapseBtn = page.locator('button[title="Collapse Sidebar"], button[title="Collapse sidebar"]').first();
  if (await collapseBtn.isVisible().catch(() => false)) {
    await collapseBtn.click();
    await wait(800);
    await screenshot('02-collapsed-rail');
  }

  // Expand the rail back.
  const expandBtn = page.locator('button[title="Expand Sidebar"], button[title="Expand sidebar"]').first();
  if (await expandBtn.isVisible().catch(() => false)) {
    await expandBtn.click();
    await wait(800);
    await screenshot('03-expanded-rail');
  }

  // Ensure we are in Home mode.
  const homeBtn = page.getByRole('button', { name: /Home/i }).first();
  await homeBtn.click().catch(() => {});
  await wait(500);

  // Open the Agents section in the shell rail.
  const agentsTab = page.locator('button').filter({ hasText: /^Agents$/ }).first();
  await agentsTab.waitFor({ state: 'visible', timeout: 10000 });
  await agentsTab.click();
  await wait(1000);
  await screenshot('04-bot-rail-in-shell');

  // Create a new group channel from the compact nested bot roster.
  await clickByTitle('New channel', { timeout: 5000 });
  await wait(800);
  await screenshot('05-group-dialog');

  // Fill group name.
  const nameInput = page.getByPlaceholder('e.g. Engineering standup');
  await nameInput.fill('Engineering Sync');
  await wait(300);

  // Pick first two bots in the members list.
  const botRows = await page.locator('div.max-h-40 >> button[type="button"]').all();
  const selectableBots = botRows.slice(0, 2);
  for (const row of selectableBots) {
    await row.click();
    await wait(200);
  }
  await screenshot('06-group-dialog-filled');

  // Save group.
  await page.getByRole('button', { name: /Create channel/i }).first().click();
  await wait(1200);
  await screenshot('07-group-created');

  // Send a message in the group chat.
  const composer = page.locator('textarea[placeholder*="Message"]').first();
  await composer.fill('Hey team, what is the smallest feature we could ship today?');
  await wait(400);
  await composer.press('Enter');
  await wait(2500);
  await screenshot('08-group-message-landed');

  // Navigate back to the Agents roster and select the first bot.
  await agentsTab.click();
  await wait(500);
  // Ensure Agents section is expanded (localStorage may have collapsed it).
  const firstBotName = page.locator('div').filter({ hasText: /^Audit Test Bot$/ }).first();
  if (!(await firstBotName.isVisible().catch(() => false))) {
    await agentsTab.click();
    await wait(500);
  }
  await firstBotName.waitFor({ state: 'visible', timeout: 10000 });
  await firstBotName.click();
  await wait(1500);
  await screenshot('09-bot-chat-session');

  // Open the brain/model picker in the bot session composer and choose Kimi.
  const modelBtn = page.locator('[data-testid="model-picker-trigger"]').first();
  if (await modelBtn.isVisible().catch(() => false)) {
    await modelBtn.click();
    await wait(1500);
    await screenshot('10-model-picker');

    // Expand the Kimi CLI provider row.
    const kimiRow = page.locator('[role="button"]').filter({ hasText: /Kimi CLI/i }).first();
    if (await kimiRow.isVisible().catch(() => false)) {
      await kimiRow.click();
      await wait(1200);
      await screenshot('10b-kimi-expanded');

      // Select the first Kimi model.
      const kimiModel = page.locator('[role="option"]').first();
      if (await kimiModel.isVisible().catch(() => false)) {
        await kimiModel.click();
        await wait(800);
      } else {
        await page.keyboard.press('Escape');
        await wait(400);
      }
    } else {
      await page.keyboard.press('Escape');
      await wait(400);
    }
  }

  // Send a message in the bot chat.
  const botComposer = page.locator('textarea[aria-label="Text Area"]').first();
  await botComposer.fill('Hi Echo, plan a small feature for me');
  await wait(400);
  await botComposer.press('Enter');
  await wait(2500);
  await screenshot('11-bot-message-landed');

  // Toggle collapse again to end on the mascot view.
  const collapseBtn2 = page.locator('button[title="Collapse Sidebar"], button[title="Collapse sidebar"]').first();
  if (await collapseBtn2.isVisible().catch(() => false)) {
    await collapseBtn2.click();
    await wait(800);
    await screenshot('13-final-collapsed');
  }
} catch (err) {
  console.error('Recording failed:', err);
  await screenshot('error-state');
} finally {
  await page.close();
  await context.close();
  await browser.close();

  const videoPath = fs.readdirSync(OUT_DIR).find((f) => f.endsWith('.webm'));
  if (videoPath) {
    const src = path.join(OUT_DIR, videoPath);
    const dst = path.join(OUT_DIR, 'demo-v2.webm');
    fs.renameSync(src, dst);
    console.log('Video:', dst);
  }
}
