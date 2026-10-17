// Offline fixtures for AxChatAppProvider adapters: a hand-built AX tree per "skin" (the labels one vendor's app exposes)
// plus a scripted app behind an AxReplayDriver that mutates on press/setValue (typing, Send, Stop, New chat, model picker,
// thread rows). STRUCTURE ONLY: nothing here was recorded from a live app; every label is the adapter's unverified guess,
// mirrored from its selector pack so a pack change that breaks resolution fails the tests.
import { build, n, type Spec } from "./build.js";
import { AxReplayDriver, type AxAction } from "./replay.js";
import { AxError, type AxNode, type AxSnapshot } from "./types.js";
import type { AxChatTurn } from "./chat-app.js";

export interface ChatSkin {
  appTitle: string;
  bundleId: string;
  /** Electron apps wrap content in an AXWebArea; native apps don't. */
  webArea: boolean;
  composer: { role: "AXTextArea" | "AXTextField"; description: string };
  send: string; stop: string; newChat: string;
  userTurn: string; assistantTurn: string;
  signIn: string;
  modelPicker: { role: "AXPopUpButton" | "AXButton"; description: string };
  modelMenu: string;
  sidebar: string;
  threadRole: "AXLink" | "AXButton" | "AXRow";
  attach: string;
}
export interface ChatScene {
  loggedOut?: boolean;
  banner?: string;
  turns?: AxChatTurn[];
  streaming?: boolean;
  composer?: string;
  model?: string;
  models?: string[];
  pickerOpen?: boolean;
  threads?: string[];
  drift?: boolean;
}

export function chatAppTree(k: ChatSkin, s: ChatScene): AxNode {
  const text = (v: string) => n("AXStaticText", { value: v });
  const btn = (title: string, role = "AXButton") => n(role, { title, actions: ["AXPress"] });
  const wrap = (kids: Spec[]) => build(n("AXApplication", { title: k.appTitle }, [n("AXWindow", { title: k.appTitle }, k.webArea ? [n("AXWebArea", { title: k.appTitle }, kids)] : kids)]));
  if (s.loggedOut) return wrap([text(`Welcome to ${k.appTitle}`), btn(k.signIn), btn("Sign up")]);
  const sidebar = n("AXGroup", { description: k.sidebar }, [btn(k.newChat), ...(s.threads ?? []).map((t) => n(k.threadRole, { title: t, actions: ["AXPress"] }, [text(t)]))]);
  const banner = s.banner ? [n("AXGroup", { subrole: "AXApplicationAlert" }, [text(s.banner)])] : [];
  const picker = n(k.modelPicker.role, { description: k.modelPicker.description, value: s.model ?? "", actions: ["AXPress"] });
  const menu = s.pickerOpen ? [n("AXMenu", { description: k.modelMenu }, (s.models ?? []).map((m) => n("AXMenuItem", { title: m, actions: ["AXPress"] })))] : [];
  // One turn = a group with one AXStaticText per paragraph line, so exactText() round-trips multi-line replies.
  const turn = (t: AxChatTurn) => n("AXGroup", { description: t.role === "user" ? k.userTurn : k.assistantTurn }, t.text.split("\n").map((line) => n("AXGroup", {}, line ? [text(line)] : [])));
  const convo = n("AXGroup", { description: "Conversation" }, (s.turns ?? []).map(turn));
  const composer = n(k.composer.role, { description: s.drift ? "composer-x9" : k.composer.description, value: s.composer ?? "" });
  const actions = s.streaming ? [btn(k.stop)] : [btn(k.send)];
  return wrap([sidebar, ...banner, picker, ...menu, convo, btn(k.attach), composer, ...actions]);
}
export const chatAppSnapshot = (k: ChatSkin, s: ChatScene): AxSnapshot => ({ formatVersion: 1, bundleId: k.bundleId, capturedAt: 0, root: chatAppTree(k, s) });

