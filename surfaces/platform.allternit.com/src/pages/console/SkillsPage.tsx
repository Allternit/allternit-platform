import React, { useCallback, useEffect, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  PlayIcon,
  TrashIcon,
  PlusSignIcon,
} from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { api, formatApiError } from "@/lib/api-client";
import {
  ListPage,
  EmptyState,
  MonoChip,
  Badge,
  SkeletonRow,
  QUIET_BUTTON_CLASS,
  DESTRUCTIVE_BUTTON_CLASS,
} from "@/components/console-ui";

/**
 * Cloud skills are task recipes persisted server-side per user
 * (`cmd/allternit-api/src/skills_routes.rs`). The richer desktop registry in
 * the ai surface (network/DOM/API modes, confidence scores) is local-only and
 * intentionally not mirrored here — this page talks to the real cloud routes.
 */

export interface CloudSkill {
  id: string;
  user_id: string;
  organization_id?: string | null;
  name: string;
  description?: string | null;
  goal_template: string;
  parameters: Record<string, unknown>;
  allowed_sites?: unknown;
  run_count: number;
  created_at: string;
  updated_at: string;
}

const INPUT_CLASS =
  "w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]";

function formatTime(iso?: string | null): string {
  if (!iso) return "—";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  const diff = Date.now() - date.getTime();
  if (diff < 60_000) return "Just now";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} min ago`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} hr ago`;
  return date.toLocaleDateString();
}

