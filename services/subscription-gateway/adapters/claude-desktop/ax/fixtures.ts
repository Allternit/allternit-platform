// Hand-built Claude Desktop AX trees (STRUCTURE ONLY, placeholder text, nothing recorded from a live app) plus a scripted
// AX "app" that mutates state on press/setValue so the provider + runConformance can run through the AX transport offline.
import { AxReplayDriver, build, n, type AxAction, type AxNode, type AxSnapshot } from "../../_shared/ax/index.js";
import { CLAUDE_BUNDLE_ID } from "./selectors.js";
import type { Scenario } from "../fixtures/markup.js";

export function claudeAxTree(s: Scenario): AxNode {
  const text = (v: string) => n("AXStaticText", { value: v });
  const web = (kids: ReturnType<typeof n>[]) => build(n("AXApplication", { title: "Claude" }, [n("AXWindow", { title: "Claude" }, [n("AXWebArea", { title: "Claude" }, kids)])]));
  if (s.loggedOut) return web([text("Talk with Claude"), n("AXButton", { title: "Continue with Google", actions: ["AXPress"] }), n("AXButton", { title: "Continue with email", actions: ["AXPress"] })]);
  const btn = (title: string) => n("AXButton", { title, actions: ["AXPress"] });
  const side = n("AXGroup", { description: "Sidebar" }, [
    btn("New chat"),
    n("AXRadioButton", { title: "Chat", value: s.cowork ? "0" : "1", actions: ["AXPress"] }),
    n("AXRadioButton", { title: "Cowork", value: s.cowork ? "1" : "0", actions: ["AXPress"] }),
    ...(s.cowork ? [btn("New task")] : []),
  ]);
  const turns = (s.turns ?? []).map((t) => n("AXGroup", { description: t.role === "user" ? "Your message" : "Claude response" }, [text(t.text)]));
  const convo = n("AXGroup", { description: "Conversation" }, [
    ...turns,
    ...(s.tool ? [n("AXGroup", { description: "Tool use" }, [text(s.tool)])] : []),
    ...(s.artifact ? [n("AXGroup", { description: "Artifact" }, [text(s.artifact)])] : []),
    ...(s.approval ? [n("AXGroup", { description: "Permission request" }, [text(`Allow Claude to use ${s.approval}?`), btn("Deny"), btn("Allow once"), btn("Always allow")])] : []),
  ]);
  const banner = s.banner ? [n("AXGroup", { subrole: "AXApplicationAlert" }, [text(s.banner)])] : [];
  const composer = n("AXTextArea", { description: s.drift === "composer" ? "composer-x9" : "Write your prompt to Claude", value: s.composerText ?? "", actions: [] });
  return web([side, ...banner, convo, composer, ...(s.streaming ? [btn("Stop response")] : s.composerText ? [btn("Send message")] : [])]);
}

export function claudeAxSnapshot(s: Scenario): AxSnapshot {
  return { formatVersion: 1, bundleId: CLAUDE_BUNDLE_ID, capturedAt: 0, root: claudeAxTree(s) };
}

export type AxScriptMode = "normal" | "down" | "untrusted" | "rate_limited" | "logged_out" | "drift";
export interface AxScriptOptions { mode?: AxScriptMode; reply?: (u: string) => string; frames?: number; approval?: string }
/** A scripted Claude "app" behind an AxReplayDriver: typing, Send, Stop, New chat, Cowork, approval buttons all mutate state. */
export function scriptedClaudeAx(o: AxScriptOptions = {}) {
  const st = { turns: [] as Scenario["turns"] & {}, composer: "", cowork: false, newTasks: 0, sends: 0, approval: o.approval, resolution: undefined as string | undefined, phase: 0, full: "", active: false, stopped: false, visible: 0 };
  const frames = o.frames ?? 3;
  const scenario = (): Scenario => {
    let turns = st.turns.map((t) => ({ ...t })); let streaming = false;
    if (st.active) {
      if (!st.stopped && st.phase < frames) st.phase += 1;
      streaming = !st.stopped && st.phase < frames;
      st.visible = st.stopped ? st.visible : Math.ceil((st.full.length * Math.min(st.phase, frames)) / frames);
      turns = [...turns, { role: "assistant", text: st.full.slice(0, st.visible) }];
      if (!streaming) { st.turns = turns; st.active = false; }
    }
    const s: Scenario = { turns, streaming, composerText: st.composer, cowork: st.cowork, approval: st.approval, drift: o.mode === "drift" ? "composer" : undefined };
    if (o.mode === "rate_limited") s.banner = "You have reached your usage limit. Your limit resets in 2 hours";
    if (o.mode === "logged_out") s.loggedOut = true;
    return s;
  };
  const onAction = (a: AxAction) => {
    const label = a.node?.title ?? a.node?.description ?? "";
    if (a.kind === "setValue") { st.composer = a.value ?? ""; return true; }
    if (a.kind !== "press") return true;
    if (/^send message$/i.test(label)) {
      if (!st.composer.trim() || st.active) return false;
      const text = st.composer; st.composer = ""; st.sends += 1; st.turns.push({ role: "user", text });
      st.full = (o.reply ?? ((u) => `Echo: ${u}`))(text); st.phase = 0; st.visible = 0; st.active = true; st.stopped = false; return true;
    }
    if (/^stop response$/i.test(label)) { if (!st.active || st.stopped) return false; st.stopped = true; return true; }
    if (/^new chat$/i.test(label)) { st.turns = []; st.active = false; st.composer = ""; return true; }
    if (/^cowork$/i.test(label)) { st.cowork = true; return true; }
    if (/^new task$/i.test(label)) { st.newTasks += 1; st.turns = []; st.active = false; return true; }
    if (/^(allow once|deny)$/i.test(label)) { st.resolution = /deny/i.test(label) ? "denied" : "approved"; st.approval = undefined; return true; }
    return false;
  };
  const ax = new AxReplayDriver({ bundleId: CLAUDE_BUNDLE_ID, trusted: o.mode !== "untrusted", running: o.mode !== "down", frame: () => claudeAxTree(scenario()), onAction });
  return { ax, state: st };
}
