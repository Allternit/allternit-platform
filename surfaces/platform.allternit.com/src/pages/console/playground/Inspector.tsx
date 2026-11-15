import React, { useState } from "react";
import { cn } from "@/lib/utils";
import { computeCostUsd, formatUsd } from "@/lib/playground-request";
import type { ChatRunState } from "./useChatRun";

type Tab = "request" | "response" | "headers";

const PRE_CLASS =
  "m-0 max-h-[360px] overflow-auto rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-3 font-mono text-[11px] leading-relaxed text-[var(--text-primary)]";

/** Request, response, headers, latency, tokens and cost for the last call. */
export function Inspector({
  state,
  promptPrice,
  completionPrice,
}: {
  state: ChatRunState;
  promptPrice?: number;
  completionPrice?: number;
}): React.ReactNode {
  const [tab, setTab] = useState<Tab>("request");
  if (!state.request) {
    return (
      <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
        Run a request to inspect it here.
      </p>
    );
  }
  const cost = computeCostUsd(state.usage, promptPrice, completionPrice);
  const tabs: Tab[] = ["request", "response", "headers"];
  const text =
    tab === "request"
      ? JSON.stringify(state.request, null, 2)
      : tab === "response"
        ? state.responseJson == null
          ? "No response body yet."
          : JSON.stringify(state.responseJson, null, 2)
        : state.responseHeaders
          ? Object.entries(state.responseHeaders).map(([k, v]) => `${k}: ${v}`).join("\n")
          : "No response headers yet.";

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap gap-x-4 gap-y-1 text-[12px] text-[var(--text-tertiary)]">
        {state.status != null && <span>status <b className="text-[var(--text-secondary)]">{state.status}</b></span>}
        {state.latencyMs != null && <span>latency <b className="text-[var(--text-secondary)]">{state.latencyMs} ms</b></span>}
        {state.requestId && (
          <span>
            x-request-id <code className="break-all font-mono text-[var(--text-secondary)]">{state.requestId}</code>
          </span>
        )}
        {state.usage?.prompt_tokens != null && <span>prompt {state.usage.prompt_tokens}</span>}
        {state.usage?.completion_tokens != null && <span>completion {state.usage.completion_tokens}</span>}
        {cost != null && <span>cost <b className="text-[var(--text-secondary)]">{formatUsd(cost)}</b></span>}
      </div>
      <div role="tablist" className="inline-flex rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-0.5">
        {tabs.map((t) => (
          <button key={t} type="button" role="tab" aria-selected={tab === t} onClick={() => setTab(t)}
            className={cn(
              "rounded-md px-3 py-1 text-[12px] font-semibold capitalize transition-colors",
              tab === t ? "bg-[var(--accent-primary)] text-[var(--ui-text-inverse)]" : "text-[var(--text-secondary)]"
            )}>
            {t}
          </button>
        ))}
      </div>
      <pre className={PRE_CLASS}>{text}</pre>
    </div>
  );
}
