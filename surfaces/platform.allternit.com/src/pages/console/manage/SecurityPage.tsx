import React, { useCallback, useEffect, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  CheckIcon,
} from "@hugeicons/core-free-icons";
import { formatApiError, AllternitApiError } from "@/lib/api-client";
import {
  type RetentionPolicy,
  type DataResidencyPolicy,
  getRetentionPolicy,
  setRetentionPolicy,
  getDataResidencyPolicy,
  setDataResidencyPolicy,
  listResidencyRegions,
} from "@/lib/security-settings";
import {
  FormPage,
  SkeletonRow,
  QUIET_BUTTON_CLASS,
  Badge,
} from "@/components/console-ui";
import { cn } from "@/lib/utils";
import { isCloudGateway } from "@/lib/availability";
import { OrgPolicyPanel } from "@/pages/console/manage/OrgPolicyPanel";

function formatDateTime(iso?: string | null): string {
  if (!iso) return "Never changed";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return `Last saved ${date.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  })}`;
}

interface RetentionDraft {
  chat: string;
  project: string;
  artifact: string;
  zdr: boolean;
  zdrRegions: string[];
}

/**
 * Organization security settings: data retention windows and data residency
 * pinning. Both routes are owner/admin gated — a 403 renders the page as an
 * honest read-only notice instead of an error wall.
 *
 * The governance/threat-stats surface from the ai.allternit.com donor was
 * deliberately not ported: its /api/v1/policies, /api/v1/security, and
 * /api/v1/purposes routes do not exist on this gateway, so there is nothing
 * real to call behind it.
 */
