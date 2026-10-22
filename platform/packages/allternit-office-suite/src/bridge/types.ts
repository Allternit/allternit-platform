/**
 * Host contract for the Allternit Office Suite.
 *
 * The suite components are intentionally platform-agnostic. The embedding host
 * (the platform surface, a standalone web app, an Electron wrapper, etc.)
 * implements this contract and passes it via `OfficeHostProvider`.
 */
import type { ReactNode } from 'react';

export interface RecentFile {
  id: string;
  name: string;
  path?: string;
  openedAt: number;
  size?: number;
}

export interface OpenOptions {
  accept?: Record<string, string[]>;
  multiple?: boolean;
}

export interface OpenedFile {
  id?: string;
  name: string;
  bytes: Uint8Array;
}

export interface SaveOptions {
  suggestedName?: string;
  prompt?: boolean;
}

export interface OfficeMessage {
  role: 'user' | 'assistant' | 'system';
  content: string;
}

export interface OfficeModelInfo {
  id: string;
  name: string;
  provider?: string;
}

export type OfficeAppKey = 'docs' | 'sheets' | 'slides' | 'pdf';

export interface OfficeModelOption {
  id: string;
  /** provider/model runtime id */
  runtimeId: string;
  label: string;
  /** provider name, e.g. "Kimi" or "OpenAI" */
  provider?: string | undefined;
}

export interface OfficeToolExecution {
  output?: string | undefined;
  isError?: boolean | undefined;
  summary: string;
  mutated?: boolean | undefined;
  display?: { [key: string]: unknown } | undefined;
}

export interface OfficeAgentLoopEvents {
  onText?: ((text: string) => void) | undefined;
  onToolStart?: ((call: { name: string; input?: unknown }) => void) | undefined;
  onToolExecuted?:
    | ((payload: {
        call: { name: string; input?: unknown };
        execution: OfficeToolExecution;
      }) => void)
    | undefined;
  onDone?:
    | ((result: { text: string; cancelled: boolean; turnLimit: boolean }) => void)
    | undefined;
  onTurnEnd?: ((payload?: unknown) => void) | undefined;
  onError?: ((error: string) => void) | undefined;
}

export interface OfficeAgentLoopOptions {
  events?: OfficeAgentLoopEvents | undefined;
  skill?: unknown | undefined;
  systemSuffix?: string | (() => string) | undefined;
  transport?: unknown | undefined;
  modelId?: string | undefined;
  maxTurns?: number | undefined;
  [key: string]: unknown;
}

export interface OfficeAgentLoop {
  busy: boolean;
  run(instruction: string, images?: unknown[]): void;
  cancel(): void;
  setModelId(modelId: string | undefined): void;
  reset(): void;
  restore(messages: readonly { role: string; text: string }[]): void;
}

export interface OfficeAgentLoopConstructor {
  new (options: OfficeAgentLoopOptions): OfficeAgentLoop;
}

/**
 * AI services supplied by the host. The vendored office apps consume this
 * contract for model selection and the agent loop instead of importing
 * `@allternit/office-ai` directly.
 */
export interface OfficeAiClient {
  /** Resolve the effective runtime model id for an office app. */
  resolveModelId: (appKey: OfficeAppKey) => string | undefined;
  /** Persist a per-app model override. */
  setModelOverride: (appKey: OfficeAppKey, modelId: string | undefined) => void;
  /** Return the current model picker options. */
  getModelOptions: () => OfficeModelOption[];
  /** Refresh picker options from the host's model catalog. */
  refreshModelOptions: () => Promise<OfficeModelOption[]>;
  /** Human-readable label for a picker value. */
  getModelLabel: (value?: string) => string;
  /** Agent loop constructor used by the office apps. */
  AgentLoop: OfficeAgentLoopConstructor;

  /** Optional custom chat stream for hosts that want to own the wire protocol. */
  stream?: ((messages: OfficeMessage[], abortSignal?: AbortSignal) => AsyncIterable<string>) | undefined;
  /** Optional model catalog for custom chat streams. */
  getModels?: (() => Promise<OfficeModelInfo[]>) | undefined;
}

/**
 * Optional storage abstraction for model selection and other persisted UI state.
 */
export interface OfficeStorageProvider {
  getItem: (key: string) => string | null | Promise<string | null>;
  setItem: (key: string, value: string) => void | Promise<void>;
  removeItem: (key: string) => void | Promise<void>;
}

