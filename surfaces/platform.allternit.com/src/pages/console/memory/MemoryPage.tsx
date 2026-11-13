import React, { useCallback, useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AiBrain01Icon,
  AlertCircleIcon,
  ArrowLeft01Icon,
  PlusSignIcon,
  Search01Icon,
  TrashIcon,
} from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { formatApiError, isNoRuntime } from "@/lib/api-client";
import { NoRuntimeState } from "@/components/console-ui/NoRuntimeState";
import {
  type MemoryEntryRecord,
  type MemoryStoreRecord,
  createMemoryStore,
  deleteMemoryEntry,
  deleteMemoryStore,
  formatDate,
  formatTime,
  getMemoryEntry,
  listMemoryEntries,
  listMemoryStores,
  putMemoryEntry,
  searchMemoryEntries,
} from "@/lib/managed-agents";
import {
  ListPage,
  EmptyState,
  MonoChip,
  Badge,
  SkeletonRow,
  QUIET_BUTTON_CLASS,
  DESTRUCTIVE_BUTTON_CLASS,
} from "@/components/console-ui";

const INPUT_CLASS =
  "w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]";

const TH_CLASS = "text-left text-[11px] font-semibold uppercase tracking-wide text-[var(--text-tertiary)] px-3 py-2";
const TD_CLASS = "px-3 py-2.5 text-[13px] text-[var(--text-primary)]";

/** Values longer than this collapse behind an expand toggle. */
const VALUE_PREVIEW_LENGTH = 500;

