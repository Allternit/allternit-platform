import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { createHash } from "node:crypto";
import type { Browser } from "playwright";
import type {
  ArtifactFile,
  ArtifactSink,
  ProviderArtifactRef,
  ProviderId,
} from "@allternit/subscription-fabric-contracts";
import { captureImages, sniffMime } from "../src/index";
import { launchBrowser, loadPack } from "./helpers";
import { createResolver } from "../src/index";

let browser: Browser;
beforeAll(async () => {
  browser = await launchBrowser();
}, 30000);
afterAll(async () => {
  await browser.close();
}, 30000);

const PNG_B64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
const GIF_B64 = "R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

function recordingSink() {
  const calls: Array<{ op: string; artifactId?: string; ref?: ProviderArtifactRef }> = [];
  const committed: ArtifactFile[] = [];
  const sink: ArtifactSink = {
    begin: async (ref) => {
      calls.push({ op: "begin", ref });
      return `artifact-${calls.length}`;
    },
    write: async (artifact_id) => {
      calls.push({ op: "write", artifactId: artifact_id });
    },
    commit: async (artifact_id, file) => {
      calls.push({ op: "commit", artifactId: artifact_id });
      committed.push(file);
    },
    fail: async (artifact_id) => {
      calls.push({ op: "fail", artifactId: artifact_id });
    },
  };
  return { sink, calls, committed };
}

describe("captureImages (§A3.1 + §A6.5/§A6.6)", () => {
  it("captures data-URL images with verified MIME and real sha256; skips disallowed origins", async () => {
    const page = await browser.newPage();
    await page.setContent(`
      <div data-testid="fw-response">
        <img data-artifact-id="fw-img-png" src="data:image/png;base64,${PNG_B64}">
        <img data-artifact-id="fw-img-gif" src="data:image/gif;base64,${GIF_B64}">
        <img src="https://disallowed.example.test/tracker.png">
      </div>`);
    const resolver = createResolver(page, loadPack());
    const { sink, calls, committed } = recordingSink();
    const result = await captureImages(page, resolver, sink, {
      provider: "fixture-web" as ProviderId,
      allowedOrigins: ["https://fixture-web.test"],
    });

    expect(result.files).toHaveLength(2);
    const png = committed.find((f) => f.mime_type === "image/png");
    const pngBytes = Uint8Array.from(Buffer.from(PNG_B64, "base64"));
    expect(png?.sha256).toBe(createHash("sha256").update(pngBytes).digest("hex"));
    expect(png?.size_bytes).toBe(pngBytes.length);
    expect(png?.format).toBe("png");
    expect(committed.some((f) => f.mime_type === "image/gif")).toBe(true);

    expect(result.skipped).toHaveLength(1);
    expect(result.skipped[0]).toMatchObject({
      url: "https://disallowed.example.test/tracker.png",
      reason: "origin not allowlisted",
    });

    // §A6.6 — bytes flow only through the sink: begin → write → commit, no fail.
    expect(calls.map((c) => c.op)).toEqual(["begin", "write", "commit", "begin", "write", "commit"]);
    const beginPng = calls.find((c) => c.ref?.provider_artifact_id === "fw-img-png");
    expect(beginPng?.ref?.provider).toBe("fixture-web");
    await page.close();
  });

  it("captures blob: images from inside the page (ChatGPT live UI) with verified MIME", async () => {
    const page = await browser.newPage();
    await page.route("https://fixture-web.test/**", (route) =>
      route.fulfill({ contentType: "text/html", body: `<div data-testid="fw-response"></div>` })
    );
    await page.goto("https://fixture-web.test/thread");
    await page.evaluate((b64) => {
      const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      const img = document.createElement("img");
      img.src = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
      document.querySelector("[data-testid='fw-response']")!.appendChild(img);
    }, PNG_B64);
    const { sink, committed } = recordingSink();
    const result = await captureImages(page, createResolver(page, loadPack()), sink, {
      provider: "fixture-web" as ProviderId,
      allowedOrigins: ["https://fixture-web.test"],
    });
    expect(result.skipped).toEqual([]);
    expect(result.files).toHaveLength(1);
    const pngBytes = Uint8Array.from(Buffer.from(PNG_B64, "base64"));
    expect(committed[0].sha256).toBe(createHash("sha256").update(pngBytes).digest("hex"));
    expect(committed[0].mime_type).toBe("image/png");
    await page.close();
  });

  it("sniffMime recognizes the required formats and rejects unknown bytes", () => {
    expect(sniffMime(Uint8Array.from([0x89, 0x50, 0x4e, 0x47, 0x0d]))?.mime_type).toBe("image/png");
    expect(sniffMime(Uint8Array.from([0xff, 0xd8, 0xff, 0xe0]))?.mime_type).toBe("image/jpeg");
    expect(sniffMime(new TextEncoder().encode("GIF89a...."))?.mime_type).toBe("image/gif");
    expect(sniffMime(new TextEncoder().encode("RIFF....WEBP"))?.mime_type).toBe("image/webp");
    expect(sniffMime(new TextEncoder().encode("%PDF-1.7"))?.mime_type).toBe("application/pdf");
    expect(sniffMime(Uint8Array.from([0x50, 0x4b, 0x03, 0x04]))?.mime_type).toBe("application/zip");
    expect(sniffMime(new TextEncoder().encode("plain text"))).toBeNull();
  });
});
