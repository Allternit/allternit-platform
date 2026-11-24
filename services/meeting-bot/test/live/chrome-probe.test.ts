// Chrome path probe order and headless UA (src/chrome.ts).
import { strict as assert } from "node:assert";
import { test } from "node:test";
import { findChrome, resolveWindowMode, windowModeArgs, type ProbeEnv } from "../../src/chrome.js";

const probe = (platform: NodeJS.Platform, present: string[], env: Record<string, string> = {}): ProbeEnv => ({
  platform,
  env,
  exists: (p) => present.includes(p),
});

test("probe prefers Chrome, then Edge, then Chromium", () => {
  const chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
  const edge = "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge";
  const chromium = "/Applications/Chromium.app/Contents/MacOS/Chromium";
  assert.equal(findChrome(probe("darwin", [chromium, edge, chrome])), chrome);
  assert.equal(findChrome(probe("darwin", [chromium, edge])), edge);
  assert.equal(findChrome(probe("darwin", [chromium])), chromium);
  assert.equal(findChrome(probe("darwin", [])), null);
  const env = { ProgramFiles: "C:\\PF", LOCALAPPDATA: "C:\\L" };
  const edgeWin = "C:\\PF\\Microsoft\\Edge\\Application\\msedge.exe";
  const chromeLocal = "C:\\L\\Google\\Chrome\\Application\\chrome.exe";
  assert.equal(findChrome(probe("win32", [edgeWin, chromeLocal], env)), chromeLocal);
  assert.equal(findChrome(probe("linux", ["/usr/bin/chromium", "/opt/google/chrome/chrome"], { PATH: "/usr/bin" })), "/opt/google/chrome/chrome");
  assert.equal(findChrome(probe("linux", ["/x/chrome"], { CHROME_PATH: "/x/chrome" })), "/x/chrome");
});

test("headless args use a desktop UA without HeadlessChrome", () => {
  const args = windowModeArgs("headless", "darwin", "Mozilla/5.0 HeadlessChrome/131.0.0.0 Safari/537.36");
  assert.ok(args.includes("--headless=new") && args.includes("--mute-audio") && args.includes("--use-mock-keychain"));
  const ua = args.find((a) => a.startsWith("--user-agent="));
  assert.ok(ua && !ua.includes("HeadlessChrome") && ua.includes("Chrome/131"));
  assert.ok(!windowModeArgs("headless", "linux").includes("--use-mock-keychain"));
  assert.ok(windowModeArgs("offscreen", "linux").includes("--window-position=-32000,-32000"));
});

test("window mode default", () => {
  const p = (env: Record<string, string>, boot: boolean): ProbeEnv => ({ platform: "linux", env, exists: () => boot });
  assert.equal(resolveWindowMode(p({}, true)), "headless");
  assert.equal(resolveWindowMode(p({ DISPLAY: ":0" }, true)), "normal");
  assert.equal(resolveWindowMode(p({ DISPLAY: ":0" }, false)), "headless");
  assert.equal(resolveWindowMode(p({ MEETING_BOT_WINDOW: "offscreen" }, true)), "offscreen");
});
