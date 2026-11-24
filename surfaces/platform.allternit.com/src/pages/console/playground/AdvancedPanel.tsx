import React, { useMemo } from "react";
import { cn } from "@/lib/utils";
import { SETTINGS_SELECT_CLASS } from "@/components/console-ui";
import { toolNames, type PlaygroundParams, type ResponseFormatKind } from "@/lib/playground-request";

const INPUT_CLASS =
  "w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]";
const AREA_CLASS = cn(INPUT_CLASS, "resize-y font-mono text-[12px]");
const LABEL_CLASS = "text-[12px] font-semibold text-[var(--text-secondary)]";

const SAMPLE_TOOLS = JSON.stringify(
  [
    {
      type: "function",
      function: {
        name: "get_weather",
        description: "Get the current weather for a city.",
        parameters: {
          type: "object",
          properties: { city: { type: "string" } },
          required: ["city"],
        },
      },
    },
  ],
  null,
  2
);

function NumberField({
  id,
  label,
  value,
  onChange,
  min,
  max,
  step,
  placeholder = "Default",
}: {
  id: string;
  label: string;
  value: number | null;
  onChange: (v: number | null) => void;
  min?: number;
  max?: number;
  step?: number;
  placeholder?: string;
}): React.ReactNode {
  return (
    <div className="space-y-1">
      <label htmlFor={id} className={LABEL_CLASS}>
        {label}
      </label>
      <input
        id={id}
        type="number"
        min={min}
        max={max}
        step={step}
        placeholder={placeholder}
        value={value ?? ""}
        onChange={(e) => {
          const n = e.target.value === "" ? null : Number(e.target.value);
          onChange(n != null && Number.isFinite(n) ? n : null);
        }}
        className={INPUT_CLASS}
      />
    </div>
  );
}

function jsonError(text: string): string | null {
  if (!text.trim()) return null;
  try {
    JSON.parse(text);
    return null;
  } catch (err) {
    return err instanceof Error ? err.message : "Invalid JSON";
  }
}

interface AdvancedPanelProps {
  params: PlaygroundParams;
  onChange: (patch: Partial<PlaygroundParams>) => void;
  supportsTools: boolean;
  idPrefix: string;
}

/** Sampling params, tools editor and response format for one request. */
export function AdvancedPanel({
  params,
  onChange,
  supportsTools,
  idPrefix,
}: AdvancedPanelProps): React.ReactNode {
  const names = useMemo(() => toolNames(params.toolsJson), [params.toolsJson]);
  const toolsErr = jsonError(params.toolsJson);
  const schemaErr = params.responseFormat === "json_schema" ? jsonError(params.schemaJson) : null;

  return (
    <div className="space-y-5">
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
        <NumberField id={`${idPrefix}-temp`} label="Temperature" min={0} max={2} step={0.1}
          value={params.temperature} onChange={(v) => onChange({ temperature: v ?? 0 })} />
        <NumberField id={`${idPrefix}-top-p`} label="Top p" min={0} max={1} step={0.05}
          value={params.topP} onChange={(v) => onChange({ topP: v })} />
        <NumberField id={`${idPrefix}-max-tokens`} label="Max tokens" min={1} step={1}
          value={params.maxTokens} onChange={(v) => onChange({ maxTokens: Math.max(1, Math.round(v ?? 1)) })} />
        <NumberField id={`${idPrefix}-presence`} label="Presence penalty" min={-2} max={2} step={0.1}
          value={params.presencePenalty} onChange={(v) => onChange({ presencePenalty: v })} />
        <NumberField id={`${idPrefix}-frequency`} label="Frequency penalty" min={-2} max={2} step={0.1}
          value={params.frequencyPenalty} onChange={(v) => onChange({ frequencyPenalty: v })} />
        <NumberField id={`${idPrefix}-seed`} label="Seed" step={1} placeholder="Random"
          value={params.seed} onChange={(v) => onChange({ seed: v == null ? null : Math.round(v) })} />
      </div>

      <div className="space-y-1">
        <label htmlFor={`${idPrefix}-stop`} className={LABEL_CLASS}>
          Stop sequences (one per line)
        </label>
        <textarea id={`${idPrefix}-stop`} rows={2} value={params.stop}
          onChange={(e) => onChange({ stop: e.target.value })} className={AREA_CLASS} />
      </div>

      <label className="flex items-center gap-2 text-[13px] text-[var(--text-primary)]">
        <input type="checkbox" checked={params.stream}
          onChange={(e) => onChange({ stream: e.target.checked })} />
        Stream the response
      </label>

      <div className="space-y-2">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <label htmlFor={`${idPrefix}-tools`} className={LABEL_CLASS}>
            Tools (JSON array of function definitions)
          </label>
          <button type="button" className="text-[12px] font-semibold text-[var(--accent-primary)]"
            onClick={() => onChange({ toolsJson: SAMPLE_TOOLS })}>
            Insert example
          </button>
        </div>
        <textarea id={`${idPrefix}-tools`} rows={6} value={params.toolsJson} spellCheck={false}
          placeholder='[{"type":"function","function":{"name":"…","parameters":{}}}]'
          onChange={(e) => onChange({ toolsJson: e.target.value })} className={AREA_CLASS} />
        {toolsErr && <p className="m-0 text-[12px] text-[var(--status-error)]">Not valid JSON: {toolsErr}</p>}
        {params.toolsJson.trim() && !supportsTools && (
          <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
            The selected model does not list tool support. The request is sent as written.
          </p>
        )}
        {names.length > 0 && (
          <div className="space-y-1">
            <label htmlFor={`${idPrefix}-tool-choice`} className={LABEL_CLASS}>
              Tool choice
            </label>
            <select id={`${idPrefix}-tool-choice`} value={params.toolChoice}
              onChange={(e) => onChange({ toolChoice: e.target.value })}
              className={cn(SETTINGS_SELECT_CLASS, "w-full")}>
              <option value="auto">auto</option>
              <option value="none">none</option>
              <option value="required">required</option>
              {names.map((n) => (
                <option key={n} value={`fn:${n}`}>
                  force {n}
                </option>
              ))}
            </select>
          </div>
        )}
      </div>

      <div className="space-y-2">
        <label htmlFor={`${idPrefix}-format`} className={LABEL_CLASS}>
          Response format
        </label>
        <select id={`${idPrefix}-format`} value={params.responseFormat}
          onChange={(e) => onChange({ responseFormat: e.target.value as ResponseFormatKind })}
          className={cn(SETTINGS_SELECT_CLASS, "w-full")}>
          <option value="text">text</option>
          <option value="json_object">json_object</option>
          <option value="json_schema">json_schema</option>
        </select>
        {params.responseFormat === "json_schema" && (
          <>
            <input aria-label="Schema name" value={params.schemaName}
              onChange={(e) => onChange({ schemaName: e.target.value })}
              placeholder="Schema name" className={INPUT_CLASS} />
            <textarea aria-label="JSON schema" rows={6} spellCheck={false} value={params.schemaJson}
              onChange={(e) => onChange({ schemaJson: e.target.value })}
              placeholder='{"type":"object","properties":{…}}' className={AREA_CLASS} />
            {schemaErr && <p className="m-0 text-[12px] text-[var(--status-error)]">Not valid JSON: {schemaErr}</p>}
          </>
        )}
      </div>
    </div>
  );
}
