// §A3.1 — captureDownload / captureImages; §A6.6 — MIME verify via magic bytes.
import { createHash } from "node:crypto";
import type { Page } from "playwright";
import type {
  ArtifactFile,
  ArtifactSink,
  ProviderArtifactRef,
  ProviderId,
} from "@allternit/subscription-fabric-contracts";
import type { SdkSelectorResolver } from "./selectors";

export interface SniffedMime {
  mime_type: string;
  format: string;
}

// Magic-byte sniffing; declared MIME alone is never trusted (§A6.6).
export function sniffMime(bytes: Uint8Array): SniffedMime | null {
  const b = bytes;
  if (b.length >= 4 && b[0] === 0x89 && b[1] === 0x50 && b[2] === 0x4e && b[3] === 0x47)
    return { mime_type: "image/png", format: "png" };
  if (b.length >= 3 && b[0] === 0xff && b[1] === 0xd8 && b[2] === 0xff)
    return { mime_type: "image/jpeg", format: "jpeg" };
  if (b.length >= 6 && ascii(b, 0, 6) === "GIF87a") return { mime_type: "image/gif", format: "gif" };
  if (b.length >= 6 && ascii(b, 0, 6) === "GIF89a") return { mime_type: "image/gif", format: "gif" };
  if (b.length >= 12 && ascii(b, 0, 4) === "RIFF" && ascii(b, 8, 12) === "WEBP")
    return { mime_type: "image/webp", format: "webp" };
  if (b.length >= 5 && ascii(b, 0, 5) === "%PDF-") return { mime_type: "application/pdf", format: "pdf" };
  if (b.length >= 4 && b[0] === 0x50 && b[1] === 0x4b && b[2] === 0x03 && b[3] === 0x04)
    return { mime_type: "application/zip", format: "zip" };
  return null;
}

function ascii(b: Uint8Array, start: number, end: number): string {
  return String.fromCharCode(...b.subarray(start, end));
}

function sha256(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

export class CaptureError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "CaptureError";
  }
}

// §A6.6 — captured bytes land only via the sink (content-addressed quarantine
// is the store's job); nothing here auto-opens or writes to a live path.
async function sinkCommit(
  sink: ArtifactSink,
  ref: ProviderArtifactRef,
  bytes: Uint8Array<ArrayBuffer>,
  sniffed: SniffedMime,
  meta?: { title?: string }
): Promise<ArtifactFile> {
  const file: ArtifactFile = {
    data: bytes,
    mime_type: sniffed.mime_type,
    format: sniffed.format,
    sha256: sha256(bytes),
    size_bytes: bytes.length,
  };
  const artifactId = await sink.begin(ref, {
    mime_type: sniffed.mime_type,
    format: sniffed.format,
    title: meta?.title,
  });
  try {
    await sink.write(artifactId, bytes);
    await sink.commit(artifactId, file);
  } catch (err) {
    await sink.fail(artifactId, err instanceof Error ? err.message : String(err));
    throw err;
  }
  return file;
}

export async function captureDownload(
  page: Page,
  sink: ArtifactSink,
  trigger: () => Promise<void>,
  opts: { provider: ProviderId }
): Promise<ArtifactFile> {
  const [download] = await Promise.all([page.waitForEvent("download"), trigger()]);
  const stream = await download.createReadStream();
  if (!stream) throw new CaptureError("download stream unavailable");
  const chunks: Buffer[] = [];
  for await (const chunk of stream) chunks.push(chunk as Buffer);
  const bytes = Uint8Array.from(Buffer.concat(chunks));
  const sniffed = sniffMime(bytes);
  if (!sniffed) throw new CaptureError("download MIME could not be verified (magic bytes)");
  const ref: ProviderArtifactRef = {
    provider: opts.provider,
    provider_artifact_id: download.suggestedFilename(),
    provider_url: download.url(),
    provider_url_expires_at: null,
  };
  return sinkCommit(sink, ref, bytes, sniffed, { title: download.suggestedFilename() });
}

