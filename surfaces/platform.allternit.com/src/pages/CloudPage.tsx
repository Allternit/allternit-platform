import React, { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import {
  ArrowRight,
  ArrowSquareOut,
  CaretDown,
  CaretUp,
  Check,
  Coins,
  Copy,
  Key,
  LockSimple,
  MagnifyingGlass,
  Stack,
  Warning,
} from "@phosphor-icons/react";
import { PublicPageShell } from "@/components/PublicPageShell";
import { SignInModalButton } from "@/components/SignInModalButton";
import { fetchLiveModelCatalog, type LiveModelInfo } from "@/lib/model-catalog";
import { usePlatformAuth } from "@/lib/platform-auth-client";
import { cn } from "@/lib/utils";

const BASE_URL = "https://api.allternit.com/v1";
const QUICKSTART_MODEL = "deepseek-v4-pro";
const KEY_PATH = "/api-keys?create=inference";
const BILLING_PATH = "/billing";
const DOCS = "https://docs.allternit.com";

function cloudApiBase(): string {
  return String(import.meta.env.VITE_ALLTERNIT_CLOUD_API_URL || "https://api.allternit.com").replace(
    /\/$/,
    "",
  );
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

/** USD per token -> "$x.xx" per 1M tokens. */
export function formatPerMillion(perToken: number | null | undefined): string {
  if (perToken === null || perToken === undefined || !Number.isFinite(perToken)) return "—";
  const perMillion = perToken * 1e6;
  if (perMillion === 0) return "$0.00";
  if (perMillion < 0.01) return `$${perMillion.toPrecision(2)}`;
  return `$${perMillion.toFixed(2)}`;
}

export function formatContext(tokens: number | null | undefined): string {
  if (!tokens || !Number.isFinite(tokens)) return "—";
  if (tokens >= 1_000_000) {
    const m = tokens / 1_000_000;
    return `${m.toFixed(1).replace(/\.0$/, "")}M`;
  }
  if (tokens >= 1000) return `${Math.round(tokens / 1000)}K`;
  return String(tokens);
}

function formatUsd(value: number): string {
  return Number.isInteger(value) ? `$${value}` : `$${value.toFixed(2)}`;
}

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

interface CloudModel {
  id: string;
  name: string;
  provider: string;
  context: number | null;
  lane: "standard" | "open";
  prompt: number | null;
  completion: number | null;
}

function toCloudModel(m: LiveModelInfo): CloudModel {
  return {
    id: m.id,
    name: m.name ?? m.id,
    provider: m.provider ?? m.owned_by ?? "—",
    context: typeof m.context_length === "number" ? m.context_length : null,
    lane: m.lane === "open" ? "open" : "standard",
    prompt: typeof m.prompt_price === "number" ? m.prompt_price : null,
    completion: typeof m.completion_price === "number" ? m.completion_price : null,
  };
}

interface CreditPack {
  id: string;
  label: string;
  credits_usd: number;
  price_usd: number;
}

interface BillingPlan {
  id: string;
  label: string;
  price_usd: number;
  monthly_credits_usd: number;
  rollover_cap_usd?: number;
}

type Load<T> = { status: "loading" } | { status: "error"; message: string } | { status: "ready"; data: T };

function useJson<T>(path: string, pick: (payload: unknown) => T): [Load<T>, () => void] {
  const [state, setState] = useState<Load<T>>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    const controller = new AbortController();
    setState({ status: "loading" });
    fetch(`${cloudApiBase()}${path}`, { headers: { Accept: "application/json" }, signal: controller.signal })
      .then(async (res) => {
        if (!res.ok) throw new Error(`Request failed (${res.status})`);
        return pick(await res.json());
      })
      .then((data) => setState({ status: "ready", data }))
      .catch((err: unknown) => {
        if (controller.signal.aborted) return;
        setState({ status: "error", message: err instanceof Error ? err.message : "Request failed" });
      });
    return () => controller.abort();
    // pick is a stable module-level function at every call site
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path, attempt]);

  return [state, () => setAttempt((n) => n + 1)];
}

const pickPacks = (p: unknown): CreditPack[] => {
  const packs = (p as { packs?: CreditPack[] })?.packs;
  return Array.isArray(packs) ? packs : [];
};
const pickPlans = (p: unknown): BillingPlan[] => {
  const plans = (p as { plans?: BillingPlan[] })?.plans;
  return Array.isArray(plans) ? plans : [];
};

