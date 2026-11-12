import React, { useCallback, useEffect, useMemo, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  UserGroupIcon,
  AlertCircleIcon,
  CheckIcon,
  CopyIcon,
  Building01Icon,
} from "@hugeicons/core-free-icons";
import {
  useClerk,
  usePlatformAuth,
  usePlatformOrganization,
  isPlatformAuthDisabled,
} from "@/lib/platform-auth-client";
import { hasOrganizationAdminAccess } from "@/components/settings/OrganizationAccessPanel";
import {
  ListPage,
  EmptyState,
  SkeletonRow,
  Badge,
  QUIET_BUTTON_CLASS,
  DESTRUCTIVE_BUTTON_CLASS,
} from "@/components/console-ui";
import { cn } from "@/lib/utils";
import { ORG_ROLES } from "@/lib/governance";

const TH_CLASS =
  "text-left text-[11px] font-semibold uppercase tracking-wide text-[var(--text-tertiary)] px-3 py-2";
const TD_CLASS = "px-3 py-2.5 text-[13px] text-[var(--text-primary)]";

function normalizeRole(role?: string | null): string {
  return role?.replace(/^org:/, "") || "member";
}

function clerkErrorMessage(err: unknown, fallback: string): string {
  // Clerk API errors carry a `errors` array with long_message fields.
  const errors = (err as { errors?: Array<{ long_message?: string; message?: string }> })?.errors;
  const first = errors?.[0]?.long_message ?? errors?.[0]?.message;
  if (first) return first;
  if (err instanceof Error && err.message) return err.message;
  return fallback;
}

interface MemberRow {
  id: string;
  userId: string;
  name: string;
  email: string;
  imageUrl: string | null;
  role: string;
  joinedAt: string | null;
}

interface InvitationRow {
  id: string;
  email: string;
  role: string;
  createdAt: string | null;
}

/**
 * Organization membership management, on Clerk's organization API (the same
 * boundary the rest of the platform console uses for admin gating). Members
 * without an admin role get a read-only table — the server enforces the same
 * boundary on every admin route.
 */
