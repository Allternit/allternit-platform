// Client for Allternit Desktop's phone gateway: the same loopback endpoint bots
// reach linked phones through (surfaces/allternit-desktop/src/main/phone-gateway.ts).
// Desktop writes { url, token } to ~/.allternit/phone-gateway.json when the gateway
// starts; the file is re-read on every call because the token changes per launch.
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

export type PhoneRoute = "android-app" | "iphone-app-ictrl";

/** Shape of one `phone.devices` entry (phone-link-backend.ts LinkedPhoneDeviceInfo, subset). */
export interface PhoneDevice {
  serial: string;
  platform: "ios" | "android";
  via?: string;
  route?: string;
  name?: string;
  health?: { state?: string; action?: string };
}

/** A phone tool result: `{ ok: true, ...}` or `{ ok: false, error, message }` (phone-tools.ts ToolResult). */
export type PhoneToolResult = { ok: true; [key: string]: unknown } | { ok: false; error: string; message: string };

export interface PhoneGateway {
  call(name: string, args?: Record<string, unknown>): Promise<PhoneToolResult>;
}

export const PHONE_GATEWAY_TOKEN_HEADER = "x-allternit-phone-token";
export const DEFAULT_PHONE_GATEWAY_FILE = join(homedir(), ".allternit", "phone-gateway.json");

export class DesktopPhoneGateway implements PhoneGateway {
  constructor(
    private readonly opts: { file?: string; fetchImpl?: typeof fetch; timeoutMs?: number } = {},
  ) {}

  private endpoint(): { url: string; token: string } | null {
    try {
      const raw = JSON.parse(readFileSync(this.opts.file ?? DEFAULT_PHONE_GATEWAY_FILE, "utf8")) as { url?: unknown; token?: unknown };
      return typeof raw.url === "string" && typeof raw.token === "string" ? { url: raw.url, token: raw.token } : null;
    } catch {
      return null;
    }
  }

  /**
   * Any other phone-gateway route (the pairing handover pair, /phone/handover*).
   * `status` 0 = Desktop or its gateway isn't running here.
   */
  async request(method: "GET" | "POST", path: string, body?: unknown): Promise<{ status: number; body: unknown }> {
    const ep = this.endpoint();
    if (!ep) return { status: 0, body: { ok: false, error: "desktop_unavailable", message: "Allternit Desktop isn't running on this computer." } };
    const f = this.opts.fetchImpl ?? fetch;
    try {
      const res = await f(`${ep.url.replace(/\/+$/, "")}${path}`, {
        method,
        headers: { "content-type": "application/json", [PHONE_GATEWAY_TOKEN_HEADER]: ep.token },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
        signal: AbortSignal.timeout(this.opts.timeoutMs ?? 60_000),
      });
      return { status: res.status, body: await res.json().catch(() => null) };
    } catch (e) {
      return { status: 0, body: { ok: false, error: "desktop_unavailable", message: `Couldn't reach Allternit Desktop's phone gateway: ${(e as Error).message}` } };
    }
  }

  async call(name: string, args: Record<string, unknown> = {}): Promise<PhoneToolResult> {
    const ep = this.endpoint();
    if (!ep) return { ok: false, error: "desktop_unavailable", message: "Allternit Desktop's phone gateway isn't running on this computer." };
    const f = this.opts.fetchImpl ?? fetch;
    try {
      const res = await f(`${ep.url.replace(/\/+$/, "")}/phone/call`, {
        method: "POST",
        headers: { "content-type": "application/json", [PHONE_GATEWAY_TOKEN_HEADER]: ep.token },
        body: JSON.stringify({ name, arguments: args }),
        signal: AbortSignal.timeout(this.opts.timeoutMs ?? 60_000),
      });
      const body = (await res.json().catch(() => null)) as PhoneToolResult | null;
      if (!body || typeof body !== "object") return { ok: false, error: "phone_failed", message: `phone gateway answered ${res.status}` };
      return body;
    } catch (e) {
      return { ok: false, error: "desktop_unavailable", message: `Couldn't reach Allternit Desktop's phone gateway: ${(e as Error).message}` };
    }
  }
}

/** Linked phones Desktop knows about (empty when Desktop or its gateway isn't running). */
export async function linkedPhones(gw: PhoneGateway): Promise<PhoneDevice[]> {
  const r = await gw.call("phone.devices");
  if (!r.ok) return [];
  const devices = (r as { devices?: unknown }).devices;
  return Array.isArray(devices) ? (devices as PhoneDevice[]).filter((d) => d && typeof d.serial === "string") : [];
}