export function MemoryPage(): React.ReactNode {
  const navigate = useNavigate();
  const [stores, setStores] = useState<MemoryStoreRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  const [name, setName] = useState("");
  const [creating, setCreating] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setStores(await listMemoryStores());
    } catch (err) {
      setError(formatApiError(err, "Unable to load memory stores"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const handleCreate = useCallback(async () => {
    setCreating(true);
    setError(null);
    try {
      const created = await createMemoryStore(name.trim());
      setShowCreate(false);
      setName("");
      navigate(`/memory/${created.id}`);
    } catch (err) {
      setError(formatApiError(err, "Unable to create store"));
    } finally {
      setCreating(false);
    }
  }, [name, navigate]);

  const handleDelete = useCallback(
    async (store: MemoryStoreRecord) => {
      if (!window.confirm(`Delete store "${store.name}" and all ${store.entry_count} entries?`)) return;
      setBusyId(store.id);
      setError(null);
      try {
        await deleteMemoryStore(store.id);
        await load();
      } catch (err) {
        setError(formatApiError(err, "Delete failed"));
      } finally {
        setBusyId(null);
      }
    },
    [load]
  );

  return (
    <ListPage
      title="Memory"
      subtitle="Shared memory stores give managed agents durable, searchable context across sessions."
      searchPlaceholder={undefined}
      onSearch={undefined}
      primaryAction={{ label: "+ New store", onClick: () => setShowCreate((v) => !v) }}
      emptyState={
        <EmptyState
          icon={<HugeiconsIcon icon={AiBrain01Icon} size={32} />}
          title="No memory stores"
          caption="A memory store is a named, searchable key/value space agents read and write across sessions. Create one, bind it to a session, and the agent keeps what it learns."
          ctaLabel="New store"
          onCtaClick={() => setShowCreate(true)}
        />
      }
    >
      {error && (isNoRuntime(error) ? (
        <NoRuntimeState className="mb-4" />
      ) : (
        <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      ))}

      {showCreate && (
        <div className="mb-4 flex items-center gap-2 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void handleCreate();
              }
            }}
            placeholder="Store name, e.g. customer-preferences"
            className={cn(INPUT_CLASS, "max-w-sm")}
          />
          <button
            type="button"
            onClick={() => void handleCreate()}
            disabled={creating || !name.trim()}
            className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            <HugeiconsIcon icon={PlusSignIcon} size={13} />
            {creating ? "Creating…" : "Create store"}
          </button>
          <button type="button" onClick={() => setShowCreate(false)} className={QUIET_BUTTON_CLASS}>
            Cancel
          </button>
        </div>
      )}

      {loading ? (
        <SkeletonRow lines={4} />
      ) : stores.length > 0 ? (
        <div className="overflow-x-auto rounded-xl border border-solid border-[var(--border-subtle)]">
          <table className="w-full border-collapse">
            <thead>
              <tr className="border-b border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]">
                <th className={TH_CLASS}>ID</th>
                <th className={TH_CLASS}>Name</th>
                <th className={TH_CLASS}>Entries</th>
                <th className={TH_CLASS}>Last write</th>
                <th className={TH_CLASS}>Created</th>
                <th className={TH_CLASS} aria-label="Actions" />
              </tr>
            </thead>
            <tbody>
              {stores.map((store) => (
                <tr
                  key={store.id}
                  onClick={() => navigate(`/memory/${store.id}`)}
                  className="cursor-pointer border-b border-solid border-[var(--border-subtle)] last:border-b-0 transition-colors hover:bg-[var(--surface-hover)]"
                >
                  <td className={TD_CLASS}>
                    <MonoChip>{store.id.slice(0, 8)}</MonoChip>
                  </td>
                  <td className={cn(TD_CLASS, "font-medium")}>{store.name}</td>
                  <td className={TD_CLASS}>
                    <Badge>{store.entry_count} entries</Badge>
                  </td>
                  <td className={cn(TD_CLASS, "text-[var(--text-secondary)]")}>
                    {formatTime(store.last_write_at)}
                  </td>
                  <td className={cn(TD_CLASS, "text-[var(--text-secondary)]")}>
                    {formatTime(store.created_at)}
                  </td>
                  <td className={TD_CLASS} onClick={(e) => e.stopPropagation()}>
                    <button
                      type="button"
                      onClick={() => void handleDelete(store)}
                      disabled={busyId === store.id}
                      className={cn(DESTRUCTIVE_BUTTON_CLASS, "px-2.5 py-1 text-[12px]")}
                    >
                      <HugeiconsIcon icon={TrashIcon} size={12} />
                      Delete
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : null}
    </ListPage>
  );
}

export function MemoryStorePage(): React.ReactNode {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const [store, setStore] = useState<MemoryStoreRecord | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [entries, setEntries] = useState<MemoryEntryRecord[]>([]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);

  const [namespace, setNamespace] = useState("");
  const [search, setSearch] = useState("");
  const [searchResults, setSearchResults] = useState<MemoryEntryRecord[] | null>(null);
  const [searching, setSearching] = useState(false);

  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [entryValue, setEntryValue] = useState("");
  const [entryMeta, setEntryMeta] = useState<MemoryEntryRecord | null>(null);
  const [expandedValues, setExpandedValues] = useState<Set<string>>(new Set());
  const [savingEntry, setSavingEntry] = useState(false);

  const [newKey, setNewKey] = useState("");
  const [newValue, setNewValue] = useState("");

  const loadFirstPage = useCallback(
    async (storeId: string, ns: string) => {
      const page = await listMemoryEntries(storeId, {
        namespace: ns || undefined,
        limit: 50,
      });
      setEntries(page.entries);
      setNextCursor(page.next_cursor ?? null);
    },
    []
  );

  useEffect(() => {
    if (!id) return;
    let active = true;
    void (async () => {
      try {
        const stores = await listMemoryStores();
        const found = stores.find((s) => s.id === id);
        if (!active) return;
        if (!found) throw new Error("Memory store not found");
        setStore(found);
        await loadFirstPage(id, "");
      } catch (err) {
        if (active) setError(formatApiError(err, "Unable to load store"));
      } finally {
        if (active) setLoading(false);
      }
    })();
    return () => {
      active = false;
    };
  }, [id, loadFirstPage]);

  const handleLoadMore = useCallback(async () => {
    if (!id || !nextCursor) return;
    setLoadingMore(true);
    try {
      const page = await listMemoryEntries(id, {
        namespace: namespace || undefined,
        cursor: nextCursor,
        limit: 50,
      });
      setEntries((prev) => [...prev, ...page.entries]);
      setNextCursor(page.next_cursor ?? null);
    } catch (err) {
      setError(formatApiError(err, "Unable to load more entries"));
    } finally {
      setLoadingMore(false);
    }
  }, [id, nextCursor, namespace]);

  const handleNamespaceChange = useCallback(
    async (ns: string) => {
      if (!id) return;
      setNamespace(ns);
      setSelectedKey(null);
      setEntryMeta(null);
      setSearchResults(null);
      setSearch("");
      try {
        await loadFirstPage(id, ns);
      } catch (err) {
        setError(formatApiError(err, "Unable to load entries"));
      }
    },
    [id, loadFirstPage]
  );

  const handleSearch = useCallback(
    async (q: string) => {
      setSearch(q);
      if (!id || !q.trim()) {
        setSearchResults(null);
        return;
      }
      setSearching(true);
      try {
        setSearchResults(await searchMemoryEntries(id, q.trim()));
      } catch (err) {
        setError(formatApiError(err, "Search failed"));
      } finally {
        setSearching(false);
      }
    },
    [id]
  );

  const openEntry = useCallback(
    async (key: string) => {
      if (!id) return;
      setSelectedKey(key);
      setError(null);
      try {
        const entry = await getMemoryEntry(id, key, namespace || undefined);
        setEntryMeta(entry);
        setEntryValue(entry.value);
      } catch (err) {
        setError(formatApiError(err, "Unable to load entry"));
        setEntryMeta(null);
        setEntryValue("");
      }
    },
    [id, namespace]
  );

  const handleSaveEntry = useCallback(async () => {
    if (!id || !selectedKey) return;
    setSavingEntry(true);
    setError(null);
    try {
      const saved = await putMemoryEntry(id, selectedKey, entryValue, namespace || undefined);
      setEntryMeta(saved);
      setEntries((prev) =>
        prev.map((e) => (e.key === saved.key && e.namespace === saved.namespace ? saved : e))
      );
    } catch (err) {
      setError(formatApiError(err, "Save failed"));
    } finally {
      setSavingEntry(false);
    }
  }, [id, selectedKey, entryValue, namespace]);

  const handleDeleteEntry = useCallback(async () => {
    if (!id || !selectedKey) return;
    if (!window.confirm(`Delete entry "${selectedKey}"?`)) return;
    setError(null);
    try {
      await deleteMemoryEntry(id, selectedKey, namespace || undefined);
      setSelectedKey(null);
      setEntryMeta(null);
      setEntries((prev) => prev.filter((e) => e.key !== selectedKey));
    } catch (err) {
      setError(formatApiError(err, "Delete failed"));
    }
  }, [id, selectedKey, namespace]);

  const handleCreateEntry = useCallback(async () => {
    if (!id || !newKey.trim()) return;
    setError(null);
    try {
      const saved = await putMemoryEntry(id, newKey.trim(), newValue, namespace || undefined);
      setNewKey("");
      setNewValue("");
      setEntries((prev) => [...prev, saved]);
    } catch (err) {
      setError(formatApiError(err, "Unable to create entry"));
    }
  }, [id, newKey, newValue, namespace]);

  if (loading) {
    return <SkeletonRow lines={5} />;
  }

  if (error && !store) {
    return (
      <div className="space-y-4">
        <button
          type="button"
          onClick={() => navigate("/memory")}
          className="inline-flex items-center gap-1.5 text-[13px] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
        >
          <HugeiconsIcon icon={ArrowLeft01Icon} size={14} />
          Back to memory
        </button>
        {isNoRuntime(error) ? (
          <NoRuntimeState />
        ) : (
          <p className="flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
            <HugeiconsIcon icon={AlertCircleIcon} size={14} />
            {error}
          </p>
        )}
      </div>
    );
  }

  if (!store) return null;

  const displayed = searchResults ?? entries;

  return (
    <div className="space-y-5">
      <button
        type="button"
        onClick={() => navigate("/memory")}
        className="inline-flex items-center gap-1.5 text-[13px] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
      >
        <HugeiconsIcon icon={ArrowLeft01Icon} size={14} />
        Back to memory
      </button>

      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 space-y-2">
          <div className="flex flex-wrap items-center gap-2">
            <h1 className="m-0 text-[20px] font-semibold tracking-tight text-[var(--text-primary)]">
              {store.name}
            </h1>
            <Badge>{store.entry_count} entries</Badge>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <MonoChip>{store.id}</MonoChip>
            <span className="text-[12px] text-[var(--text-secondary)]">
              last write {formatTime(store.last_write_at)}
            </span>
          </div>
        </div>
        <label className="flex items-center gap-1.5 text-[12px] text-[var(--text-secondary)]">
          Namespace
          <input
            type="text"
            value={namespace}
            onChange={(e) => void handleNamespaceChange(e.target.value)}
            placeholder="default"
            className="w-36 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-2 py-1.5 font-mono text-[12px] text-[var(--text-primary)] outline-none focus:border-[var(--accent-primary)]"
          />
        </label>
      </div>

      {error && (isNoRuntime(error) ? (
        <NoRuntimeState />
      ) : (
        <p className="flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      ))}

      <div className="relative">
        <HugeiconsIcon
          icon={Search01Icon}
          size={14}
          className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-[var(--text-tertiary)]"
        />
        <input
          type="text"
          value={search}
          onChange={(e) => void handleSearch(e.target.value)}
          placeholder={`Search keys and values in this store${searching ? "…" : ""}`}
          className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] py-2 pl-9 pr-3 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--accent-primary)]"
        />
      </div>
      {searchResults !== null && (
        <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
          {searchResults.length} match{searchResults.length === 1 ? "" : "es"} for “{search}” —{" "}
          <button type="button" className="underline underline-offset-2" onClick={() => void handleSearch("")}>
            clear search
          </button>
        </p>
      )}

      <div className="grid gap-4 lg:grid-cols-2">
        <div className="space-y-2">
          {displayed.length === 0 ? (
            <p className="rounded-xl border border-solid border-[var(--border-subtle)] p-4 text-[13px] text-[var(--text-tertiary)]">
              {searchResults !== null
                ? "Nothing matches the search."
                : "No entries in this namespace yet — create one below or let an agent write one."}
            </p>
          ) : (
            displayed.map((entry) => (
              <button
                key={entry.id}
                type="button"
                onClick={() => void openEntry(entry.key)}
                className={cn(
                  "block w-full rounded-xl border border-solid p-3 text-left transition-colors",
                  selectedKey === entry.key
                    ? "border-[var(--accent-primary)]/50 bg-[var(--accent-primary)]/5"
                    : "border-[var(--border-subtle)] bg-[var(--bg-secondary)] hover:border-[var(--border-default)]"
                )}
              >
                <div className="flex items-center gap-2">
                  <code className="font-mono text-[12px] font-semibold text-[var(--text-primary)]">{entry.key}</code>
                  {entry.namespace !== "default" && <Badge>{entry.namespace}</Badge>}
                  <span className="ml-auto text-[11px] text-[var(--text-tertiary)]">
                    {formatTime(entry.updated_at)}
                  </span>
                </div>
                <p className="m-0 mt-1 whitespace-pre-wrap break-words font-mono text-[12px] text-[var(--text-secondary)]">
                  {entry.value.length > VALUE_PREVIEW_LENGTH && !expandedValues.has(entry.id)
                    ? `${entry.value.slice(0, VALUE_PREVIEW_LENGTH)}…`
                    : entry.value}
                </p>
                {entry.value.length > VALUE_PREVIEW_LENGTH && (
                  <span
                    role="button"
                    tabIndex={0}
                    onClick={(e) => {
                      e.stopPropagation();
                      setExpandedValues((prev) => {
                        const next = new Set(prev);
                        if (next.has(entry.id)) next.delete(entry.id);
                        else next.add(entry.id);
                        return next;
                      });
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.stopPropagation();
                        setExpandedValues((prev) => new Set(prev).add(entry.id));
                      }
                    }}
                    className="mt-1 inline-block text-[11px] font-semibold text-[var(--accent-primary)]"
                  >
                    {expandedValues.has(entry.id) ? "Show less" : "Expand"}
                  </span>
                )}
              </button>
            ))
          )}
          {searchResults === null && nextCursor && (
            <button
              type="button"
              onClick={() => void handleLoadMore()}
              disabled={loadingMore}
              className={cn(QUIET_BUTTON_CLASS, "w-full justify-center")}
            >
              {loadingMore ? "Loading…" : "Load more"}
            </button>
          )}
        </div>

        <div className="space-y-3">
          {selectedKey !== null ? (
            <div className="space-y-3 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
              <div className="flex items-center gap-2">
                <code className="flex-1 font-mono text-[13px] font-semibold text-[var(--text-primary)]">
                  {selectedKey}
                </code>
                {entryMeta && (
                  <span className="text-[11px] text-[var(--text-tertiary)]">
                    updated {formatDate(entryMeta.updated_at)}
                  </span>
                )}
              </div>
              <textarea
                value={entryValue}
                onChange={(e) => setEntryValue(e.target.value)}
                rows={8}
                spellCheck={false}
                className={cn(INPUT_CLASS, "resize-y font-mono text-[12px] leading-relaxed")}
              />
              <div className="flex gap-2">
                <button
                  type="button"
                  onClick={() => void handleSaveEntry()}
                  disabled={savingEntry}
                  className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  {savingEntry ? "Saving…" : "Save entry"}
                </button>
                <button
                  type="button"
                  onClick={() => void handleDeleteEntry()}
                  className={DESTRUCTIVE_BUTTON_CLASS}
                >
                  <HugeiconsIcon icon={TrashIcon} size={13} />
                  Delete
                </button>
              </div>
            </div>
          ) : (
            <p className="m-0 rounded-xl border border-dashed border-[var(--border-subtle)] p-4 text-[12px] text-[var(--text-tertiary)]">
              Select an entry to view and edit it.
            </p>
          )}

          <div className="space-y-2 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
            <h3 className="m-0 text-[13px] font-semibold text-[var(--text-primary)]">New entry</h3>
            <input
              type="text"
              value={newKey}
              onChange={(e) => setNewKey(e.target.value)}
              placeholder="Key"
              className={cn(INPUT_CLASS, "font-mono text-[12px]")}
            />
            <textarea
              value={newValue}
              onChange={(e) => setNewValue(e.target.value)}
              rows={3}
              placeholder="Value"
              className={cn(INPUT_CLASS, "resize-y font-mono text-[12px]")}
            />
            <button
              type="button"
              onClick={() => void handleCreateEntry()}
              disabled={!newKey.trim()}
              className="inline-flex items-center gap-1.5 rounded-lg border border-solid border-[var(--border-subtle)] bg-transparent px-3 py-2 text-[13px] font-medium text-[var(--text-primary)] transition-colors hover:bg-[var(--bg-secondary)] disabled:opacity-50"
            >
              <HugeiconsIcon icon={PlusSignIcon} size={13} />
              Write entry
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

export default MemoryPage;
