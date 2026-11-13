import React, { useEffect, useMemo, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import { Analytics01Icon, ArrowRight01Icon, SparklesIcon } from "@hugeicons/core-free-icons";
import {
  Bar,
  BarChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { cn } from "@/lib/utils";
import { AllternitApiError } from "@/lib/api-client";
import {
  EmptyState,
  GaugeCard,
  ModelCard,
  ResourceCard,
  SkeletonCard,
} from "@/components/console-ui";
import { getConsoleCredits, type ConsoleCredits } from "@/lib/credits";
import { getSpendLimit, type SpendLimit } from "@/lib/spend-limits";
import { isCloudGateway, isHiddenOnCloud } from "@/lib/availability";
import {
  dateDaysAgo,
  formatTokenCount,
  getGatewayCaching,
  getGatewayUsage,
  getUsageReport,
  sumReportRows,
  microdollarsToUsd,
  tomorrowDate,
  type CachingResponse,
  type UsageDayRow,
  type UsageResponse,
} from "@/lib/gateway-analytics";
import { fetchConsoleModels, type ConsoleModel } from "@/lib/console-models";
import { usePlatformOrganization, usePlatformUser } from "@/lib/platform-auth-client";

const MUTED = "#71717a";

type LoadState<T> =
  | { status: "loading" }
  | { status: "ok"; data: T }
  | { status: "error" }
  | { status: "hidden" };

function greetingForHour(hour: number): string {
  if (hour < 12) return "Good morning";
  if (hour < 18) return "Good afternoon";
  return "Good evening";
}

function firstOfNextMonth(): string {
  const now = new Date();
  return new Date(now.getFullYear(), now.getMonth() + 1, 1).toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
  });
}

function formatContextWindow(tokens?: number): string | null {
  if (!tokens) return null;
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(0)}M`;
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}K`;
  return tokens.toLocaleString();
}

/** Honest generic taglines derived from the catalog metadata we actually get. */
function taglineForModel(model: ConsoleModel): string {
  const context = formatContextWindow(model.context_window);
  const parts: string[] = [];
  switch (model.quality_tier) {
    case "fast":
      parts.push("Low-latency, cost-efficient model for everyday tasks");
      break;
    case "high":
      parts.push("High-capability model for complex work");
      break;
    case "reasoning":
      parts.push("Reasoning model that works through problems step by step");
      break;
    default:
      parts.push(`${model.owned_by} model`);
  }
  if (context) parts.push(`${context} context window`);
  return parts.join(" · ");
}

function badgesForModel(model: ConsoleModel): string[] {
  const badges: string[] = [];
  if (model.quality_tier) badges.push(model.quality_tier);
  if (model.context_window && model.context_window >= 500_000) badges.push("long context");
  return badges;
}

function centsToUsd(cents: number): number {
  return cents / 100;
}

function formatUsd(value: number): string {
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  }).format(value);
}

/** Compact per-card fallback when an endpoint fails — not a wall of errors. */
function CardFallback({ label, message }: { label: string; message: string }): React.ReactNode {
  return (
    <div className="flex h-full flex-col rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
      <span className="text-[12px] font-medium uppercase tracking-wide text-[var(--text-tertiary)]">
        {label}
      </span>
      <p className="m-0 mt-3 flex-1 text-[13px] text-[var(--text-tertiary)]">{message}</p>
    </div>
  );
}

function ActionLink({ to, label }: { to: string; label: string }): React.ReactNode {
  return (
    <Link
      to={to}
      className="mt-3 inline-flex items-center gap-1 self-start rounded-lg border border-solid border-[var(--border-subtle)] px-3 py-1.5 text-[13px] font-medium text-[var(--text-primary)] transition-colors hover:bg-[var(--bg-primary)]"
    >
      {label}
      <HugeiconsIcon icon={ArrowRight01Icon} size={13} />
    </Link>
  );
}

