/**
 * CloudCredentialsPanel - BYOC (Bring Your Own Cloud) credential management.
 *
 * Enterprise customers connect their own AWS/GCP/Azure account here so ACU
 * sandboxes provision into the customer's cloud instead of allternit's.
 */

import React, { useCallback, useEffect, useState } from 'react';
import { ArrowClockwise, Cloud, Plus, ShieldCheck, Warning } from '@phosphor-icons/react';
import { SectionHeading } from '@/components/settings/SectionHeading';
import { QUIET_BUTTON_CLASS } from '@/components/settings/buttonStyles';
import { Badge } from '@/components/settings/Badge';
import { CloudCredentialsList } from '@/components/settings/CloudCredentialsList';
import { AddCloudCredentialForm } from '@/components/settings/AddCloudCredentialForm';
import {
  listCloudCredentials,
  createCloudCredential,
  revokeCloudCredential,
  type CloudCredential,
  type CreateCloudCredentialInput,
} from '@/lib/design/cloud-credentials';
import { isNoRuntime } from '@/lib/api-client';
import { NoRuntimeState } from '@/components/console-ui/NoRuntimeState';

export function CloudCredentialsPanel() {
  const [isModalOpen, setIsModalOpen] = useState(false);
  const [credentials, setCredentials] = useState<CloudCredential[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadCredentials = useCallback(async () => {
    try {
      setIsLoading(true);
      setError(null);
      const data = await listCloudCredentials();
      setCredentials(data);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load cloud credentials');
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadCredentials();
  }, [loadCredentials]);

  const handleConnect = useCallback(async (input: CreateCloudCredentialInput) => {
    await createCloudCredential(input);
    await loadCredentials();
  }, [loadCredentials]);

  const handleRevoke = useCallback(async (id: string) => {
    try {
      await revokeCloudCredential(id);
      setCredentials((prev) => prev.map((c) => (c.id === id ? { ...c, status: 'revoked' as const } : c)));
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to revoke credential');
    }
  }, []);

  return (
    <div className="space-y-4">
      <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/45 p-4">
        <div className="flex items-start gap-3">
          <div className="size-9 rounded-lg bg-[var(--accent-primary)]/10 text-[var(--accent-primary)] flex items-center justify-center shrink-0">
            <Cloud size={18} weight="duotone" />
          </div>
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2 flex-wrap">
              <SectionHeading className="m-0">Cloud accounts</SectionHeading>
              <Badge>{credentials.filter((credential) => credential.status === 'active').length} active</Badge>
            </div>
            <p className="text-[11px] text-[var(--text-secondary)] mt-1 mb-0 leading-relaxed">
              Connect a least-privilege AWS role, Google Cloud service account, or Azure service principal.
              Secrets are sealed before storage and never returned to the browser.
            </p>
          </div>
        </div>
      </div>

      {error && isNoRuntime(error) && <NoRuntimeState />}
      {error && !isNoRuntime(error) && (
        <div className="flex items-start gap-2 rounded-lg border border-solid border-[rgba(239,68,68,0.25)] bg-[var(--status-error-bg)] px-3 py-2.5 text-[12px] text-[var(--status-error)]">
          <Warning size={18} weight="fill" />
          <span className="flex-1 leading-relaxed">{error}</span>
          <button type="button" onClick={() => setError(null)} className={QUIET_BUTTON_CLASS}>
            Dismiss
          </button>
        </div>
      )}

      <div className="flex items-center gap-2">
        <button type="button" onClick={() => setIsModalOpen(true)} className={QUIET_BUTTON_CLASS}>
          <Plus size={15} weight="bold" />
          Connect account
        </button>
        <div className="flex-1" />
        <span className="hidden sm:inline-flex items-center gap-1 text-[10px] text-[var(--text-tertiary)]">
          <ShieldCheck size={12} /> Organization scoped
        </span>
        <button type="button" onClick={() => void loadCredentials()} disabled={isLoading} className={QUIET_BUTTON_CLASS}>
          <ArrowClockwise size={13} className={isLoading ? 'animate-spin' : ''} />
          Refresh
        </button>
      </div>

      <CloudCredentialsList
        credentials={credentials}
        onAddCredential={() => setIsModalOpen(true)}
        onRevokeCredential={handleRevoke}
        isLoading={isLoading}
      />

      <AddCloudCredentialForm
        isOpen={isModalOpen}
        onClose={() => setIsModalOpen(false)}
        onSubmit={async (input) => {
          await handleConnect(input);
          setIsModalOpen(false);
        }}
      />
    </div>
  );
}

export default CloudCredentialsPanel;