/**
 * Handle returned by a sheets engine session. Opaque outside the host.
 */
export interface XlsxSessionHandle {
  readonly sessionId: string;
}

/**
 * Optional engine host for full Sheets recalc/session management.
 * If omitted, the suite falls back to the simpler `office-sheets-editor`
 * path (read-only / client-side editing without server recalc).
 */
export interface XlsxEngineHost {
  openSession: (bytes: Uint8Array) => Promise<XlsxSessionHandle>;
  readRange: (session: XlsxSessionHandle, sheet: number, row: number, col: number) => Promise<unknown>;
  recalc: (session: XlsxSessionHandle) => Promise<Uint8Array>;
  save: (session: XlsxSessionHandle) => Promise<Uint8Array>;
  close: (session: XlsxSessionHandle) => Promise<void>;
}

/**
 * Context handed to an extension when it renders inside an app's AI slot.
 */
export interface OfficeExtensionContext {
  /** The office app whose AI chat section the extension occupies. */
  appKey: OfficeAppKey;
  /** The embedding host (same instance the app adapters consume). */
  host: OfficeHost;
  /** Optional: hide/close the surrounding AI section, when the host surface supports it. */
  close?: () => void;
}

/**
 * Describes an extension that occupies the per-app AI chat section. The host
 * registers extensions on `OfficeHost.extensions`; `OfficeAiSlot` renders them
 * as the app's only chat panes (with no registered extensions the app's
 * built-in AI panel renders unchanged).
 */
export interface OfficeExtensionDescriptor {
  /** Stable unique id, e.g. 'allternit-assistant'. */
  id: string;
  /** Human-readable name shown on the extension's tab. */
  name: string;
  /** Small brand glyph rendered on the tab (emoji, short text, or element). */
  icon?: ReactNode;
  /** Render the extension panel. Called inside the slot's React tree. */
  render: (ctx: OfficeExtensionContext) => ReactNode;
}

/**
 * Single host contract consumed by all Allternit Office Suite apps.
 */
export interface OfficeHost {
  /** Read the user's current locale, e.g. 'en'. */
  getLanguage: () => string;

  /** Open one or more files from the host's file system. */
  openFile: (options?: OpenOptions) => Promise<OpenedFile | OpenedFile[] | null>;

  /** Save bytes to the host's file system. */
  saveFile: (bytes: Uint8Array, name: string, options?: SaveOptions) => Promise<void>;

  /** List recently opened files for the host surface. */
  getRecentFiles: () => Promise<RecentFile[]>;

  /** Optional: print the current document. */
  print?: () => void;

  /** Optional: pick an image from the host file system. */
  pickImage?: () => Promise<Uint8Array | null>;

  /** Optional: AI chat client. If absent, AI panels render a disabled/empty state. */
  ai?: OfficeAiClient;

  /** Optional: persisted storage for model selection, panel widths, etc. */
  storage?: OfficeStorageProvider;

  /** Optional: full Sheets engine. If absent, Sheets uses the lightweight editor path. */
  xlsxEngine?: XlsxEngineHost;

  /** Optional: extensions that occupy the per-app AI chat section. When absent
   * or empty, every app renders its built-in AI panel exactly as before. */
  extensions?: OfficeExtensionDescriptor[];

  /**
   * Optional: the host's own chat beside the editor (e.g. an Allternit
   * artifact session). When set, no app renders an AI chat section at all;
   * "open AI" actions call `open`, and ribbon/context-menu AI presets reach
   * the host through the assistant preset event (`onAssistantPreset`).
   */
  hostChat?: OfficeHostChat;

  /**
   * Optional: the design system the document is made with. Editors offer its
   * palette (the colour popover's Theme tab) and fonts first.
   */
  designSystem?: OfficeDesignSystem;
}

export interface OfficeDesignSystem {
  name: string;
  /** Named colours, e.g. { name: 'talv-ember', value: '#E86A2B' }. */
  colors: Array<{ name: string; value: string }>;
  /** Font families, most important first. */
  fonts: string[];
}

export interface OfficeHostChat {
  /** An app asked to show its AI chat — show/focus the host's chat instead. */
  open: (appKey: OfficeAppKey) => void;
  /** Open the host's comment box, anchored to what's selected in the editor. */
  openComments?: () => void;
}