export interface CaptureImagesOptions {
  provider: ProviderId;
  // §A6.5 — network image bodies are fetched only from allowlisted origins;
  // data: URLs are inline content and need no network.
  allowedOrigins: string[];
  key?: string; // pack key locating the image container; default "response"
  // Which images inside the container to take (default "img") — e.g. only
  // the ones a reused chat did not already show.
  imgSelector?: string;
}

export interface CaptureImagesResult {
  files: ArtifactFile[];
  skipped: Array<{ url: string; reason: string }>;
}

function decodeDataUrl(url: string): { mime: string; bytes: Uint8Array<ArrayBuffer> } | null {
  const m = /^data:([^;,]+)?(;base64)?,(.*)$/s.exec(url);
  if (!m) return null;
  const mime = m[1] ?? "text/plain";
  const data = m[3];
  const bytes = m[2]
    ? Uint8Array.from(Buffer.from(data, "base64"))
    : Uint8Array.from(new TextEncoder().encode(decodeURIComponent(data)));
  return { mime, bytes };
}

export async function captureImages(
  page: Page,
  resolver: SdkSelectorResolver,
  sink: ArtifactSink,
  opts: CaptureImagesOptions
): Promise<CaptureImagesResult> {
  const container = await resolver.tryResolveLocator(opts.key ?? "response");
  if (!container) return { files: [], skipped: [] };
  const imgs = await container.locator(opts.imgSelector ?? "img").all();
  const files: ArtifactFile[] = [];
  const skipped: Array<{ url: string; reason: string }> = [];

  for (const img of imgs) {
    const src = await img.getAttribute("src");
    if (!src) continue;

    let bytes: Uint8Array<ArrayBuffer>;
    if (src.startsWith("data:")) {
      const decoded = decodeDataUrl(src);
      if (!decoded) {
        skipped.push({ url: "data:…", reason: "unparsable data URL" });
        continue;
      }
      bytes = decoded.bytes;
    } else {
      let origin: string;
      try {
        origin = new URL(src).origin;
      } catch {
        skipped.push({ url: src, reason: "unparsable URL" });
        continue;
      }
      if (!opts.allowedOrigins.includes(origin)) {
        skipped.push({ url: src, reason: "origin not allowlisted" });
        continue;
      }
      if (src.startsWith("blob:")) {
        // blob: URLs exist only inside the page (ChatGPT renders generated
        // images this way) — read the bytes there; magic-byte sniffing below
        // still gates what is committed. No named inner functions: the body
        // is serialized into the page (see extract.ts on __name).
        const b64 = await img.evaluate(async (el) => {
          const res = await fetch((el as HTMLImageElement).currentSrc || (el as HTMLImageElement).src);
          if (!res.ok) return null;
          const buf = new Uint8Array(await res.arrayBuffer());
          let bin = "";
          for (let i = 0; i < buf.length; i += 0x8000) {
            bin += String.fromCharCode(...buf.subarray(i, i + 0x8000));
          }
          return btoa(bin);
        });
        if (!b64) {
          skipped.push({ url: src.slice(0, 64), reason: "blob fetch failed" });
          continue;
        }
        bytes = Uint8Array.from(Buffer.from(b64, "base64"));
      } else {
        const response = await page.context().request.get(src);
        if (!response.ok()) {
          skipped.push({ url: src, reason: `fetch failed: ${response.status()}` });
          continue;
        }
        bytes = Uint8Array.from(await response.body());
      }
    }

    const sniffed = sniffMime(bytes);
    if (!sniffed) {
      skipped.push({ url: src.slice(0, 64), reason: "MIME could not be verified (magic bytes)" });
      continue;
    }
    const ref: ProviderArtifactRef = {
      provider: opts.provider,
      provider_artifact_id: (await img.getAttribute("data-artifact-id")) ?? sha256(bytes).slice(0, 16),
      provider_url: src.startsWith("data:") ? "data:" : src,
      provider_url_expires_at: null,
    };
    files.push(await sinkCommit(sink, ref, bytes, sniffed));
  }
  return { files, skipped };
}
