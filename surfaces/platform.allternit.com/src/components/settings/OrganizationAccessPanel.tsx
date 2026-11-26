import React, { useCallback, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  Buildings,
  CheckCircle,
  CreditCard,
  ShieldCheck,
  UsersThree,
  WarningCircle,
} from "@phosphor-icons/react";
import {
  PlatformOrganizationSwitcher,
  isPlatformAuthDisabled,
  usePlatformAuth,
  usePlatformOrganization,
  usePlatformUser,
} from "@/lib/platform-auth-client";
import {
  createPersonalOrganization,
  getCurrentUserProfile,
  type CurrentUserProfile,
} from "@/lib/design/current-user";
import { formatApiError } from "@/lib/api-client";
import { Badge } from "@/components/settings/Badge";
import { EmptyState } from "@/components/settings/EmptyState";
import { MonoChip } from "@/components/settings/MonoChip";
import { SectionHeading } from "@/components/settings/SectionHeading";
import { SkeletonRow } from "@/components/settings/SkeletonRow";
import { QUIET_BUTTON_CLASS } from "@/components/settings/buttonStyles";
import { cn } from "@/lib/utils";

function normalizeRole(role?: string | null) {
  return role?.replace(/^org:/, "") || "member";
}

export function hasOrganizationAdminAccess(role?: string | null) {
  const normalized = normalizeRole(role);
  return normalized === "owner" || normalized === "admin";
}

function useFallbackOrganization(enabled: boolean) {
  const [profile, setProfile] = useState<CurrentUserProfile | null>(null);
  const [loaded, setLoaded] = useState(!enabled);
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!enabled) return;
    try {
      const result = await getCurrentUserProfile();
      setProfile(result);
    } catch {
      setProfile(null);
    } finally {
      setLoaded(true);
    }
  }, [enabled]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const create = useCallback(async () => {
    if (creating) return;
    setCreating(true);
    setCreateError(null);
    try {
      await createPersonalOrganization();
      await refresh();
    } catch (err) {
      setCreateError(formatApiError(err, "Failed to create organization"));
    } finally {
      setCreating(false);
    }
  }, [creating, refresh]);

  return { profile, loaded, creating, createError, create };
}

