import React, { useCallback, useEffect, useMemo, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  PlayIcon,
  StopIcon,
  SourceCodeIcon,
  LayoutIcon,
  ArrowLeftRightIcon,
} from "@hugeicons/core-free-icons";
import { useLocation } from "react-router-dom";
import { cn } from "@/lib/utils";
import { formatApiError } from "@/lib/api-client";
import { isCloudGateway } from "@/lib/availability";
import { useProjectContext } from "@/lib/project-context";
import { getPlaygroundKey, gatewayBaseUrl } from "@/lib/console-gateway";
import {
  DEFAULT_PARAMS,
  buildRequestBody,
  imageContent,
  messagesToEditable,
  paramsFromBody,
  redactImages,
  type PlaygroundParams,
} from "@/lib/playground-request";
import { fetchReplayRequest, getSharedPrompt, type PromptBody, type SavedPrompt } from "@/lib/saved-prompts";
import { fetchConsoleModels, type ConsoleModel } from "@/lib/console-models";
import {
  loadModelAutoPolicy,
  saveModelAutoPolicy,
  resolveAutoModel,
  type ModelAutoPolicy,
} from "@/lib/model-auto-policy";
import { PLAYGROUND_TEMPLATES } from "@/lib/playground-templates";
import type { ChatCompletionMessage } from "@/lib/console-gateway";
import { MessagesEditor, type EditableMessage } from "./playground/MessagesEditor";
import { AdvancedPanel } from "./playground/AdvancedPanel";
import { Inspector } from "./playground/Inspector";
import { ExportPanel } from "./playground/ExportPanel";
import { PromptsPanel } from "./playground/PromptsPanel";
import { ModelPicker } from "./playground/ModelPicker";
import { OutputPane } from "./playground/OutputPane";
import { useChatRun } from "./playground/useChatRun";

const STORAGE_KEY = "allternit:console:playground";

const INPUT_CLASS =
  "w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]";

const SEGMENT_CLASS =
  "inline-flex items-center gap-1 rounded-lg px-3 py-1.5 text-[12px] font-semibold transition-colors";

interface PersistedState {
  model: string;
  systemPrompt: string;
  messages: EditableMessage[];
  params: PlaygroundParams;
}

function loadPersisted(): PersistedState {
  const fallback: PersistedState = {
    model: "auto",
    systemPrompt: "",
    messages: [{ id: "msg-initial", role: "user", content: "" }],
    params: DEFAULT_PARAMS,
  };
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return fallback;
    const parsed = JSON.parse(raw) as Partial<PersistedState> & {
      temperature?: number;
      maxTokens?: number;
    };
    const legacy: Partial<PlaygroundParams> = {};
    if (typeof parsed.temperature === "number") legacy.temperature = parsed.temperature;
    if (typeof parsed.maxTokens === "number") legacy.maxTokens = parsed.maxTokens;
    return {
      model: typeof parsed.model === "string" ? parsed.model : fallback.model,
      systemPrompt: typeof parsed.systemPrompt === "string" ? parsed.systemPrompt : "",
      // Images and tool turns are not persisted: data URLs are large and stale tool ids are useless.
      messages: Array.isArray(parsed.messages)
        ? parsed.messages
            .filter(
              (m): m is EditableMessage =>
                !!m &&
                typeof m.id === "string" &&
                (m.role === "user" || m.role === "assistant") &&
                typeof m.content === "string" &&
                !m.toolCalls
            )
            .map(({ id, role, content }) => ({ id, role, content }))
        : fallback.messages,
      params: { ...DEFAULT_PARAMS, ...legacy, ...(parsed.params ?? {}) },
    };
  } catch {
    return fallback;
  }
}

function buildMessages(
  systemPrompt: string,
  messages: EditableMessage[],
  withImages: boolean
): ChatCompletionMessage[] {
  const out: ChatCompletionMessage[] = [];
  if (systemPrompt.trim()) out.push({ role: "system", content: systemPrompt.trim() });
  for (const m of messages) {
    if (m.role === "tool") {
      out.push({ role: "tool", content: m.content, tool_call_id: m.toolCallId });
    } else if (m.role === "assistant") {
      if (m.content.trim() || m.toolCalls?.length) {
        out.push({
          role: "assistant",
          content: m.content.trim() ? m.content : null,
          ...(m.toolCalls?.length ? { tool_calls: m.toolCalls } : {}),
        });
      }
    } else if (m.content.trim() || (withImages && m.images?.length)) {
      out.push({ role: "user", content: imageContent(m.content, withImages ? (m.images ?? []) : []) });
    }
  }
  return out;
}

