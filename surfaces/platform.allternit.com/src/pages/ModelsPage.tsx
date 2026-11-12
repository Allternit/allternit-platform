import React, { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { Info, MagnifyingGlass, Warning } from "@phosphor-icons/react";
import {
  CatalogModel,
  fetchLiveModelCatalog,
  liveModelToCatalog,
} from "@/lib/model-catalog";
import { usePlatformAuth } from "@/lib/platform-auth-client";
import { PublicPageShell } from "@/components/PublicPageShell";
import { Pricing, formatCents, getPricing } from "@/lib/pricing";

export function ModelsPage() {
  const [query, setQuery] = useState("");
  const [models, setModels] = useState<CatalogModel[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const auth = usePlatformAuth();
  const [pricing, setPricing] = useState<Pricing | null>(null);

  // Pricing needs a console session; signed-out visitors just see the catalog.
  useEffect(() => {
    if (!auth.isLoaded || !auth.isSignedIn) return;
    let cancelled = false;
    getPricing()
      .then((p) => {
        if (!cancelled) setPricing(p);
      })
      .catch(() => {
        if (!cancelled) setPricing(null);
      });
    return () => {
      cancelled = true;
    };
  }, [auth.isLoaded, auth.isSignedIn]);

  const deprecations = useMemo(() => {
    const rows = [...(pricing?.deprecations ?? [])];
    for (const m of models) {
      if (m.retires && !rows.some((r) => r.model === m.id)) {
        rows.push({ model: m.id, retires_at: m.retires, replacement: null });
      }
    }
    return rows;
  }, [pricing, models]);

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
        setModels(catalog);
      } catch (err) {
        if (cancelled) return;
        setModels([]);
        setError(err instanceof Error ? err.message : "Unknown error");
      } finally {
        if (!cancelled) setLoading(false);
      }
    }

    void load();
    return () => {
      cancelled = true;
      controller.abort();
    };
  }, [attempt]);

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
          {loading && (
            <div className="px-4 py-8 text-center text-[13px] text-[var(--text-secondary)]">
              Loading the model catalog...
            </div>
          )}
          {!loading && error && (
            <div className="flex flex-col items-center gap-3 px-4 py-8 text-center">
              <p className="flex items-center gap-2 text-[13px] text-[var(--text-secondary)]">
                <Warning size={14} />
                The model catalog did not load. Check your connection and try again. ({error})
              </p>
              <button
                type="button"
                onClick={() => setAttempt((n) => n + 1)}
                className="rounded-lg border border-[var(--border-subtle)] px-3 py-1.5 text-[13px] font-medium text-[var(--text-primary)] transition-colors hover:bg-[var(--surface-hover)]"
              >
                Try again
              </button>
            </div>
          )}
          {!loading && !error && models.length === 0 && (
            <div className="px-4 py-8 text-center text-[13px] text-[var(--text-secondary)]">
              No models are listed right now.
            </div>
          )}
          {!loading && !error && models.length > 0 && (
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
                  <th className="px-4 py-3">Capabilities</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-[var(--border-subtle)]">
                {filtered.map((model) => (
                  <tr key={model.id} className="hover:bg-[var(--surface-hover)] transition-colors">
                    <td className="px-4 py-3 font-medium text-[var(--text-primary)]">
                      {model.name}
                      <span className="ml-2 text-[11px] text-[var(--text-tertiary)]">{model.id}</span>
                      {model.isDefault && (
                        <span className="ml-2 inline-flex rounded-full bg-[var(--accent-primary)]/10 px-2 py-0.5 text-[11px] font-medium text-[var(--accent-primary)]">
                          Default
                        </span>
                      )}
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
                    <td className="px-4 py-3">
                      <div className="flex flex-wrap gap-1.5">
                        {model.capabilities.map((cap) => (
                          <span
                            key={cap}
                            className="rounded-md bg-[var(--bg-tertiary)] px-2 py-0.5 text-[11px] font-medium text-[var(--text-secondary)]"
                          >
                            {cap}
                          </span>
                        ))}
                        {model.retires && (
                          <span className="rounded-md bg-[var(--status-warning)]/10 px-2 py-0.5 text-[11px] font-medium text-[var(--status-warning)]">
                            Retires {model.retires}
                          </span>
                        )}
                        {model.capabilities.length === 0 && !model.retires && (
                          <span className="text-[var(--text-tertiary)]">—</span>
                        )}
                      </div>
                    </td>
                  </tr>
                ))}
                {filtered.length === 0 && (
                  <tr>
                    <td
                      colSpan={8}
                      className="px-4 py-8 text-center text-[13px] text-[var(--text-secondary)]"
                    >
                      No models match “{query}”.
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>
          )}
        </div>

        {deprecations.length > 0 && (
          <section className="mt-8">
            <h2 className="mb-3 text-[18px] font-semibold text-[var(--text-primary)]">Deprecations</h2>
            <div className="overflow-x-auto rounded-2xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)]">
              <table className="w-full text-left text-[13px]">
                <thead className="bg-[var(--bg-primary)] text-[11px] font-semibold uppercase tracking-wider text-[var(--text-tertiary)]">
                  <tr>
                    <th className="px-4 py-3">Model</th>
                    <th className="px-4 py-3">Retires</th>
                    <th className="px-4 py-3">Replacement</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-[var(--border-subtle)]">
                  {deprecations.map((d) => (
                    <tr key={d.model}>
                      <td className="px-4 py-3 font-medium text-[var(--text-primary)]">{d.model}</td>
                      <td className="px-4 py-3 text-[var(--text-secondary)]">{d.retires_at}</td>
                      <td className="px-4 py-3 text-[var(--text-secondary)]">{d.replacement || "—"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </section>
        )}

        {pricing && pricing.tools.length > 0 && (
          <section className="mt-8">
            <h2 className="mb-3 text-[18px] font-semibold text-[var(--text-primary)]">Hosted tools</h2>
            <div className="overflow-x-auto rounded-2xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)]">
              <table className="w-full text-left text-[13px]">
                <thead className="bg-[var(--bg-primary)] text-[11px] font-semibold uppercase tracking-wider text-[var(--text-tertiary)]">
                  <tr>
                    <th className="px-4 py-3">Tool</th>
                    <th className="px-4 py-3">Unit</th>
                    <th className="px-4 py-3">Price</th>
                    <th className="px-4 py-3">Provider</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-[var(--border-subtle)]">
                  {pricing.tools.map((t) => (
                    <tr key={`${t.tool}-${t.unit}`}>
                      <td className="px-4 py-3 font-medium text-[var(--text-primary)]">{t.tool}</td>
                      <td className="px-4 py-3 text-[var(--text-secondary)]">{t.unit}</td>
                      <td className="px-4 py-3 text-[var(--text-secondary)]">${t.price_usd}</td>
                      <td className="px-4 py-3 text-[var(--text-secondary)]">{t.provider}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </section>
        )}

        {pricing && pricing.meters.length > 0 && (
          <section className="mt-8">
            <h2 className="mb-1 text-[18px] font-semibold text-[var(--text-primary)]">Usage meters</h2>
            <p className="mb-3 text-[13px] text-[var(--text-secondary)]">
              Prices per unit for metered products, on Pay as you go and on Growth.
            </p>
            <div className="overflow-x-auto rounded-2xl border border-[var(--border-subtle)] bg-[var(--bg-secondary)]">
              <table className="w-full text-left text-[13px]">
                <thead className="bg-[var(--bg-primary)] text-[11px] font-semibold uppercase tracking-wider text-[var(--text-tertiary)]">
                  <tr>
                    <th className="px-4 py-3">Meter</th>
                    <th className="px-4 py-3">Unit</th>
                    <th className="px-4 py-3">Pay as you go</th>
                    <th className="px-4 py-3">Growth</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-[var(--border-subtle)]">
                  {pricing.meters.map((m) => (
                    <tr key={m.meter}>
                      <td className="px-4 py-3 font-medium text-[var(--text-primary)]">{m.meter}</td>
                      <td className="px-4 py-3 text-[var(--text-secondary)]">{m.unit}</td>
                      <td className="px-4 py-3 text-[var(--text-secondary)]">{formatCents(m.payg_cents)}</td>
                      <td className="px-4 py-3 text-[var(--text-secondary)]">{formatCents(m.growth_cents)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </section>
        )}
      </div>
    </PublicPageShell>
  );
}