export function OrganizationAccessPanel({ compact = false }: { compact?: boolean }) {
  const navigate = useNavigate();
  const auth = usePlatformAuth();
  const { isLoaded: userLoaded, isSignedIn } = usePlatformUser();
  const { isLoaded: organizationLoaded, organization, membership } = usePlatformOrganization();
  const authDisabled = isPlatformAuthDisabled();
  const {
    profile: fallbackProfile,
    loaded: fallbackLoaded,
    creating: fallbackCreating,
    createError: fallbackCreateError,
    create: createFallbackOrganization,
  } = useFallbackOrganization(authDisabled);
  const role = normalizeRole(auth.orgRole ?? membership?.role);
  const canManage = hasOrganizationAdminAccess(role);

  if (authDisabled) {
    if (!fallbackLoaded) return <SkeletonRow lines={compact ? 2 : 4} />;

    if (!fallbackProfile?.organization_id) {
      return (
        <EmptyState
          art="sheets"
          title="No organization yet"
          caption={
            fallbackCreateError ??
            "This build has no Clerk organization configured. Create a personal one to manage cloud credentials and metered billing."
          }
          ctaLabel={fallbackCreating ? "Creating…" : "Create organization"}
          onCtaClick={() => void createFallbackOrganization()}
        />
      );
    }

    const fallbackRole = normalizeRole(fallbackProfile.organization_role);
    const fallbackCanManage = hasOrganizationAdminAccess(fallbackRole);
    return (
      <div className={cn("space-y-4", compact && "space-y-3")}>
        {!compact && (
          <div>
            <SectionHeading className="mb-1">Organization & access</SectionHeading>
            <p className="text-[12px] text-[var(--text-secondary)] m-0 leading-relaxed">
              This build has no Clerk organization configured, so your account is scoped to a personal organization.
            </p>
          </div>
        )}
        <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/45 p-4">
          <div className="flex items-start gap-3">
            <div className="size-10 rounded-xl bg-[var(--accent-primary)]/10 text-[var(--accent-primary)] flex items-center justify-center shrink-0">
              <Buildings size={20} weight="duotone" />
            </div>
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2 flex-wrap">
                <span className="text-[14px] font-semibold text-[var(--text-primary)]">
                  Personal organization
                </span>
                <Badge className={fallbackCanManage ? "text-[var(--status-success)] bg-[var(--status-success)]/10" : undefined}>
                  {fallbackRole}
                </Badge>
              </div>
              <div className="mt-3">
                <div className="text-[10px] uppercase tracking-wide text-[var(--text-tertiary)] mb-1">Organization ID</div>
                <MonoChip className="max-w-full break-all">{fallbackProfile.organization_id}</MonoChip>
              </div>
            </div>
          </div>
        </div>
      </div>
    );
  }

  if (!userLoaded || !organizationLoaded) {
    return <SkeletonRow lines={compact ? 2 : 4} />;
  }

  if (!isSignedIn) {
    return (
      <EmptyState
        icon={<Buildings size={32} weight="thin" />}
        title="Sign in to manage an organization"
        caption="Organization membership and metered billing are tied to your Allternit account."
        ctaLabel="Sign in"
        onCtaClick={() => navigate("/sign-in")}
      />
    );
  }

  if (!auth.orgId || !organization) {
    return (
      <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/45 p-4">
        <div className="flex items-start gap-3">
          <div className="size-9 rounded-lg bg-[var(--accent-primary)]/10 text-[var(--accent-primary)] flex items-center justify-center shrink-0">
            <Buildings size={18} />
          </div>
          <div className="min-w-0 flex-1">
            <div className="text-[13px] font-semibold text-[var(--text-primary)]">
              Select your organization
            </div>
            <p className="text-[12px] leading-relaxed text-[var(--text-secondary)] mt-1 mb-3">
              Your account can remain personal for local compute. Select or create
              an organization only when managing shared infrastructure and billing.
            </p>
            <PlatformOrganizationSwitcher />
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className={cn("space-y-4", compact && "space-y-3")}>
      {!compact && (
        <div>
          <SectionHeading className="mb-1">Organization & access</SectionHeading>
          <p className="text-[12px] text-[var(--text-secondary)] m-0 leading-relaxed">
            The active Clerk organization is the security and billing boundary for enterprise infrastructure.
          </p>
        </div>
      )}

      <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/45 p-4">
        <div className="flex items-start gap-3">
          <div className="size-10 rounded-xl bg-[var(--accent-primary)]/10 text-[var(--accent-primary)] flex items-center justify-center shrink-0">
            <Buildings size={20} weight="duotone" />
          </div>
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2 flex-wrap">
              <span className="text-[14px] font-semibold text-[var(--text-primary)]">
                {organization.name}
              </span>
              <Badge className={canManage ? "text-[var(--status-success)] bg-[var(--status-success)]/10" : undefined}>
                {role}
              </Badge>
              {canManage && (
                <span className="inline-flex items-center gap-1 text-[11px] font-medium text-[var(--status-success)]">
                  <CheckCircle size={13} weight="fill" /> Billing access
                </span>
              )}
            </div>
            {organization.slug && (
              <div className="text-[11px] text-[var(--text-tertiary)] mt-1">{organization.slug}</div>
            )}
            <div className="mt-3">
              <PlatformOrganizationSwitcher />
            </div>
          </div>
        </div>

        <div className="mt-4 pt-4 border-t border-solid border-[var(--border-subtle)] grid grid-cols-1 sm:grid-cols-2 gap-3">
          <div>
            <div className="text-[10px] uppercase tracking-wide text-[var(--text-tertiary)] mb-1">Organization ID</div>
            <MonoChip className="max-w-full break-all">{auth.orgId}</MonoChip>
          </div>
          <div>
            <div className="text-[10px] uppercase tracking-wide text-[var(--text-tertiary)] mb-1">Session role</div>
            <div className="text-[12px] text-[var(--text-primary)] capitalize">{role}</div>
          </div>
        </div>
      </div>

      {!compact && (
        <div className="grid grid-cols-1 sm:grid-cols-3 gap-2">
          {[
            { icon: <UsersThree size={16} />, label: "Members", value: canManage ? "Manage" : "View only", enabled: canManage },
            { icon: <ShieldCheck size={16} />, label: "Cloud accounts", value: canManage ? "Manage" : "Restricted", enabled: canManage },
            { icon: <CreditCard size={16} />, label: "Metered billing", value: canManage ? "View & export" : "Restricted", enabled: canManage },
          ].map((permission) => (
            <div key={permission.label} className="rounded-lg border border-solid border-[var(--border-subtle)] p-3">
              <div className="flex items-center gap-2 text-[var(--text-secondary)]">
                {permission.icon}
                <span className="text-[11px]">{permission.label}</span>
              </div>
              <div className={cn(
                "text-[11px] font-medium mt-2",
                permission.enabled ? "text-[var(--status-success)]" : "text-[var(--text-tertiary)]",
              )}>
                {permission.value}
              </div>
            </div>
          ))}
        </div>
      )}

      {!canManage && (
        <div className="flex items-start gap-2 rounded-lg border border-solid border-[rgba(245,158,11,0.25)] bg-[rgba(245,158,11,0.06)] px-3 py-2.5 text-[12px] text-[var(--text-secondary)]">
          <WarningCircle size={16} className="text-[var(--status-warning)] shrink-0 mt-0.5" />
          <span>
            An organization owner or admin must promote this membership before it can manage credentials or billing.
          </span>
        </div>
      )}

      {!compact && (
        <div className="flex justify-end">
          <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => navigate("/organizations")}>
            Open enterprise BYOC
          </button>
        </div>
      )}
    </div>
  );
}

export default OrganizationAccessPanel;