export function PlaygroundPage(): React.ReactNode {
  const location = useLocation();
  const cloud = isCloudGateway();
  const { project } = useProjectContext();
  const projectId = project?.id ?? null;

  const [persisted, setPersisted] = useState<PersistedState>(loadPersisted);
  const [view, setView] = useState<"form" | "code">("form");
  const [compare, setCompare] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [formErrors, setFormErrors] = useState<string[]>([]);

  const [models, setModels] = useState<ConsoleModel[]>([]);
  const [modelsError, setModelsError] = useState<string | null>(null);
  const [policy, setPolicy] = useState<ModelAutoPolicy>(loadModelAutoPolicy);

  const single = useChatRun();
  const laneA = useChatRun();
  const laneB = useChatRun();

  const [laneBConfig, setLaneBConfig] = useState({
    model: "auto",
    temperature: 0.7,
    maxTokens: 4096,
  });

  const setParams = useCallback(
    (patch: Partial<PlaygroundParams>) =>
      setPersisted((p) => ({ ...p, params: { ...p.params, ...patch } })),
    []
  );

  useEffect(() => {
    try {
      // Image data URLs stay out of localStorage.
      const slim = {
        ...persisted,
        messages: persisted.messages.map(({ images: _i, ...rest }) => rest),
      };
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(slim));
    } catch {
      // Persistence is best-effort.
    }
  }, [persisted]);

  // Deep link from dashboard model cards: /playground?model=<id>.
  useEffect(() => {
    const param = new URLSearchParams(window.location.search).get("model");
    if (param) {
      setPersisted((prev) => (prev.model === param ? prev : { ...prev, model: param }));
    }
  }, []);

  // Live model list. On cloud it is read with the minted project key.
  useEffect(() => {
    let active = true;
    setModelsError(null);
    const load = async () => {
      let key: string | undefined;
      if (cloud && projectId) {
        try {
          key = await getPlaygroundKey(projectId);
        } catch {
          key = undefined;
        }
      }
      return fetchConsoleModels(key);
    };
    load()
      .then((list) => {
        if (active) setModels(list);
      })
      .catch((err) => {
        if (active) setModelsError(formatApiError(err, "model catalog"));
      });
    return () => {
      active = false;
    };
  }, [cloud, projectId]);

  const handlePolicyChange = useCallback((next: ModelAutoPolicy) => {
    setPolicy(next);
    saveModelAutoPolicy(next);
  }, []);

  const resolveModel = useCallback(
    (model: string): string | null =>
      model === "auto" ? resolveAutoModel(models, policy) : model,
    [models, policy]
  );

  const selectedModel = useMemo(() => {
    const id = resolveModel(persisted.model);
    return models.find((m) => m.id === id) ?? null;
  }, [models, persisted.model, resolveModel]);

  // Unknown (no catalog) means allowed: only a known "no vision" model blocks images.
  const supportsVision = selectedModel ? selectedModel.supports_vision === true : true;
  const supportsTools = selectedModel ? selectedModel.supports_tools === true : true;
  const hasImages = persisted.messages.some((m) => (m.images?.length ?? 0) > 0);
  const imagesBlocked = hasImages && !supportsVision;

  const effectiveMessages = useMemo(
    () => buildMessages(persisted.systemPrompt, persisted.messages, supportsVision),
    [persisted.systemPrompt, persisted.messages, supportsVision]
  );

  const anyRunning = single.state.running || laneA.state.running || laneB.state.running;

  const requestFor = useCallback(
    (model: string, messages: ChatCompletionMessage[], params: PlaygroundParams) =>
      buildRequestBody(model, messages, params),
    []
  );

  const runLane = useCallback(
    async (
      lane: typeof single,
      model: string,
      messages: ChatCompletionMessage[],
      params: PlaygroundParams
    ) => {
      const { body, errors } = requestFor(model, messages, params);
      setFormErrors(errors);
      if (errors.length > 0) return;
      await lane.run({ body, projectId });
    },
    [requestFor, projectId]
  );

  const handleRunSingle = useCallback(async () => {
    const model = resolveModel(persisted.model);
    if (!model) return;
    await runLane(single, model, effectiveMessages, persisted.params);
  }, [resolveModel, persisted, effectiveMessages, single, runLane]);

  const handleToolResults = useCallback(
    async (results: Record<string, string>) => {
      const model = resolveModel(persisted.model);
      if (!model) return;
      const calls = single.state.toolCalls;
      const added: EditableMessage[] = [
        {
          id: `msg-${Date.now()}-a`,
          role: "assistant",
          content: single.state.text,
          toolCalls: calls,
        },
        ...calls.map((t, i) => ({
          id: `msg-${Date.now()}-t${i}`,
          role: "tool" as const,
          content: results[t.id] ?? "",
          toolCallId: t.id,
        })),
      ];
      const nextMessages = [...persisted.messages, ...added];
      setPersisted((p) => ({ ...p, messages: nextMessages }));
      await runLane(
        single,
        model,
        buildMessages(persisted.systemPrompt, nextMessages, supportsVision),
        persisted.params
      );
    },
    [resolveModel, persisted, single, runLane, supportsVision]
  );

  const handleRunCompare = useCallback(async () => {
    const modelA = resolveModel(persisted.model);
    const modelB = resolveModel(laneBConfig.model);
    // Sequential, per the compare-mode contract; each lane reports its own error.
    if (modelA) await runLane(laneA, modelA, effectiveMessages, persisted.params);
    if (modelB) {
      await runLane(laneB, modelB, effectiveMessages, {
        ...persisted.params,
        temperature: laneBConfig.temperature,
        maxTokens: laneBConfig.maxTokens,
      });
    }
  }, [resolveModel, persisted, laneBConfig, effectiveMessages, laneA, laneB, runLane]);

  const handleStop = useCallback(() => {
    single.stop();
    laneA.stop();
    laneB.stop();
  }, [single, laneA, laneB]);

  const exportBuild = useMemo(
    () =>
      buildRequestBody(
        resolveModel(persisted.model) ?? persisted.model,
        redactImages(effectiveMessages),
        persisted.params
      ),
    [resolveModel, persisted, effectiveMessages]
  );

  const applyTemplate = useCallback(
    (templateId: string) => {
      const template = PLAYGROUND_TEMPLATES.find((t) => t.id === templateId);
      if (!template) return;
      setPersisted((prev) => ({
        ...prev,
        systemPrompt: template.systemPrompt,
        messages: [
          { id: `msg-${Date.now()}`, role: "user", content: template.starterMessage },
        ],
      }));
      single.reset();
    },
    [single]
  );

  const loadBody = useCallback(
    (body: PromptBody | Record<string, unknown>) => {
      const rec = body as Record<string, unknown>;
      const { systemPrompt, messages } = messagesToEditable(rec.messages);
      const hasParams = typeof rec.params === "object" && rec.params !== null;
      setPersisted((prev) => ({
        model: typeof rec.model === "string" ? rec.model : prev.model,
        systemPrompt: typeof rec.systemPrompt === "string" ? rec.systemPrompt : systemPrompt,
        messages: messages.length > 0 ? messages : prev.messages,
        params: hasParams
          ? { ...DEFAULT_PARAMS, ...(rec.params as Partial<PlaygroundParams>) }
          : paramsFromBody(rec),
      }));
      single.reset();
      setView("form");
    },
    [single]
  );

  // Shared prompt (/playground/shared/:token) and replay (?replay=<id> or navigation state).
  useEffect(() => {
    let active = true;
    const sharedMatch = location.pathname.match(/\/playground\/shared\/([^/]+)/);
    const search = new URLSearchParams(location.search);
    const state = (location.state ?? {}) as { replay?: unknown; replayRequestId?: string };
    const replayId =
      search.get("replay") ??
      state.replayRequestId ??
      (typeof state.replay === "string" ? state.replay : null);

    if (sharedMatch) {
      getSharedPrompt(decodeURIComponent(sharedMatch[1]))
        .then((p) => {
          if (!active) return;
          loadBody(p.body);
          setNotice(`Loaded shared prompt "${p.name}". Nothing runs until you press Run.`);
        })
        .catch((err) => {
          if (active) setNotice(formatApiError(err, "This shared prompt could not be loaded. Sign in and check the link."));
        });
    } else if (state.replay && typeof state.replay === "object") {
      loadBody(state.replay as Record<string, unknown>);
      setNotice("Loaded the request to replay. Nothing runs until you press Run.");
    } else if (replayId) {
      fetchReplayRequest(replayId, search.get("scope") === "org" ? "org" : "me")
        .then((body) => {
          if (!active) return;
          if (body) {
            loadBody(body);
            setNotice("Loaded the request to replay. Nothing runs until you press Run.");
          } else {
            setNotice("That request's body was not stored, so it cannot be replayed. Request bodies are kept only when the project turns on body logging.");
          }
        })
        .catch((err) => {
          if (active) setNotice(formatApiError(err, "Could not load that request."));
        });
    }
    return () => {
      active = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [location.pathname, location.search, location.key]);

  const getPromptBody = useCallback(
    (): PromptBody => ({
      model: persisted.model,
      systemPrompt: persisted.systemPrompt,
      messages: buildMessages("", persisted.messages, false),
      params: persisted.params,
    }),
    [persisted]
  );

  const autoUnresolved = persisted.model === "auto" && resolveModel("auto") === null;
  const cloudNeedsProject = cloud && !projectId;
  const runDisabled =
    effectiveMessages.length === 0 || autoUnresolved || imagesBlocked || cloudNeedsProject;

  const price = {
    promptPrice: selectedModel?.prompt_price,
    completionPrice: selectedModel?.completion_price,
  };

  const detailsClass =
    "rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] p-3";
  const summaryClass = "cursor-pointer text-[13px] font-semibold text-[var(--text-primary)]";

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h1 className="m-0 text-[20px] font-semibold tracking-tight text-[var(--text-primary)]">
            Playground
          </h1>
          <p className="m-0 mt-1 text-[13px] text-[var(--text-secondary)]">
            Try models, tune parameters, and iterate on prompts before shipping
            them to code.
          </p>
          {isCloudGateway() ? (
            <p className="m-0 mt-1 text-[12px] text-[var(--text-tertiary)]">
              Playground runs use your Allternit credits. The project spend cap does not apply.
            </p>
          ) : null}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <div className="inline-flex rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-0.5">
            <button
              type="button"
              onClick={() => setView("form")}
              className={cn(
                SEGMENT_CLASS,
                view === "form"
                  ? "bg-[var(--accent-primary)] text-[var(--ui-text-inverse)]"
                  : "text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
              )}
            >
              <HugeiconsIcon icon={LayoutIcon} size={13} />
              Form
            </button>
            <button
              type="button"
              onClick={() => setView("code")}
              className={cn(
                SEGMENT_CLASS,
                view === "code"
                  ? "bg-[var(--accent-primary)] text-[var(--ui-text-inverse)]"
                  : "text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
              )}
            >
              <HugeiconsIcon icon={SourceCodeIcon} size={13} />
              Code
            </button>
          </div>
          <button
            type="button"
            onClick={() => setCompare((v) => !v)}
            className={cn(
              "inline-flex items-center gap-1.5 rounded-lg border border-solid px-3 py-1.5 text-[12px] font-semibold transition-colors",
              compare
                ? "border-[var(--accent-primary)]/40 bg-[var(--accent-primary)]/10 text-[var(--accent-primary)]"
                : "border-[var(--border-subtle)] bg-[var(--bg-secondary)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
            )}
          >
            <HugeiconsIcon icon={ArrowLeftRightIcon} size={13} />
            Compare
          </button>
        </div>
      </div>

      {notice && (
        <p className="m-0 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[12px] text-[var(--text-secondary)]">
          {notice}
        </p>
      )}
      {cloudNeedsProject && (
        <p className="m-0 text-[12px] text-[var(--status-error)]">
          Select a project to run requests. The Playground bills the selected project.
        </p>
      )}
      {formErrors.map((e) => (
        <p key={e} className="m-0 text-[12px] text-[var(--status-error)]">
          {e}
        </p>
      ))}
      {imagesBlocked && (
        <p className="m-0 text-[12px] text-[var(--status-error)]">
          The selected model does not accept images. Pick a model with vision or remove the images.
        </p>
      )}

      {view === "code" ? (
        <div className="space-y-4">
          <p className="m-0 text-[13px] text-[var(--text-secondary)]">
            This is the exact request the form sends, as code you can run.
          </p>
          <ExportPanel body={exportBuild.body} baseUrl={gatewayBaseUrl()} errors={exportBuild.errors} />
        </div>
      ) : compare ? (
        <div className="space-y-5">
          <div className="grid gap-5 lg:grid-cols-2">
            <div className="min-w-0 space-y-4 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] p-4">
              <ModelPicker
                idPrefix="lane-a"
                models={models}
                modelsError={modelsError}
                value={persisted.model}
                onChange={(model) => setPersisted((p) => ({ ...p, model }))}
                policy={policy}
                onPolicyChange={handlePolicyChange}
              />
              <ParamRow
                temperature={persisted.params.temperature}
                maxTokens={persisted.params.maxTokens}
                onTemperature={(v) => setParams({ temperature: v })}
                onMaxTokens={(v) => setParams({ maxTokens: v })}
                idPrefix="lane-a"
              />
              <OutputPane
                title="Output A"
                state={laneA.state}
                idleCaption="Run to see output for this model."
              />
            </div>
            <div className="min-w-0 space-y-4 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] p-4">
              <ModelPicker
                idPrefix="lane-b"
                models={models}
                modelsError={modelsError}
                value={laneBConfig.model}
                onChange={(model) => setLaneBConfig((c) => ({ ...c, model }))}
                policy={policy}
                onPolicyChange={handlePolicyChange}
              />
              <ParamRow
                temperature={laneBConfig.temperature}
                maxTokens={laneBConfig.maxTokens}
                onTemperature={(v) => setLaneBConfig((c) => ({ ...c, temperature: v }))}
                onMaxTokens={(v) => setLaneBConfig((c) => ({ ...c, maxTokens: v }))}
                idPrefix="lane-b"
              />
              <OutputPane
                title="Output B"
                state={laneB.state}
                idleCaption="Run to see output for this model."
              />
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <RunButton
              running={laneA.state.running || laneB.state.running}
              onRun={() => void handleRunCompare()}
              onStop={handleStop}
              disabled={runDisabled}
            />
            {autoUnresolved && <AutoUnresolvedNote />}
          </div>
        </div>
      ) : (
        <div className="grid items-start gap-6 lg:grid-cols-[minmax(0,1fr)_300px]">
          <div className="min-w-0 space-y-5">
            <div className="space-y-1.5">
              <label
                htmlFor="pg-system"
                className="text-[12px] font-semibold text-[var(--text-secondary)]"
              >
                System prompt
              </label>
              <textarea
                id="pg-system"
                value={persisted.systemPrompt}
                onChange={(e) =>
                  setPersisted((p) => ({ ...p, systemPrompt: e.target.value }))
                }
                placeholder="You are a helpful assistant."
                rows={3}
                className={cn(INPUT_CLASS, "resize-y font-mono text-[12px]")}
              />
            </div>

            <MessagesEditor
              messages={persisted.messages}
              onChange={(messages) => setPersisted((p) => ({ ...p, messages }))}
              supportsVision={supportsVision}
            />

            <ModelPicker
              idPrefix="pg"
              models={models}
              modelsError={modelsError}
              value={persisted.model}
              onChange={(model) => setPersisted((p) => ({ ...p, model }))}
              policy={policy}
              onPolicyChange={handlePolicyChange}
            />

            <details className={detailsClass} open>
              <summary className={summaryClass}>Parameters, tools and response format</summary>
              <div className="pt-3">
                <AdvancedPanel
                  idPrefix="pg"
                  params={persisted.params}
                  onChange={setParams}
                  supportsTools={supportsTools}
                />
              </div>
            </details>

            <div className="flex flex-wrap items-center gap-2">
              <RunButton
                running={single.state.running}
                onRun={() => void handleRunSingle()}
                onStop={handleStop}
                disabled={runDisabled}
              />
              {autoUnresolved && <AutoUnresolvedNote />}
            </div>

            <OutputPane
              title="Output"
              state={single.state}
              idleCaption="Run the request to see the model's response stream here."
              onContinueWithToolResults={(r) => void handleToolResults(r)}
            />

            <details className={detailsClass}>
              <summary className={summaryClass}>Inspector</summary>
              <div className="pt-3">
                <Inspector state={single.state} {...price} />
              </div>
            </details>

            <details className={detailsClass}>
              <summary className={summaryClass}>Export code</summary>
              <div className="pt-3">
                <ExportPanel body={exportBuild.body} baseUrl={gatewayBaseUrl()} errors={exportBuild.errors} />
              </div>
            </details>

            <details className={detailsClass}>
              <summary className={summaryClass}>Saved prompts and sharing</summary>
              <div className="pt-3">
                <PromptsPanel
                  projectId={projectId}
                  getBody={getPromptBody}
                  onLoad={(p: SavedPrompt) => {
                    loadBody(p.body);
                    setNotice(`Opened "${p.name}".`);
                  }}
                />
              </div>
            </details>
          </div>

          <aside className="space-y-2 lg:sticky lg:top-0">
            <h2 className="m-0 text-[13px] font-semibold text-[var(--text-primary)]">
              Start with a template
            </h2>
            <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
              Fills the request form — nothing runs until you press Run.
            </p>
            <div className="space-y-2 pt-1">
              {PLAYGROUND_TEMPLATES.map((template) => (
                <button
                  key={template.id}
                  type="button"
                  onClick={() => applyTemplate(template.id)}
                  className="w-full rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-3 text-left transition-colors hover:border-[var(--accent-primary)]/40"
                >
                  <span className="block text-[13px] font-semibold text-[var(--text-primary)]">
                    {template.label}
                  </span>
                  <span className="mt-0.5 block text-[12px] leading-relaxed text-[var(--text-tertiary)]">
                    {template.description}
                  </span>
                </button>
              ))}
            </div>
          </aside>
        </div>
      )}

      {anyRunning && (
        <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
          Requests are billed to {cloud ? "the selected project" : "your organization"} through the gateway.
        </p>
      )}
    </div>
  );
}

