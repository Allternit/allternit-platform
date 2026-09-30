// The probe waits for a single-page app to draw before judging sign-in.
import { describe, expect, it } from "vitest";
import { settleForProbe } from "../src/worker/pool.js";

const after = (ms: number, key: string) => {
  const t0 = Date.now();
  return async (k: string) => (k === key && Date.now() - t0 >= ms ? {} : null);
};

describe("settleForProbe", () => {
  it("returns as soon as the signed-in marker renders", async () => {
    const t0 = Date.now();
    await settleForProbe(after(120, "logged_in_probe"), { loggedIn: "logged_in_probe" }, 5000, 10);
    expect(Date.now() - t0).toBeLessThan(1000);
  });

  it("a verification check or a logged-out marker also ends the wait", async () => {
    for (const key of ["challenge", "logged_out_probe"]) {
      const t0 = Date.now();
      await settleForProbe(after(50, key), { loggedIn: "logged_in_probe" }, 5000, 10);
      expect(Date.now() - t0).toBeLessThan(1000);
    }
  });

  it("gives up after the budget when nothing decisive shows", async () => {
    const t0 = Date.now();
    await settleForProbe(async () => null, { loggedIn: "logged_in_probe" }, 150, 10);
    expect(Date.now() - t0).toBeGreaterThanOrEqual(150);
  });
});
