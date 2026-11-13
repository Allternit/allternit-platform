import React, { useMemo } from "react";
import { cn } from "@/lib/utils";
import {
  SETTINGS_SELECT_CLASS,
} from "@/components/console-ui";
import type { ConsoleModel } from "@/lib/console-models";
import {
  type ModelAutoPolicy,
  type AutoPolicyStrategy,
  resolveAutoModel,
} from "@/lib/model-auto-policy";

const STRATEGIES: { value: AutoPolicyStrategy; label: string }[] = [
  { value: "manual", label: "Manual (no auto-resolution)" },
  { value: "cheapest", label: "Cheapest" },
  { value: "fastest", label: "Fastest" },
  { value: "strongest", label: "Strongest" },
  { value: "balanced", label: "Balanced" },
];

const INPUT_CLASS =
  "w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]";

interface ModelPickerProps {
  models: ConsoleModel[];
  modelsError: string | null;
  value: string;
  onChange: (value: string) => void;
  policy: ModelAutoPolicy;
  onPolicyChange: (policy: ModelAutoPolicy) => void;
  idPrefix: string;
}

/**
 * Model selector for a playground lane. Includes an `auto` option that
 * resolves client-side through the shared auto policy (same localStorage key
 * as the ai surface). Falls back to a free-text input when the live catalog
 * cannot be loaded.
 */
export function ModelPicker({
  models,
  modelsError,
  value,
  onChange,
  policy,
  onPolicyChange,
  idPrefix,
}: ModelPickerProps): React.ReactNode {
  const providers = useMemo(
    () => Array.from(new Set(models.map((m) => m.owned_by))).sort(),
    [models]
  );

  const resolvedAuto = useMemo(
    () => resolveAutoModel(models, policy),
    [models, policy]
  );

  return (
    <div className="space-y-3">
      <div className="space-y-1.5">
        <label
          htmlFor={`${idPrefix}-model`}
          className="text-[12px] font-semibold text-[var(--text-secondary)]"
        >
          Model
        </label>
        {models.length > 0 ? (
          <select
            id={`${idPrefix}-model`}
            value={value}
            onChange={(e) => onChange(e.target.value)}
            className={cn(SETTINGS_SELECT_CLASS, "w-full")}
          >
            <option value="auto">auto — resolved by policy</option>
            {models.map((m) => (
              <option key={m.id} value={m.id}>
                {m.display_name ?? m.id} ({m.id})
              </option>
            ))}
          </select>
        ) : (
          <input
            id={`${idPrefix}-model`}
            type="text"
            value={value}
            onChange={(e) => onChange(e.target.value)}
            placeholder="model id, e.g. deepseek-v4.1-flash"
            className={INPUT_CLASS}
          />
        )}
        {modelsError && (
          <p className="m-0 text-[12px] text-[var(--status-error)]">
            Live model catalog unavailable — {modelsError} Enter a model id
            manually.
          </p>
        )}
        {value === "auto" && policy.strategy !== "manual" && (
          <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
            Auto resolves to{" "}
            <code className="font-mono text-[var(--accent-primary)]">
              {resolvedAuto ?? "no matching model"}
            </code>
          </p>
        )}
      </div>

      {value === "auto" && (
        <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-3 space-y-3">
          <div className="space-y-1.5">
            <label
              htmlFor={`${idPrefix}-strategy`}
              className="text-[12px] font-semibold text-[var(--text-secondary)]"
            >
              Auto-selection strategy
            </label>
            <select
              id={`${idPrefix}-strategy`}
              value={policy.strategy}
              onChange={(e) =>
                onPolicyChange({
                  ...policy,
                  strategy: e.target.value as AutoPolicyStrategy,
                })
              }
              className={cn(SETTINGS_SELECT_CLASS, "w-full")}
            >
              {STRATEGIES.map((s) => (
                <option key={s.value} value={s.value}>
                  {s.label}
                </option>
              ))}
            </select>
          </div>

          {providers.length > 0 && (
            <div className="space-y-1.5">
              <span className="text-[12px] font-semibold text-[var(--text-secondary)]">
                Allowed providers
              </span>
              <div className="flex flex-wrap gap-1.5">
                {providers.map((provider) => {
                  const allowed = policy.allowedProviders.includes(provider);
                  return (
                    <button
                      key={provider}
                      type="button"
                      onClick={() =>
                        onPolicyChange({
                          ...policy,
                          allowedProviders: allowed
                            ? policy.allowedProviders.filter(
                                (p) => p !== provider
                              )
                            : [...policy.allowedProviders, provider],
                        })
                      }
                      className={cn(
                        "rounded-full border border-solid px-2.5 py-1 text-[11px] font-semibold transition-colors",
                        allowed
                          ? "border-[var(--accent-primary)]/40 bg-[var(--accent-primary)]/10 text-[var(--accent-primary)]"
                          : "border-[var(--border-subtle)] bg-transparent text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                      )}
                    >
                      {provider}
                    </button>
                  );
                })}
              </div>
              <p className="m-0 text-[11px] text-[var(--text-tertiary)]">
                None selected means every provider is allowed.
              </p>
            </div>
          )}

          <div className="grid grid-cols-2 gap-2">
            <div className="space-y-1">
              <label
                htmlFor={`${idPrefix}-max-in`}
                className="text-[11px] font-semibold text-[var(--text-secondary)]"
              >
                Max input ¢/1M
              </label>
              <input
                id={`${idPrefix}-max-in`}
                type="number"
                min={0}
                placeholder="No limit"
                value={policy.maxInputCentsPer1m ?? ""}
                onChange={(e) =>
                  onPolicyChange({
                    ...policy,
                    maxInputCentsPer1m:
                      e.target.value === ""
                        ? null
                        : Math.max(0, parseInt(e.target.value, 10) || 0),
                  })
                }
                className={INPUT_CLASS}
              />
            </div>
            <div className="space-y-1">
              <label
                htmlFor={`${idPrefix}-max-out`}
                className="text-[11px] font-semibold text-[var(--text-secondary)]"
              >
                Max output ¢/1M
              </label>
              <input
                id={`${idPrefix}-max-out`}
                type="number"
                min={0}
                placeholder="No limit"
                value={policy.maxOutputCentsPer1m ?? ""}
                onChange={(e) =>
                  onPolicyChange({
                    ...policy,
                    maxOutputCentsPer1m:
                      e.target.value === ""
                        ? null
                        : Math.max(0, parseInt(e.target.value, 10) || 0),
                  })
                }
                className={INPUT_CLASS}
              />
            </div>
          </div>

          <p className="m-0 text-[11px] text-[var(--text-tertiary)]">
            Saved to this browser (shared with the ai surface's model gateway
            view).
          </p>
        </div>
      )}

      {value === "auto" && policy.strategy === "manual" && (
        <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
          Strategy is "manual", so auto will not resolve a model. Pick a
          strategy above or choose a concrete model.
        </p>
      )}

      {value !== "auto" && value.trim() !== "" && (
        <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
          Requests send{" "}
          <code className="font-mono text-[var(--text-secondary)]">
            "model": "{value}"
          </code>
        </p>
      )}
    </div>
  );
}