function ParamRow({
  temperature,
  maxTokens,
  onTemperature,
  onMaxTokens,
  idPrefix,
}: {
  temperature: number;
  maxTokens: number;
  onTemperature: (v: number) => void;
  onMaxTokens: (v: number) => void;
  idPrefix: string;
}): React.ReactNode {
  return (
    <div className="grid grid-cols-2 gap-3">
      <div className="space-y-1">
        <label
          htmlFor={`${idPrefix}-temp`}
          className="text-[12px] font-semibold text-[var(--text-secondary)]"
        >
          Temperature
        </label>
        <input
          id={`${idPrefix}-temp`}
          type="number"
          min={0}
          max={2}
          step={0.1}
          value={temperature}
          onChange={(e) => onTemperature(Number(e.target.value) || 0)}
          className={INPUT_CLASS}
        />
      </div>
      <div className="space-y-1">
        <label
          htmlFor={`${idPrefix}-max-tokens`}
          className="text-[12px] font-semibold text-[var(--text-secondary)]"
        >
          Max tokens
        </label>
        <input
          id={`${idPrefix}-max-tokens`}
          type="number"
          min={1}
          value={maxTokens}
          onChange={(e) =>
            onMaxTokens(Math.max(1, parseInt(e.target.value, 10) || 1))
          }
          className={INPUT_CLASS}
        />
      </div>
    </div>
  );
}

function RunButton({
  running,
  onRun,
  onStop,
  disabled,
}: {
  running: boolean;
  onRun: () => void;
  onStop: () => void;
  disabled: boolean;
}): React.ReactNode {
  return running ? (
    <button
      type="button"
      onClick={onStop}
      className="inline-flex items-center gap-1.5 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--text-primary)] transition-colors hover:bg-[var(--surface-hover)]"
    >
      <HugeiconsIcon icon={StopIcon} size={13} />
      Stop
    </button>
  ) : (
    <button
      type="button"
      onClick={onRun}
      disabled={disabled}
      className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
    >
      <HugeiconsIcon icon={PlayIcon} size={13} />
      Run
    </button>
  );
}

function AutoUnresolvedNote(): React.ReactNode {
  return (
    <span className="text-[12px] text-[var(--status-error)]">
      No model matches the Auto policy. Pick a model from the list or change the policy.
    </span>
  );
}

export default PlaygroundPage;
