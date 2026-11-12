import React, { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { formatApiError } from "@/lib/api-client";
import { getProjectLimits, type LimitSource, type ProjectLimits } from "@/lib/platform-projects";
import { SkeletonCard, QUIET_BUTTON_CLASS } from "@/components/console-ui";
import {
  CARD_CLASS,
  ErrorBanner,
  PlatformHeader,
  ProjectGate,
  formatDateTime,
  usePlatformProjects,
} from "../platform/shared";

function sourceLabel(source: LimitSource): string {
  return source === "override" ? "Project override" : "Plan";
}

function Row({ label, value, hint }: { label: string; value: React.ReactNode; hint?: string }) {
  return (
    <div className="flex flex-wrap items-baseline justify-between gap-2 border-0 border-b border-solid border-[var(--border-subtle)] py-3 last:border-b-0">
      <div className="min-w-0">
        <div className="text-[13px] font-medium text-[var(--text-primary)]">{label}</div>
        {hint && <div className="text-[12px] text-[var(--text-tertiary)]">{hint}</div>}
      </div>
      <div className="text-[13px] text-[var(--text-secondary)]">{value}</div>
    </div>
  );
}

function LimitsCard({ projectId }: { projectId: string }) {
  const [limits, setLimits] = useState<ProjectLimits | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setLimits(await getProjectLimits(projectId));
    } catch (err) {
      setLimits(null);
      setError(formatApiError(err, "Unable to load rate limits for this project"));
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    void load();
  }, [load]);

  if (loading) return <SkeletonCard rows={3} />;
  if (error || !limits) {
    return (
      <div className="space-y-3">
        <ErrorBanner message={error ?? "Unable to load rate limits for this project"} />
        <button type="button" onClick={() => void load()} className={QUIET_BUTTON_CLASS}>
          Retry
        </button>
      </div>
    );
  }

  const rl = limits.rate_limit;
  const usage =
    rl.used === null || rl.remaining === null
      ? "Not available right now"
      : `${rl.used.toLocaleString()} used, ${rl.remaining.toLocaleString()} remaining`;

  return (
    <div className={`${CARD_CLASS} px-4`}>
      <Row
        label="Requests per minute"
        value={
          <>
            {rl.requests_per_minute.toLocaleString()}{" "}
            <span className="text-[var(--text-tertiary)]">({sourceLabel(rl.source)})</span>
          </>
        }
      />
      <Row
        label="Current window"
        hint={`${rl.window_seconds}-second window`}
        value={
          <>
            {usage}
            <div className="text-[12px] text-[var(--text-tertiary)]">Resets {formatDateTime(rl.resets_at)}</div>
          </>
        }
      />
      <Row
        label="Concurrent calls"
        value={
          <>
            {limits.concurrent_calls.limit.toLocaleString()}{" "}
            <span className="text-[var(--text-tertiary)]">({sourceLabel(limits.concurrent_calls.source)})</span>
          </>
        }
      />
    </div>
  );
}

/** Read-only per-project rate limits, shown on Allternit Cloud. */
export function ProjectRateLimits(): React.ReactNode {
  const state = usePlatformProjects();
  const linkClass = "font-semibold text-[var(--accent-primary)] hover:underline";
  return (
    <div className="mx-auto w-full max-w-3xl px-4 py-6">
      <PlatformHeader
        title="Rate limits"
        subtitle="Limits that apply to requests made with this project's API keys."
        state={state}
      />
      <ProjectGate state={state}>{({ project }) => <LimitsCard projectId={project.id} />}</ProjectGate>
      <p className="mt-4 text-[12px] text-[var(--text-tertiary)]">
        These limits are read-only here. Spend and plan changes are on{" "}
        <Link to="/platform/billing" className={linkClass}>
          Project billing
        </Link>
        . Keys and the spend cap for a project are on the{" "}
        <Link to="/platform/projects" className={linkClass}>
          Projects page
        </Link>
        .
      </p>
    </div>
  );
}

export default ProjectRateLimits;
