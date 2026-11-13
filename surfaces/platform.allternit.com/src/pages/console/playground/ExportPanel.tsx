import React, { useState } from "react";
import { CodeTemplateBlock } from "@/components/console-ui";
import { cn } from "@/lib/utils";
import {
  exportModeRequest,
  KEY_PLACEHOLDER,
  type ExportLanguage,
  type PlaygroundMode,
} from "@/lib/playground-request";

const LANGS: { id: ExportLanguage; label: string; block: string }[] = [
  { id: "curl", label: "curl", block: "bash" },
  { id: "python", label: "Python", block: "python" },
  { id: "typescript", label: "TypeScript", block: "typescript" },
];

/**
 * curl / Python / TypeScript for the current request. Chat mode uses the openai
 * SDKs; Responses and Messages use allternit-platform / @allternit/platform.
 * The key is always a placeholder.
 */
export function ExportPanel({
  body,
  baseUrl,
  errors,
  mode = "chat",
}: {
  body: Record<string, unknown>;
  mode?: PlaygroundMode;
  baseUrl: string;
  errors: string[];
}): React.ReactNode {
  const [lang, setLang] = useState<ExportLanguage>("curl");
  const current = LANGS.find((l) => l.id === lang)!;
  return (
    <div className="space-y-3">
      <div className="inline-flex rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-0.5">
        {LANGS.map((l) => (
          <button key={l.id} type="button" onClick={() => setLang(l.id)}
            className={cn(
              "rounded-md px-3 py-1 text-[12px] font-semibold transition-colors",
              lang === l.id ? "bg-[var(--accent-primary)] text-[var(--ui-text-inverse)]" : "text-[var(--text-secondary)]"
            )}>
            {l.label}
          </button>
        ))}
      </div>
      {errors.map((e) => (
        <p key={e} className="m-0 text-[12px] text-[var(--status-error)]">{e}</p>
      ))}
      <CodeTemplateBlock language={current.block} code={exportModeRequest(mode, lang, body, baseUrl)} />
      <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
        Create a key on the API keys page and export it as <code className="font-mono">{KEY_PLACEHOLDER}</code>.
        The Playground key is temporary and is never included here.
      </p>
    </div>
  );
}
