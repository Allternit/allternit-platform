import React, { useCallback, useEffect, useState } from "react";
import { formatApiError } from "@/lib/api-client";
import { QUIET_BUTTON_CLASS, DESTRUCTIVE_BUTTON_CLASS } from "@/components/console-ui";
import {
  deletePrompt,
  listPrompts,
  savePrompt,
  sharePrompt,
  unsharePrompt,
  updatePrompt,
  type PromptBody,
  type SavedPrompt,
} from "@/lib/saved-prompts";

const INPUT_CLASS =
  "min-w-0 flex-1 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-1.5 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]";

interface PromptsPanelProps {
  projectId: string | null;
  /** Snapshot of the current editor (images are stripped by the caller). */
  getBody: () => PromptBody;
  onLoad: (prompt: SavedPrompt) => void;
}

/** Saved prompts for the selected project: save, load, rename, delete, share. */
export function PromptsPanel({ projectId, getBody, onLoad }: PromptsPanelProps): React.ReactNode {
  const [items, setItems] = useState<SavedPrompt[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [renaming, setRenaming] = useState<{ id: string; name: string } | null>(null);

  const reload = useCallback(async () => {
    if (!projectId) return;
    setLoading(true);
    setError(null);
    try {
      setItems(await listPrompts(projectId));
    } catch (err) {
      setError(formatApiError(err, "Could not load saved prompts."));
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    setItems([]);
    void reload();
  }, [reload]);

  const guard = async (fn: () => Promise<void>) => {
    setError(null);
    setNotice(null);
    try {
      await fn();
    } catch (err) {
      setError(formatApiError(err, "That did not work."));
    }
  };

  if (!projectId) {
    return <p className="m-0 text-[12px] text-[var(--text-tertiary)]">Select a project to save prompts.</p>;
  }

  return (
    <div className="space-y-3">
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          const trimmed = name.trim();
          if (!trimmed) return;
          void guard(async () => {
            await savePrompt(projectId, trimmed, getBody());
            setName("");
            await reload();
          });
        }}
      >
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Name this prompt"
          aria-label="Prompt name" className={INPUT_CLASS} />
        <button type="submit" disabled={!name.trim()} className={QUIET_BUTTON_CLASS}>Save</button>
      </form>
      {error && <p className="m-0 text-[12px] text-[var(--status-error)]">{error}</p>}
      {notice && <p className="m-0 break-all text-[12px] text-[var(--text-secondary)]">{notice}</p>}
      {loading && items.length === 0 && <p className="m-0 text-[12px] text-[var(--text-tertiary)]">Loading…</p>}
      {!loading && items.length === 0 && !error && (
        <p className="m-0 text-[12px] text-[var(--text-tertiary)]">No saved prompts in this project yet.</p>
      )}
      <ul className="m-0 list-none space-y-2 p-0">
        {items.map((p) => (
          <li key={p.id} className="space-y-2 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-3">
            {renaming?.id === p.id ? (
              <form
                className="flex gap-2"
                onSubmit={(e) => {
                  e.preventDefault();
                  const next = renaming.name.trim();
                  if (!next) return;
                  void guard(async () => {
                    await updatePrompt(projectId, p.id, { name: next });
                    setRenaming(null);
                    await reload();
                  });
                }}
              >
                <input autoFocus value={renaming.name} aria-label="New name"
                  onChange={(e) => setRenaming({ id: p.id, name: e.target.value })} className={INPUT_CLASS} />
                <button type="submit" className={QUIET_BUTTON_CLASS}>Rename</button>
              </form>
            ) : (
              <span className="block break-words text-[13px] font-semibold text-[var(--text-primary)]">{p.name}</span>
            )}
            <div className="flex flex-wrap gap-1.5">
              <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => onLoad(p)}>Open</button>
              <button type="button" className={QUIET_BUTTON_CLASS}
                onClick={() => void guard(async () => {
                  await updatePrompt(projectId, p.id, { body: getBody() });
                  setNotice(`Updated "${p.name}" with the current editor.`);
                  await reload();
                })}>
                Overwrite
              </button>
              <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setRenaming({ id: p.id, name: p.name })}>Rename</button>
              <button type="button" className={QUIET_BUTTON_CLASS}
                onClick={() => void guard(async () => {
                  const { share_url } = await sharePrompt(projectId, p.id);
                  try { await navigator.clipboard.writeText(share_url); setNotice(`Share link copied: ${share_url}`); }
                  catch { setNotice(`Share link: ${share_url}`); }
                  await reload();
                })}>
                {p.share_url ? "Copy share link" : "Share"}
              </button>
              {p.share_url && (
                <button type="button" className={QUIET_BUTTON_CLASS}
                  onClick={() => void guard(async () => { await unsharePrompt(projectId, p.id); await reload(); })}>
                  Stop sharing
                </button>
              )}
              <button type="button" className={DESTRUCTIVE_BUTTON_CLASS}
                onClick={() => {
                  if (!window.confirm(`Delete "${p.name}"?`)) return;
                  void guard(async () => { await deletePrompt(projectId, p.id); await reload(); });
                }}>
                Delete
              </button>
            </div>
          </li>
        ))}
      </ul>
      <p className="m-0 text-[11px] text-[var(--text-tertiary)]">
        Share links open the prompt for any signed-in user. Images are not saved.
      </p>
    </div>
  );
}
