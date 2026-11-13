import React, { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  BotIcon,
  AlertCircleIcon,
  MagicWandIcon,
  Tick01Icon,
} from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { api, formatApiError } from "@/lib/api-client";
import {
  CodeTemplateBlock,
  Badge,
  SkeletonCard,
  QUIET_BUTTON_CLASS,
} from "@/components/console-ui";
import { fetchConsoleModels, type ConsoleModel } from "@/lib/console-models";
import {
  loadModelAutoPolicy,
  saveModelAutoPolicy,
  resolveAutoModel,
  type ModelAutoPolicy,
} from "@/lib/model-auto-policy";
import { ModelPicker } from "./playground/ModelPicker";

const TEXTAREA_CLASS =
  "w-full resize-y rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-4 py-3 text-[14px] leading-relaxed text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]";

const INPUT_CLASS =
  "w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]";

interface AgentTemplate {
  id: string;
  name: string;
  description?: string | null;
  category?: string | null;
  spec?: unknown;
  is_builtin?: boolean;
  created_at?: string | null;
}

interface PrototypeAgent {
  id: string;
  name: string;
  description?: string | null;
  model?: string;
  status?: string;
  temperature?: number;
  created_at?: string;
  updated_at?: string;
}

interface CreatedAgent {
  agent: PrototypeAgent;
  example?: { curl?: string; sdk?: string };
}

interface InstantiatedCrew {
  pattern?: string;
  orchestrator?: { id: string; name: string };
  subagents?: { id: string; name: string }[];
}

const PLACEHOLDER = `e.g. A research agent that monitors competitor pricing pages every morning, flags changes above 5%, and drafts a summary with links for review before anything is sent out.`;

