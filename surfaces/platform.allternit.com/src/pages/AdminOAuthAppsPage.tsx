import React, { useCallback, useEffect, useState } from "react";
import { ShieldCheck } from "@phosphor-icons/react";
import { AllternitApiError, formatApiError } from "@/lib/api-client";
import { EmptyState, SkeletonRow } from "@/components/console-ui";
import { QUIET_BUTTON_CLASS } from "@/components/console-ui/buttonStyles";
import { StateBadge } from "@/components/oauth/parts";
import { adminDecideOAuthApp, adminListOAuthApps, type OAuthApp } from "@/lib/oauth-apps";
import { CARD_CLASS, ErrorBanner, PlatformHeader, formatDate } from "@/pages/console/platform/shared";

/**
 * Admin review queue for "Sign in with Allternit" apps. The console has no
 * client-side admin flag, so the page renders for everyone and the server's 403
 * becomes a plain message (same approach as the other /admin pages).
 */
export function AdminOAuthAppsPage() {
  const [apps, setApps] = useState<OAuthApp[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [forbidden, setForbidden] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);

  const load = useCallback(async () => {
    setError(null);
    try {
      setApps(await adminListOAuthApps("in_review"));
    } catch (err) {
      setApps([]);
      if (err instanceof AllternitApiError && err.statusCode === 403) setForbidden(true);
      else setError(formatApiError(err, "Unable to load apps in review"));
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  const decide = async (app: OAuthApp, action: "approve" | "suspend") => {
    setBusyId(app.id);
    setError(null);
    try {
      await adminDecideOAuthApp(app.id, action);
      setApps((c) => (c ?? []).filter((a) => a.id !== app.id));
    } catch (err) {
      setError(formatApiError(err, `Unable to ${action} ${app.name}`));
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="mx-auto w-full max-w-5xl">
      <PlatformHeader title="OAuth app review" subtitle="Apps waiting for approval to go live." />
      {forbidden ? (
        <div className={CARD_CLASS}>
          <EmptyState icon={<ShieldCheck size={32} aria-hidden />} title="Admins only" caption="This page is for Allternit admins." />
        </div>
      ) : (
        <>
          {error && <ErrorBanner message={error} className="mb-3" />}
          {apps === null ? (
            <div className={CARD_CLASS} role="status" aria-label="Loading apps in review">
              <SkeletonRow />
              <SkeletonRow />
            </div>
          ) : apps.length === 0 ? (
            <div className={CARD_CLASS}>
              <EmptyState art="bell" size="panel" title="Nothing in review" caption="Apps that developers submit for review show up here." />
            </div>
          ) : (
            <ul className="m-0 list-none space-y-3 p-0">
              {apps.map((a) => (
                <li key={a.id} className={`${CARD_CLASS} flex flex-wrap items-start justify-between gap-3 p-4`}>
                  <div className="min-w-0 text-[13px]">
                    <div className="flex items-center gap-2">
                      <span className="font-semibold text-[var(--text-primary)]">{a.name}</span>
                      <StateBadge state={a.state} />
                    </div>
                    <div className="mt-1 text-[var(--text-secondary)]">
                      <code>{a.client_id}</code> · {a.client_type} · created {formatDate(a.created_at)}
                    </div>
                    <div className="mt-1 break-all text-[var(--text-secondary)]">Scopes: {a.scopes.join(", ") || "none"}</div>
                    {a.homepage_url && <div className="mt-1 break-all text-[var(--text-secondary)]">{a.homepage_url}</div>}
                    {a.privacy_url && <div className="break-all text-[var(--text-secondary)]">{a.privacy_url}</div>}
                    <div className="mt-1 break-all text-[var(--text-tertiary)]">Redirects: {a.redirect_uris.join(", ")}</div>
                  </div>
                  <div className="flex gap-2">
                    <button type="button" disabled={busyId === a.id} onClick={() => decide(a, "approve")} className="inline-flex items-center rounded-lg bg-[var(--accent-primary)] px-3 py-1.5 text-[13px] font-semibold text-[var(--ui-text-inverse)] hover:brightness-110 disabled:opacity-50">
                      Approve
                    </button>
                    <button type="button" disabled={busyId === a.id} onClick={() => decide(a, "suspend")} className={QUIET_BUTTON_CLASS}>
                      Suspend
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </div>
  );
}
