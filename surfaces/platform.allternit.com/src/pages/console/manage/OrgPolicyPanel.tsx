import React, { useCallback, useEffect, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import { AlertCircleIcon, CheckIcon } from "@hugeicons/core-free-icons";
import { FormPage, SkeletonRow, QUIET_BUTTON_CLASS } from "@/components/console-ui";
import { hasOrganizationAdminAccess } from "@/components/settings/OrganizationAccessPanel";
import { usePlatformAuth } from "@/lib/platform-auth-client";
import { formatApiError, AllternitApiError } from "@/lib/api-client";
import { getOrgPolicy, putOrgPolicy, parseCidrList, type OrgPolicy } from "@/lib/governance";

const INPUT_CLASS =
  "w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)] disabled:opacity-60";

/** Set to a non-empty value to show the SSO / SCIM panel (Clerk enterprise tier). Off by default. */
const SSO_ENABLED = Boolean(String(import.meta.env.VITE_ORG_SSO ?? "").trim());

interface Draft {
  requireExpiry: boolean;
  maxTtl: string;
  liveAdminOnly: boolean;
  allowlist: string;
  alertEmail: string;
}

function toDraft(p: OrgPolicy): Draft {
  return {
    requireExpiry: p.require_key_expiry,
    maxTtl: p.max_key_ttl_days ? String(p.max_key_ttl_days) : "",
    liveAdminOnly: p.live_keys_admin_only,
    allowlist: p.ip_allowlist.join("\n"),
    alertEmail: p.spend_alert_email ?? "",
  };
}

function Toggle({
  checked,
  disabled,
  onChange,
  title,
  hint,
}: {
  checked: boolean;
  disabled: boolean;
  onChange: (v: boolean) => void;
  title: string;
  hint: string;
}) {
  return (
    <label className="flex cursor-pointer items-start gap-2.5">
      <input type="checkbox" className="mt-0.5" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span>
        <span className="block text-[13px] font-medium text-[var(--text-primary)]">{title}</span>
        <span className="block text-[12px] text-[var(--text-tertiary)]">{hint}</span>
      </span>
    </label>
  );
}

/**
 * Security on Allternit Cloud = organization policy: key expiry rules,
 * live-key access, the org IP allowlist and the spend alert email. Admins
 * edit; everyone else reads.
 */
export function OrgPolicyPanel(): React.ReactNode {
  const { orgRole } = usePlatformAuth();
  const isAdmin = hasOrganizationAdminAccess(orgRole);
  const [policy, setPolicy] = useState<OrgPolicy | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [forbidden, setForbidden] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [flash, setFlash] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoadError(null);
    try {
      const p = await getOrgPolicy();
      setPolicy(p);
      setDraft(toDraft(p));
      setForbidden(false);
    } catch (err) {
      if (err instanceof AllternitApiError && err.statusCode === 403) setForbidden(true);
      else setLoadError(formatApiError(err, "Unable to load the organization policy."));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const save = useCallback(async () => {
    if (!draft || !policy) return;
    setError(null);
    const ttlRaw = draft.maxTtl.trim();
    const ttl = ttlRaw === "" ? null : Number(ttlRaw);
    if (ttl !== null && (!Number.isInteger(ttl) || ttl < 1 || ttl > 3650)) {
      setError("Maximum key lifetime must be a whole number of days between 1 and 3650, or blank for no limit.");
      return;
    }
    const { cidrs, invalid } = parseCidrList(draft.allowlist);
    if (invalid.length > 0) {
      setError(`Fix the IP allowlist: ${invalid.join(", ")} is not a valid address or CIDR range.`);
      return;
    }
    const email = draft.alertEmail.trim();
    if (email && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) {
      setError("Enter a valid email address for spend alerts, or leave it blank.");
      return;
    }
    setSaving(true);
    try {
      const saved = await putOrgPolicy({
        ...policy,
        require_key_expiry: draft.requireExpiry,
        max_key_ttl_days: ttl,
        live_keys_admin_only: draft.liveAdminOnly,
        ip_allowlist: cidrs,
        spend_alert_email: email || null,
      });
      setPolicy(saved);
      setDraft(toDraft(saved));
      setFlash("Policy saved. It applies to new and existing keys from the next request.");
      window.setTimeout(() => setFlash(null), 4000);
    } catch (err) {
      setError(formatApiError(err, "Unable to save the policy."));
    } finally {
      setSaving(false);
    }
  }, [draft, policy]);

  const heading = (
    <h1 className="m-0 text-[20px] font-semibold tracking-tight text-[var(--text-primary)]">Security</h1>
  );

  if (forbidden) {
    return (
      <div className="flex min-h-full flex-col">
        {heading}
        <div className="mt-6 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
          <p className="m-0 text-[13px] text-[var(--text-secondary)]">
            Organization policy is managed by organization admins. Ask an admin to review or change key expiry and
            IP rules.
          </p>
        </div>
      </div>
    );
  }
  if (loadError) {
    return (
      <div className="flex min-h-full flex-col">
        {heading}
        <p className="mb-4 mt-6 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {loadError}
        </p>
        <div>
          <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => void load()}>
            Retry
          </button>
        </div>
      </div>
    );
  }
  if (!policy || !draft) {
    return (
      <div className="flex min-h-full flex-col">
        {heading}
        <SkeletonRow lines={6} />
      </div>
    );
  }

  const ro = !isAdmin;
  const set = (patch: Partial<Draft>) => setDraft((d) => (d ? { ...d, ...patch } : d));

  const sections = [
    {
      id: "keys",
      title: "API keys",
      description: "Rules for every project key in the organization.",
      children: (
        <div className="max-w-xl space-y-4">
          {ro && (
            <p className="m-0 rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] px-3 py-2 text-[12px] text-[var(--text-secondary)]">
              You can view this policy. Only organization admins can change it.
            </p>
          )}
          {error && (
            <p role="alert" className="m-0 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
              <HugeiconsIcon icon={AlertCircleIcon} size={14} />
              {error}
            </p>
          )}
          {flash && (
            <p role="status" className="m-0 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-success)]/30 bg-[var(--status-success)]/10 px-3 py-2 text-[13px] text-[var(--status-success)]">
              <HugeiconsIcon icon={CheckIcon} size={14} />
              {flash}
            </p>
          )}
          <Toggle
            checked={draft.requireExpiry}
            disabled={ro}
            onChange={(v) => set({ requireExpiry: v })}
            title="Require keys to expire"
            hint="New keys must carry an expiry date. Keys with no expiry can no longer be created."
          />
          <label className="block">
            <span className="mb-1 block text-[12px] font-semibold text-[var(--text-secondary)]">Maximum key lifetime (days)</span>
            <input
              type="number"
              min={1}
              max={3650}
              value={draft.maxTtl}
              disabled={ro}
              onChange={(e) => set({ maxTtl: e.target.value })}
              placeholder="Blank = no limit"
              className={INPUT_CLASS}
            />
          </label>
          <Toggle
            checked={draft.liveAdminOnly}
            disabled={ro}
            onChange={(v) => set({ liveAdminOnly: v })}
            title="Only admins can create live keys"
            hint="Developers can still create sandbox keys."
          />
        </div>
      ),
    },
    {
      id: "network",
      title: "Network",
      description: "Limit where API requests can come from.",
      children: (
        <div className="max-w-xl">
          <label className="block">
            <span className="mb-1 block text-[12px] font-semibold text-[var(--text-secondary)]">Organization IP allowlist</span>
            <textarea
              rows={3}
              value={draft.allowlist}
              disabled={ro}
              onChange={(e) => set({ allowlist: e.target.value })}
              placeholder="203.0.113.0/24, 2001:db8::/32"
              className={`${INPUT_CLASS} font-mono`}
            />
            <span className="mt-1 block text-[12px] text-[var(--text-tertiary)]">
              Separate with commas or new lines. Blank allows any address. Key requests from other addresses are
              rejected. Console sign-in is not affected.
            </span>
          </label>
        </div>
      ),
    },
    {
      id: "spend",
      title: "Spend alerts",
      description: "Who hears about spend thresholds.",
      children: (
        <div className="max-w-xl">
          <label className="block">
            <span className="mb-1 block text-[12px] font-semibold text-[var(--text-secondary)]">Alert email</span>
            <input
              type="email"
              value={draft.alertEmail}
              disabled={ro}
              onChange={(e) => set({ alertEmail: e.target.value })}
              placeholder="billing@example.com"
              className={INPUT_CLASS}
            />
          </label>
        </div>
      ),
    },
    ...(SSO_ENABLED
      ? [
          {
            id: "sso",
            title: "Single sign-on and SCIM",
            description: "Enterprise sign-in for your organization.",
            children: (
              <div className="max-w-xl space-y-2 text-[13px] text-[var(--text-secondary)]">
                <p className="m-0">
                  SAML single sign-on, SCIM provisioning and custom roles are set up in the Clerk dashboard for your
                  organization, then apply here without a deploy.
                </p>
                <a
                  href="https://dashboard.clerk.com"
                  target="_blank"
                  rel="noreferrer"
                  className="font-semibold text-[var(--accent-primary)] hover:underline"
                >
                  Open Clerk setup
                </a>
              </div>
            ),
          },
        ]
      : []),
  ];

  return (
    <FormPage
      title="Security"
      breadcrumb={["Manage", "Security"]}
      sections={sections}
      onSubmit={(e) => {
        e.preventDefault();
        if (!ro) void save();
      }}
      footerActions={
        <div className="flex items-center gap-2">
          <button
            type="submit"
            disabled={ro || saving}
            className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {saving ? "Saving…" : "Save policy"}
          </button>
          <button type="button" className={QUIET_BUTTON_CLASS} disabled={ro || saving} onClick={() => setDraft(toDraft(policy))}>
            Reset
          </button>
        </div>
      }
    />
  );
}

export default OrgPolicyPanel;