function useCloudModels(): [Load<CloudModel[]>, () => void] {
  const [state, setState] = useState<Load<CloudModel[]>>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    const controller = new AbortController();
    setState({ status: "loading" });
    fetchLiveModelCatalog(controller.signal)
      .then((res) => setState({ status: "ready", data: (res.data ?? []).map(toCloudModel) }))
      .catch((err: unknown) => {
        if (controller.signal.aborted) return;
        setState({
          status: "error",
          message: err instanceof Error ? err.message : "Unable to load models",
        });
      });
    return () => controller.abort();
  }, [attempt]);

  return [state, () => setAttempt((n) => n + 1)];
}

// ---------------------------------------------------------------------------
// Small pieces
// ---------------------------------------------------------------------------

async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const textarea = document.createElement("textarea");
    textarea.value = text;
    document.body.appendChild(textarea);
    textarea.select();
    try {
      document.execCommand("copy");
    } catch {
      // ignore
    }
    document.body.removeChild(textarea);
  }
}

function CopyButton({ text, label = "Copy" }: { text: string; label?: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      onClick={() => {
        void copyText(text).then(() => {
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1600);
        });
      }}
      className="inline-flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-[12px] font-medium text-[var(--text-secondary)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)]"
      aria-label={copied ? "Copied" : label}
    >
      {copied ? <Check size={13} className="text-[var(--status-success)]" /> : <Copy size={13} />}
      {copied ? "Copied" : label}
    </button>
  );
}

function SectionHeading({ id, title, children }: { id: string; title: string; children?: React.ReactNode }) {
  return (
    <div className="max-w-2xl">
      <h2 id={id} className="scroll-mt-20 text-[26px] font-bold tracking-tight text-[var(--text-primary)] md:text-[32px]">
        {title}
      </h2>
      {children && (
        <p className="mt-3 text-[15px] leading-relaxed text-[var(--text-secondary)]">{children}</p>
      )}
    </div>
  );
}

function ErrorRow({ message, onRetry }: { message: string; onRetry: () => void }) {
  return (
    <div className="flex flex-wrap items-center gap-3 rounded-xl border border-[var(--status-error)]/25 bg-[var(--status-error)]/[0.06] px-4 py-3 text-[13px] text-[var(--text-primary)]">
      <Warning size={16} className="shrink-0 text-[var(--status-error)]" />
      <span className="min-w-0 flex-1">{message}</span>
      <button
        type="button"
        onClick={onRetry}
        className="rounded-lg border border-[var(--border-default)] px-3 py-1 text-[12px] font-medium transition-colors hover:bg-[var(--surface-hover)]"
      >
        Try again
      </button>
    </div>
  );
}

const PRIMARY_BTN =
  "inline-flex items-center justify-center gap-2 rounded-lg bg-[#D97757] px-5 py-2.5 text-[14px] font-semibold text-[#FDF8F3] transition-all hover:brightness-110";
const SECONDARY_BTN =
  "inline-flex items-center justify-center gap-2 rounded-lg border border-[var(--border-default)] px-5 py-2.5 text-[14px] font-semibold text-[var(--text-primary)] transition-colors hover:bg-[var(--surface-hover)]";

/** Go to `to` when signed in; otherwise sign up (or in) and come back to `to`. */
function AuthLink({
  to,
  signedIn,
  mode = "sign-up",
  className,
  children,
}: {
  to: string;
  signedIn: boolean;
  mode?: "sign-in" | "sign-up";
  className: string;
  children: React.ReactNode;
}) {
  if (signedIn) {
    return (
      <Link to={to} className={className}>
        {children}
      </Link>
    );
  }
  return (
    <SignInModalButton mode={mode} redirectUrl={to} className={className}>
      {children}
    </SignInModalButton>
  );
}

// ---------------------------------------------------------------------------
// Quickstart snippets
// ---------------------------------------------------------------------------

const SNIPPETS: Array<{ id: string; label: string; code: string }> = [
  {
    id: "curl",
    label: "curl",
    code: `curl ${BASE_URL}/chat/completions \\
  -H "Authorization: Bearer $ALLTERNIT_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{
    "model": "${QUICKSTART_MODEL}",
    "messages": [{"role": "user", "content": "Say hello in five words."}]
  }'`,
  },
  {
    id: "python",
    label: "Python",
    code: `# pip install openai
import os
from openai import OpenAI

client = OpenAI(
    base_url="${BASE_URL}",
    api_key=os.environ["ALLTERNIT_API_KEY"],
)

response = client.chat.completions.create(
    model="${QUICKSTART_MODEL}",
    messages=[{"role": "user", "content": "Say hello in five words."}],
)
print(response.choices[0].message.content)`,
  },
  {
    id: "js",
    label: "JavaScript",
    code: `// npm install openai
import OpenAI from "openai";

const client = new OpenAI({
  baseURL: "${BASE_URL}",
  apiKey: process.env.ALLTERNIT_API_KEY,
});

const response = await client.chat.completions.create({
  model: "${QUICKSTART_MODEL}",
  messages: [{ role: "user", content: "Say hello in five words." }],
});
console.log(response.choices[0].message.content);`,
  },
];

