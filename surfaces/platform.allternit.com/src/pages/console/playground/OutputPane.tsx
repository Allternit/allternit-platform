import React, { useEffect, useState } from "react";
import { cn } from "@/lib/utils";
import type { PlaygroundMode } from "@/lib/playground-request";
import type { ChatRunState } from "./useChatRun";

const FINISH_LABEL: Record<PlaygroundMode, string> = {
  chat: "finish reason",
  responses: "status",
  messages: "stop reason",
};

const OUTPUT_CLASS =
  "min-h-[160px] whitespace-pre-wrap rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-3 font-mono text-[12px] leading-relaxed text-[var(--text-primary)]";

interface OutputPaneProps {
  title: string;
  state: ChatRunState;
  /** Shown while idle to explain what will happen on Run. */
  idleCaption: string;
  /** Provide to let the user answer tool calls and continue the conversation. */
  onContinueWithToolResults?: (results: Record<string, string>) => void;
}

/** Streaming completion output for one playground lane. */
export function OutputPane({
  title,
  state,
  idleCaption,
  onContinueWithToolResults,
}: OutputPaneProps): React.ReactNode {
  const [results, setResults] = useState<Record<string, string>>({});
  const callKey = state.toolCalls.map((t) => t.id).join(",");
  useEffect(() => setResults({}), [callKey]);
  const showTools = state.toolCalls.length > 0 && !state.running;
  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between">
        <span className="text-[12px] font-semibold text-[var(--text-secondary)]">
          {title}
        </span>
        {state.running && (
          <span className="text-[11px] text-[var(--text-tertiary)]">
            Streaming…
          </span>
        )}
      </div>

      <div className={cn(OUTPUT_CLASS, state.running && "border-[var(--accent-primary)]/40")}>
        {state.text || (state.running ? "" : idleCaption)}
        {state.running && <span className="animate-pulse">▍</span>}
      </div>

      {showTools && (
        <div className="space-y-2 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] p-3">
          <span className="text-[12px] font-semibold text-[var(--text-secondary)]">
            Tool calls ({state.finishReason ?? "tool_calls"})
          </span>
          {state.toolCalls.map((t) => (
            <div key={t.id} className="space-y-1">
              <code className="block break-all font-mono text-[12px] text-[var(--text-primary)]">
                {t.function.name}({t.function.arguments})
              </code>
              {onContinueWithToolResults && (
                <textarea
                  aria-label={`Result for ${t.function.name}`}
                  rows={2}
                  value={results[t.id] ?? ""}
                  onChange={(e) => setResults((r) => ({ ...r, [t.id]: e.target.value }))}
                  placeholder="Tool result (text or JSON)"
                  className="w-full resize-y rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 font-mono text-[12px] text-[var(--text-primary)] outline-none"
                />
              )}
            </div>
          ))}
          {onContinueWithToolResults && (
            <button
              type="button"
              disabled={state.toolCalls.some((t) => !(results[t.id] ?? "").trim())}
              onClick={() => onContinueWithToolResults(results)}
              className="rounded-lg bg-[var(--accent-primary)] px-3 py-1.5 text-[12px] font-semibold text-[var(--ui-text-inverse)] disabled:opacity-50"
            >
              Send results and continue
            </button>
          )}
        </div>
      )}

      {state.error && (
        <p className="m-0 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[12px] text-[var(--status-error)]">
          {state.error}
        </p>
      )}
      {state.stopped && !state.error && (
        <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
          Stopped — showing partial output.
        </p>
      )}

      {(state.usage || state.modelUsed || state.finishReason) && !state.running && (
        <div className="flex flex-wrap gap-3 text-[11px] text-[var(--text-tertiary)]">
          {state.modelUsed && (
            <span>
              model:{" "}
              <code className="font-mono text-[var(--text-secondary)]">
                {state.modelUsed}
              </code>
            </span>
          )}
          {state.usage?.prompt_tokens !== undefined && (
            <span>prompt tokens: {state.usage.prompt_tokens}</span>
          )}
          {state.usage?.completion_tokens !== undefined && (
            <span>completion tokens: {state.usage.completion_tokens}</span>
          )}
          {state.usage?.total_tokens !== undefined && (
            <span>total tokens: {state.usage.total_tokens}</span>
          )}
          {state.finishReason && (
            <span>
              {FINISH_LABEL[state.mode]}:{" "}
              <code className="font-mono text-[var(--text-secondary)]">
                {state.finishReason}
                {state.incompleteReason ? ` (${state.incompleteReason})` : ""}
              </code>
            </span>
          )}
        </div>
      )}
    </div>
  );
}
