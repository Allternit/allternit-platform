// Hand-built ChatGPT.app AX trees (structure only, placeholder text) + a small scripted app for offline tests.
import { AxReplayDriver, build, n, type AxAction, type AxNode, type AxSnapshot } from "../../_shared/ax/index.js";
import { CHATGPT_APP_BUNDLE_ID } from "./selectors.js";
import type { Scenario } from "../fixtures/markup.js";

export function chatgptAppAxTree(s: Scenario): AxNode {
  const text = (v: string) => n("AXStaticText", { value: v });
  const btn = (title: string) => n("AXButton", { title, actions: ["AXPress"] });
  const win = (kids: ReturnType<typeof n>[]) => build(n("AXApplication", { title: "ChatGPT" }, [n("AXWindow", { title: "ChatGPT" }, kids)]));
  if (s.loggedOut) return win([text("Get started"), btn("Log in")]);
  const nav = n("AXGroup", { description: "Sidebar" }, [btn("Dots")]);
  const banner = s.banner ? [n("AXGroup", { subrole: "AXApplicationAlert" }, [text(s.banner)])] : [];
  if ((s.view ?? "dot") === "list") {
    return win([nav, ...banner, n("AXList", { description: "Dots" }, (s.dots ?? []).map((d) => n("AXButton", { title: d.name, actions: ["AXPress"] }, [text(d.name), ...(d.handle ? [text(d.handle)] : [])])))]);
  }
  const dot = s.dot ?? { id: "nova-dot", name: "Nova", handle: "@nova-dot" };
  const kids = [
    nav, ...banner,
    n("AXGroup", { description: "Dot header" }, [n("AXHeading", { value: dot.name }), ...(dot.handle ? [text(dot.handle)] : [])]),
    ...(s.activity ? [n("AXGroup", { description: "Activity" }, [text(s.activity)])] : []),
    n("AXGroup", { description: "Thread" }, [
      ...(s.turns ?? []).map((t) => n("AXGroup", { description: t.role === "user" ? "You said" : "ChatGPT said" }, [text(t.text)])),
      ...(s.confirmation ? [n("AXGroup", { description: s.confirmation.policy === "hand_off" ? "Hand off" : "Ask first" }, [text(s.confirmation.text), ...(s.confirmation.policy === "hand_off" ? [btn("Open computer")] : [btn("Approve"), btn("Deny")])])] : []),
    ]),
    ...(s.showTasks || s.tasks ? [n("AXGroup", { description: "Tasks" }, [["in_progress", "In progress"], ["scheduled", "Scheduled"], ["completed", "Completed"]].flatMap(([k, label]) => [n("AXHeading", { value: label }), ...(s.tasks ?? []).filter((t) => t.state === k).map((t) => text(t.title))]))] : []),
    n("AXTextArea", { description: s.drift === "composer" ? "composer-x9" : "Ask ChatGPT", value: s.composerText ?? "" }),
    ...(s.streaming ? [btn("Stop")] : s.composerText ? [btn("Send")] : []),
  ];
  return win(kids);
}
export const chatgptAppAxSnapshot = (s: Scenario): AxSnapshot => ({ formatVersion: 1, bundleId: CHATGPT_APP_BUNDLE_ID, capturedAt: 0, root: chatgptAppAxTree(s) });

/** Scripted app: dots list -> open a dot -> type/send/stop/approve. */
export function scriptedChatGptApp(o: { mode?: "normal" | "untrusted" | "down" | "logged_out" | "drift" | "rate_limited" | "blocked"; reply?: (u: string) => string; confirmation?: Scenario["confirmation"]; frames?: number } = {}) {
  const dots = [{ id: "nova-dot", name: "Nova", handle: "@nova-dot" }];
  const st = { view: "list" as "list" | "dot", composer: "", turns: [] as NonNullable<Scenario["turns"]>, active: false, phase: 0, full: "", stopped: false, visible: 0, sends: 0, confirmation: o.confirmation, resolution: undefined as string | undefined };
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
    return { view: st.view, dots, dot: dots[0], turns, streaming, composerText: st.composer, confirmation: st.confirmation, loggedOut: o.mode === "logged_out", drift: o.mode === "drift" ? "composer" : undefined, banner: o.mode === "rate_limited" ? "You've reached your usage limit. Your quota resets at 14:00." : o.mode === "blocked" ? "Unusual activity detected. Verify you are human to continue" : undefined };
  };
  const onAction = (a: AxAction) => {
    const label = a.node?.title ?? a.node?.description ?? "";
    if (a.kind === "setValue") { st.composer = a.value ?? ""; return true; }
    if (a.kind !== "press") return true;
    if (/^dots$/i.test(label)) { st.view = "list"; return true; }
    if (/^nova$/i.test(label)) { st.view = "dot"; st.composer = ""; return true; }
    if (/^send$/i.test(label)) {
      if (!st.composer.trim() || st.active) return false;
      const text = st.composer; st.composer = ""; st.sends += 1; st.turns.push({ role: "user", text });
      st.full = (o.reply ?? ((u) => `Echo: ${u}`))(text); st.phase = 0; st.visible = 0; st.active = true; st.stopped = false; return true;
    }
    if (/^stop$/i.test(label)) { if (!st.active || st.stopped) return false; st.stopped = true; return true; }
    if (/^(approve|deny)$/i.test(label)) { st.resolution = /deny/i.test(label) ? "denied" : "approved"; st.confirmation = undefined; return true; }
    return false;
  };
  const ax = new AxReplayDriver({ bundleId: CHATGPT_APP_BUNDLE_ID, trusted: o.mode !== "untrusted", running: o.mode !== "down", frame: () => chatgptAppAxTree(scenario()), onAction });
  return { ax, state: st };
}
