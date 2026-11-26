import React, { useCallback, useEffect, useMemo, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import { AlertCircleIcon, InformationCircleIcon } from "@hugeicons/core-free-icons";
import { formatApiError } from "@/lib/api-client";
import {
  TAG_RESOURCE_TYPES,
  type ResourceTag,
  type TagResourceType,
  listTags,
  createTag,
  deleteTag,
} from "@/lib/tags";
import {
  ListPage,
  EmptyState,
  SkeletonRow,
  Badge,
  MonoChip,
  QUIET_BUTTON_CLASS,
  DESTRUCTIVE_BUTTON_CLASS,
} from "@/components/console-ui";
import { cn } from "@/lib/utils";

const TH_CLASS =
  "text-left text-[11px] font-semibold uppercase tracking-wide text-[var(--text-tertiary)] px-3 py-2";
const TD_CLASS = "px-3 py-2.5 text-[13px] text-[var(--text-primary)]";

function formatDate(iso?: string | null): string {
  if (!iso) return "—";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

/**
 * Resource tags — key/value labels on agents, sessions, gateway keys, and
 * deployments. The row shape is exactly what tag_routes.rs returns: there is
 * no created-by column, so none is rendered.
 */
export function TagsPage(): React.ReactNode {
  const [tags, setTags] = useState<ResourceTag[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");

  const [showCreate, setShowCreate] = useState(false);
  const [formResourceType, setFormResourceType] = useState<TagResourceType>("agent");
  const [formResourceId, setFormResourceId] = useState("");
  const [formKey, setFormKey] = useState("");
  const [formValue, setFormValue] = useState("");
  const [saving, setSaving] = useState(false);
  const [upsertNotice, setUpsertNotice] = useState<string | null>(null);

  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setTags(await listTags());
    } catch (err) {
      setError(formatApiError(err, "Unable to load tags."));
      setTags([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return tags;
    return tags.filter(
      (tag) =>
        tag.key.toLowerCase().includes(q) ||
        tag.value.toLowerCase().includes(q) ||
        tag.resource_id.toLowerCase().includes(q)
    );
  }, [tags, search]);

  const keyValid = formKey.trim().length >= 1 && formKey.trim().length <= 128;
  const valueValid = formValue.trim().length <= 128;

  const handleCreate = useCallback(async () => {
    if (!keyValid || !valueValid || !formResourceId.trim()) return;
    setSaving(true);
    setError(null);
    setUpsertNotice(null);
    try {
      const result = await createTag({
        resource_type: formResourceType,
        resource_id: formResourceId,
        key: formKey,
        value: formValue,
      });
      if (result.updated) {
        setUpsertNotice(
          `A tag with key "${result.tag.key}" already existed on that resource — its value was replaced.`
        );
      }
      setFormResourceId("");
      setFormKey("");
      setFormValue("");
      setShowCreate(false);
      await load();
    } catch (err) {
      setError(formatApiError(err, "Unable to save the tag."));
    } finally {
      setSaving(false);
    }
  }, [formResourceType, formResourceId, formKey, formValue, keyValid, valueValid, load]);

  const handleDelete = useCallback(
    async (tag: ResourceTag) => {
      setBusyId(tag.id);
      setError(null);
      try {
        await deleteTag(tag.id);
        setConfirmDeleteId(null);
        await load();
      } catch (err) {
        setError(formatApiError(err, "Unable to delete the tag."));
      } finally {
        setBusyId(null);
      }
    },
    [load]
  );

  return (
    <ListPage
      title="Tags"
      subtitle="Key/value labels on agents, sessions, gateway keys, and deployments."
      searchPlaceholder="Search tags"
      onSearch={setSearch}
      primaryAction={{ label: showCreate ? "Close" : "Add tag", onClick: () => setShowCreate((prev) => !prev) }}
    >
      {error && (
        <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      )}

      <div className="mb-5 flex items-start gap-2 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/60 px-3 py-2.5 text-[12px] text-[var(--text-secondary)]">
        <HugeiconsIcon icon={InformationCircleIcon} size={15} className="mt-0.5 shrink-0 text-[var(--accent-primary)]" />
        <span>
          Tags appear as filters and group-by dimensions in Analytics — Usage
          group-by: tag, and the Logs tag filter. Gateway-key tags are inherited
          onto LLM usage events, so model spend rolls up by tag automatically.
        </span>
      </div>

      {upsertNotice && (
        <p className="mb-4 rounded-lg border border-solid border-[var(--status-warning)]/30 bg-[var(--status-warning)]/10 px-3 py-2 text-[12px] text-[var(--text-secondary)]">
          {upsertNotice}
        </p>
      )}

      {showCreate && (
        <div className="mb-5 max-w-2xl rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <label className="block">
              <span className="mb-1 block text-[12px] font-semibold text-[var(--text-secondary)]">
                Resource type
              </span>
              <select
                value={formResourceType}
                onChange={(event) => setFormResourceType(event.target.value as TagResourceType)}
                className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none focus:border-[var(--border-default)]"
              >
                {TAG_RESOURCE_TYPES.map((resourceType) => (
                  <option key={resourceType} value={resourceType}>
                    {resourceType}
                  </option>
                ))}
              </select>
            </label>
            <label className="block">
              <span className="mb-1 block text-[12px] font-semibold text-[var(--text-secondary)]">
                Resource ID
              </span>
              <input
                value={formResourceId}
                onChange={(event) => setFormResourceId(event.target.value)}
                placeholder="e.g. the agent, session, key, or deployment id"
                className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 font-mono text-[12px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]"
              />
            </label>
            <label className="block">
              <span className="mb-1 block text-[12px] font-semibold text-[var(--text-secondary)]">
                Key
              </span>
              <input
                value={formKey}
                onChange={(event) => setFormKey(event.target.value)}
                placeholder="team"
                maxLength={128}
                className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 font-mono text-[12px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]"
              />
              <span className="mt-1 block text-[11px] text-[var(--text-tertiary)]">
                1–128 characters. Re-using a key on the same resource replaces its value.
              </span>
            </label>
            <label className="block">
              <span className="mb-1 block text-[12px] font-semibold text-[var(--text-secondary)]">
                Value
              </span>
              <input
                value={formValue}
                onChange={(event) => setFormValue(event.target.value)}
                placeholder="alpha"
                maxLength={128}
                className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 font-mono text-[12px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]"
              />
              <span className="mt-1 block text-[11px] text-[var(--text-tertiary)]">
                At most 128 characters.
              </span>
            </label>
          </div>
          <div className="mt-4 flex items-center justify-end gap-2">
            <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setShowCreate(false)}>
              Cancel
            </button>
            <button
              type="button"
              disabled={saving || !keyValid || !valueValid || !formResourceId.trim()}
              onClick={() => void handleCreate()}
              className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {saving ? "Saving…" : "Save tag"}
            </button>
          </div>
        </div>
      )}

      {loading && tags.length === 0 ? (
        <SkeletonRow lines={4} />
      ) : filtered.length === 0 ? (
        <EmptyState
          art="search"
          title={search ? "No tags match" : "No tags yet"}
          caption={
            search
              ? `Nothing matches "${search}".`
              : "Tag resources with key/value pairs so usage and spend can be attributed per team, project, or customer."
          }
          ctaLabel={search ? "Clear search" : showCreate ? undefined : "Add tag"}
          onCtaClick={search ? () => setSearch("") : showCreate ? undefined : () => setShowCreate(true)}
        />
      ) : (
        <div className="overflow-hidden rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40">
          <table className="w-full">
            <thead>
              <tr className="border-b border-solid border-[var(--border-subtle)]">
                <th className={TH_CLASS}>Key</th>
                <th className={TH_CLASS}>Value</th>
                <th className={cn(TH_CLASS, "hidden lg:table-cell")}>Resource</th>
                <th className={cn(TH_CLASS, "hidden md:table-cell")}>Created</th>
                <th className={cn(TH_CLASS, "text-right")}>Actions</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((tag) => (
                <tr key={tag.id} className="border-b border-solid border-[var(--border-subtle)] last:border-b-0">
                  <td className={TD_CLASS}>
                    <MonoChip>{tag.key}</MonoChip>
                  </td>
                  <td className={cn(TD_CLASS, "font-mono text-[12px]")}>{tag.value}</td>
                  <td className={cn(TD_CLASS, "hidden lg:table-cell")}>
                    <div className="flex items-center gap-1.5">
                      <Badge>{tag.resource_type}</Badge>
                      <code className="max-w-[220px] truncate font-mono text-[11px] text-[var(--text-tertiary)]">
                        {tag.resource_id}
                      </code>
                    </div>
                  </td>
                  <td className={cn(TD_CLASS, "hidden text-[var(--text-secondary)] md:table-cell")}>
                    {formatDate(tag.created_at)}
                  </td>
                  <td className={cn(TD_CLASS, "text-right")}>
                    {confirmDeleteId === tag.id ? (
                      <span className="inline-flex items-center gap-2">
                        <span className="text-[12px] text-[var(--text-secondary)]">Delete?</span>
                        <button
                          type="button"
                          className={QUIET_BUTTON_CLASS}
                          disabled={busyId === tag.id}
                          onClick={() => setConfirmDeleteId(null)}
                        >
                          Cancel
                        </button>
                        <button
                          type="button"
                          className={DESTRUCTIVE_BUTTON_CLASS}
                          disabled={busyId === tag.id}
                          onClick={() => void handleDelete(tag)}
                        >
                          {busyId === tag.id ? "Deleting…" : "Delete"}
                        </button>
                      </span>
                    ) : (
                      <button
                        type="button"
                        className={DESTRUCTIVE_BUTTON_CLASS}
                        onClick={() => setConfirmDeleteId(tag.id)}
                      >
                        Delete
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </ListPage>
  );
}

export default TagsPage;
