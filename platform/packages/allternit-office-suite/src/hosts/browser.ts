import type {
  OfficeAiClient,
  OfficeExtensionDescriptor,
  OfficeHost,
  OpenedFile,
  OpenOptions,
  RecentFile,
} from '../bridge/types';
import {
  getOfficeModelLabel,
  getOfficeModelOptions,
  refreshOfficeModelOptions,
  resolveOfficeModelId,
  setOfficeModelOverride,
  OfficeAgentLoop,
} from '../ai';

export interface BrowserHostOptions {
  /** Override save behavior. Defaults to a file download. */
  saveFile?: (bytes: Uint8Array, name: string) => void | Promise<void>;
  /** Override file open behavior. Defaults to an `<input type="file">` picker. */
  openFile?: (options?: OpenOptions) => Promise<OpenedFile | OpenedFile[] | null>;
  /** Override recent files. Defaults to an empty list. */
  getRecentFiles?: () => Promise<RecentFile[]>;
  /** Override the reported locale. Defaults to `'en'`. */
  getLanguage?: () => string;
  /** Override the default AI client. Defaults to the platform-style office-ai layer. */
  ai?: OfficeAiClient;
  /** Extensions that occupy the per-app AI chat section (e.g. the Allternit Office Agent). */
  extensions?: OfficeExtensionDescriptor[];
  /** The host's chat beside the editor; replaces every app's AI section. */
  hostChat?: OfficeHost['hostChat'];
  designSystem?: OfficeHost['designSystem'];
}

function buildAcceptString(accept?: Record<string, string[]>): string {
  if (!accept) return '';
  return Object.entries(accept)
    .flatMap(([mime, exts]) => [mime, ...exts])
    .join(',');
}

function pickFiles(options?: OpenOptions): Promise<OpenedFile | OpenedFile[] | null> {
  return new Promise((resolve) => {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = buildAcceptString(options?.accept);
    if (options?.multiple) input.multiple = true;

    let resolved = false;
    const finish = (value: OpenedFile | OpenedFile[] | null) => {
      if (resolved) return;
      resolved = true;
      resolve(value);
    };

    input.onchange = async () => {
      const files = Array.from(input.files ?? []);
      if (files.length === 0) return finish(null);
      const opened = await Promise.all(
        files.map(async (file) => ({
          name: file.name,
          bytes: new Uint8Array(await file.arrayBuffer()),
        })),
      );
      finish(options?.multiple ? opened : opened[0] ?? null);
    };

    // Fallback for cancel / Esc when onchange does not fire.
    window.addEventListener('focus', () => setTimeout(() => finish(null), 400), { once: true });
    input.click();
  });
}

function downloadBytes(bytes: Uint8Array, name: string): void {
  const blob = new Blob([bytes as unknown as BlobPart], { type: 'application/octet-stream' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.click();
  URL.revokeObjectURL(url);
}

function createBrowserAi(): OfficeAiClient {
  return {
    resolveModelId: resolveOfficeModelId,
    setModelOverride: setOfficeModelOverride,
    getModelOptions: getOfficeModelOptions,
    refreshModelOptions: refreshOfficeModelOptions,
    getModelLabel: getOfficeModelLabel,
    AgentLoop: OfficeAgentLoop as OfficeAiClient['AgentLoop'],
  };
}

/**
 * Create a browser-based `OfficeHost`.
 *
 * This is the default host for standalone web surfaces. It downloads files on
 * save and uses a native file picker on open. Platform surfaces should override
 * `saveFile` (and optionally `openFile`) with their own persistence layer.
 */
export function createBrowserHost(options: BrowserHostOptions = {}): OfficeHost {
  return {
    getLanguage: () => options.getLanguage?.() ?? 'en',
    openFile: options.openFile ?? pickFiles,
    saveFile: async (bytes: Uint8Array, name: string) => {
      await (options.saveFile ?? downloadBytes)(bytes, name);
    },
    getRecentFiles: options.getRecentFiles ?? (async () => []),
    ai: options.ai ?? createBrowserAi(),
    ...(options.extensions ? { extensions: options.extensions } : {}),
    ...(options.hostChat ? { hostChat: options.hostChat } : {}),
    ...(options.designSystem ? { designSystem: options.designSystem } : {}),
  };
}
