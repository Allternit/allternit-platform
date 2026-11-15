import React, { useCallback, useEffect, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  Timer01Icon,
} from "@hugeicons/core-free-icons";
import { api, formatApiError, AllternitApiError } from "@/lib/api-client";
import {
  ListPage,
  EmptyState,
  SkeletonRow,
  GaugeCard,
  StatCard,
  QUIET_BUTTON_CLASS,
} from "@/components/console-ui";
import {
  getAdminRateLimits,
  putAdminRateLimits,
  type AdminRateLimits,
} from "@/lib/gateway-analytics";
import { isCloudGateway } from "../../../lib/availability";
import { ProjectRateLimits } from "./ProjectRateLimits";
import { ensureConsoleGatewayKey, gatewayAuthOptions } from "@/lib/console-gateway";

interface CallerRateLimits {
  object: string;
  requests_remaining: number;
  requests_limit: number;
  tokens_remaining: number;
  tokens_limit: number;
  reset_at: string;
}

export function RateLimitsPage(): React.ReactNode {
  return isCloudGateway() ? <ProjectRateLimits /> : <RuntimeRateLimitsPage />;
}

function RuntimeRateLimitsPage(): React.ReactNode {
  const [caller, setCaller] = useState<CallerRateLimits | null>(null);
  const [callerLoading, setCallerLoading] = useState(true);
  const [callerError, setCallerError] = useState<string | null>(null);
  const [admin, setAdmin] = useState<AdminRateLimits | null>(null);
  const [adminForbidden, setAdminForbidden] = useState(false);
  const [adminLoading, setAdminLoading] = useState(true);
  const [adminError, setAdminError] = useState<string | null>(null);
  const [apiRpm, setApiRpm] = useState("");
  const [gatewayRpm, setGatewayRpm] = useState("");
  const [saving, setSaving] = useState(false);
  const [savedFlash, setSavedFlash] = useState(false);

  const loadCaller = useCallback(async () => {
    setCallerLoading(true);
    setCallerError(null);
    try {
      const key = await ensureConsoleGatewayKey();
      const data = await api.get<CallerRateLimits>(
        "/v1/rate-limits",
        gatewayAuthOptions(key)
      );
      setCaller(data);
    } catch (err) {
      setCallerError(formatApiError(err, "Unable to load your current rate limits"));
      setCaller(null);
    } finally {
      setCallerLoading(false);
    }
  }, []);

  const loadAdmin = useCallback(async () => {
    setAdminLoading(true);
    setAdminError(null);
    try {
      const data = await getAdminRateLimits();
      setAdmin(data);
      setApiRpm(data.api_rate_limit_rpm === null ? "" : String(data.api_rate_limit_rpm));
      setGatewayRpm(
        data.gateway_rate_limit_rpm === null ? "" : String(data.gateway_rate_limit_rpm)
      );
    } catch (err) {
      if (err instanceof AllternitApiError && err.statusCode === 403) {
        setAdminForbidden(true);
      } else {
        setAdminError(formatApiError(err, "Unable to load organization rate limits"));
      }
      setAdmin(null);
    } finally {
      setAdminLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadCaller();
    void loadAdmin();
  }, [loadCaller, loadAdmin]);

  const handleSave = useCallback(async () => {
    setSaving(true);
    setAdminError(null);
    setSavedFlash(false);
    try {
      const body: { api_rate_limit_rpm?: number | null; gateway_rate_limit_rpm?: number | null } =
        {};
      const apiParsed = apiRpm.trim() === "" ? null : Number(apiRpm);
      const gatewayParsed = gatewayRpm.trim() === "" ? null : Number(gatewayRpm);
      if (apiParsed !== null && (!Number.isInteger(apiParsed) || apiParsed < 1)) {
        setAdminError("API rate limit must be a whole number of requests per minute, 1 or greater — or leave it blank to inherit the default.");
        return;
      }
      if (gatewayParsed !== null && (!Number.isInteger(gatewayParsed) || gatewayParsed < 1)) {
        setAdminError("Gateway rate limit must be a whole number of requests per minute, 1 or greater — or leave it blank for no org cap.");
        return;
      }
      if (apiParsed !== (admin?.api_rate_limit_rpm ?? null)) body.api_rate_limit_rpm = apiParsed;
      if (gatewayParsed !== (admin?.gateway_rate_limit_rpm ?? null)) {
        body.gateway_rate_limit_rpm = gatewayParsed;
      }
      if (Object.keys(body).length === 0) {
        setSavedFlash(true);
        return;
      }
      const updated = await putAdminRateLimits(body);
      setAdmin(updated);
      setSavedFlash(true);
      window.setTimeout(() => setSavedFlash(false), 2400);
    } catch (err) {
      setAdminError(formatApiError(err, "Unable to save rate limits"));
    } finally {
      setSaving(false);
    }
  }, [apiRpm, gatewayRpm, admin]);

  return (
    <ListPage
      title="Rate limits"
      subtitle="Your live quota snapshot, plus organization-wide caps for the platform API and the LLM gateway."
    >
      <section className="mb-8">
        <h2 className="m-0 mb-1 text-[15px] font-semibold text-[var(--text-primary)]">
          Your current usage
        </h2>
        <p className="m-0 mb-3 text-[12px] text-[var(--text-tertiary)]">
          Live counters for the console API key, reset on the gateway's sliding
          window.
        </p>

        {callerError && (
          <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
            <HugeiconsIcon icon={AlertCircleIcon} size={14} />
            {callerError}
          </p>
        )}

        {callerLoading && !caller ? (
          <SkeletonRow lines={3} />
        ) : caller ? (
          <div className="space-y-3">
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <GaugeCard
                label="Requests remaining"
                used={caller.requests_remaining}
                limit={Math.max(1, caller.requests_limit)}
                resetHint={`Resets ${new Date(caller.reset_at).toLocaleString()}`}
              />
              <GaugeCard
                label="Token budget remaining"
                used={caller.tokens_remaining}
                limit={Math.max(1, caller.tokens_limit)}
                resetHint="Monthly token budget for this API key"
              />
            </div>
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
              <StatCard
                label="Request limit"
                value={caller.requests_limit.toLocaleString()}
                hint="Requests per minute"
              />
              <StatCard
                label="Tokens remaining"
                value={caller.tokens_remaining.toLocaleString()}
                hint={`of ${caller.tokens_limit.toLocaleString()} cents`}
              />
              <StatCard
                label="Window resets"
                value={new Date(caller.reset_at).toLocaleTimeString()}
              />
            </div>
          </div>
        ) : (
          !callerError && (
            <EmptyState
              icon={<HugeiconsIcon icon={Timer01Icon} size={32} />}
              title="No quota data"
              caption="The gateway did not return a quota snapshot for this key."
              ctaLabel="Retry"
              onCtaClick={() => void loadCaller()}
            />
          )
        )}
      </section>

      <section>
        <h2 className="m-0 mb-1 text-[15px] font-semibold text-[var(--text-primary)]">
          Organization rate limits
        </h2>
        <p className="m-0 mb-3 text-[12px] text-[var(--text-tertiary)]">
          Caps applied to every member and key in the active organization.
        </p>

        {adminLoading ? (
          <SkeletonRow lines={4} />
        ) : adminForbidden ? (
          <div className="rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
            <p className="m-0 text-[13px] text-[var(--text-secondary)]">
              Organization rate limits are visible to owners and admins only.
              Your current limits are shown in the snapshot above; contact an
              organization admin to change them.
            </p>
          </div>
        ) : adminError ? (
          <p className="mb-4 flex items-center gap-2 rounded-lg border border-solid border-[var(--status-error)]/30 bg-[var(--status-error)]/10 px-3 py-2 text-[13px] text-[var(--status-error)]">
            <HugeiconsIcon icon={AlertCircleIcon} size={14} />
            {adminError}
          </p>
        ) : admin ? (
          <div className="max-w-xl space-y-4 rounded-xl border border-solid border-[var(--border-subtle)] bg-[var(--bg-secondary)] p-4">
            <div className="space-y-1.5">
              <label
                htmlFor="api-rate-limit"
                className="text-[12px] font-semibold text-[var(--text-secondary)]"
              >
                Platform API rate limit (requests per minute)
              </label>
              <input
                id="api-rate-limit"
                type="number"
                min={1}
                value={apiRpm}
                onChange={(e) => setApiRpm(e.target.value)}
                placeholder={`Default: ${admin.defaults.api_rate_limit_rpm ?? 600}`}
                className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 font-mono text-[12px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]"
              />
              <p className="m-0 text-[11px] text-[var(--text-tertiary)]">
                {admin.semantics.api_rate_limit_rpm}
              </p>
            </div>

            <div className="space-y-1.5">
              <label
                htmlFor="gateway-rate-limit"
                className="text-[12px] font-semibold text-[var(--text-secondary)]"
              >
                LLM gateway rate limit (requests per minute)
              </label>
              <input
                id="gateway-rate-limit"
                type="number"
                min={1}
                value={gatewayRpm}
                onChange={(e) => setGatewayRpm(e.target.value)}
                placeholder="Blank = no org-level cap"
                className="w-full rounded-lg border border-solid border-[var(--border-subtle)] bg-[var(--bg-primary)] px-3 py-2 font-mono text-[12px] text-[var(--text-primary)] outline-none placeholder:text-[var(--text-tertiary)] focus:border-[var(--border-default)]"
              />
              <p className="m-0 text-[11px] text-[var(--text-tertiary)]">
                {admin.semantics.gateway_rate_limit_rpm}
              </p>
            </div>

            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={() => void handleSave()}
                disabled={saving}
                className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent-primary)] px-3.5 py-2 text-[13px] font-semibold text-[var(--ui-text-inverse)] transition-colors hover:brightness-110 disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {saving ? "Saving…" : "Save rate limits"}
              </button>
              <button
                type="button"
                onClick={() => {
                  setApiRpm(admin.api_rate_limit_rpm === null ? "" : String(admin.api_rate_limit_rpm));
                  setGatewayRpm(
                    admin.gateway_rate_limit_rpm === null ? "" : String(admin.gateway_rate_limit_rpm)
                  );
                }}
                className={QUIET_BUTTON_CLASS}
              >
                Reset
              </button>
              {savedFlash && (
                <span className="text-[12px] text-[var(--status-success)]">Saved.</span>
              )}
            </div>
          </div>
        ) : null}
      </section>
    </ListPage>
  );
}

export default RateLimitsPage;
