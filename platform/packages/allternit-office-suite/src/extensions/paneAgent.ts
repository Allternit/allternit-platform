/**
 * Registry of each office app's live agent skill — the same tools its own AI
 * panel runs (systemPrompt + tools + buildContext + executeTool), bound to the
 * document that is open right now.
 *
 * A host that shows the editor beside its own chat (OfficeHost.hostChat, e.g.
 * an Allternit artifact session) reads it so its agent edits the open
 * document through the app's tools instead of guessing from chat.
 *
 * One entry per app: the app instance that registered last wins, and its
 * cleanup only clears its own entry.
 */
import type { OfficeAppKey, OfficeToolExecution } from '../bridge/types';

export interface PaneAgentToolDef {
  name: string;
  description?: string;
  inputSchema?: unknown;
}

export interface PaneAgentSkill {
  systemPrompt?: unknown;
  tools?: readonly PaneAgentToolDef[];
  buildContext?: () => string;
  executeTool?: (call: { id: string; name: string; input: Record<string, unknown> }, signal?: AbortSignal) => unknown;
  /** What the user is pointing at right now (`Slide 3 "Rain is a given."`), for anchoring a comment; null when nothing is. */
  anchor?: () => string | null;
}

const skills = new Map<OfficeAppKey, PaneAgentSkill>();

/** Register the app's live skill; returns the cleanup for its own entry. */
export function registerPaneAgent(appKey: OfficeAppKey, skill: PaneAgentSkill): () => void {
  skills.set(appKey, skill);
  return () => {
    if (skills.get(appKey) === skill) skills.delete(appKey);
  };
}

export function getPaneAgent(appKey: OfficeAppKey): PaneAgentSkill | null {
  return skills.get(appKey) ?? null;
}

/** Tool catalog of the app's skill (name, description, JSON input schema). */
export function paneAgentTools(appKey: OfficeAppKey): PaneAgentToolDef[] {
  return (getPaneAgent(appKey)?.tools ?? [])
    .filter((t) => typeof t?.name === 'string' && t.name)
    .map((t) => ({
      name: t.name,
      ...(typeof t.description === 'string' ? { description: t.description } : {}),
      ...(t.inputSchema !== undefined ? { inputSchema: t.inputSchema } : {}),
    }));
}

/** The skill's guide (its system prompt) — how the tools are meant to be used. */
export function paneAgentGuide(appKey: OfficeAppKey): string {
  const prompt = getPaneAgent(appKey)?.systemPrompt;
  return typeof prompt === 'string' ? prompt : '';
}

/** What the skill feeds its own model each turn: the open document's context. */
export function readPaneAgentContext(appKey: OfficeAppKey): string {
  const fn = getPaneAgent(appKey)?.buildContext;
  if (typeof fn !== 'function') return '';
  const value = fn.call(getPaneAgent(appKey));
  return typeof value === 'string' ? value : '';
}

/** What the user has selected in the app (see PaneAgentSkill.anchor), or null. */
export function readPaneAgentAnchor(appKey: OfficeAppKey): string | null {
  const fn = getPaneAgent(appKey)?.anchor;
  if (typeof fn !== 'function') return null;
  try {
    const value = fn.call(getPaneAgent(appKey));
    return typeof value === 'string' && value.trim() ? value.trim() : null;
  } catch {
    return null;
  }
}

const persisters = new Map<OfficeAppKey, () => Promise<unknown>>();

/**
 * Register how the app saves its document now (its own save, skipped when
 * nothing changed). Office autosave is off by default, so a host agent's
 * edits are persisted through this after each change instead of waiting for
 * the user to press Save.
 */
export function registerPaneAgentPersist(appKey: OfficeAppKey, persist: () => Promise<unknown>): () => void {
  persisters.set(appKey, persist);
  return () => {
    if (persisters.get(appKey) === persist) persisters.delete(appKey);
  };
}

/** Save the app's document if it has unsaved changes. Resolves false when it can't. */
export async function persistPaneAgent(appKey: OfficeAppKey): Promise<boolean> {
  const persist = persisters.get(appKey);
  if (!persist) return false;
  try {
    return (await persist()) !== false;
  } catch {
    return false;
  }
}

let callSeq = 0;

/** Who is running the current host-agent tool call, per app (set by runPaneAgentTool for its duration). */
const callAuthors = new Map<OfficeAppKey, string>();

/**
 * The agent whose tool call is running against the app right now, or null
 * (the app's own AI panel, or nothing running). The Docs skill reads it as
 * its `getTrack`, so a host agent's edits land as tracked changes under its
 * name and the editor's own accept/reject UI reviews them.
 */
export function paneAgentAuthor(appKey: OfficeAppKey): string | null {
  return callAuthors.get(appKey) ?? null;
}

/** Run one of the app's tools against the open document. `author`: the agent making the call (tracked changes). */
export async function runPaneAgentTool(
  appKey: OfficeAppKey,
  name: string,
  input: Record<string, unknown>,
  signal?: AbortSignal,
  author?: string,
): Promise<OfficeToolExecution> {
  const skill = getPaneAgent(appKey);
  if (!skill || typeof skill.executeTool !== 'function') {
    return { isError: true, summary: name, output: 'The editor is not open.' };
  }
  if (!paneAgentTools(appKey).some((t) => t.name === name)) {
    return { isError: true, summary: name, output: `Unknown tool: ${name}` };
  }
  if (author) callAuthors.set(appKey, author);
  try {
    callSeq += 1;
    const raw = (await skill.executeTool.call(skill, { id: `pane-${callSeq}`, name, input }, signal)) as
      | Partial<OfficeToolExecution>
      | null
      | undefined;
    return {
      output: raw?.output === undefined || raw?.output === null ? '' : String(raw.output),
      isError: Boolean(raw?.isError),
      summary: typeof raw?.summary === 'string' && raw.summary ? raw.summary : name,
      mutated: Boolean(raw?.mutated),
    };
  } catch (error) {
    return { isError: true, summary: name, output: error instanceof Error ? error.message : String(error) };
  } finally {
    if (author && callAuthors.get(appKey) === author) callAuthors.delete(appKey);
  }
}