export function BuilderPage(): React.ReactNode {
  const [description, setDescription] = useState("");
  const [name, setName] = useState("");
  const [model, setModel] = useState("auto");
  const [models, setModels] = useState<ConsoleModel[]>([]);
  const [modelsError, setModelsError] = useState<string | null>(null);
  const [policy, setPolicy] = useState<ModelAutoPolicy>(loadModelAutoPolicy);

  const [generating, setGenerating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<CreatedAgent | null>(null);

  const [templates, setTemplates] = useState<AgentTemplate[] | null>(null);
  const [templatesError, setTemplatesError] = useState<string | null>(null);
  const [usingTemplate, setUsingTemplate] = useState<string | null>(null);
  const [instantiated, setInstantiated] = useState<InstantiatedCrew | null>(null);

  useEffect(() => {
    let active = true;
    fetchConsoleModels()
      .then((list) => {
        if (active) setModels(list);
      })
      .catch((err) => {
        if (active) setModelsError(formatApiError(err, "model catalog"));
      });
    api
      .get<{ templates: AgentTemplate[] }>("/api/v1/agent-templates")
      .then((data) => {
        if (active) setTemplates(data.templates ?? []);
      })
      .catch((err) => {
        if (active) setTemplatesError(formatApiError(err, "Unable to load templates"));
      });
    return () => {
      active = false;
    };
  }, []);

  const handlePolicyChange = useCallback((next: ModelAutoPolicy) => {
    setPolicy(next);
    saveModelAutoPolicy(next);
  }, []);

  const resolvedModel = model === "auto" ? resolveAutoModel(models, policy) : model;

  const handleGenerate = useCallback(async () => {
    if (!description.trim()) return;
    setGenerating(true);
    setError(null);
    setCreated(null);
    setInstantiated(null);
    try {
      const derivedName =
        name.trim() ||
        description
          .trim()
          .replace(/^e\.g\.\s*/i, "")
          .split(/\s+/)
          .slice(0, 4)
          .join(" ")
          .replace(/[^a-zA-Z0-9 ]/g, "")
          .slice(0, 40) ||
        "Console prototype";
      const data = await api.post<CreatedAgent>("/api/v1/agents/prototype", {
        name: derivedName,
        description: description.trim(),
        system_prompt: description.trim(),
        model: resolvedModel ?? model,
        temperature: 0.7,
        max_tokens: 4096,
        tools: [],
      });
      setCreated(data);
    } catch (err) {
      setError(formatApiError(err, "Generation failed"));
    } finally {
      setGenerating(false);
    }
  }, [description, name, model, resolvedModel]);

  const handleUseTemplate = useCallback(async (template: AgentTemplate) => {
    setUsingTemplate(template.id);
    setError(null);
    setCreated(null);
    setInstantiated(null);
    try {
      const data = await api.post<InstantiatedCrew>(
        "/api/v1/agents/from-template",
        { template_id: template.id }
      );
      setInstantiated(data);
    } catch (err) {
      setError(formatApiError(err, `Unable to instantiate "${template.name}"`));
    } finally {
      setUsingTemplate(null);
    }
  }, []);

  return (
    <div className="space-y-6">
      <div>
        <h1 className="m-0 text-[20px] font-semibold tracking-tight text-[var(--text-primary)]">
          Builder
        </h1>
        <p className="m-0 mt-1 text-[13px] text-[var(--text-secondary)]">
          Describe the agent you want and save it as a prototype, or start from
          a proven pattern template.
        </p>
      </div>

      {error && (
        <p className="flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      )}

      <div className="grid items-start gap-6 lg:grid-cols-2">
        <section className="space-y-4 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-5">
          <h2 className="m-0 text-[15px] font-semibold text-[var(--text-primary)]">
            Describe your agent
          </h2>
          <div className="space-y-1">
            <label htmlFor="builder-name" className="text-[12px] font-semibold text-[var(--text-secondary)]">
              Name <span className="font-normal text-[var(--text-tertiary)]">(optional — derived from the description if blank)</span>
            </label>
            <input
              id="builder-name"
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Pricing watch"
              className={INPUT_CLASS}
            />
          </div>
          <div className="space-y-1">
            <label htmlFor="builder-description" className="text-[12px] font-semibold text-[var(--text-secondary)]">
              What should it do?
            </label>
            <textarea
              id="builder-description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder={PLACEHOLDER}
              rows={7}
              className={TEXTAREA_CLASS}
            />
          </div>
          <ModelPicker
            idPrefix="builder"
            models={models}
            modelsError={modelsError}
            value={model}
            onChange={setModel}
            policy={policy}
            onPolicyChange={handlePolicyChange}
          />
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={() => void handleGenerate()}
              disabled={generating || !description.trim() || (model === "auto" && !resolvedModel)}
              className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              <HugeiconsIcon icon={MagicWandIcon} size={13} />
              {generating ? "Saving prototype…" : "Generate"}
            </button>
            {model === "auto" && !resolvedModel && (
              <span className="text-[12px] text-[var(--status-error)]">
                No model matches the Auto policy. Pick a model from the list or change the policy.
              </span>
            )}
          </div>
          <p className="m-0 text-[11px] text-[var(--text-tertiary)]">
            Generation saves a prototype agent (status{" "}
            <code className="font-mono">prototype</code>) — it is not executed
            and nothing is billed. Runs happen from the Agents page.
          </p>

          {created && (
            <div className="space-y-3 rounded-lg border border-solid border-[var(--status-success)]/30 bg-[var(--status-success)]/5 p-4">
              <div className="flex items-center gap-2">
                <HugeiconsIcon icon={Tick01Icon} size={15} className="text-[var(--status-success)]" />
                <h3 className="m-0 text-[14px] font-semibold text-[var(--text-primary)]">
                  Prototype saved: {created.agent.name}
                </h3>
                <Badge>{created.agent.status ?? "prototype"}</Badge>
              </div>
              <dl className="m-0 grid grid-cols-2 gap-x-4 gap-y-1 text-[12px]">
                <dt className="text-[var(--text-tertiary)]">Model</dt>
                <dd className="m-0 font-mono text-[var(--text-secondary)]">{created.agent.model}</dd>
                <dt className="text-[var(--text-tertiary)]">ID</dt>
                <dd className="m-0 font-mono text-[var(--text-secondary)]">{created.agent.id}</dd>
              </dl>
              <Link
                to="/agents"
                className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110"
              >
                <HugeiconsIcon icon={BotIcon} size={13} />
                Open in Agents
              </Link>
              {created.example?.curl && (
                <CodeTemplateBlock language="bash" code={created.example.curl} />
              )}
            </div>
          )}
        </section>

        <section className="space-y-3">
          <h2 className="m-0 text-[15px] font-semibold text-[var(--text-primary)]">
            Start from a template
          </h2>
          <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
            Pattern templates instantiate a runnable orchestrator with
            subagents, using your default model.
          </p>

          {templatesError && (
            <p className="flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[12px] text-[var(--status-error)]">
              <HugeiconsIcon icon={AlertCircleIcon} size={13} />
              {templatesError}
            </p>
          )}

          {templates === null && !templatesError ? (
            <div className="grid gap-3 sm:grid-cols-2">
              <SkeletonCard rows={3} />
              <SkeletonCard rows={3} />
            </div>
          ) : (
            <div className="grid gap-3 sm:grid-cols-2">
              {(templates ?? []).map((template) => (
                <div
                  key={template.id}
                  className="flex flex-col rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4"
                >
                  <div className="flex items-center gap-2">
                    <h3 className="m-0 flex-1 text-[13px] font-semibold text-[var(--text-primary)]">
                      {template.name}
                    </h3>
                    {template.is_builtin && <Badge>built-in</Badge>}
                  </div>
                  {template.category && (
                    <span className="mt-1 text-[11px] uppercase tracking-wide text-[var(--text-tertiary)]">
                      {template.category}
                    </span>
                  )}
                  {template.description && (
                    <p className="m-0 mt-2 flex-1 text-[12px] leading-relaxed text-[var(--text-secondary)]">
                      {template.description}
                    </p>
                  )}
                  <button
                    type="button"
                    onClick={() => void handleUseTemplate(template)}
                    disabled={usingTemplate !== null}
                    className={cn(QUIET_BUTTON_CLASS, "mt-3 justify-center")}
                  >
                    {usingTemplate === template.id ? "Instantiating…" : "Use template"}
                  </button>
                </div>
              ))}
            </div>
          )}

          {instantiated && (
            <div className="space-y-2 rounded-lg border border-solid border-[var(--status-success)]/30 bg-[var(--status-success)]/5 p-4">
              <div className="flex items-center gap-2">
                <HugeiconsIcon icon={Tick01Icon} size={15} className="text-[var(--status-success)]" />
                <h3 className="m-0 text-[14px] font-semibold text-[var(--text-primary)]">
                  Crew created: {instantiated.orchestrator?.name}
                </h3>
              </div>
              <p className="m-0 text-[12px] text-[var(--text-secondary)]">
                Pattern <Badge>{instantiated.pattern ?? "custom"}</Badge> with{" "}
                {instantiated.subagents?.length ?? 0} subagent
                {(instantiated.subagents?.length ?? 0) === 1 ? "" : "s"}.
              </p>
              <Link
                to="/agents"
                className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110"
              >
                <HugeiconsIcon icon={BotIcon} size={13} />
                Open in Agents
              </Link>
            </div>
          )}
        </section>
      </div>
    </div>
  );
}

export default BuilderPage;
