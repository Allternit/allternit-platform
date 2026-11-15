import React, { useCallback, useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  ArrowLeft01Icon,
  LockIcon,
  PlusSignIcon,
  TrashIcon,
} from "@hugeicons/core-free-icons";
import { cn } from "@/lib/utils";
import { formatApiError, isNoRuntime } from "@/lib/api-client";
import { NoRuntimeState } from "@/components/console-ui/NoRuntimeState";
import {
  type VaultCredential,
  type VaultRecord,
  createVault,
  deleteVault,
  deleteVaultCredential,
  formatDate,
  listVaultCredentials,
  listVaults,
  patchVault,
  putVaultCredential,
  putVaultPasswordCredential,
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

export function VaultsPage(): React.ReactNode {
  const navigate = useNavigate();
  const [vaults, setVaults] = useState<VaultRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [showCreate, setShowCreate] = useState(false);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [creating, setCreating] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setVaults(await listVaults());
    } catch (err) {
      setError(formatApiError(err, "Unable to load vaults"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const filtered = vaults.filter(
    (v) =>
      !search.trim() ||
      v.name.toLowerCase().includes(search.trim().toLowerCase()) ||
      v.id.toLowerCase().includes(search.trim().toLowerCase())
  );

  const handleCreate = useCallback(async () => {
    setCreating(true);
    setError(null);
    try {
      const created = await createVault({ name: name.trim(), description: description.trim() || undefined });
      setShowCreate(false);
      setName("");
      setDescription("");
      navigate(`/vaults/${created.id}`);
    } catch (err) {
      setError(formatApiError(err, "Unable to create vault"));
    } finally {
      setCreating(false);
    }
  }, [name, description, navigate]);

  return (
    <ListPage
      title="Vaults"
      subtitle="Encrypted secret vaults that managed agents can read at runtime — API keys, credentials, and tokens without embedding them in prompts or code."
      searchPlaceholder="Search vaults…"
      onSearch={setSearch}
      primaryAction={{ label: "+ New vault", onClick: () => setShowCreate((v) => !v) }}
      emptyState={
        <EmptyState
          icon={<HugeiconsIcon icon={LockIcon} size={32} />}
          title={search ? "No vaults match your search" : "No vaults yet"}
          caption={
            search
              ? "Try a different name."
              : "Vaults are encrypted at rest. Create one to start handing credentials to agents without putting secrets in prompts or code."
          }
          ctaLabel={search ? undefined : "New vault"}
          onCtaClick={search ? undefined : () => setShowCreate(true)}
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
        <div className="mb-4 space-y-3 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="space-y-1">
              <label htmlFor="vault-name" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                Name
              </label>
              <input
                id="vault-name"
                type="text"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Production API keys"
                className={INPUT_CLASS}
              />
            </div>
            <div className="space-y-1">
              <label htmlFor="vault-desc" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                Description
              </label>
              <input
                id="vault-desc"
                type="text"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                placeholder="What lives in this vault"
                className={INPUT_CLASS}
              />
            </div>
          </div>
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={() => void handleCreate()}
              disabled={creating || name.trim().length === 0}
              className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              <HugeiconsIcon icon={PlusSignIcon} size={13} />
              {creating ? "Creating…" : "Create vault"}
            </button>
            <button type="button" onClick={() => setShowCreate(false)} className={QUIET_BUTTON_CLASS}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {loading ? (
        <SkeletonRow lines={4} />
      ) : filtered.length > 0 ? (
        <div className="overflow-x-auto rounded-xl border border-solid border-[var(--border-subtle)]">
          <table className="w-full border-collapse">
            <thead>
              <tr className="border-b border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]">
                <th className={TH_CLASS}>ID</th>
                <th className={TH_CLASS}>Name</th>
                <th className={TH_CLASS}>Description</th>
                <th className={TH_CLASS}>Created</th>
                <th className={TH_CLASS} aria-label="Open" />
              </tr>
            </thead>
            <tbody>
              {filtered.map((vault) => (
                <tr
                  key={vault.id}
                  onClick={() => navigate(`/vaults/${vault.id}`)}
                  className="cursor-pointer border-b border-solid border-[var(--border-subtle)] last:border-b-0 transition-colors hover:bg-[var(--surface-hover)]"
                >
                  <td className={TD_CLASS}>
                    <MonoChip>{vault.id.slice(0, 8)}</MonoChip>
                  </td>
                  <td className={cn(TD_CLASS, "font-medium")}>
                    <span className="inline-flex items-center gap-2">
                      <HugeiconsIcon icon={LockIcon} size={13} className="text-[var(--text-tertiary)]" />
                      {vault.name}
                    </span>
                  </td>
                  <td className={cn(TD_CLASS, "text-[var(--text-secondary)]")}>{vault.description ?? "—"}</td>
                  <td className={cn(TD_CLASS, "text-[var(--text-secondary)]")}>
                    {formatDate(vault.created_at)}
                  </td>
                  <td className={cn(TD_CLASS, "text-[var(--text-tertiary)]")}>→</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : null}
    </ListPage>
  );
}

export function VaultDetailPage(): React.ReactNode {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const [vault, setVault] = useState<VaultRecord | null>(null);
  const [credentials, setCredentials] = useState<VaultCredential[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);

  // New credential form.
  const [credType, setCredType] = useState<"oauth" | "password">("oauth");
  const [provider, setProvider] = useState("");
  const [username, setUsername] = useState("");
  const [secret, setSecret] = useState("");
  const [originPattern, setOriginPattern] = useState("");
  const [adding, setAdding] = useState(false);

  const load = useCallback(async () => {
    if (!id) return;
    setLoading(true);
    setError(null);
    try {
      const list = await listVaults();
      const found = list.find((v) => v.id === id);
      if (!found) throw new Error("Vault not found");
      setVault(found);
      setName(found.name);
      setDescription(found.description ?? "");
      setCredentials(await listVaultCredentials(id).catch(() => []));
    } catch (err) {
      setError(formatApiError(err, "Unable to load vault"));
    } finally {
      setLoading(false);
    }
  }, [id]);

  useEffect(() => {
    void load();
  }, [load]);

  const handleSave = useCallback(async () => {
    if (!id) return;
    setSaving(true);
    setError(null);
    try {
      const updated = await patchVault(id, { name: name.trim(), description: description.trim() || undefined });
      setVault(updated);
      setEditing(false);
    } catch (err) {
      setError(formatApiError(err, "Save failed"));
    } finally {
      setSaving(false);
    }
  }, [id, name, description]);

  const handleDelete = useCallback(async () => {
    if (!id || !vault) return;
    if (!window.confirm(`Delete vault "${vault.name}" and its credentials? This cannot be undone.`)) return;
    setDeleting(true);
    setError(null);
    try {
      await deleteVault(id);
      navigate("/vaults");
    } catch (err) {
      setError(formatApiError(err, "Delete failed"));
      setDeleting(false);
    }
  }, [id, vault, navigate]);

  const handleAddCredential = useCallback(async () => {
    if (!id) return;
    setAdding(true);
    setError(null);
    try {
      if (credType === "oauth") {
        if (!provider.trim() || !secret.trim()) throw new Error("Provider and secret are required.");
        await putVaultCredential(id, { provider: provider.trim(), oauth_value: secret });
      } else {
        if (!provider.trim() || !username.trim() || !secret.trim()) {
          throw new Error("Provider, username, and password are required.");
        }
        await putVaultPasswordCredential(id, {
          provider: provider.trim(),
          username: username.trim(),
          password: secret,
          origin_pattern: originPattern.trim() || undefined,
        });
      }
      setProvider("");
      setUsername("");
      setSecret("");
      setOriginPattern("");
      setCredentials(await listVaultCredentials(id));
    } catch (err) {
      setError(err instanceof Error && !("statusCode" in err) ? err.message : formatApiError(err, "Add failed"));
    } finally {
      setAdding(false);
    }
  }, [id, credType, provider, username, secret, originPattern]);

  const handleDeleteCredential = useCallback(
    async (credential: VaultCredential) => {
      if (!id) return;
      if (!window.confirm(`Revoke the ${credential.provider} credential${credential.username ? ` for ${credential.username}` : ""}?`)) {
        return;
      }
      setError(null);
      try {
        await deleteVaultCredential(id, credential.id);
        setCredentials(await listVaultCredentials(id));
      } catch (err) {
        setError(formatApiError(err, "Revoke failed"));
      }
    },
    [id]
  );

  if (loading) {
    return <SkeletonRow lines={5} />;
  }

  if (error && !vault) {
    return (
      <div className="space-y-4">
        <LinkBack />
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

  if (!vault) return null;

  return (
    <div className="space-y-6">
      <LinkBack />

      {error && (isNoRuntime(error) ? (
        <NoRuntimeState />
      ) : (
        <p className="flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      ))}

      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 space-y-2">
          <div className="flex flex-wrap items-center gap-2">
            <h1 className="m-0 text-[20px] font-semibold tracking-tight text-[var(--text-primary)]">
              {vault.name}
            </h1>
            <Badge className="text-[var(--status-success)]">encrypted</Badge>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <MonoChip>{vault.id}</MonoChip>
            <span className="text-[12px] text-[var(--text-secondary)]">
              created {formatDate(vault.created_at)}
            </span>
          </div>
          {vault.description && <p className="m-0 text-[13px] text-[var(--text-secondary)]">{vault.description}</p>}
        </div>
        <div className="flex shrink-0 gap-2">
          <button
            type="button"
            onClick={() => setEditing((v) => !v)}
            className={QUIET_BUTTON_CLASS}
          >
            {editing ? "Cancel edit" : "Edit"}
          </button>
          <button
            type="button"
            onClick={() => void handleDelete()}
            disabled={deleting}
            className={DESTRUCTIVE_BUTTON_CLASS}
          >
            <HugeiconsIcon icon={TrashIcon} size={13} />
            {deleting ? "Deleting…" : "Delete vault"}
          </button>
        </div>
      </div>

      {editing && (
        <div className="space-y-3 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="space-y-1">
              <label htmlFor="vault-edit-name" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                Name
              </label>
              <input
                id="vault-edit-name"
                type="text"
                value={name}
                onChange={(e) => setName(e.target.value)}
                className={INPUT_CLASS}
              />
            </div>
            <div className="space-y-1">
              <label htmlFor="vault-edit-desc" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                Description
              </label>
              <input
                id="vault-edit-desc"
                type="text"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                className={INPUT_CLASS}
              />
            </div>
          </div>
          <button
            type="button"
            onClick={() => void handleSave()}
            disabled={saving || !name.trim()}
            className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      )}

      <section className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
        <h2 className="m-0 mb-1 text-[14px] font-semibold text-[var(--text-primary)]">Credentials</h2>
        <p className="m-0 mb-4 text-[12px] text-[var(--text-tertiary)]">
          Secrets are sealed server-side and never returned — this list shows metadata only.
        </p>

        <div className="space-y-3 rounded-lg border border-solid border-[var(--border-subtle)] p-3">
          <div className="flex flex-wrap items-center gap-2">
            <select
              aria-label="Credential type"
              value={credType}
              onChange={(e) => setCredType(e.target.value as "oauth" | "password")}
              className="rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-2 py-1.5 text-[12px] text-[var(--text-primary)] outline-none focus:border-[var(--accent-primary)]"
            >
              <option value="oauth">OAuth / API token</option>
              <option value="password">Username + password</option>
            </select>
            <input
              type="text"
              value={provider}
              onChange={(e) => setProvider(e.target.value)}
              placeholder="Provider (e.g. github, stripe)"
              className={cn(INPUT_CLASS, "w-52")}
            />
            {credType === "password" && (
              <>
                <input
                  type="text"
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                  placeholder="Username"
                  className={cn(INPUT_CLASS, "w-40")}
                />
                <input
                  type="text"
                  value={originPattern}
                  onChange={(e) => setOriginPattern(e.target.value)}
                  placeholder="Origin pattern (optional)"
                  className={cn(INPUT_CLASS, "w-48")}
                />
              </>
            )}
            <input
              type="password"
              value={secret}
              onChange={(e) => setSecret(e.target.value)}
              placeholder={credType === "password" ? "Password" : "Token / secret value"}
              className={cn(INPUT_CLASS, "w-56")}
            />
            <button
              type="button"
              onClick={() => void handleAddCredential()}
              disabled={adding}
              className="inline-flex items-center gap-1.5 rounded-lg border border-solid border-[var(--border-subtle)] bg-transparent px-3 py-2 text-[13px] font-medium text-[var(--text-primary)] transition-colors hover:bg-[var(--bg-secondary)] disabled:opacity-50"
            >
              <HugeiconsIcon icon={PlusSignIcon} size={13} />
              {adding ? "Adding…" : "Add credential"}
            </button>
          </div>
        </div>

        {credentials.length === 0 ? (
          <p className="m-0 mt-3 text-[13px] text-[var(--text-tertiary)]">No credentials in this vault.</p>
        ) : (
          <div className="mt-3 divide-y divide-solid divide-[var(--border-subtle)] rounded-lg border border-solid border-[var(--border-subtle)]">
            {credentials.map((credential) => (
              <div key={credential.id} className="flex items-center gap-3 px-3 py-2">
                <Badge>{credential.credential_type}</Badge>
                <span className="text-[13px] font-medium text-[var(--text-primary)]">{credential.provider}</span>
                {credential.username && (
                  <span className="font-mono text-[12px] text-[var(--text-secondary)]">{credential.username}</span>
                )}
                {credential.origin_pattern && (
                  <span className="font-mono text-[11px] text-[var(--text-tertiary)]">{credential.origin_pattern}</span>
                )}
                <span className="ml-auto text-[11px] text-[var(--text-tertiary)]">
                  {credential.use_count > 0 ? `used ${credential.use_count}×` : "unused"}
                  {credential.last_used_at ? ` · last ${formatDate(credential.last_used_at)}` : ""}
                </span>
                <button
                  type="button"
                  onClick={() => void handleDeleteCredential(credential)}
                  className="text-[var(--text-tertiary)] transition-colors hover:text-[var(--status-error)]"
                  aria-label={`Revoke ${credential.provider} credential`}
                >
                  <HugeiconsIcon icon={TrashIcon} size={13} />
                </button>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

function LinkBack(): React.ReactNode {
  return (
    <Link
      to="/vaults"
      className="inline-flex items-center gap-1.5 text-[13px] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
    >
      <HugeiconsIcon icon={ArrowLeft01Icon} size={14} />
      Back to vaults
    </Link>
  );
}

export default VaultsPage;
