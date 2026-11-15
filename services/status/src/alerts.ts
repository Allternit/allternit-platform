// Alert emails through Cloudflare Email Workers (send_email binding).
// Alerts are on from minute one, independent of AUTO_INCIDENTS.
import type { MachineStatus, Transition } from "./state";
import { ALERT } from "./wording";

export const REMINDER_MS = 60 * 60_000;

export type AlertKind = "down" | "degraded" | "recovered" | "still-down" | "still-degraded";

export interface AlertItem {
  component_id: string;
  name: string;
  kind: AlertKind;
}

export interface AlertPlan {
  items: AlertItem[];
  /** New alerted_at per component (null clears it). Apply only after a successful send. */
  alertedAt: Map<string, string | null>;
}

export interface AlertState {
  status: MachineStatus;
  alerted_at: string | null;
}

/**
 * Decide which emails one tick sends. Pure.
 * - Down / Degraded / Recovered on a transition.
 * - "Still ..." every 60 min while a component stays down or degraded.
 * - Nothing for components in maintenance.
 */
export function planAlerts(
  transitions: Transition[],
  states: Map<string, AlertState>,
  names: Map<string, string>,
  inMaintenance: Set<string>,
  now: Date,
): AlertPlan {
  const items: AlertItem[] = [];
  const alertedAt = new Map<string, string | null>();
  const nowIso = now.toISOString();
  const moved = new Set<string>();

  for (const t of transitions) {
    moved.add(t.component_id);
    if (inMaintenance.has(t.component_id)) continue;
    const name = names.get(t.component_id) ?? t.component_id;
    if (t.to === "down" || t.to === "degraded") {
      items.push({ component_id: t.component_id, name, kind: t.to });
      alertedAt.set(t.component_id, nowIso);
    } else if (t.to === "operational" && (t.from === "down" || t.from === "degraded")) {
      items.push({ component_id: t.component_id, name, kind: "recovered" });
      alertedAt.set(t.component_id, null);
    }
  }

  for (const [id, s] of states) {
    if (moved.has(id) || inMaintenance.has(id)) continue;
    if (s.status !== "down" && s.status !== "degraded") continue;
    const last = s.alerted_at ? Date.parse(s.alerted_at) : NaN;
    if (Number.isFinite(last) && now.getTime() - last < REMINDER_MS) continue;
    items.push({ component_id: id, name: names.get(id) ?? id, kind: s.status === "down" ? "still-down" : "still-degraded" });
    alertedAt.set(id, nowIso);
  }

  return { items, alertedAt };
}

const LABEL: Record<AlertKind, string> = {
  down: ALERT.down,
  degraded: ALERT.degraded,
  recovered: ALERT.recovered,
  "still-down": ALERT.stillDown,
  "still-degraded": ALERT.stillDegraded,
};
const ORDER: AlertKind[] = ["down", "degraded", "still-down", "still-degraded", "recovered"];

export function composeEmail(
  items: AlertItem[],
  opts: { now: Date; siteUrl: string; autoIncidents: boolean },
): { subject: string; text: string } {
  const groups = ORDER.map((k) => ({ k, names: items.filter((i) => i.kind === k).map((i) => i.name) })).filter(
    (g) => g.names.length,
  );
  const subject = `${ALERT.subjectPrefix} ${groups.map((g) => `${LABEL[g.k]}: ${g.names.join(", ")}`).join("; ")}`;
  const lines = groups.map((g) => `${LABEL[g.k]}: ${g.names.join(", ")}`);
  const opened = items.some((i) => i.kind === "down" || i.kind === "degraded");
  const footer = opened
    ? (opts.autoIncidents ? ALERT.footerLive : ALERT.footerDraft).replace("{site}", opts.siteUrl)
    : "";
  const text = [
    ...lines,
    "",
    `Checked at ${opts.now.toISOString().replace("T", " ").slice(0, 16)} UTC.`,
    `Status page: ${opts.siteUrl}`,
    footer,
  ]
    .filter((l, i, a) => !(l === "" && i === a.length - 1))
    .join("\n");
  return { subject, text };
}

function encodeHeader(s: string): string {
  // eslint-disable-next-line no-control-regex
  if (/^[\x20-\x7e]*$/.test(s)) return s;
  const bytes = new TextEncoder().encode(s);
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return `=?UTF-8?B?${btoa(bin)}?=`;
}

export function buildMime(from: string, to: string, subject: string, text: string, now: Date): string {
  const domain = from.split("@")[1] ?? "allternit.com";
  return [
    `From: Allternit status <${from}>`,
    `To: ${to}`,
    `Subject: ${encodeHeader(subject)}`,
    `Message-ID: <${crypto.randomUUID()}@${domain}>`,
    `Date: ${now.toUTCString()}`,
    "MIME-Version: 1.0",
    "Content-Type: text/plain; charset=utf-8",
    "Content-Transfer-Encoding: 8bit",
    "",
    text.replace(/\r?\n/g, "\r\n"),
  ].join("\r\n");
}

export interface MailEnv {
  ALERT_BINDING?: string;
  ALERT_TO?: string;
  ALERT_FROM?: string;
  [k: string]: unknown;
}

export type SendResult = "sent" | "skipped" | "failed";
export type Mailer = (subject: string, text: string, now: Date) => Promise<SendResult>;

/** Real sender: the send_email binding named by ALERT_BINDING. */
export function bindingMailer(env: MailEnv): Mailer {
  return async (subject, text, now) => {
    const binding = env[env.ALERT_BINDING || "ALERT_EMAIL"] as { send(m: unknown): Promise<unknown> } | undefined;
    const to = env.ALERT_TO || "allternitpbc@gmail.com";
    const from = env.ALERT_FROM || "status@allternit.com";
    if (!binding || typeof binding.send !== "function") {
      console.warn(`alert not sent (no send_email binding): ${subject}`);
      return "skipped";
    }
    try {
      const { EmailMessage } = await import("cloudflare:email");
      await binding.send(new EmailMessage(from, to, buildMime(from, to, subject, text, now)));
      return "sent";
    } catch (e) {
      console.error(`alert send failed: ${subject}: ${e instanceof Error ? e.message : String(e)}`);
      return "failed";
    }
  };
}
