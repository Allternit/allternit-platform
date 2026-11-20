import React, { useState } from "react";
import { Check, Copy, Plus, WarningCircle, X } from "@phosphor-icons/react";
import { cn } from "@/lib/utils";
import { QUIET_BUTTON_CLASS, DESTRUCTIVE_BUTTON_CLASS } from "@/components/console-ui/buttonStyles";
import {
  MAX_REDIRECT_URIS,
  SECRET_ONCE_LINE,
  STATE_LABEL,
  validateRedirectUri,
  type OAuthApp,
  type OAuthAppState,
} from "@/lib/oauth-apps";

const INPUT =
  "w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] p-2 px-3 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--accent-primary)]";
const PRIMARY =
  "inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-all hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-50";

const STATE_CLASS: Record<OAuthAppState, string> = {
  testing: "bg-[var(--bg-secondary)] text-[var(--text-secondary)]",
  in_review: "bg-[var(--accent-primary)]/10 text-[var(--accent-primary)]",
  live: "bg-[var(--status-success)]/10 text-[var(--status-success)]",
  suspended: "bg-[var(--status-error)]/10 text-[var(--status-error)]",
};

export function StateBadge({ state }: { state: OAuthAppState }) {
  return (
    <span className={cn("inline-flex items-center rounded-md px-1.5 py-0.5 text-[11px] font-medium", STATE_CLASS[state])}>
      {STATE_LABEL[state] ?? state}
    </span>
  );
}

/** Shown once. The secret lives only in the parent's state; closing the box clears it. */
export function SecretBox({ secret, onClose }: { secret: string; onClose: () => void }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(secret);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      /* clipboard blocked: the secret stays selectable */
    }
  };
  return (
    <div
      role="status"
      className="rounded-xl border border-solid border-[var(--accent-primary)]/25 bg-[var(--accent-primary)]/[0.03] p-4 space-y-2"
    >
      <div className="text-[14px] font-semibold text-[var(--text-primary)]">Client secret</div>
      <div className="flex flex-wrap items-center gap-2">
        <code
          data-testid="client-secret"
          className="min-w-0 flex-1 break-all rounded-lg bg-[var(--bg-secondary)] px-3 py-2 text-[12px] text-[var(--text-primary)] select-all"
        >
          {secret}
        </code>
        <button type="button" onClick={copy} className={QUIET_BUTTON_CLASS} aria-label="Copy client secret">
          {copied ? <Check size={14} aria-hidden /> : <Copy size={14} aria-hidden />} {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <p className="m-0 text-[12px] text-[var(--text-secondary)]">{SECRET_ONCE_LINE}</p>
      <button type="button" onClick={onClose} className={QUIET_BUTTON_CLASS}>
        I have copied it
      </button>
    </div>
  );
}

/** After create or rotate: the client id, plus the secret box for confidential apps only. */
export function CreatedNotice({
  app,
  secret,
  onClose,
}: {
  app: Pick<OAuthApp, "client_id" | "client_type" | "name">;
  secret: string | null;
  onClose: () => void;
}) {
  return (
    <div className="space-y-3">
      <div className="text-[13px] text-[var(--text-primary)]">
        {app.name} is ready. Client ID: <code className="break-all">{app.client_id}</code>
      </div>
      {app.client_type === "confidential" && secret ? (
        <SecretBox secret={secret} onClose={onClose} />
      ) : (
        <p className="m-0 text-[12px] text-[var(--text-secondary)]">
          Public apps have no client secret. They sign in with PKCE only.
        </p>
      )}
    </div>
  );
}

/** Redirect URI list editor. Validates as you type; the server stays the authority. */
export function RedirectEditor({
  uris,
  onAdd,
  onRemove,
  busy,
  initialDraft = "",
}: {
  uris: string[];
  onAdd: (uri: string) => void | Promise<void>;
  onRemove: (uri: string) => void | Promise<void>;
  busy?: boolean;
  initialDraft?: string;
}) {
  const [draft, setDraft] = useState(initialDraft);
  const error = draft.trim() ? validateRedirectUri(draft, uris) : null;
  const full = uris.length >= MAX_REDIRECT_URIS;
  const submit = async () => {
    if (!draft.trim() || error || full) return;
    await onAdd(draft.trim());
    setDraft("");
  };
  return (
    <div className="space-y-2">
      <ul className="m-0 list-none space-y-1.5 p-0">
        {uris.map((u) => (
          <li key={u} className="flex items-center gap-2 rounded-lg bg-[var(--bg-secondary)] px-3 py-1.5 text-[13px]">
            <span className="min-w-0 flex-1 break-all text-[var(--text-primary)]">{u}</span>
            <button
              type="button"
              onClick={() => void onRemove(u)}
              disabled={busy}
              aria-label={`Remove ${u}`}
              className="rounded-md p-1 text-[var(--text-tertiary)] hover:bg-[var(--surface-hover)] hover:text-[var(--status-error)]"
            >
              <X size={14} aria-hidden />
            </button>
          </li>
        ))}
      </ul>
      <div className="flex flex-wrap gap-2">
        <input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void submit();
            }
          }}
          placeholder="https://example.com/callback"
          aria-label="Redirect URI"
          aria-invalid={error ? true : undefined}
          aria-describedby="redirect-uri-help"
          disabled={full}
          className={cn(INPUT, "min-w-0 flex-1")}
        />
        <button type="button" onClick={() => void submit()} disabled={busy || !draft.trim() || !!error || full} className={QUIET_BUTTON_CLASS}>
          <Plus size={14} aria-hidden /> Add
        </button>
      </div>
      <p
        id="redirect-uri-help"
        role={error ? "alert" : undefined}
        className={cn("m-0 flex items-start gap-1 text-[12px]", error ? "text-[var(--status-error)]" : "text-[var(--text-tertiary)]")}
      >
        {error && <WarningCircle size={14} className="mt-0.5 shrink-0" aria-hidden />}
        {error ?? `Exact match. https, or http on localhost. Up to ${MAX_REDIRECT_URIS} (${uris.length} used).`}
      </p>
    </div>
  );
}

/** State-dependent actions on the detail page. */
export function StateActions({
  state,
  busy,
  onSubmit,
  onWithdraw,
  onRotate,
  onDelete,
  showRotate = true,
}: {
  state: OAuthAppState;
  busy?: boolean;
  onSubmit: () => void;
  onWithdraw: () => void;
  onRotate: () => void;
  onDelete: () => void;
  showRotate?: boolean;
}) {
  return (
    <div className="flex flex-wrap gap-2">
      {state === "testing" && (
        <button type="button" className={PRIMARY} disabled={busy} onClick={onSubmit}>
          Submit for review
        </button>
      )}
      {state === "in_review" && (
        <button type="button" className={QUIET_BUTTON_CLASS} disabled={busy} onClick={onWithdraw}>
          Withdraw
        </button>
      )}
      {showRotate && (
        <button type="button" className={QUIET_BUTTON_CLASS} disabled={busy} onClick={onRotate}>
          Rotate secret
        </button>
      )}
      <button type="button" className={DESTRUCTIVE_BUTTON_CLASS} disabled={busy} onClick={onDelete}>
        Delete
      </button>
    </div>
  );
}

export { INPUT as OAUTH_INPUT_CLASS, PRIMARY as OAUTH_PRIMARY_CLASS };
