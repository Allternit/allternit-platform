import React, { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { Info, MagnifyingGlass, Warning } from "@phosphor-icons/react";
import {
  CatalogModel,
  DRAFT_MODEL_CATALOG,
  fetchLiveModelCatalog,
  liveModelToCatalog,
} from "@/lib/model-catalog";
import { usePlatformAuth } from "@/lib/platform-auth-client";
import { PublicPageShell } from "@/components/PublicPageShell";

export function ModelsPage() {
  const [query, setQuery] = useState("");
  const [models, setModels] = useState<CatalogModel[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const auth = usePlatformAuth();

  useEffect(() => {
    const controller = new AbortController();
    let cancelled = false;

    async function load() {
      try {
        setLoading(true);
        setError(null);
        const response = await fetchLiveModelCatalog(controller.signal);
        if (cancelled) return;
        const catalog = response.data.map(liveModelToCatalog);
        setModels(catalog.length > 0 ? catalog : DRAFT_MODEL_CATALOG);
      } catch (err) {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : "Failed to load model catalog");
        setModels(DRAFT_MODEL_CATALOG);
      } finally {
        if (!cancelled) setLoading(false);
      }
    }

    void load();
    return () => {
      cancelled = true;
      controller.abort();
    };
  }, []);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return models;
    return models.filter(
      (m) =>
        m.name.toLowerCase().includes(q) ||
        m.family.toLowerCase().includes(q) ||
        m.provider.toLowerCase().includes(q) ||
        (m.upstreamProvider && m.upstreamProvider.toLowerCase().includes(q)),
    );
  }, [query, models]);

  return (
    <PublicPageShell>
      <div className="mx-auto max-w-7xl px-6 py-12">
        <div className="mb-8 flex flex-col gap-4 md:flex-row md:items-end md:justify-between">
          <div>
            <h1 className="text-[32px] font-bold leading-none tracking-tight text-[var(--text-primary)]">
              Model catalog
            </h1>
            <p className="mt-2 max-w-xl text-[15px] leading-relaxed text-[var(--text-secondary)]">
              Models available through Allternit Cloud, with the price a request is billed.
              Each model is served by the upstream provider shown on its card.
            </p>
          </div>
          {auth.isLoaded && auth.isSignedIn && (
            <Link
              to="/billing"
              className="inline-flex items-center justify-center rounded-lg bg-[var(--accent-primary)] px-4 py-2 text-[13px] font-medium text-[#FDF8F3] transition-colors hover:brightness-110"
            >
              Manage subscription
            </Link>
          )}
        </div>

        <div className="mb-6 flex items-center gap-2 rounded-xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[var(--text-secondary)] focus-within:border-[var(--accent-primary)]/40 focus-within:ring-1 focus-within:ring-[var(--accent-primary)]/20">
          <MagnifyingGlass size={18} />
          <input
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search models, families, or providers..."
            className="flex-1 bg-transparent text-[13px] placeholder:text-[var(--text-tertiary)] outline-none text-[var(--text-primary)]"
          />
        </div>

        <div className="mb-6 rounded-xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
          <div className="flex items-start gap-3">
            <Info size={18} className="mt-0.5 shrink-0 text-[var(--accent-primary)]" />
            <div className="text-[13px] leading-relaxed text-[var(--text-secondary)]">
              <p className="font-medium text-[var(--text-primary)]">Model providers</p>
              <p className="mt-1">
                Allternit Cloud routes inference requests to third-party model providers.
                Cloud models are currently sourced from OpenRouter, Together AI, Fireworks AI,
                DeepInfra, and Groq. Pricing and availability are subject to each provider's
                terms. Cloud-model subscriptions are UI-only until explicitly marked live.
              </p>
            </div>
          </div>
        </div>

        <div className="rounded-2xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)] overflow-hidden">
          {(loading || error) && (
            <div className="flex items-center gap-2 border-b border-[var(--border-subtle)] bg-[var(--accent-primary)]/5 px-4 py-3 text-[12px] font-medium text-[var(--accent-primary)]">
              <Warning size={14} />
              {loading
                ? "Loading live model catalog..."
                : `Live catalog unavailable. Showing representative examples. (${error})`}
            </div>
          )}
          <div className="overflow-x-auto">
            <table className="w-full text-left text-[13px]">
              <thead className="bg-[var(--bg-primary)] text-[11px] font-semibold uppercase tracking-wider text-[var(--text-tertiary)]">
                <tr>
                  <th className="px-4 py-3">Model</th>
                  <th className="px-4 py-3">Family</th>
                  <th className="px-4 py-3">Provider</th>
                  <th className="px-4 py-3">Upstream</th>
                  <th className="px-4 py-3">Input / 1M tokens</th>
                  <th className="px-4 py-3">Output / 1M tokens</th>
                  <th className="px-4 py-3">Context</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-[var(--border-subtle)]">
                {filtered.map((model) => (
                  <tr key={model.id} className="hover:bg-[var(--surface-hover)] transition-colors">
                    <td className="px-4 py-3 font-medium text-[var(--text-primary)]">
                      {model.name}
                      <span className="ml-2 text-[11px] text-[var(--text-tertiary)]">{model.id}</span>
                    </td>
                    <td className="px-4 py-3 text-[var(--text-secondary)]">{model.family}</td>
                    <td className="px-4 py-3">
                      <span
                        className={`inline-flex rounded-full px-2 py-0.5 text-[11px] font-medium ${
                          model.provider === "local"
                            ? "bg-[var(--status-success)]/10 text-[var(--status-success)]"
                            : "bg-[var(--accent-primary)]/10 text-[var(--accent-primary)]"
                        }`}
                      >
                        {model.provider}
                      </span>
                    </td>
                    <td className="px-4 py-3 text-[var(--text-secondary)]">
                      {model.upstreamProvider || "—"}
                    </td>
                    <td className="px-4 py-3 text-[var(--text-secondary)]">{model.inputPrice}</td>
                    <td className="px-4 py-3 text-[var(--text-secondary)]">{model.outputPrice}</td>
                    <td className="px-4 py-3 text-[var(--text-secondary)]">{model.context}</td>
                  </tr>
                ))}
                {filtered.length === 0 && (
                  <tr>
                    <td
                      colSpan={7}
                      className="px-4 py-8 text-center text-[13px] text-[var(--text-secondary)]"
                    >
                      No models match “{query}”.
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>
        </div>
      </div>
    </PublicPageShell>
  );
}
