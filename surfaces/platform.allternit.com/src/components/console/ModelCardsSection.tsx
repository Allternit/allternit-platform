import React, { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import {
  fetchLiveModelCatalog,
  liveModelToCatalog,
} from "@/lib/model-catalog";
import { buildModelCards, type ModelCardInfo } from "@/lib/model-showcase";
import { ModelMark } from "@/components/console/ModelMark";
import { ModelCardOverlay } from "@/components/console/ModelCardOverlay";

export function ModelCardsSection() {
  const [cards, setCards] = useState<ModelCardInfo[]>([]);
  const [active, setActive] = useState<ModelCardInfo | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    let cancelled = false;

    async function load() {
      try {
        const response = await fetchLiveModelCatalog(controller.signal);
        if (cancelled) return;
        const catalog = response.data.map(liveModelToCatalog);
        if (catalog.length > 0) setCards(buildModelCards(catalog));
      } catch {
        // No live list: the section stays hidden rather than showing placeholder models.
      }
    }

    void load();
    return () => {
      cancelled = true;
      controller.abort();
    };
  }, []);

  const cardsMemo = useMemo(() => cards, [cards]);

  if (cardsMemo.length === 0) return null;

  return (
    <section>
      <div className="mb-4 flex items-end justify-between">
        <h2 className="text-[14px] font-semibold text-[var(--text-primary)]">Models</h2>
        <Link
          to="/models"
          className="text-[12px] text-[var(--text-tertiary)] underline underline-offset-2 transition-colors hover:text-[var(--text-secondary)]"
        >
          Compare models
        </Link>
      </div>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-4">
        {cardsMemo.map((card) => (
          <button
            key={card.id}
            type="button"
            onClick={() => setActive(card)}
            className="group overflow-hidden rounded-2xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] text-left transition-colors hover:border-[var(--border-default)]"
          >
            <div
              className="flex h-28 items-center justify-center transition-transform duration-200 group-hover:scale-[1.02]"
              style={{ backgroundColor: card.banner }}
            >
              <ModelMark kind={card.mark} ink={card.ink} />
            </div>
            <div className="p-4">
              <div className="flex items-center gap-2">
                <span className="text-[14px] font-semibold text-[var(--text-primary)]">
                  {card.name}
                </span>
                {card.isNew && <span className="text-[11px] font-medium text-[#6E9BF0]">New</span>}
              </div>
              {card.tags.length > 0 && (
                <div className="mt-2 flex flex-wrap gap-1.5">
                  {card.tags.map((tag) => (
                    <span
                      key={tag}
                      className="rounded-md bg-[var(--bg-tertiary)] px-2 py-0.5 text-[11px] font-medium text-[var(--text-secondary)]"
                    >
                      {tag}
                    </span>
                  ))}
                </div>
              )}
            </div>
          </button>
        ))}
      </div>

      {active && <ModelCardOverlay model={active} onClose={() => setActive(null)} />}
    </section>
  );
}
