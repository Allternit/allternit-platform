// Offline phone gateway for tests and conformance: plays back the read_ui shapes recorded on
// real phones (2026-10-17) and walks a message through typed → streaming → finished. No
// Desktop, phone or vendor is touched.
import type { PhoneGateway, PhoneToolResult } from "../../../src/phone/gateway.js";
import type { PhoneChatAppSpec } from "./chat-provider.js";

export type ReplayMode = "normal" | "down" | "locked" | "logged_out" | "rate_limited" | "verification" | "drift" | "no_phone";

const SERIAL = "link:replay-device";

/** Idle screens as the real apps render them (subset of the recorded trees). */
function idleScreen(spec: PhoneChatAppSpec, extra: string[] = [], opts: { send?: boolean; stop?: boolean; signedOut?: boolean; typed?: string; noComposer?: boolean } = {}): string {
  if (spec.platform === "ios") {
    return [
      "Application, label: 'ChatGPT', {{0, 0}, {393, 852}}",
      "  Button, label: 'Menu', {{20, 63}, {36, 36}}",
      ...(opts.signedOut ? ["  Button, label: 'Log in', {{300, 63}, {80, 36}}"] : []),
      ...extra.map((t, i) => `  StaticText, value: '${t.replace(/'/g, "\\'")}', {{16, ${200 + i * 40}}, {361, 24}}`),
      ...(opts.noComposer ? [] : [`  TextView, label: 'New Message'${opts.typed ? `, value: '${opts.typed}'` : ""}, {{11, 413}, {369, 44}}`]),
      ...(opts.stop ? ["  Button, label: 'Stop generating', {{333, 457}, {48, 48}}"] : []),
      ...(opts.send ? ["  Button, label: 'Send', id: 'MessageInputPrimaryButtonContainerPart.PrimaryButton', {{333, 457}, {48, 48}}"] : []),
    ].join("\n");
  }
  return [
    "Window(application), {{0, 0}, {720, 1600}}, package: 'com.openai.chatgpt'",
    "  View, label: 'Menu', {{25, 101}, {77, 77}}",
    ...(opts.signedOut ? ["  TextView, label: 'Log in', {{578, 121}, {89, 36}}"] : []),
    ...extra.map((t, i) => `  TextView, label: '${t.replace(/'/g, "\\'")}', {{49, ${400 + i * 60}}, {622, 46}}`),
    ...(opts.noComposer ? [] : ["  View, label: 'Ask ChatGPT', {{106, 1430}, {593, 46}}"]),
    `  EditText, {{106, 1411}, {593, 84}}, clickable long-clickable editable`,
    ...(opts.stop ? ["  View, label: 'Stop generating', {{611, 1411}, {84, 84}}"] : []),
    ...(opts.send ? ["  View, label: 'Send Message', {{632, 1432}, {42, 42}}"] : []),
  ].join("\n");
}

export class ReplayPhoneGateway implements PhoneGateway {
  readonly calls: { name: string; args: Record<string, unknown> }[] = [];
  private typed = "";
  private sent: string[] = [];
  private streamingReads = 0;
  private replies: string[] = [];

  constructor(
    private readonly spec: PhoneChatAppSpec,
    private readonly mode: ReplayMode = "normal",
    private readonly reply: (text: string) => string = (t) => `Reply to: ${t}`,
  ) {}

  async call(name: string, args: Record<string, unknown> = {}): Promise<PhoneToolResult> {
    this.calls.push({ name, args });
    if (this.mode === "down") return { ok: false, error: "desktop_unavailable", message: "Allternit Desktop's phone gateway isn't running on this computer." };
    if (name === "phone.devices") {
      if (this.mode === "no_phone") return { ok: true, serials: [], devices: [] };
      return {
        ok: true,
        serials: [SERIAL],
        devices: [{ serial: SERIAL, platform: this.spec.platform, via: "phone-link", route: this.spec.route, name: "Replay phone", health: { state: this.mode === "locked" ? "DEVICE_LOCKED" : "HEALTHY", recovery: "automatic" } }],
      };
    }
    if (this.mode === "locked" && name !== "phone.devices") return { ok: false, error: "device_locked", message: "Unlock your phone and try again." };
    switch (name) {
      case "phone.open_app": return { ok: true, app: this.spec.appId };
      case "phone.type": this.typed = String(args.text ?? ""); return { ok: true };
      case "phone.find_and_tap": {
        const id = String(args.element_id ?? "");
        if (this.mode === "drift" && id === this.spec.composer) return { ok: false, error: "phone_failed", message: `no element matching '${id}'` };
        if (this.spec.send.test(id) && this.typed) {
          this.sent.push(this.typed);
          this.replies.push(this.reply(this.typed));
          this.typed = "";
          this.streamingReads = 2;
        }
        if (this.spec.stop.test(id)) this.streamingReads = 0;
        return { ok: true, message: `Tapped '${id}'` };
      }
      case "phone.read_ui": {
        const banners = this.mode === "rate_limited" ? ["You've reached your message limit."] : this.mode === "verification" ? ["Please verify you are human to continue."] : [];
        const history = this.sent.flatMap((s, i) => (i < this.sent.length - 1 || this.streamingReads === 0 ? [s, this.replies[i]] : [s]));
        if (this.streamingReads > 0) {
          this.streamingReads -= 1;
          return { ok: true, text: idleScreen(this.spec, [...banners, ...history], { stop: true, signedOut: this.mode === "logged_out" }) };
        }
        return { ok: true, text: idleScreen(this.spec, [...banners, ...history], { send: !!this.typed || this.sent.length > 0, signedOut: this.mode === "logged_out", typed: this.typed || undefined, noComposer: this.mode === "drift" }) };
      }
      default: return { ok: true };
    }
  }
}