export function SkillsPage(): React.ReactNode {
  const [skills, setSkills] = useState<CloudSkill[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [showCreate, setShowCreate] = useState(false);
  const [creating, setCreating] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [runResult, setRunResult] = useState<{
    skillId: string;
    goal: string;
  } | null>(null);

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [goalTemplate, setGoalTemplate] = useState("");
  const [parametersJson, setParametersJson] = useState("{}");

  const load = useCallback(async (q?: string) => {
    setLoading(true);
    setError(null);
    try {
      const data = await api.get<{ skills: CloudSkill[] }>(
        `/api/v1/skills${q ? `?search=${encodeURIComponent(q)}` : ""}`
      );
      setSkills(data.skills ?? []);
    } catch (err) {
      setError(formatApiError(err, "Unable to load skills"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const handleSearch = useCallback(
    (value: string) => {
      setSearch(value);
      void load(value.trim() || undefined);
    },
    [load]
  );

  const handleCreate = useCallback(async () => {
    setCreating(true);
    setError(null);
    try {
      let parameters: Record<string, unknown> = {};
      if (parametersJson.trim()) {
        const parsed = JSON.parse(parametersJson) as unknown;
        if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
          throw new SyntaxError("parameters must be a JSON object");
        }
        parameters = parsed as Record<string, unknown>;
      }
      await api.post("/api/v1/skills", {
        name: name.trim(),
        description: description.trim() || undefined,
        goal_template: goalTemplate.trim(),
        parameters,
      });
      setShowCreate(false);
      setName("");
      setDescription("");
      setGoalTemplate("");
      setParametersJson("{}");
      await load(search.trim() || undefined);
    } catch (err) {
      setError(
        err instanceof SyntaxError
          ? `Parameters JSON is invalid: ${err.message}`
          : formatApiError(err, "Unable to create skill")
      );
    } finally {
      setCreating(false);
    }
  }, [name, description, goalTemplate, parametersJson, load, search]);

  const handleRun = useCallback(async (skill: CloudSkill) => {
    setBusyId(skill.id);
    setError(null);
    setRunResult(null);
    try {
      const data = await api.post<{
        skill_id: string;
        goal: string;
      }>(`/api/v1/skills/${skill.id}/run`, { parameters: skill.parameters });
      setRunResult({ skillId: skill.id, goal: data.goal });
      await load(search.trim() || undefined);
    } catch (err) {
      setError(formatApiError(err, "Run failed"));
    } finally {
      setBusyId(null);
    }
  }, [load, search]);

  const handleDelete = useCallback(
    async (skill: CloudSkill) => {
      if (!window.confirm(`Delete skill "${skill.name}"?`)) return;
      setBusyId(skill.id);
      setError(null);
      try {
        await api.delete(`/api/v1/skills/${skill.id}`);
        await load(search.trim() || undefined);
      } catch (err) {
        setError(formatApiError(err, "Delete failed"));
      } finally {
        setBusyId(null);
      }
    },
    [load, search]
  );

  return (
    <ListPage
      title="Skills"
      subtitle="Reusable task recipes your agents can run — saved goals with parameters, scoped to your account."
      searchPlaceholder="Search skills…"
      onSearch={handleSearch}
      primaryAction={{
        label: "New skill",
        onClick: () => setShowCreate((v) => !v),
      }}
    >
      {error && (
        <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      )}

      <p className="mb-4 m-0 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[12px] text-[var(--text-tertiary)]">
        The cloud registry stores parameterized task recipes. The full desktop
        skill registry (network/DOM/API modes with confidence scores) lives in
        the ai surface and is not part of the cloud API.
      </p>

      {showCreate && (
        <div className="mb-4 space-y-3 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="space-y-1">
              <label htmlFor="skill-name" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                Name
              </label>
              <input
                id="skill-name"
                type="text"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Weekly competitor scan"
                className={INPUT_CLASS}
              />
            </div>
            <div className="space-y-1">
              <label htmlFor="skill-description" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                Description
              </label>
              <input
                id="skill-description"
                type="text"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="What this skill does and when to use it"
                className={INPUT_CLASS}
              />
            </div>
          </div>
          <div className="space-y-1">
            <label htmlFor="skill-goal" className="text-[12px] font-semibold text-[var(--text-secondary)]">
              Goal template — use {"{{parameter}}"} placeholders
            </label>
            <textarea
              id="skill-goal"
              value={goalTemplate}
              onChange={(e) => setGoalTemplate(e.target.value)}
              placeholder={"Summarize the top {{count}} stories on {{topic}}"}
              rows={3}
              className={cn(INPUT_CLASS, "resize-y font-mono text-[12px]")}
            />
          </div>
          <div className="space-y-1">
            <label htmlFor="skill-params" className="text-[12px] font-semibold text-[var(--text-secondary)]">
              Default parameters (JSON object)
            </label>
            <textarea
              id="skill-params"
              value={parametersJson}
              onChange={(e) => setParametersJson(e.target.value)}
              placeholder='{"count": 5, "topic": "agent infrastructure"}'
              rows={3}
              className={cn(INPUT_CLASS, "resize-y font-mono text-[12px]")}
            />
          </div>
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={() => void handleCreate()}
              disabled={creating || name.trim().length < 2 || !goalTemplate.trim()}
              className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              <HugeiconsIcon icon={PlusSignIcon} size={13} />
              {creating ? "Saving…" : "Save skill"}
            </button>
            <button type="button" onClick={() => setShowCreate(false)} className={QUIET_BUTTON_CLASS}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {loading ? (
        <SkeletonRow lines={5} />
      ) : skills.length === 0 ? (
        <EmptyState
          art="search"
          title={search ? "No skills match your search" : "No skills yet"}
          caption={
            search
              ? "Try a different name, description, or goal."
              : "Save a successful agent run as a reusable, parameterized task recipe."
          }
          ctaLabel={search ? undefined : "New skill"}
          onCtaClick={search ? undefined : () => setShowCreate(true)}
        />
      ) : (
        <div className="space-y-2">
          {skills.map((skill) => (
            <div
              key={skill.id}
              className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4"
            >
              <div className="flex flex-wrap items-start justify-between gap-3">
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <h3 className="m-0 text-[14px] font-semibold text-[var(--text-primary)]">
                      {skill.name}
                    </h3>
                    <MonoChip>{skill.id}</MonoChip>
                    <Badge>{skill.run_count} run{skill.run_count === 1 ? "" : "s"}</Badge>
                  </div>
                  {skill.description && (
                    <p className="m-0 mt-1 text-[13px] text-[var(--text-secondary)]">
                      {skill.description}
                    </p>
                  )}
                  <p className="m-0 mt-2 font-mono text-[12px] leading-relaxed whitespace-pre-wrap text-[var(--text-tertiary)]">
                    {skill.goal_template}
                  </p>
                  <p className="m-0 mt-2 text-[11px] text-[var(--text-tertiary)]">
                    Updated {formatTime(skill.updated_at)}
                    {Object.keys(skill.parameters ?? {}).length > 0 &&
                      ` · parameters: ${Object.keys(skill.parameters).join(", ")}`}
                  </p>
                  {runResult?.skillId === skill.id && (
                    <div className="mt-3 rounded-lg border border-solid border-[var(--accent-primary)]/30 bg-[var(--accent-primary)]/5 p-3">
                      <p className="m-0 text-[11px] font-semibold uppercase tracking-wide text-[var(--text-tertiary)]">
                        Rendered goal
                      </p>
                      <p className="m-0 mt-1 whitespace-pre-wrap font-mono text-[12px] text-[var(--text-primary)]">
                        {runResult.goal}
                      </p>
                    </div>
                  )}
                </div>
                <div className="flex shrink-0 gap-1.5">
                  <button
                    type="button"
                    onClick={() => void handleRun(skill)}
                    disabled={busyId === skill.id}
                    className={cn(QUIET_BUTTON_CLASS, "px-2.5 py-1 text-[12px]")}
                  >
                    <HugeiconsIcon icon={PlayIcon} size={12} />
                    {busyId === skill.id ? "Running…" : "Run"}
                  </button>
                  <button
                    type="button"
                    onClick={() => void handleDelete(skill)}
                    disabled={busyId === skill.id}
                    className={cn(DESTRUCTIVE_BUTTON_CLASS, "px-2.5 py-1 text-[12px]")}
                  >
                    <HugeiconsIcon icon={TrashIcon} size={12} />
                    Delete
                  </button>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}
    </ListPage>
  );
}

export default SkillsPage;