export function DashboardPage(): React.ReactNode {
  const { user } = usePlatformUser();
  const { organization, membership } = usePlatformOrganization();
  const navigate = useNavigate();

  const [credits, setCredits] = useState<LoadState<ConsoleCredits>>({ status: "loading" });
  const [spend, setSpend] = useState<LoadState<SpendLimit>>(
    isCloudGateway() ? { status: "hidden" } : { status: "loading" },
  );
  // Caching is a paired-runtime route; on cloud the card is hidden.
  const [caching, setCaching] = useState<LoadState<CachingResponse>>(
    isHiddenOnCloud("caching") ? { status: "hidden" } : { status: "loading" },
  );
  const [usage, setUsage] = useState<LoadState<UsageResponse>>({ status: "loading" });
  const [models, setModels] = useState<LoadState<ConsoleModel[]>>({ status: "loading" });

  useEffect(() => {
    let active = true;
    // Cloud wallet first (cloud-api, works for every account); the device
    // (org fabric) ledger only shows when a paired node answers.
    getConsoleCredits()
      .then((data) => {
        if (!active) return;
        setCredits(data.cloud || data.device ? { status: "ok", data } : { status: "error" });
      })
      .catch(() => active && setCredits({ status: "error" }));
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    // No /api/v1/admin/spend-limits on Allternit Cloud; the card stays hidden.
    if (isCloudGateway()) return;
    let active = true;
    getSpendLimit()
      .then((data) => active && setSpend({ status: "ok", data }))
      .catch((err) => {
        if (!active) return;
        // Admin-only route — non-admin members don't get a gauge.
        if (err instanceof AllternitApiError && err.statusCode === 403) {
          setSpend({ status: "hidden" });
        } else {
          setSpend({ status: "error" });
        }
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    if (isHiddenOnCloud("caching")) return;
    let active = true;
    getGatewayCaching("30d")
      .then((data) => active && setCaching({ status: "ok", data }))
      .catch(() => active && setCaching({ status: "error" }));
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    // Cloud reads the per-user report; the relayed /api/v1/gateway/usage needs a paired runtime.
    const request: Promise<UsageResponse> = isCloudGateway()
      ? getUsageReport({ from: dateDaysAgo(7), to: tomorrowDate(), groupBy: "day" }).then((rows) => {
          const totals = sumReportRows(rows);
          return {
            from: dateDaysAgo(7),
            to: tomorrowDate(),
            group_by: "day" as const,
            usage: rows.map((r) => ({
              day: r.group,
              prompt_tokens: r.prompt_tokens,
              completion_tokens: r.completion_tokens,
              requests: r.requests,
            })) as unknown as UsageDayRow[],
            totals: {
              requests: totals.requests,
              prompt_tokens: totals.prompt_tokens,
              completion_tokens: totals.completion_tokens,
              reasoning_tokens: 0,
              cached_tokens: 0,
              spend_microdollars: Math.round(totals.cost_usd * 1_000_000),
            },
          };
        })
      : getGatewayUsage({ from: dateDaysAgo(7), to: tomorrowDate() });
    request
      .then((data) => active && setUsage({ status: "ok", data }))
      .catch(() => active && setUsage({ status: "error" }));
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    fetchConsoleModels()
      .then((list) => active && setModels({ status: "ok", data: list }))
      .catch(() => active && setModels({ status: "error" }));
    return () => {
      active = false;
    };
  }, []);

  const firstName =
    user?.firstName || user?.primaryEmailAddress?.emailAddress?.split("@")[0] || "there";

  // ─── Card 1: credits (Cloud wallet, plus device ledger when paired) ────────
  const creditsCard = useMemo(() => {
    if (credits.status === "loading") return <SkeletonCard rows={2} className="h-full" />;
    if (credits.status === "error") return <CardFallback label="Cloud credits" message="Balance is unavailable right now." />;
    if (credits.status === "hidden" || !credits.data) return null;
    const { cloud, device } = credits.data;
    const deviceUsd = device ? device.balance_cents / 100 : null;
    return (
      <div className="flex h-full flex-col rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
        <span className="text-[12px] font-medium uppercase tracking-wide text-[var(--text-tertiary)]">
          {cloud ? "Cloud credits" : "Device credits"}
        </span>
        <div className="mt-2 text-[24px] font-semibold tracking-tight text-[var(--text-primary)]">
          {formatUsd(cloud ? cloud.balance_usd : deviceUsd ?? 0)}
        </div>
        <div className="mt-1 flex-1 space-y-0.5 text-[12px] text-[var(--text-tertiary)]">
          {cloud && (
            <p className="m-0">
              {formatUsd(cloud.month_to_date_usage_usd)} used this month
              {cloud.free_inference
                ? ` · ${formatUsd(cloud.free_inference.remaining_usd)} free left`
                : ""}
            </p>
          )}
          {cloud && deviceUsd !== null && (
            <p className="m-0">
              Device credits · <span className="text-[var(--text-secondary)]">{formatUsd(deviceUsd)}</span>
            </p>
          )}
          {!cloud && (
            <p className="m-0">
              {organization?.name ?? "Your organization"}
              {membership?.role ? ` · ${membership.role}` : ""}
            </p>
          )}
        </div>
        <ActionLink to="/billing" label="Add funds" />
      </div>
    );
  }, [credits, organization?.name, membership?.role]);

  // ─── Card 2: spend this month ───────────────────────────────────────────────
  const spendCard = useMemo(() => {
    if (spend.status === "loading") return <SkeletonCard rows={2} className="h-full" />;
    if (spend.status === "error") return <CardFallback label="Spend this month" message="Spend is unavailable right now." />;
    if (spend.status !== "ok" || spend.data.monthly_usd_cap <= 0) return null;
    return (
      <GaugeCard
        className="h-full"
        label="Spend this month"
        used={centsToUsd(spend.data.current_month_spend)}
        limit={centsToUsd(spend.data.monthly_usd_cap)}
        resetHint={`Resets ${firstOfNextMonth()}`}
      />
    );
  }, [spend]);

  // ─── Card 3: prompt caching ─────────────────────────────────────────────────
  const cachingCard = useMemo(() => {
    if (caching.status === "loading") return <SkeletonCard rows={2} className="h-full" />;
    if (caching.status === "error") return <CardFallback label="Prompt caching" message="Caching stats are unavailable right now." />;
    if (caching.status !== "ok") return null;
    const { totals } = caching.data;
    const hits = totals.context_cache_hits + totals.prompt_cache_hits;
    const hitRate = totals.requests > 0 ? (hits / totals.requests) * 100 : null;
    const savings = microdollarsToUsd(totals.estimated_savings_microdollars);
    return (
      <div className="flex h-full flex-col rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
        <span className="text-[12px] font-medium uppercase tracking-wide text-[var(--text-tertiary)]">
          Prompt caching
        </span>
        <div className="mt-2 text-[24px] font-semibold tracking-tight text-[var(--text-primary)]">
          {hitRate === null ? "—" : `${hitRate.toFixed(0)}%`}
          <span className="ml-1 text-[13px] font-normal text-[var(--text-tertiary)]">hit rate</span>
        </div>
        <p className="m-0 mt-1 flex-1 text-[12px] text-[var(--text-tertiary)]">
          {totals.estimated_savings_microdollars > 0
            ? `~${savings} saved in the last 30 days`
            : "Reuse prefixes and context caches to cut repeat input cost."}
        </p>
        <ActionLink to="/analytics/caching" label="Set up" />
      </div>
    );
  }, [caching]);

  // ─── Card 4: token volume (last 7 days) ─────────────────────────────────────
  const usageCard = useMemo(() => {
    if (usage.status === "loading") return <SkeletonCard rows={3} className="h-full" />;
    if (usage.status === "error") return <CardFallback label="Token volume" message="Usage is unavailable right now." />;
    if (usage.status !== "ok") return null;
    const dayRows = usage.data.group_by === "day" ? (usage.data.usage as UsageDayRow[]) : [];
    const totalTokens =
      usage.data.totals.prompt_tokens + usage.data.totals.completion_tokens;
    if (dayRows.length === 0 || totalTokens === 0) {
      return (
        <div className="flex h-full flex-col rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
          <span className="text-[12px] font-medium uppercase tracking-wide text-[var(--text-tertiary)]">
            Token volume
          </span>
          <EmptyState
            className="flex-1 py-6"
            icon={<HugeiconsIcon icon={Analytics01Icon} size={24} />}
            title="No activity in the last 7 days"
            caption="Send your first request and usage will show up here."
            ctaLabel="Try a prompt"
            onCtaClick={() => navigate("/playground")}
          />
        </div>
      );
    }
    const byDay = new Map<string, { day: string; tokens: number }>();
    for (const row of dayRows) {
      const existing = byDay.get(row.day) ?? { day: row.day, tokens: 0 };
      existing.tokens += row.prompt_tokens + row.completion_tokens;
      byDay.set(row.day, existing);
    }
    const chart = Array.from(byDay.values())
      .sort((a, b) => a.day.localeCompare(b.day))
      .map((d) => ({
        day: new Date(`${d.day}T00:00:00Z`).toLocaleDateString("en-US", {
          month: "short",
          day: "numeric",
          timeZone: "UTC",
        }),
        tokens: d.tokens,
      }));
    return (
      <div className="flex h-full flex-col rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
        <div className="flex items-baseline justify-between gap-2">
          <span className="text-[12px] font-medium uppercase tracking-wide text-[var(--text-tertiary)]">
            Token volume
          </span>
          <span className="text-[13px] font-semibold text-[var(--text-primary)]">
            {formatTokenCount(totalTokens)}
            <span className="ml-1 font-normal text-[var(--text-tertiary)]">7d</span>
          </span>
        </div>
        <div className="mt-2 h-24 flex-1">
          <ResponsiveContainer width="100%" height="100%">
            <BarChart data={chart} margin={{ left: 0, right: 0, top: 0, bottom: 0 }}>
              <CartesianGrid stroke="var(--border-subtle)" vertical={false} />
              <XAxis
                dataKey="day"
                axisLine={false}
                tickLine={false}
                tick={{ fill: MUTED, fontSize: 9 }}
              />
              <YAxis hide domain={[0, "dataMax"]} />
              <Tooltip
                contentStyle={{
                  backgroundColor: "var(--bg-secondary)",
                  border: "1px solid var(--border-subtle)",
                  borderRadius: "8px",
                }}
                formatter={(value: number) => formatTokenCount(value)}
              />
              <Bar dataKey="tokens" name="Tokens" fill="var(--accent-primary)" radius={[2, 2, 0, 0]} />
            </BarChart>
          </ResponsiveContainer>
        </div>
      </div>
    );
  }, [usage]);

  // ─── Model cards ────────────────────────────────────────────────────────────
  const modelCards = useMemo(() => {
    if (models.status === "loading") {
      return (
        <>
          <SkeletonCard rows={2} />
          <SkeletonCard rows={2} />
          <SkeletonCard rows={2} />
        </>
      );
    }
    if (models.status === "error") {
      return (
        <div className="col-span-full">
          <CardFallback label="Models" message="The model catalog is unavailable right now." />
        </div>
      );
    }
    if (models.status !== "ok" || models.data.length === 0) {
      return (
        <div className="col-span-full">
          <CardFallback label="Models" message="No models are enabled for this organization yet." />
        </div>
      );
    }
    return models.data.slice(0, 6).map((model) => (
      <ModelCard
        key={model.id}
        name={model.display_name || model.id}
        tagline={taglineForModel(model)}
        badges={badgesForModel(model)}
      >
        <dl className="m-0 grid grid-cols-1 gap-x-4 gap-y-1.5 text-[12px]">
          <div className="flex justify-between gap-2">
            <dt className="text-[var(--text-tertiary)]">Model ID</dt>
            <dd className="m-0 font-mono text-[11px] text-[var(--text-primary)]">{model.id}</dd>
          </div>
          <div className="flex justify-between gap-2">
            <dt className="text-[var(--text-tertiary)]">Input</dt>
            <dd className="m-0 text-[var(--text-primary)]">
              {model.pricing?.input_cents_per_1m != null
                ? `$${(model.pricing.input_cents_per_1m / 100).toFixed(2)} / 1M tokens`
                : "—"}
            </dd>
          </div>
          <div className="flex justify-between gap-2">
            <dt className="text-[var(--text-tertiary)]">Output</dt>
            <dd className="m-0 text-[var(--text-primary)]">
              {model.pricing?.output_cents_per_1m != null
                ? `$${(model.pricing.output_cents_per_1m / 100).toFixed(2)} / 1M tokens`
                : "—"}
            </dd>
          </div>
          <div className="flex justify-between gap-2">
            <dt className="text-[var(--text-tertiary)]">Context window</dt>
            <dd className="m-0 text-[var(--text-primary)]">
              {formatContextWindow(model.context_window) ?? "—"}
            </dd>
          </div>
        </dl>
        <Link
          to={`/playground?model=${encodeURIComponent(model.id)}`}
          className="mt-3 inline-flex items-center gap-1 rounded-lg border border-solid border-[var(--border-subtle)] px-3 py-1.5 text-[13px] font-medium text-[var(--text-primary)] transition-colors hover:bg-[var(--bg-primary)]"
        >
          Try in playground
          <HugeiconsIcon icon={ArrowRight01Icon} size={13} />
        </Link>
      </ModelCard>
    ));
  }, [models]);

  return (
    <div className="space-y-8">
      {/* Greeting + top actions (Anthropic dashboard pattern) */}
      <div className="flex flex-col gap-4 sm:flex-row sm:items-end sm:justify-between">
        <div>
          <h1 className="m-0 text-[24px] font-semibold tracking-tight text-[var(--text-primary)]">
            {greetingForHour(new Date().getHours())}, {firstName}.
          </h1>
          <p className="m-0 mt-1 text-[13px] text-[var(--text-secondary)]">
            Here&apos;s what&apos;s happening across your Allternit organization.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Link
            to="/api-keys"
            className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110"
          >
            Get API key
          </Link>
          <Link
            to="/builder"
            className={cn(
              "inline-flex items-center gap-1.5 rounded-lg border border-solid border-[var(--border-subtle)] px-3.5 py-2 text-[13px] font-semibold text-[var(--text-primary)] transition-colors hover:bg-[var(--bg-secondary)]"
            )}
          >
            <HugeiconsIcon icon={SparklesIcon} size={14} />
            Build an agent
          </Link>
        </div>
      </div>

      {/* Row 1: credits / spend / caching / token volume */}
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-4">
        {creditsCard}
        {spendCard}
        {cachingCard}
        {usageCard}
      </div>

      {/* Row 2: models */}
      <section>
        <div className="mb-3 flex items-end justify-between">
          <h2 className="m-0 text-[16px] font-semibold text-[var(--text-primary)]">Models</h2>
          <Link
            to="/models"
            className="text-[12px] text-[var(--text-tertiary)] underline underline-offset-2 transition-colors hover:text-[var(--text-secondary)]"
          >
            Compare models
          </Link>
        </div>
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3">{modelCards}</div>
      </section>

      {/* Row 3: resources */}
      <section>
        <h2 className="m-0 mb-3 text-[16px] font-semibold text-[var(--text-primary)]">Resources</h2>
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-4">
          {!isHiddenOnCloud("batches") && (
            <ResourceCard
              title="Batches"
              description="Send large volumes of requests asynchronously and retrieve results when they complete."
              action={{ label: "Go to Batches", to: "/batches" }}
            />
          )}
          {!isHiddenOnCloud("caching") && (
            <ResourceCard
              title="Prompt caching"
              description="Reuse long prefixes and context caches to cut repeat input cost on every request."
              action={{ label: "Go to caching", to: "/analytics/caching" }}
            />
          )}
          <ResourceCard
            title="Cloud Agents"
            description="Build, deploy, and monitor agents that run in the Allternit cloud."
            action={{ label: "Go to Agents", to: "/agents" }}
          />
          <ResourceCard
            title="Documentation"
            description="Guides and API references for the gateway, SDKs, and console surfaces."
            action={{ label: "Read the docs", to: "/docs" }}
          />
        </div>
      </section>
    </div>
  );
}

export default DashboardPage;
