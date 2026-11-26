import React, { useCallback, useEffect, useMemo, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  CheckIcon,
  CopyIcon,
  ArrowReloadHorizontalIcon,
} from "@hugeicons/core-free-icons";
import { formatApiError, AllternitApiError } from "@/lib/api-client";
import {
  type ServiceAccount,
  type CreatedServiceAccount,
  type RotatedServiceAccount,
  listServiceAccounts,
  createServiceAccount,
  updateServiceAccount,
  deleteServiceAccount,
  rotateServiceAccount,
} from "@/lib/service-accounts";
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

/** Suggested scope presets — the backend stores scopes as an opaque list. */
const SCOPE_PRESETS = [
  { value: "gateway:read", label: "gateway:read" },
  { value: "gateway:write", label: "gateway:write" },
  { value: "compute", label: "compute" },
  { value: "billing:read", label: "billing:read" },
];

function formatDate(iso?: string | null): string {
  if (!iso) return "—";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

interface SecretReveal {
  kind: "created" | "rotated";
  accountId: string;
  clientId: string;
  clientSecret: string;
}

export function ServiceAccountsPage(): React.ReactNode {
  const [accounts, setAccounts] = useState<ServiceAccount[]>([]);
  const [loading, setLoading] = useState(true);
  const [forbidden, setForbidden] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");

  const [showCreate, setShowCreate] = useState(false);
  const [formName, setFormName] = useState("");
  const [formScopes, setFormScopes] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);

  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [editScopes, setEditScopes] = useState<string[]>([]);

  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [confirmRotateId, setConfirmRotateId] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [reveal, setReveal] = useState<SecretReveal | null>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await listServiceAccounts();
      setAccounts(data);
      setForbidden(false);
    } catch (err) {
      if (err instanceof AllternitApiError && err.statusCode === 403) {
        setForbidden(true);
        setAccounts([]);
      } else {
        setError(formatApiError(err, "Unable to load service accounts."));
        setAccounts([]);
      }
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return accounts;
    return accounts.filter(
      (account) =>
        account.name.toLowerCase().includes(q) || account.client_id.toLowerCase().includes(q)
    );
  }, [accounts, search]);

  const copyText = useCallback(async (text: string, id: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedId(id);
      window.setTimeout(() => setCopiedId(null), 2000);
    } catch {
      // leave the chip selectable
    }
  }, []);

  const handleCreate = useCallback(async () => {
    if (!formName.trim()) return;
    setSaving(true);
    setError(null);
    try {
      const created: CreatedServiceAccount = await createServiceAccount({
        name: formName,
        scopes: formScopes,
      });
      setReveal({
        kind: "created",
        accountId: created.id,
        clientId: created.client_id,
        clientSecret: created.client_secret,
      });
      setShowCreate(false);
      setFormName("");
      setFormScopes([]);
      await load();
    } catch (err) {
      setError(formatApiError(err, "Unable to create the service account."));
    } finally {
      setSaving(false);
    }
  }, [formName, formScopes, load]);

  const startEdit = useCallback((account: ServiceAccount) => {
    setEditingId(account.id);
    setEditName(account.name);
    setEditScopes(account.scopes ?? []);
    setReveal(null);
  }, []);

  const handleSaveEdit = useCallback(
    async (account: ServiceAccount) => {
      setBusyId(account.id);
      setError(null);
      try {
        await updateServiceAccount(account.id, { name: editName, scopes: editScopes });
        setEditingId(null);
        await load();
      } catch (err) {
        setError(formatApiError(err, "Unable to update the service account."));
      } finally {
        setBusyId(null);
      }
    },
    [editName, editScopes, load]
  );

  const handleRotate = useCallback(
    async (account: ServiceAccount) => {
      setBusyId(account.id);
      setError(null);
      try {
        const rotated: RotatedServiceAccount = await rotateServiceAccount(account.id);
        setReveal({
          kind: "rotated",
          accountId: rotated.id,
          clientId: rotated.client_id,
          clientSecret: rotated.client_secret,
        });
        setConfirmRotateId(null);
        await load();
      } catch (err) {
        setError(formatApiError(err, "Unable to rotate the secret."));
      } finally {
        setBusyId(null);
      }
    },
    [load]
  );

  const handleDelete = useCallback(
    async (account: ServiceAccount) => {
      setBusyId(account.id);
      setError(null);
      try {
        await deleteServiceAccount(account.id);
        setConfirmDeleteId(null);
        await load();
      } catch (err) {
        setError(formatApiError(err, "Unable to delete the service account."));
      } finally {
        setBusyId(null);
      }
    },
    [load]
  );

  const scopePicker = (
    selected: string[],
    onChange: (next: string[]) => void
  ): React.ReactNode => (
    <div>
      <div className="mb-1.5 text-[12px] font-semibold text-[var(--text-secondary)]">
        Scopes
      </div>
      <div className="flex flex-wrap gap-2">
        {SCOPE_PRESETS.map((preset) => (
          <label
            key={preset.value}
            className={cn(
              "inline-flex cursor-pointer items-center gap-1.5 rounded-lg border border-solid px-3 py-1.5 text-[12px] font-medium transition-colors",
              selected.includes(preset.value)
                ? "border-[var(--accent-primary)]/30 bg-[var(--accent-primary)]/10 text-[var(--accent-primary)]"
                : "border-[var(--border-subtle)] bg-[var(--bg-primary)] text-[var(--text-secondary)] hover:border-[var(--border-default)]"
            )}
          >
            <input
              type="checkbox"
              className="sr-only"
              checked={selected.includes(preset.value)}
              onChange={(event) =>
                onChange(
                  event.target.checked
                    ? [...selected, preset.value]
                    : selected.filter((scope) => scope !== preset.value)
                )
              }
            />
            {preset.label}
          </label>
        ))}
      </div>
      <p className="m-0 mt-1.5 text-[11px] text-[var(--text-tertiary)]">
        Leave all unchecked for an unrestricted account. The gateway enforces the
        stored scope list on token-authenticated calls.
      </p>
    </div>
  );

  return (
    <ListPage
      title="Service accounts"
      subtitle="Non-human identities with scoped credentials — for CI pipelines, schedulers, and integrations."
      searchPlaceholder="Search service accounts"
      onSearch={setSearch}
      primaryAction={
        forbidden
          ? undefined
          : { label: showCreate ? "Close" : "Create service account", onClick: () => setShowCreate((prev) => !prev) }
      }
    >
      {error && (
        <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      )}

      {forbidden ? (
        <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
          <p className="m-0 text-[13px] text-[var(--text-secondary)]">
            Service accounts are visible to organization owners and admins only. Contact
            an organization admin to create or rotate credentials for automation.
          </p>
        </div>
      ) : (
        <>
          {reveal && (
            <div className="mb-5 rounded-xl border border-solid border-[var(--status-success)]/25 bg-[var(--status-success)]/[0.06] p-4">
              <div className="mb-3 flex items-start gap-2">
                <HugeiconsIcon icon={CheckIcon} size={16} className="mt-0.5 shrink-0 text-[var(--status-success)]" />
                <div>
                  <div className="text-[13px] font-semibold text-[var(--text-primary)]">
                    {reveal.kind === "created" ? "Service account created" : "Secret rotated"} — copy the
                    client secret now
                  </div>
                  <p className="m-0 mt-0.5 text-[12px] text-[var(--text-secondary)]">
                    It will not be shown again. Store it in a secret manager and authenticate
                    with the client ID + client secret pair.
                  </p>
                </div>
              </div>
              <div className="space-y-2">
                <div className="flex items-center gap-2 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2">
                  <span className="w-20 shrink-0 text-[11px] uppercase tracking-wide text-[var(--text-tertiary)]">
                    Client ID
                  </span>
                  <code className="flex-1 truncate font-mono text-[12px] text-[var(--text-primary)]">
                    {reveal.clientId}
                  </code>
                  <button
                    type="button"
                    aria-label="Copy client ID"
                    className="p-1 text-[var(--text-tertiary)] transition-colors hover:text-[var(--text-primary)]"
                    onClick={() => void copyText(reveal.clientId, "reveal-id")}
                  >
                    <HugeiconsIcon
                      icon={copiedId === "reveal-id" ? CheckIcon : CopyIcon}
                      size={13}
                      className={copiedId === "reveal-id" ? "text-[var(--status-success)]" : undefined}
                    />
                  </button>
                </div>
                <div className="flex items-center gap-2 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2">
                  <span className="w-20 shrink-0 text-[11px] uppercase tracking-wide text-[var(--text-tertiary)]">
                    Secret
                  </span>
                  <code className="flex-1 truncate font-mono text-[12px] text-[var(--text-primary)]">
                    {reveal.clientSecret}
                  </code>
                  <button
                    type="button"
                    aria-label="Copy client secret"
                    className="p-1 text-[var(--text-tertiary)] transition-colors hover:text-[var(--text-primary)]"
                    onClick={() => void copyText(reveal.clientSecret, "reveal-secret")}
                  >
                    <HugeiconsIcon
                      icon={copiedId === "reveal-secret" ? CheckIcon : CopyIcon}
                      size={13}
                      className={copiedId === "reveal-secret" ? "text-[var(--status-success)]" : undefined}
                    />
                  </button>
                </div>
              </div>
              <div className="mt-3 flex justify-end">
                <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setReveal(null)}>
                  Done
                </button>
              </div>
            </div>
          )}

          {showCreate && (
            <div className="mb-5 max-w-xl rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
              <label className="block">
                <span className="mb-1 block text-[12px] font-semibold text-[var(--text-secondary)]">
                  Name
                </span>
                <input
                  value={formName}
                  onChange={(event) => setFormName(event.target.value)}
                  placeholder="CI pipeline"
                  maxLength={128}
                  className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]"
                />
                <span className="mt-1 block text-[11px] text-[var(--text-tertiary)]">
                  1–128 characters.
                </span>
              </label>
              <div className="mt-3">{scopePicker(formScopes, setFormScopes)}</div>
              <div className="mt-4 flex items-center justify-end gap-2">
                <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setShowCreate(false)}>
                  Cancel
                </button>
                <button
                  type="button"
                  disabled={saving || !formName.trim()}
                  onClick={() => void handleCreate()}
                  className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  {saving ? "Creating…" : "Create service account"}
                </button>
              </div>
            </div>
          )}

          {loading && accounts.length === 0 ? (
            <SkeletonRow lines={4} />
          ) : filtered.length === 0 ? (
            <EmptyState
              art="search"
              title={search ? "No service accounts match" : "No service accounts yet"}
              caption={
                search
                  ? `Nothing matches "${search}".`
                  : "Create a service account to give automation a scoped identity with its own credentials."
              }
              ctaLabel={search ? "Clear search" : showCreate ? undefined : "Create service account"}
              onCtaClick={search ? () => setSearch("") : showCreate ? undefined : () => setShowCreate(true)}
            />
          ) : (
            <div className="overflow-hidden rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40">
              <table className="w-full">
                <thead>
                  <tr className="border-b border-solid border-[var(--border-subtle)]">
                    <th className={TH_CLASS}>Name</th>
                    <th className={TH_CLASS}>Client ID</th>
                    <th className={cn(TH_CLASS, "hidden lg:table-cell")}>Scopes</th>
                    <th className={cn(TH_CLASS, "hidden md:table-cell")}>Created</th>
                    <th className={cn(TH_CLASS, "hidden md:table-cell")}>Last rotated</th>
                    <th className={cn(TH_CLASS, "text-right")}>Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {filtered.map((account) =>
                    editingId === account.id ? (
                      <tr key={account.id} className="border-b border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/60">
                        <td className={cn(TD_CLASS, "align-top")} colSpan={3}>
                          <input
                            value={editName}
                            onChange={(event) => setEditName(event.target.value)}
                            maxLength={128}
                            className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none focus:border-[var(--border-default)]"
                          />
                          <div className="mt-3">{scopePicker(editScopes, setEditScopes)}</div>
                        </td>
                        <td className={cn(TD_CLASS, "hidden md:table-cell")} />
                        <td className={cn(TD_CLASS, "hidden md:table-cell")} />
                        <td className={cn(TD_CLASS, "align-top text-right")}>
                          <span className="inline-flex gap-2">
                            <button
                              type="button"
                              className={QUIET_BUTTON_CLASS}
                              disabled={busyId === account.id}
                              onClick={() => setEditingId(null)}
                            >
                              Cancel
                            </button>
                            <button
                              type="button"
                              disabled={busyId === account.id || !editName.trim()}
                              onClick={() => void handleSaveEdit(account)}
                              className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3 py-1.5 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50"
                            >
                              Save
                            </button>
                          </span>
                        </td>
                      </tr>
                    ) : (
                      <tr key={account.id} className="border-b border-solid border-[var(--border-subtle)] last:border-b-0">
                        <td className={cn(TD_CLASS, "font-medium")}>{account.name}</td>
                        <td className={TD_CLASS}>
                          <button
                            type="button"
                            className="p-0"
                            aria-label={`Copy client ID for ${account.name}`}
                            onClick={() => void copyText(account.client_id, account.id)}
                          >
                            <MonoChip>
                              {account.client_id}
                              <HugeiconsIcon
                                icon={copiedId === account.id ? CheckIcon : CopyIcon}
                                size={11}
                                className={cn("ml-1.5", copiedId === account.id && "text-[var(--status-success)]")}
                              />
                            </MonoChip>
                          </button>
                        </td>
                        <td className={cn(TD_CLASS, "hidden lg:table-cell")}>
                          {account.scopes && account.scopes.length > 0 ? (
                            <div className="flex flex-wrap gap-1">
                              {account.scopes.map((scope) => (
                                <Badge key={scope}>{scope}</Badge>
                              ))}
                            </div>
                          ) : (
                            <span className="text-[12px] text-[var(--text-tertiary)]">unrestricted</span>
                          )}
                        </td>
                        <td className={cn(TD_CLASS, "hidden text-[var(--text-secondary)] md:table-cell")}>
                          {formatDate(account.created_at)}
                        </td>
                        <td className={cn(TD_CLASS, "hidden text-[var(--text-secondary)] md:table-cell")}>
                          {formatDate(account.last_rotated_at)}
                        </td>
                        <td className={cn(TD_CLASS, "text-right")}>
                          <span className="inline-flex items-center gap-1">
                            {confirmRotateId === account.id ? (
                              <>
                                <span className="text-[12px] text-[var(--text-secondary)]">Rotate secret?</span>
                                <button
                                  type="button"
                                  className={QUIET_BUTTON_CLASS}
                                  disabled={busyId === account.id}
                                  onClick={() => setConfirmRotateId(null)}
                                >
                                  Cancel
                                </button>
                                <button
                                  type="button"
                                  className={DESTRUCTIVE_BUTTON_CLASS}
                                  disabled={busyId === account.id}
                                  onClick={() => void handleRotate(account)}
                                >
                                  {busyId === account.id ? "Rotating…" : "Rotate"}
                                </button>
                              </>
                            ) : confirmDeleteId === account.id ? (
                              <>
                                <span className="text-[12px] text-[var(--text-secondary)]">Delete?</span>
                                <button
                                  type="button"
                                  className={QUIET_BUTTON_CLASS}
                                  disabled={busyId === account.id}
                                  onClick={() => setConfirmDeleteId(null)}
                                >
                                  Cancel
                                </button>
                                <button
                                  type="button"
                                  className={DESTRUCTIVE_BUTTON_CLASS}
                                  disabled={busyId === account.id}
                                  onClick={() => void handleDelete(account)}
                                >
                                  {busyId === account.id ? "Deleting…" : "Delete"}
                                </button>
                              </>
                            ) : (
                              <>
                                <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => startEdit(account)}>
                                  Edit
                                </button>
                                <button
                                  type="button"
                                  className={QUIET_BUTTON_CLASS}
                                  disabled={busyId === account.id}
                                  onClick={() => {
                                    setConfirmRotateId(account.id);
                                    setConfirmDeleteId(null);
                                  }}
                                >
                                  <HugeiconsIcon icon={ArrowReloadHorizontalIcon} size={12} /> Rotate
                                </button>
                                <button
                                  type="button"
                                  className={DESTRUCTIVE_BUTTON_CLASS}
                                  onClick={() => {
                                    setConfirmDeleteId(account.id);
                                    setConfirmRotateId(null);
                                  }}
                                >
                                  Delete
                                </button>
                              </>
                            )}
                          </span>
                        </td>
                      </tr>
                    )
                  )}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}
    </ListPage>
  );
}

export default ServiceAccountsPage;
