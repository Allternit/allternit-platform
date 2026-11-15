import React, { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import { Search01Icon, Cancel01Icon, ArrowRight01Icon, BookOpen02Icon } from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { consoleNavEntries, type ConsoleNavEntry } from "./navConfig";

/** Pages reachable from the rail footer rather than the nav, still searchable. */
const paletteEntries: ConsoleNavEntry[] = [
  ...consoleNavEntries,
  { to: "/docs", label: "Docs", group: null, icon: BookOpen02Icon },
];

interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
}

/**
 * Global ⌘K / Ctrl-K command palette. The index is built from the same
 * navConfig the sidebar renders, so the two can never drift.
 */
export function CommandPalette({ open, onClose }: CommandPaletteProps): React.ReactNode {
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const results = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return paletteEntries;
    return paletteEntries.filter((entry) => {
      const haystack = entry.group
        ? `${entry.group} ${entry.label}`.toLowerCase()
        : entry.label.toLowerCase();
      return haystack.includes(q);
    });
  }, [query]);

  useEffect(() => {
    if (open) {
      setQuery("");
      setActiveIndex(0);
      window.setTimeout(() => inputRef.current?.focus(), 0);
    }
  }, [open]);

  useEffect(() => {
    setActiveIndex(0);
  }, [query]);

  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;

  const choose = (index: number) => {
    const entry = results[index];
    if (!entry) return;
    navigate(entry.to);
    onClose();
  };

  const onInputKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActiveIndex((prev) => Math.min(prev + 1, results.length - 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActiveIndex((prev) => Math.max(prev - 1, 0));
    } else if (event.key === "Enter") {
      event.preventDefault();
      choose(activeIndex);
    }
  };

  return (
    <div
      className="fixed inset-0 z-[80] flex items-start justify-center bg-black/60 px-4 pt-[15vh]"
      onClick={onClose}
    >
      <div
        role="dialog"
        aria-label="Command palette"
        className="w-full max-w-lg overflow-hidden rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] shadow-2xl"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="flex items-center gap-2 border-b border-solid border-[var(--border-subtle)] px-3 py-2.5">
          <HugeiconsIcon icon={Search01Icon} size={16} className="text-[var(--text-tertiary)]" />
          <input
            ref={inputRef}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={onInputKeyDown}
            placeholder="Search Console..."
            className="flex-1 bg-transparent text-[14px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)]"
          />
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="rounded-md p-1 text-[var(--text-tertiary)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-primary)]"
          >
            <HugeiconsIcon icon={Cancel01Icon} size={14} />
          </button>
        </div>

        <div ref={listRef} className="max-h-[50vh] overflow-y-auto p-1.5">
          {results.length === 0 && (
            <p className="px-3 py-6 text-center text-[13px] text-[var(--text-tertiary)]">
              No matches{query ? ` for “${query}”` : ""}.
            </p>
          )}
          {results.map((entry, index) => (
            <button
              key={entry.to}
              type="button"
              onClick={() => choose(index)}
              onMouseEnter={() => setActiveIndex(index)}
              className={cn(
                "flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-[13px] transition-colors",
                index === activeIndex
                  ? "bg-[var(--surface-hover)] text-[var(--text-primary)]"
                  : "text-[var(--text-secondary)]"
              )}
            >
              <HugeiconsIcon icon={entry.icon} size={15} className="shrink-0 text-[var(--text-tertiary)]" />
              <span className="min-w-0 flex-1">
                {entry.group && (
                  <span className="text-[var(--text-tertiary)]">{entry.group} / </span>
                )}
                {entry.label}
              </span>
              {index === activeIndex && (
                <HugeiconsIcon icon={ArrowRight01Icon} size={13} className="text-[var(--text-tertiary)]" />
              )}
            </button>
          ))}
        </div>

        <div className="flex items-center gap-3 border-t border-solid border-[var(--border-subtle)] px-3 py-1.5 text-[11px] text-[var(--text-tertiary)]">
          <span>↑↓ navigate</span>
          <span>↵ open</span>
          <span>esc close</span>
        </div>
      </div>
    </div>
  );
}