function Quickstart() {
  const [active, setActive] = useState(SNIPPETS[0].id);
  const snippet = SNIPPETS.find((s) => s.id === active) ?? SNIPPETS[0];
  return (
    <div className="min-w-0 overflow-hidden rounded-2xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)]">
      <div className="flex items-center justify-between gap-2 border-b border-[var(--border-subtle)] px-2 py-1.5">
        <div role="tablist" aria-label="Code language" className="flex gap-1">
          {SNIPPETS.map((s) => (
            <button
              key={s.id}
              type="button"
              role="tab"
              aria-selected={s.id === active}
              onClick={() => setActive(s.id)}
              className={cn(
                "rounded-md px-2.5 py-1 text-[12px] font-medium transition-colors",
                s.id === active
                  ? "bg-[#D97757]/10 text-[#D97757]"
                  : "text-[var(--text-secondary)] hover:text-[var(--text-primary)]",
              )}
            >
              {s.label}
            </button>
          ))}
        </div>
        <CopyButton text={snippet.code} />
      </div>
      <pre className="m-0 overflow-x-auto p-4 font-mono text-[12px] leading-relaxed text-[var(--text-primary)] md:text-[13px]">
        <code>{snippet.code}</code>
      </pre>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Model table
// ---------------------------------------------------------------------------

type SortKey = "id" | "provider" | "context" | "prompt" | "completion";

function compareModels(a: CloudModel, b: CloudModel, key: SortKey): number {
  if (key === "id" || key === "provider") return a[key].localeCompare(b[key]);
  const av = a[key];
  const bv = b[key];
  if (av === null && bv === null) return 0;
  if (av === null) return 1;
  if (bv === null) return -1;
  return av - bv;
}

function LaneBadge({ lane }: { lane: CloudModel["lane"] }) {
  if (lane !== "open") return <span className="text-[12px] text-[var(--text-tertiary)]">Standard</span>;
  return (
    <span className="inline-flex rounded-full border border-[#D97757]/30 bg-[#D97757]/10 px-2 py-0.5 text-[11px] font-semibold text-[#D97757]">
      Open
    </span>
  );
}

function ModelTable({ state, retry }: { state: Load<CloudModel[]>; retry: () => void }) {
  const [provider, setProvider] = useState("all");
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<{ key: SortKey; dir: 1 | -1 }>({ key: "provider", dir: 1 });

  const models = state.status === "ready" ? state.data : [];
  const providers = useMemo(() => Array.from(new Set(models.map((m) => m.provider))).sort(), [models]);

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    return models
      .filter((m) => provider === "all" || m.provider === provider)
      .filter((m) => !q || m.id.toLowerCase().includes(q) || m.name.toLowerCase().includes(q))
      .sort((a, b) => compareModels(a, b, sort.key) * sort.dir || a.id.localeCompare(b.id));
  }, [models, provider, query, sort]);

  const header = (key: SortKey, label: string, align: "left" | "right" = "left") => {
    const activeKey = sort.key === key;
    return (
      <th
        scope="col"
        aria-sort={activeKey ? (sort.dir === 1 ? "ascending" : "descending") : "none"}
        className={cn("px-4 py-2.5 font-medium", align === "right" && "text-right")}
      >
        <button
          type="button"
          onClick={() => setSort((s) => ({ key, dir: s.key === key ? ((s.dir * -1) as 1 | -1) : 1 }))}
          className={cn(
            "inline-flex items-center gap-1 transition-colors hover:text-[var(--text-primary)]",
            activeKey && "text-[var(--text-primary)]",
          )}
        >
          {label}
          {activeKey ? sort.dir === 1 ? <CaretUp size={11} /> : <CaretDown size={11} /> : null}
        </button>
      </th>
    );
  };

  return (
    <div className="mt-8 min-w-0">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
        <label className="relative min-w-0 flex-1 sm:max-w-xs">
          <span className="sr-only">Search models</span>
          <MagnifyingGlass
            size={15}
            className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-[var(--text-tertiary)]"
          />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search models"
            className="w-full rounded-lg border border-[var(--border-subtle)] bg-[var(--bg-secondary)] py-2 pl-9 pr-3 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[#D97757]/60"
          />
        </label>
        <label className="flex items-center gap-2 text-[13px] text-[var(--text-secondary)]">
          Provider
          <select
            value={provider}
            onChange={(e) => setProvider(e.target.value)}
            className="min-w-0 flex-1 rounded-lg border border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none focus:border-[#D97757]/60 sm:flex-none"
          >
            <option value="all">All providers</option>
            {providers.map((p) => (
              <option key={p} value={p}>
                {p}
              </option>
            ))}
          </select>
        </label>
        {state.status === "ready" && (
          <span className="text-[12px] text-[var(--text-tertiary)] sm:ml-auto">
            {rows.length} of {models.length} models · prices per 1M tokens
          </span>
        )}
      </div>

      <div className="mt-4">
        {state.status === "loading" && (
          <div className="space-y-2" aria-busy="true" aria-label="Loading models">
            {Array.from({ length: 6 }).map((_, i) => (
              <div key={i} className="h-11 animate-pulse rounded-lg bg-[var(--bg-secondary)]" />
            ))}
          </div>
        )}
        {state.status === "error" && (
          <ErrorRow message={`Could not load the model list: ${state.message}`} onRetry={retry} />
        )}
        {state.status === "ready" && rows.length === 0 && (
          <div className="rounded-xl border border-dashed border-[var(--border-default)] px-4 py-10 text-center text-[13px] text-[var(--text-secondary)]">
            {models.length === 0 ? "No models are listed right now. Check back soon." : "No models match this search."}
          </div>
        )}
        {state.status === "ready" && rows.length > 0 && (
          <>
            {/* Desktop / tablet: sortable table */}
            <div className="hidden overflow-x-auto rounded-xl border border-[var(--border-subtle)] md:block">
              <table className="w-full text-left text-[13px]">
                <thead className="border-b border-[var(--border-subtle)] bg-[var(--bg-secondary)] text-[12px] text-[var(--text-secondary)]">
                  <tr>
                    {header("id", "Model")}
                    {header("provider", "Provider")}
                    {header("context", "Context", "right")}
                    {header("prompt", "Input / 1M", "right")}
                    {header("completion", "Output / 1M", "right")}
                    <th scope="col" className="px-4 py-2.5 font-medium">
                      Lane
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((m) => (
                    <tr key={m.id} className="border-b border-[var(--border-subtle)] last:border-0 hover:bg-[var(--surface-hover)]">
                      <td className="px-4 py-2.5">
                        <div className="flex items-center gap-1">
                          <code className="font-mono text-[12.5px] text-[var(--text-primary)]">{m.id}</code>
                        </div>
                        {m.name !== m.id && (
                          <div className="mt-0.5 text-[12px] text-[var(--text-tertiary)]">{m.name}</div>
                        )}
                      </td>
                      <td className="px-4 py-2.5 text-[var(--text-secondary)]">{m.provider}</td>
                      <td className="px-4 py-2.5 text-right tabular-nums text-[var(--text-secondary)]">
                        {formatContext(m.context)}
                      </td>
                      <td className="px-4 py-2.5 text-right tabular-nums text-[var(--text-primary)]">
                        {formatPerMillion(m.prompt)}
                      </td>
                      <td className="px-4 py-2.5 text-right tabular-nums text-[var(--text-primary)]">
                        {formatPerMillion(m.completion)}
                      </td>
                      <td className="px-4 py-2.5">
                        <LaneBadge lane={m.lane} />
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>

            {/* Phone: one card per model, sorted by the same control */}
            <div className="md:hidden">
              <label className="mb-3 flex items-center gap-2 text-[13px] text-[var(--text-secondary)]">
                Sort by
                <select
                  value={`${sort.key}:${sort.dir}`}
                  onChange={(e) => {
                    const [key, dir] = e.target.value.split(":");
                    setSort({ key: key as SortKey, dir: Number(dir) as 1 | -1 });
                  }}
                  className="min-w-0 flex-1 rounded-lg border border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[13px] text-[var(--text-primary)]"
                >
                  <option value="provider:1">Provider</option>
                  <option value="id:1">Model name</option>
                  <option value="prompt:1">Input price, low to high</option>
                  <option value="completion:1">Output price, low to high</option>
                  <option value="context:-1">Context, largest first</option>
                </select>
              </label>
              <ul className="space-y-2">
                {rows.map((m) => (
                  <li key={m.id} className="rounded-xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-3">
                    <div className="flex items-start justify-between gap-2">
                      <div className="min-w-0">
                        <code className="break-all font-mono text-[13px] text-[var(--text-primary)]">{m.id}</code>
                        <div className="mt-0.5 text-[12px] text-[var(--text-tertiary)]">
                          {m.provider} · {formatContext(m.context)} context
                        </div>
                      </div>
                      <LaneBadge lane={m.lane} />
                    </div>
                    <div className="mt-2 flex gap-4 text-[12px] text-[var(--text-secondary)]">
                      <span>
                        In <span className="tabular-nums text-[var(--text-primary)]">{formatPerMillion(m.prompt)}</span>
                      </span>
                      <span>
                        Out <span className="tabular-nums text-[var(--text-primary)]">{formatPerMillion(m.completion)}</span>
                      </span>
                      <span className="text-[var(--text-tertiary)]">per 1M tokens</span>
                    </div>
                  </li>
                ))}
              </ul>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Pricing
// ---------------------------------------------------------------------------

function Pricing({ signedIn }: { signedIn: boolean }) {
  const [plans, retryPlans] = useJson("/api/v1/billing/plans", pickPlans);
  const [packs, retryPacks] = useJson("/api/v1/billing/packs", pickPacks);

  return (
    <div className="mt-8 grid min-w-0 gap-5 lg:grid-cols-5">
      <div className="min-w-0 lg:col-span-3">
        <h3 className="text-[15px] font-semibold text-[var(--text-primary)]">Monthly plans</h3>
        <p className="mt-1 text-[13px] text-[var(--text-secondary)]">
          Each plan adds credits every month. Unused plan credits roll over up to a cap.
        </p>
        <div className="mt-3">
          {plans.status === "loading" && (
            <div className="grid gap-3 sm:grid-cols-3">
              {[0, 1, 2].map((i) => (
                <div key={i} className="h-32 animate-pulse rounded-xl bg-[var(--bg-secondary)]" />
              ))}
            </div>
          )}
          {plans.status === "error" && (
            <ErrorRow message={`Could not load plans: ${plans.message}`} onRetry={retryPlans} />
          )}
          {plans.status === "ready" && plans.data.length === 0 && (
            <p className="text-[13px] text-[var(--text-tertiary)]">No plans are on sale right now.</p>
          )}
          {plans.status === "ready" && plans.data.length > 0 && (
            <div className="grid gap-3 sm:grid-cols-3">
              {plans.data.map((plan) => (
                <div key={plan.id} className="flex flex-col rounded-xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
                  <div className="text-[13px] font-semibold text-[var(--text-secondary)]">{plan.label}</div>
                  <div className="mt-1 text-[24px] font-bold tracking-tight text-[var(--text-primary)]">
                    {formatUsd(plan.price_usd)}
                    <span className="text-[13px] font-medium text-[var(--text-tertiary)]">/mo</span>
                  </div>
                  <div className="mt-1 text-[13px] text-[var(--text-primary)]">
                    {formatUsd(plan.monthly_credits_usd)} credits a month
                  </div>
                  {typeof plan.rollover_cap_usd === "number" && (
                    <div className="mt-0.5 text-[12px] text-[var(--text-tertiary)]">
                      Rolls over up to {formatUsd(plan.rollover_cap_usd)}
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      </div>

      <div className="min-w-0 lg:col-span-2">
        <h3 className="text-[15px] font-semibold text-[var(--text-primary)]">Credit packs</h3>
        <p className="mt-1 text-[13px] text-[var(--text-secondary)]">One-time top-ups. One credit is one US dollar.</p>
        <div className="mt-3">
          {packs.status === "loading" && (
            <div className="grid grid-cols-2 gap-3">
              {[0, 1, 2, 3].map((i) => (
                <div key={i} className="h-[60px] animate-pulse rounded-xl bg-[var(--bg-secondary)]" />
              ))}
            </div>
          )}
          {packs.status === "error" && (
            <ErrorRow message={`Could not load credit packs: ${packs.message}`} onRetry={retryPacks} />
          )}
          {packs.status === "ready" && packs.data.length === 0 && (
            <p className="text-[13px] text-[var(--text-tertiary)]">No credit packs are on sale right now.</p>
          )}
          {packs.status === "ready" && packs.data.length > 0 && (
            <div className="grid grid-cols-2 gap-3">
              {packs.data.map((pack) => (
                <div key={pack.id} className="rounded-xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-4 py-3">
                  <div className="text-[18px] font-bold text-[var(--text-primary)]">{formatUsd(pack.credits_usd)}</div>
                  <div className="text-[12px] text-[var(--text-tertiary)]">
                    {pack.price_usd === pack.credits_usd ? "in credits" : `for ${formatUsd(pack.price_usd)}`}
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>

      <div className="grid min-w-0 gap-3 sm:grid-cols-3 lg:col-span-5">
        {[
          {
            title: "$2 free each month",
            body: "Every account gets $2 of model usage a month at no charge, so you can try the API before you pay.",
          },
          {
            title: "Auto top-up",
            body: "Turn it on in Billing and a credit pack is bought from your saved card when your balance drops below a threshold you set, up to a monthly limit.",
          },
          {
            title: "Pay per token",
            body: "Each request costs the model's input and output price for the tokens it used, taken from your credits.",
          },
        ].map((item) => (
          <div key={item.title} className="rounded-xl border border-[var(--border-subtle)] px-4 py-3">
            <div className="text-[13px] font-semibold text-[var(--text-primary)]">{item.title}</div>
            <p className="mt-1 text-[13px] leading-relaxed text-[var(--text-secondary)]">{item.body}</p>
          </div>
        ))}
      </div>

      <div className="flex flex-wrap items-center gap-3 lg:col-span-5">
        <AuthLink to={BILLING_PATH} signedIn={signedIn} className={PRIMARY_BTN}>
          <Coins size={16} /> Buy credits
        </AuthLink>
        <a href={`${DOCS}/api/cloud-billing`} target="_blank" rel="noopener noreferrer" className={SECONDARY_BTN}>
          Billing docs <ArrowSquareOut size={14} />
        </a>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

const FACTS = [
  {
    icon: <Key size={20} weight="duotone" />,
    title: "One key",
    body: "One Allternit API key works for every model in the list, through one chat-completions endpoint.",
  },
  {
    icon: <Stack size={20} weight="duotone" />,
    title: "Open and frontier models",
    body: "Hosted models from several inference providers, priced per token and listed live below.",
  },
  {
    icon: <Coins size={20} weight="duotone" />,
    title: "Prepaid credits",
    body: "Add credits with a pack or a monthly plan. Requests stop when the balance runs out, unless you turn on auto top-up.",
  },
  {
    icon: <LockSimple size={20} weight="duotone" />,
    title: "No data retention by us",
    body: "We store token counts and cost for each request, not your prompts or outputs. Requests sent through OpenRouter ask providers for zero data retention.",
  },
];

const DOC_LINKS = [
  { href: `${DOCS}/api/chat-completions`, title: "Chat completions", body: "Request and response format, streaming, errors." },
  { href: `${DOCS}/api/models`, title: "Models", body: "The model list endpoint and its fields." },
  { href: `${DOCS}/api/cloud-billing`, title: "Billing", body: "Credits, plans, packs and auto top-up." },
  { href: `${DOCS}/api/open-lane`, title: "Open lane", body: "How open models are served and moderated." },
  { href: `${DOCS}/api/data-retention`, title: "Data retention", body: "What is stored for each request, and what is not." },
  { href: `${DOCS}/guides/open-models`, title: "Open models guide", body: "Turning on open models and what stays blocked." },
];

export function CloudPage() {
  const auth = usePlatformAuth();
  const signedIn = Boolean(auth.isLoaded && auth.isSignedIn);
  const [models, retryModels] = useCloudModels();
  const hasOpenModels = models.status === "ready" && models.data.some((m) => m.lane === "open");

  useEffect(() => {
    const previous = document.title;
    document.title = "Allternit Cloud API · Models and pricing";
    return () => {
      document.title = previous;
    };
  }, []);

  return (
    <PublicPageShell>
      {/* Hero */}
      <section className="relative overflow-hidden border-b border-[var(--border-subtle)] bg-[var(--surface-canvas)] px-4 pb-14 pt-10 sm:px-6 md:pb-20 md:pt-16">
        <div
          className="pointer-events-none absolute inset-0"
          style={{
            backgroundImage: "radial-gradient(circle at 75% 15%, rgba(217,119,87,0.12), transparent 40%)",
          }}
        />
        <div className="relative mx-auto grid max-w-6xl items-center gap-10 lg:grid-cols-2 lg:gap-14">
          <div className="min-w-0">
            <div className="mb-5 inline-flex items-center gap-2 rounded-full border border-[#D97757]/30 bg-[#D97757]/10 px-3 py-1 text-[11px] font-semibold tracking-[0.08em] text-[#D97757]">
              ALLTERNIT CLOUD API
            </div>
            <h1 className="text-[34px] font-bold leading-[1.08] tracking-tight text-[var(--text-primary)] sm:text-[44px] lg:text-[52px]">
              One API key for open and frontier models
            </h1>
            <p className="mt-5 max-w-xl text-[16px] leading-relaxed text-[var(--text-secondary)] md:text-[17px]">
              A chat-completions compatible endpoint that routes to the model you name. Pay per token from
              prepaid credits. We keep token counts, not your prompts.
            </p>

            <div className="mt-6 flex max-w-xl items-center gap-2 rounded-lg border border-[var(--border-subtle)] bg-[var(--bg-secondary)] py-1.5 pl-3 pr-1.5">
              <span className="shrink-0 text-[12px] text-[var(--text-tertiary)]">Base URL</span>
              <code className="min-w-0 flex-1 break-all font-mono text-[13px] text-[var(--text-primary)]">{BASE_URL}</code>
              <CopyButton text={BASE_URL} />
            </div>

            <div className="mt-6 flex flex-col gap-3 sm:flex-row">
              <AuthLink to={KEY_PATH} signedIn={signedIn} className={PRIMARY_BTN}>
                {signedIn ? "Create an API key" : "Sign up and get a key"} <ArrowRight size={16} />
              </AuthLink>
              {signedIn ? (
                <Link to={BILLING_PATH} className={SECONDARY_BTN}>
                  Buy credits
                </Link>
              ) : (
                <SignInModalButton mode="sign-in" redirectUrl={KEY_PATH} className={SECONDARY_BTN}>
                  Sign in
                </SignInModalButton>
              )}
            </div>
            <p className="mt-3 text-[12px] text-[var(--text-tertiary)]">
              $2 of free usage every month on every account.
            </p>
          </div>

          <Quickstart />
        </div>
      </section>

      {/* Facts */}
      <section className="px-4 py-14 sm:px-6 md:py-20">
        <div className="mx-auto grid max-w-6xl gap-4 sm:grid-cols-2 lg:grid-cols-4">
          {FACTS.map((f) => (
            <div key={f.title} className="rounded-2xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-5">
              <div className="mb-3 inline-flex rounded-lg bg-[#D97757]/10 p-2 text-[#D97757]">{f.icon}</div>
              <h3 className="text-[15px] font-semibold text-[var(--text-primary)]">{f.title}</h3>
              <p className="mt-1.5 text-[13px] leading-relaxed text-[var(--text-secondary)]">{f.body}</p>
            </div>
          ))}
        </div>
      </section>

      {/* Quickstart steps */}
      <section className="border-y border-[var(--border-subtle)] bg-[var(--bg-secondary)]/30 px-4 py-14 sm:px-6 md:py-20">
        <div className="mx-auto max-w-6xl">
          <SectionHeading id="quickstart" title="First call in about two minutes" />
          <ol className="mt-8 grid gap-4 md:grid-cols-4">
            {[
              {
                title: "Sign in",
                body: signedIn ? "You are signed in." : "Create an Allternit account or sign in to the one you have.",
              },
              {
                title: "Create a key",
                body: "On the API Keys page, create a key with the Run models scope. Copy it now; it is shown once.",
              },
              {
                title: "Add credits",
                body: "Your $2 monthly free usage covers a first test. Buy a pack or a plan when you need more.",
              },
              {
                title: "Call the API",
                body: "Set ALLTERNIT_API_KEY and run one of the snippets above, or point any chat-completions SDK at the base URL.",
              },
            ].map((step, i) => (
              <li key={step.title} className="rounded-2xl border border-[var(--border-subtle)] bg-[var(--bg-primary)] p-5">
                <div className="flex size-7 items-center justify-center rounded-full bg-[#D97757]/10 text-[13px] font-bold text-[#D97757]">
                  {i + 1}
                </div>
                <h3 className="mt-3 text-[15px] font-semibold text-[var(--text-primary)]">{step.title}</h3>
                <p className="mt-1.5 text-[13px] leading-relaxed text-[var(--text-secondary)]">{step.body}</p>
              </li>
            ))}
          </ol>
          <div className="mt-6">
            <AuthLink to={KEY_PATH} signedIn={signedIn} className={PRIMARY_BTN}>
              <Key size={16} /> {signedIn ? "Go to API keys" : "Get an API key"}
            </AuthLink>
          </div>
        </div>
      </section>

      {/* Models */}
      <section className="px-4 py-14 sm:px-6 md:py-20">
        <div className="mx-auto max-w-6xl">
          <SectionHeading id="models" title="Models and prices">
            Live from <code className="font-mono text-[13px]">GET /v1/models</code>. Use the model id as{" "}
            <code className="font-mono text-[13px]">model</code> in your request. Prices are in US dollars per one million
            tokens.
          </SectionHeading>
          <ModelTable state={models} retry={retryModels} />
        </div>
      </section>

      {/* Pricing */}
      <section className="border-y border-[var(--border-subtle)] bg-[var(--bg-secondary)]/30 px-4 py-14 sm:px-6 md:py-20">
        <div className="mx-auto max-w-6xl">
          <SectionHeading id="pricing" title="Pricing">
            Requests are paid from your credit balance. Add credits with a monthly plan or a one-time pack.
          </SectionHeading>
          <Pricing signedIn={signedIn} />
        </div>
      </section>

      {/* Open models */}
      <section className="px-4 py-14 sm:px-6 md:py-20">
        <div className="mx-auto grid max-w-6xl gap-8 lg:grid-cols-2">
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-3">
              <h2 id="open-models" className="scroll-mt-20 text-[26px] font-bold tracking-tight text-[var(--text-primary)] md:text-[32px]">
                Open models
              </h2>
              {models.status === "ready" && !hasOpenModels && (
                <span className="rounded-full border border-[var(--border-default)] px-2.5 py-0.5 text-[12px] font-medium text-[var(--text-secondary)]">
                  Coming soon
                </span>
              )}
            </div>
            <p className="mt-3 text-[15px] leading-relaxed text-[var(--text-secondary)]">
              Open-weight models with far fewer refusals for legal requests that other models often turn down, such as
              mature fiction, frank medical questions and security research. They are marked{" "}
              <span className="font-semibold text-[#D97757]">Open</span> in the model list.
            </p>
            <p className="mt-3 text-[15px] leading-relaxed text-[var(--text-secondary)]">
              {models.status === "ready" && !hasOpenModels
                ? "No open models are listed right now. They will appear in the table above when they are available."
                : "To use them, you confirm your age and accept the open-model use policy in your account first."}
            </p>
            <a
              href={`${DOCS}/guides/open-models`}
              target="_blank"
              rel="noopener noreferrer"
              className="mt-4 inline-flex items-center gap-1.5 text-[14px] font-semibold text-[#D97757] hover:underline"
            >
              Read the open models guide <ArrowSquareOut size={14} />
            </a>
          </div>
          <div className="min-w-0 rounded-2xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-5">
            <h3 className="text-[15px] font-semibold text-[var(--text-primary)]">Before you use them</h3>
            <ul className="mt-2 space-y-1.5 text-[13px] leading-relaxed text-[var(--text-secondary)]">
              <li>An age check and acceptance of the open-model use policy are required.</li>
              <li>The models themselves can still decline a request.</li>
            </ul>
            <h3 className="mt-5 text-[15px] font-semibold text-[var(--text-primary)]">Always blocked</h3>
            <ul className="mt-2 space-y-1.5 text-[13px] leading-relaxed text-[var(--text-secondary)]">
              <li>Sexual content involving minors.</li>
              <li>Real help making or spreading mass-casualty weapons.</li>
              <li>Sexual content about real, identifiable people.</li>
            </ul>
            <p className="mt-3 text-[12px] text-[var(--text-tertiary)]">
              A blocked request returns <code className="font-mono">content_policy_hard_line</code> and nothing is
              generated.
            </p>
          </div>
        </div>
      </section>

      {/* Docs */}
      <section className="border-t border-[var(--border-subtle)] bg-[var(--bg-secondary)]/30 px-4 py-14 sm:px-6 md:py-20">
        <div className="mx-auto max-w-6xl">
          <SectionHeading id="docs" title="Documentation" />
          <div className="mt-8 grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {DOC_LINKS.map((d) => (
              <a
                key={d.href}
                href={d.href}
                target="_blank"
                rel="noopener noreferrer"
                className="group rounded-xl border border-[var(--border-subtle)] bg-[var(--bg-primary)] p-4 transition-colors hover:border-[#D97757]/40"
              >
                <div className="flex items-center justify-between gap-2 text-[14px] font-semibold text-[var(--text-primary)]">
                  {d.title}
                  <ArrowSquareOut size={14} className="text-[var(--text-tertiary)] group-hover:text-[#D97757]" />
                </div>
                <p className="mt-1 text-[13px] text-[var(--text-secondary)]">{d.body}</p>
              </a>
            ))}
          </div>
        </div>
      </section>
    </PublicPageShell>
  );
}