function RuntimeSecurityPage(): React.ReactNode {
  const [forbidden, setForbidden] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  const [retention, setRetention] = useState<RetentionPolicy | null>(null);
  const [retentionDraft, setRetentionDraft] = useState<RetentionDraft>({
    chat: "",
    project: "",
    artifact: "",
    zdr: false,
    zdrRegions: [],
  });
  const [residency, setResidency] = useState<DataResidencyPolicy | null>(null);
  const [regions, setRegions] = useState<string[]>([]);
  const [pinned, setPinned] = useState<string[]>([]);
  const [defaultRegion, setDefaultRegion] = useState("");
  const [enforce, setEnforce] = useState(false);

  const [savingRetention, setSavingRetention] = useState(false);
  const [savingResidency, setSavingResidency] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [flash, setFlash] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoadError(null);
    try {
      const [retentionPolicy, residencyPolicy, availableRegions] = await Promise.all([
        getRetentionPolicy(),
        getDataResidencyPolicy(),
        listResidencyRegions(),
      ]);
      setRetention(retentionPolicy);
      setRetentionDraft({
        chat: retentionPolicy.chat_retention_days?.toString() ?? "",
        project: retentionPolicy.project_retention_days?.toString() ?? "",
        artifact: retentionPolicy.artifact_retention_days?.toString() ?? "",
        zdr: retentionPolicy.zero_data_residence,
        zdrRegions: retentionPolicy.zdr_regions,
      });
      setResidency(residencyPolicy);
      setPinned(residencyPolicy.pinned_regions);
      setDefaultRegion(residencyPolicy.default_region ?? "");
      setEnforce(residencyPolicy.enforce_region_pinning);
      setRegions(availableRegions);
      setForbidden(false);
    } catch (err) {
      if (err instanceof AllternitApiError && err.statusCode === 403) {
        setForbidden(true);
      } else {
        setLoadError(formatApiError(err, "Unable to load security settings."));
      }
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const showFlash = useCallback((message: string) => {
    setFlash(message);
    window.setTimeout(() => setFlash(null), 3000);
  }, []);

  const parseDays = (raw: string): number | null | "invalid" => {
    const trimmed = raw.trim();
    if (trimmed === "") return null;
    const parsed = Number(trimmed);
    if (!Number.isInteger(parsed) || parsed < 0) return "invalid";
    return parsed;
  };

  const handleSaveRetention = useCallback(async () => {
    setError(null);
    const chat = parseDays(retentionDraft.chat);
    const project = parseDays(retentionDraft.project);
    const artifact = parseDays(retentionDraft.artifact);
    if (chat === "invalid" || project === "invalid" || artifact === "invalid") {
      setError("Retention windows must be whole numbers of days, 0 or greater — or blank to keep indefinitely.");
      return;
    }
    setSavingRetention(true);
    try {
      const updated = await setRetentionPolicy({
        chat_retention_days: chat,
        project_retention_days: project,
        artifact_retention_days: artifact,
        zero_data_residence: retentionDraft.zdr,
        zdr_regions: retentionDraft.zdrRegions,
      });
      setRetention(updated);
      showFlash("Retention policy saved.");
    } catch (err) {
      setError(formatApiError(err, "Unable to save the retention policy."));
    } finally {
      setSavingRetention(false);
    }
  }, [retentionDraft, showFlash]);

  const handleSaveResidency = useCallback(async () => {
    setError(null);
    if (defaultRegion && !pinned.includes(defaultRegion)) {
      setError("The default region must be one of the pinned regions.");
      return;
    }
    setSavingResidency(true);
    try {
      const updated = await setDataResidencyPolicy({
        pinned_regions: pinned,
        default_region: defaultRegion || null,
        enforce_region_pinning: enforce,
      });
      setResidency(updated);
      setPinned(updated.pinned_regions);
      setDefaultRegion(updated.default_region ?? "");
      setEnforce(updated.enforce_region_pinning);
      showFlash("Data residency saved — applies from the next request.");
    } catch (err) {
      setError(formatApiError(err, "Unable to save data residency."));
    } finally {
      setSavingResidency(false);
    }
  }, [pinned, defaultRegion, enforce, showFlash]);

  if (forbidden) {
    return (
      <div className="flex min-h-full flex-col">
        <h1 className="m-0 text-[20px] font-semibold tracking-tight text-[var(--text-primary)]">
          Security
        </h1>
        <p className="m-0 mt-1 text-[13px] text-[var(--text-secondary)]">
          Organization data retention and residency controls.
        </p>
        <div className="mt-6 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
          <p className="m-0 text-[13px] text-[var(--text-secondary)]">
            Retention and residency settings are managed by organization owners and
            admins. Contact an admin to review or change where data is stored and how
            long it is kept.
          </p>
        </div>
      </div>
    );
  }

  if (loadError) {
    return (
      <div className="flex min-h-full flex-col">
        <h1 className="m-0 text-[20px] font-semibold tracking-tight text-[var(--text-primary)]">
          Security
        </h1>
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

  if (!retention || !residency) {
    return (
      <div className="flex min-h-full flex-col">
        <h1 className="m-0 text-[20px] font-semibold tracking-tight text-[var(--text-primary)]">
          Security
        </h1>
        <SkeletonRow lines={6} />
      </div>
    );
  }

  const retentionInput = (
    id: string,
    label: string,
    value: string,
    onChange: (next: string) => void
  ): React.ReactNode => (
    <div className="space-y-1.5">
      <label htmlFor={id} className="text-[12px] font-semibold text-[var(--text-secondary)]">
        {label}
      </label>
      <input
        id={id}
        type="number"
        min={0}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        placeholder="Blank = keep indefinitely"
        className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 font-mono text-[12px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]"
      />
    </div>
  );

  return (
    <FormPage
      title="Security"
      breadcrumb={["Manage", "Security"]}
      footerActions={
        <p className="m-0 text-[12px] text-[var(--text-tertiary)]">
          Changes apply to the active organization and take effect on the next request.
        </p>
      }
      sections={[
        {
          id: "retention",
          title: "Data retention",
          description: `How long Allternit keeps each data class before deletion. Blank means no automatic deletion. ${formatDateTime(retention.updated_at)}.`,
          children: (
            <div className="max-w-xl space-y-4">
              {error && (
                <p className="flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
                  <HugeiconsIcon icon={AlertCircleIcon} size={14} />
                  {error}
                </p>
              )}
              {flash && (
                <p className="flex items-center gap-2 rounded-lg border border-solid border-[var(--status-success)]/30 bg-[var(--status-success)]/10 px-3 py-2 text-[13px] text-[var(--status-success)]">
                  <HugeiconsIcon icon={CheckIcon} size={14} />
                  {flash}
                </p>
              )}
              {retentionInput("retention-chat", "Chat retention (days)", retentionDraft.chat, (next) =>
                setRetentionDraft((prev) => ({ ...prev, chat: next }))
              )}
              {retentionInput("retention-project", "Project retention (days)", retentionDraft.project, (next) =>
                setRetentionDraft((prev) => ({ ...prev, project: next }))
              )}
              {retentionInput("retention-artifact", "Artifact retention (days)", retentionDraft.artifact, (next) =>
                setRetentionDraft((prev) => ({ ...prev, artifact: next }))
              )}
              <label className="flex cursor-pointer items-start gap-2.5">
                <input
                  type="checkbox"
                  className="mt-0.5"
                  checked={retentionDraft.zdr}
                  onChange={(event) =>
                    setRetentionDraft((prev) => ({ ...prev, zdr: event.target.checked }))
                  }
                />
                <span>
                  <span className="block text-[13px] font-medium text-[var(--text-primary)]">
                    Zero data residence
                  </span>
                  <span className="block text-[12px] text-[var(--text-tertiary)]">
                    Process in regions that leave no data at rest outside the pinned set.
                  </span>
                </span>
              </label>
              <div className="flex items-center gap-2">
                <button
                  type="button"
                  disabled={savingRetention}
                  onClick={() => void handleSaveRetention()}
                  className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  {savingRetention ? "Saving…" : "Save retention"}
                </button>
                <button
                  type="button"
                  className={QUIET_BUTTON_CLASS}
                  onClick={() =>
                    setRetentionDraft({
                      chat: retention.chat_retention_days?.toString() ?? "",
                      project: retention.project_retention_days?.toString() ?? "",
                      artifact: retention.artifact_retention_days?.toString() ?? "",
                      zdr: retention.zero_data_residence,
                      zdrRegions: retention.zdr_regions,
                    })
                  }
                >
                  Reset
                </button>
              </div>
            </div>
          ),
        },
        {
          id: "residency",
          title: "Data residency",
          description: `Pin inference and storage to specific regions. ${formatDateTime(residency.updated_at)}.`,
          children: (
            <div className="max-w-xl space-y-4">
              <div>
                <div className="mb-1.5 text-[12px] font-semibold text-[var(--text-secondary)]">
                  Pinned regions
                </div>
                <div className="flex flex-wrap gap-2">
                  {regions.map((region) => (
                    <label
                      key={region}
                      className={cn(
                        "inline-flex cursor-pointer items-center gap-1.5 rounded-lg border border-solid px-3 py-1.5 font-mono text-[12px] font-medium transition-colors",
                        pinned.includes(region)
                          ? "border-[var(--accent-primary)]/30 bg-[var(--accent-primary)]/10 text-[var(--accent-primary)]"
                          : "border-[var(--border-subtle)] bg-[var(--bg-primary)] text-[var(--text-secondary)] hover:border-[var(--border-default)]"
                      )}
                    >
                      <input
                        type="checkbox"
                        className="sr-only"
                        checked={pinned.includes(region)}
                        onChange={(event) => {
                          const next = event.target.checked
                            ? [...pinned, region]
                            : pinned.filter((pinnedRegion) => pinnedRegion !== region);
                          setPinned(next);
                          if (defaultRegion === region && !next.includes(region)) {
                            setDefaultRegion("");
                          }
                        }}
                      />
                      {region}
                    </label>
                  ))}
                </div>
                <p className="m-0 mt-1.5 text-[11px] text-[var(--text-tertiary)]">
                  Requests route within the pinned set. With no regions pinned, routing
                  follows the platform default.
                </p>
              </div>
              <div className="space-y-1.5">
                <label htmlFor="default-region" className="text-[12px] font-semibold text-[var(--text-secondary)]">
                  Default region
                </label>
                <select
                  id="default-region"
                  value={defaultRegion}
                  onChange={(event) => setDefaultRegion(event.target.value)}
                  className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 font-mono text-[12px] text-[var(--text-primary)] outline-none focus:border-[var(--border-default)]"
                >
                  <option value="">No default</option>
                  {pinned.map((region) => (
                    <option key={region} value={region}>
                      {region}
                    </option>
                  ))}
                </select>
                <p className="m-0 text-[11px] text-[var(--text-tertiary)]">
                  Must be one of the pinned regions.
                </p>
              </div>
              <label className="flex cursor-pointer items-start gap-2.5">
                <input
                  type="checkbox"
                  className="mt-0.5"
                  checked={enforce}
                  onChange={(event) => setEnforce(event.target.checked)}
                />
                <span>
                  <span className="block text-[13px] font-medium text-[var(--text-primary)]">
                    Enforce region pinning
                  </span>
                  <span className="block text-[12px] text-[var(--text-tertiary)]">
                    Reject requests that cannot be served entirely inside the pinned set.
                  </span>
                </span>
              </label>
              {residency.pinned_regions.length > 0 && (
                <div className="flex flex-wrap items-center gap-1.5 text-[12px] text-[var(--text-tertiary)]">
                  Currently saved:
                  {residency.pinned_regions.map((region) => (
                    <Badge key={region}>{region}</Badge>
                  ))}
                  {residency.default_region && <Badge>{residency.default_region}</Badge>}
                  {residency.enforce_region_pinning && <Badge>enforced</Badge>}
                </div>
              )}
              <div className="flex items-center gap-2">
                <button
                  type="button"
                  disabled={savingResidency}
                  onClick={() => void handleSaveResidency()}
                  className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  {savingResidency ? "Saving…" : "Save residency"}
                </button>
                <button
                  type="button"
                  className={QUIET_BUTTON_CLASS}
                  onClick={() => {
                    setPinned(residency.pinned_regions);
                    setDefaultRegion(residency.default_region ?? "");
                    setEnforce(residency.enforce_region_pinning);
                  }}
                >
                  Reset
                </button>
              </div>
            </div>
          ),
        },
      ]}
    />
  );
}

/**
 * Security. On Allternit Cloud this is the organization policy (key expiry,
 * IP allowlist, spend alerts); on a runtime gateway it is data retention and
 * residency.
 */
export function SecurityPage(): React.ReactNode {
  return isCloudGateway() ? <OrgPolicyPanel /> : <RuntimeSecurityPage />;
}

export default SecurityPage;