export function MembersPage(): React.ReactNode {
  const navigate = useNavigate();
  const clerk = useClerk();
  const auth = usePlatformAuth();
  const { isLoaded: orgLoaded, organization } = usePlatformOrganization();
  const authDisabled = isPlatformAuthDisabled();

  const [members, setMembers] = useState<MemberRow[]>([]);
  const [invitations, setInvitations] = useState<InvitationRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");

  const [showInvite, setShowInvite] = useState(false);
  const [inviteEmail, setInviteEmail] = useState("");
  const [inviteRole, setInviteRole] = useState<string>("developer");
  const [inviting, setInviting] = useState(false);

  const [busyId, setBusyId] = useState<string | null>(null);
  const [confirmRemoveId, setConfirmRemoveId] = useState<string | null>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);

  const role = normalizeRole(auth.orgRole);
  const canManage = hasOrganizationAdminAccess(auth.orgRole);
  const activeOrg = clerk?.organization ?? null;
  const currentUserId = auth.userId;

  const load = useCallback(async () => {
    if (!activeOrg) return;
    setLoading(true);
    setError(null);
    try {
      const [membershipResult, invitationResult] = await Promise.all([
        activeOrg.getMemberships({ pageSize: 100 }),
        canManage ? activeOrg.getInvitations({ pageSize: 100 }) : Promise.resolve(null),
      ]);
      setMembers(
        (membershipResult.data ?? []).map((membership: Record<string, any>) => ({
          id: membership.id,
          userId: membership.publicUserData?.userId ?? membership.id,
          name:
            [membership.publicUserData?.firstName, membership.publicUserData?.lastName]
              .filter(Boolean)
              .join(" ") || membership.publicUserData?.identifier?.split("@")[0] ||
            "Unknown member",
          email: membership.publicUserData?.identifier ?? "—",
          imageUrl: membership.publicUserData?.imageUrl ?? null,
          role: normalizeRole(membership.role),
          joinedAt: membership.createdAt ? new Date(membership.createdAt).toISOString() : null,
        }))
      );
      setInvitations(
        (invitationResult?.data ?? [])
          .filter((invitation: Record<string, any>) => invitation.status === "pending")
          .map((invitation: Record<string, any>) => ({
            id: invitation.id,
            email: invitation.emailAddress,
            role: normalizeRole(invitation.role),
            createdAt: invitation.createdAt ? new Date(invitation.createdAt).toISOString() : null,
          }))
      );
    } catch (err) {
      setError(clerkErrorMessage(err, "Unable to load organization members."));
      setMembers([]);
    } finally {
      setLoading(false);
    }
  }, [activeOrg, canManage]);

  useEffect(() => {
    void load();
  }, [load]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return members;
    return members.filter(
      (member) =>
        member.name.toLowerCase().includes(q) || member.email.toLowerCase().includes(q)
    );
  }, [members, search]);

  const handleInvite = useCallback(async () => {
    const email = inviteEmail.trim();
    if (!email || !activeOrg) return;
    setInviting(true);
    setError(null);
    try {
      await activeOrg.inviteMember({ emailAddress: email, role: `org:${inviteRole}` });
      setInviteEmail("");
      setShowInvite(false);
      await load();
    } catch (err) {
      setError(clerkErrorMessage(err, "Unable to send the invitation."));
    } finally {
      setInviting(false);
    }
  }, [inviteEmail, inviteRole, activeOrg, load]);

  const handleRoleChange = useCallback(
    async (member: MemberRow, nextRole: string) => {
      if (!activeOrg || nextRole === member.role) return;
      setBusyId(member.id);
      setError(null);
      try {
        await activeOrg.updateMember({ userId: member.userId, role: `org:${nextRole}` });
        await load();
      } catch (err) {
        setError(clerkErrorMessage(err, "Unable to change the member's role."));
      } finally {
        setBusyId(null);
      }
    },
    [activeOrg, load]
  );

  const handleRemove = useCallback(
    async (member: MemberRow) => {
      if (!activeOrg) return;
      setBusyId(member.id);
      setError(null);
      try {
        await activeOrg.removeMember(member.userId);
        setConfirmRemoveId(null);
        await load();
      } catch (err) {
        setError(clerkErrorMessage(err, "Unable to remove the member."));
      } finally {
        setBusyId(null);
      }
    },
    [activeOrg, load]
  );

  const handleRevokeInvitation = useCallback(
    async (invitation: InvitationRow) => {
      if (!activeOrg) return;
      setBusyId(invitation.id);
      setError(null);
      try {
        await activeOrg.revokeInvitation(invitation.id);
        await load();
      } catch (err) {
        setError(clerkErrorMessage(err, "Unable to revoke the invitation."));
      } finally {
        setBusyId(null);
      }
    },
    [activeOrg, load]
  );

  const copyText = useCallback(async (text: string, id: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedId(id);
      window.setTimeout(() => setCopiedId(null), 2000);
    } catch {
      // clipboard unavailable — leave the chip selectable
    }
  }, []);

  if (authDisabled) {
    return (
      <ListPage title="Members" subtitle="Organization membership and roles.">
        <EmptyState
          icon={<HugeiconsIcon icon={Building01Icon} size={32} />}
          title="Organizations are not configured"
          caption="Member management requires Clerk organizations, which are not enabled in this build. Open Organizations to manage your personal workspace boundary."
          ctaLabel="Open organizations"
          onCtaClick={() => navigate("/organizations")}
        />
      </ListPage>
    );
  }

  if (!orgLoaded) {
    return (
      <ListPage title="Members" subtitle="Organization membership and roles.">
        <SkeletonRow lines={4} />
      </ListPage>
    );
  }

  if (!auth.orgId || !organization || !activeOrg) {
    return (
      <ListPage title="Members" subtitle="Organization membership and roles.">
        <EmptyState
          icon={<HugeiconsIcon icon={UserGroupIcon} size={32} />}
          title="No active organization"
          caption="Members live inside an organization. Select or create one to view and manage membership."
          ctaLabel="Open organizations"
          onCtaClick={() => navigate("/organizations")}
        />
      </ListPage>
    );
  }

  return (
    <ListPage
      title="Members"
      subtitle={`${organization.name} — ${activeOrg.membersCount ?? members.length} member${
        (activeOrg.membersCount ?? members.length) === 1 ? "" : "s"
      }. Owners and admins can invite, promote, and remove.`}
      searchPlaceholder="Search members"
      onSearch={setSearch}
      primaryAction={
        canManage
          ? {
              label: showInvite ? "Close invite" : "Invite member",
              onClick: () => setShowInvite((prev) => !prev),
            }
          : undefined
      }
    >
      {error && (
        <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} />
          {error}
        </p>
      )}

      {!canManage && (
        <div className="mb-4 flex items-start gap-2 rounded-lg border border-solid border-[rgba(245,158,11,0.25)] bg-[rgba(245,158,11,0.06)] px-3 py-2.5 text-[12px] text-[var(--text-secondary)]">
          <HugeiconsIcon icon={AlertCircleIcon} size={14} className="mt-0.5 shrink-0 text-[var(--status-warning)]" />
          <span>
            You are viewing this organization as a {role}. Membership changes require an
            owner or admin role.
          </span>
        </div>
      )}

      {showInvite && canManage && (
        <div className="mb-5 max-w-xl rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-[1fr_140px]">
            <label className="block">
              <span className="mb-1 block text-[12px] font-semibold text-[var(--text-secondary)]">
                Email address
              </span>
              <input
                type="email"
                value={inviteEmail}
                onChange={(event) => setInviteEmail(event.target.value)}
                placeholder="teammate@example.com"
                className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]"
              />
            </label>
            <label className="block">
              <span className="mb-1 block text-[12px] font-semibold text-[var(--text-secondary)]">
                Role
              </span>
              <select
                value={inviteRole}
                onChange={(event) => setInviteRole(event.target.value)}
                className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 text-[13px] text-[var(--text-primary)] outline-none focus:border-[var(--border-default)]"
              >
                {ORG_ROLES.map((r) => (
                  <option key={r.value} value={r.value}>
                    {r.label}
                  </option>
                ))}
              </select>
              <span className="mt-1 block text-[11px] text-[var(--text-tertiary)]">
                {ORG_ROLES.find((r) => r.value === inviteRole)?.hint}
              </span>
            </label>
          </div>
          <div className="mt-3 flex items-center justify-end gap-2">
            <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setShowInvite(false)}>
              Cancel
            </button>
            <button
              type="button"
              disabled={inviting || !inviteEmail.trim()}
              onClick={() => void handleInvite()}
              className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {inviting ? "Sending…" : "Send invitation"}
            </button>
          </div>
        </div>
      )}

      {loading && members.length === 0 ? (
        <SkeletonRow lines={5} />
      ) : filtered.length === 0 ? (
        <EmptyState
          icon={<HugeiconsIcon icon={UserGroupIcon} size={32} />}
          title={search ? "No members match" : "No members yet"}
          caption={
            search
              ? `No member matches "${search}".`
              : "This organization has no members listed yet."
          }
          ctaLabel={search ? "Clear search" : undefined}
          onCtaClick={search ? () => setSearch("") : undefined}
        />
      ) : (
        <div className="overflow-hidden rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40">
          <table className="w-full">
            <thead>
              <tr className="border-b border-solid border-[var(--border-subtle)]">
                <th className={TH_CLASS}>Member</th>
                <th className={TH_CLASS}>Role</th>
                <th className={cn(TH_CLASS, "hidden md:table-cell")}>Joined</th>
                {canManage && <th className={cn(TH_CLASS, "text-right")}>Actions</th>}
              </tr>
            </thead>
            <tbody>
              {filtered.map((member) => (
                <tr key={member.id} className="border-b border-solid border-[var(--border-subtle)] last:border-b-0">
                  <td className={TD_CLASS}>
                    <div className="flex items-center gap-3">
                      {member.imageUrl ? (
                        <img
                          src={member.imageUrl}
                          alt=""
                          className="size-8 shrink-0 rounded-full border border-solid border-[var(--border-subtle)]"
                        />
                      ) : (
                        <span className="flex size-8 shrink-0 items-center justify-center rounded-full bg-[var(--accent-primary)]/10 text-[12px] font-semibold text-[var(--accent-primary)]">
                          {member.name.slice(0, 1).toUpperCase()}
                        </span>
                      )}
                      <div className="min-w-0">
                        <div className="flex items-center gap-1.5">
                          <span className="truncate font-medium">{member.name}</span>
                          {member.userId === currentUserId && <Badge>you</Badge>}
                        </div>
                        <div className="truncate text-[12px] text-[var(--text-tertiary)]">
                          {member.email}
                        </div>
                      </div>
                    </div>
                  </td>
                  <td className={TD_CLASS}>
                    {canManage ? (
                      <select
                        value={member.role}
                        disabled={busyId === member.id || member.userId === currentUserId}
                        onChange={(event) => void handleRoleChange(member, event.target.value)}
                        className="rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-2 py-1 text-[12px] text-[var(--text-primary)] outline-none focus:border-[var(--border-default)] disabled:opacity-50"
                      >
                        {ORG_ROLES.map((r) => (
                          <option key={r.value} value={r.value}>
                            {r.label}
                          </option>
                        ))}
                        {!ORG_ROLES.some((r) => r.value === member.role) && (
                          <option value={member.role} disabled>
                            {member.role}
                          </option>
                        )}
                      </select>
                    ) : (
                      <Badge className="capitalize">{member.role}</Badge>
                    )}
                  </td>
                  <td className={cn(TD_CLASS, "hidden text-[var(--text-secondary)] md:table-cell")}>
                    {member.joinedAt
                      ? new Date(member.joinedAt).toLocaleDateString(undefined, {
                          year: "numeric",
                          month: "short",
                          day: "numeric",
                        })
                      : "—"}
                  </td>
                  {canManage && (
                    <td className={cn(TD_CLASS, "text-right")}>
                      {member.userId === currentUserId ? null : confirmRemoveId === member.id ? (
                        <span className="inline-flex items-center gap-2">
                          <button
                            type="button"
                            className={QUIET_BUTTON_CLASS}
                            onClick={() => setConfirmRemoveId(null)}
                          >
                            Cancel
                          </button>
                          <button
                            type="button"
                            disabled={busyId === member.id}
                            className={DESTRUCTIVE_BUTTON_CLASS}
                            onClick={() => void handleRemove(member)}
                          >
                            {busyId === member.id ? "Removing…" : "Remove"}
                          </button>
                        </span>
                      ) : (
                        <button
                          type="button"
                          className={DESTRUCTIVE_BUTTON_CLASS}
                          onClick={() => setConfirmRemoveId(member.id)}
                        >
                          Remove
                        </button>
                      )}
                    </td>
                  )}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {canManage && invitations.length > 0 && (
        <section className="mt-8">
          <h2 className="m-0 mb-1 text-[15px] font-semibold text-[var(--text-primary)]">
            Pending invitations
          </h2>
          <p className="m-0 mb-3 text-[12px] text-[var(--text-tertiary)]">
            Invitations that have not been accepted yet.
          </p>
          <div className="overflow-hidden rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)]/40">
            <table className="w-full">
              <tbody>
                {invitations.map((invitation) => (
                  <tr key={invitation.id} className="border-b border-solid border-[var(--border-subtle)] last:border-b-0">
                    <td className={TD_CLASS}>
                      <div className="flex items-center gap-2">
                        <span className="truncate">{invitation.email}</span>
                        <button
                          type="button"
                          className="p-1 text-[var(--text-tertiary)] transition-colors hover:text-[var(--text-primary)]"
                          aria-label={`Copy ${invitation.email}`}
                          onClick={() => void copyText(invitation.email, invitation.id)}
                        >
                          <HugeiconsIcon
                            icon={copiedId === invitation.id ? CheckIcon : CopyIcon}
                            size={12}
                            className={copiedId === invitation.id ? "text-[var(--status-success)]" : undefined}
                          />
                        </button>
                      </div>
                    </td>
                    <td className={cn(TD_CLASS, "w-24")}>
                      <Badge className="capitalize">{invitation.role}</Badge>
                    </td>
                    <td className={cn(TD_CLASS, "hidden text-[var(--text-secondary)] sm:table-cell")}>
                      {invitation.createdAt
                        ? new Date(invitation.createdAt).toLocaleDateString()
                        : "—"}
                    </td>
                    <td className={cn(TD_CLASS, "text-right")}>
                      <button
                        type="button"
                        disabled={busyId === invitation.id}
                        className={DESTRUCTIVE_BUTTON_CLASS}
                        onClick={() => void handleRevokeInvitation(invitation)}
                      >
                        {busyId === invitation.id ? "Revoking…" : "Revoke"}
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}

      <p className="mt-6 text-[12px] text-[var(--text-tertiary)]">
        Roles: {ORG_ROLES.map((r) => `${r.label} (${r.hint.toLowerCase()})`).join("; ")}. Only admins can change a
        role. If a role is not set up for your organization in Clerk, the change is rejected and the reason is shown
        above.
      </p>
      <p className="mt-3 text-[12px] text-[var(--text-tertiary)]">
        Member roles gate every admin surface in this console — service accounts, spend
        limits, security, and rate limits follow the same owner/admin boundary.{" "}
        <Link to="/organizations" className="text-[var(--accent-primary)] hover:underline">
          Organization details
        </Link>
      </p>
    </ListPage>
  );
}

export default MembersPage;