export type ChatScriptMode = "normal" | "down" | "untrusted" | "rate_limited" | "blocked" | "logged_out" | "drift";
export interface ChatScriptOptions {
  mode?: ChatScriptMode;
  reply?: (u: string) => string;
  /** Snapshots a reply takes to finish streaming. */
  frames?: number;
  models?: string[];
  threads?: Record<string, AxChatTurn[]>;
  rateBanner?: string;
  blockedBanner?: string;
  /** Bundle id the scripted app answers to (default skin.bundleId); others read as not running. */
  runningBundleId?: string;
}
/** A scripted chat app: New chat, typing, Send, streaming over `frames` snapshots, Stop, model picker, thread rows. */
export function scriptedChatApp(k: ChatSkin, o: ChatScriptOptions = {}) {
  const models = o.models ?? ["Fast", "Thinking"];
  const threads = o.threads ?? {};
  const st = {
    turns: [] as AxChatTurn[], composer: "", model: models[0], pickerOpen: false, sends: 0, newChats: 0, opened: undefined as string | undefined,
    active: false, phase: 0, full: "", visible: 0, stopped: false, presses: [] as string[],
  };
  const frames = o.frames ?? 3;
  const scene = (): ChatScene => {
    let turns = st.turns.map((t) => ({ ...t })); let streaming = false;
    if (st.active) {
      if (!st.stopped && st.phase < frames) st.phase += 1;
      streaming = !st.stopped && st.phase < frames;
      st.visible = st.stopped ? st.visible : Math.ceil((st.full.length * Math.min(st.phase, frames)) / frames);
      turns = [...turns, { role: "assistant", text: st.full.slice(0, st.visible) }];
      if (!streaming) { st.turns = turns; st.active = false; }
    }
    return {
      turns, streaming, composer: st.composer, model: st.model, models, pickerOpen: st.pickerOpen, threads: Object.keys(threads),
      loggedOut: o.mode === "logged_out", drift: o.mode === "drift",
      banner: o.mode === "rate_limited" ? (o.rateBanner ?? "You've reached your usage limit. Try again in 2 hours.") : o.mode === "blocked" ? (o.blockedBanner ?? "Unusual activity detected. Verify you are human to continue.") : undefined,
    };
  };
  const onAction = (a: AxAction) => {
    const label = a.node?.title || a.node?.description || "";
    if (a.kind === "setValue") { st.composer = a.value ?? ""; return true; }
    if (a.kind !== "press") return true;
    st.presses.push(label);
    if (label === k.send) {
      if (!st.composer.trim() || st.active) return false;
      const text = st.composer; st.composer = ""; st.sends += 1; st.turns.push({ role: "user", text });
      st.full = (o.reply ?? ((u) => `Echo: ${u}`))(text); st.phase = 0; st.visible = 0; st.active = true; st.stopped = false; return true;
    }
    if (label === k.stop) { if (!st.active || st.stopped) return false; st.stopped = true; return true; }
    if (label === k.newChat) { st.turns = []; st.active = false; st.composer = ""; st.newChats += 1; st.opened = undefined; return true; }
    if (label === k.modelPicker.description) { st.pickerOpen = !st.pickerOpen; return true; }
    if (a.node?.role === "AXMenuItem" && models.includes(label)) { st.model = label; st.pickerOpen = false; return true; }
    if (threads[label]) { st.turns = threads[label].map((t) => ({ ...t })); st.active = false; st.opened = label; return true; }
    return false;
  };
  const running = o.runningBundleId ?? k.bundleId;
  const ax = new AxReplayDriver({ bundleId: running, trusted: o.mode !== "untrusted", running: o.mode !== "down", frame: () => chatAppTree(k, scene()), onAction });
  // Multi-bundle apps: only `running` answers attach; other ids read as not running (like the real bridge).
  const attach = ax.attach.bind(ax);
  ax.attach = async (b: string) => { if (b !== running) { throw new AxError("not_running", `${b} is not running`); } return attach(b); };
  return { ax, state: st };
}
