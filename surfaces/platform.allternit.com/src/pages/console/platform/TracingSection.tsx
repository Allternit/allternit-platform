/**
 * Project settings → Tracing: send the project's spans (agent turns, phone
 * runs, calls) to the developer's own OTLP/HTTP collector, such as Langfuse or
 * Arize Phoenix (`GET/PUT/DELETE /v1/projects/:id/tracing`, `POST …/test`).
 * Header values are write-only: the API returns only their names.
 */
import React, { useEffect, useMemo, useState } from "react";
import { CheckCircle, CircleNotch, PaperPlaneTilt, Plus, Trash, X } from "@phosphor-icons/react";
import { AllternitApiError, formatApiError } from "@/lib/api-client";
import { v1, type V1ProjectTracing, type V1TracingTest } from "@/lib/platform-v1";
import { Badge, SkeletonRow } from "@/components/console-ui";
import { DESTRUCTIVE_BUTTON_CLASS, QUIET_BUTTON_CLASS } from "@/components/console-ui/buttonStyles";
import { cn } from "@/lib/utils";
import { ErrorBanner, INPUT_CLASS, PRIMARY_BUTTON_CLASS, formatDateTime } from "./shared";

const PRESETS: { label: string; endpoint: string; header: string; hint: string }[] = [
  {
    label: "Langfuse",
    endpoint: "https://cloud.langfuse.com/api/public/otel",
    header: "Authorization",
    hint: "Basic + base64 of public_key:secret_key. US region: https://us.cloud.langfuse.com/api/public/otel.",
  },
  {
    label: "Arize Phoenix",
    endpoint: "https://app.phoenix.arize.com/s/your-space/v1/traces",
    header: "Authorization",
    hint: "Bearer + your Phoenix API key. Self-hosted: https://your-phoenix-host/v1/traces.",
  },
];

interface HeaderRow {
  name: string;
  value: string;
}

export function TracingSection({ projectId }: { projectId: string }) {
  const client = useMemo(() => v1(projectId), [projectId]);
  const [config, setConfig] = useState<V1ProjectTracing | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [endpoint, setEndpoint] = useState("");
  const [service, setService] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [replaceHeaders, setReplaceHeaders] = useState(true);
  const [headers, setHeaders] = useState<HeaderRow[]>([{ name: "Authorization", value: "" }]);
  const [busy, setBusy] = useState<"save" | "test" | "delete" | null>(null);
  const [test, setTest] = useState<V1TracingTest | null>(null);

  const apply = (c: V1ProjectTracing | null) => {
    setConfig(c);
    setEndpoint(c?.endpoint ?? "");
    setService(c?.service_name ?? "");
    setEnabled(c?.enabled ?? true);
    setReplaceHeaders(!c);
    setHeaders(c ? c.header_names.map((name) => ({ name, value: "" })) : [{ name: "Authorization", value: "" }]);
  };

  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);
    client
      .getTracing()
      .then((c) => active && apply(c))
      .catch((err) => {
        if (!active) return;
        if (err instanceof AllternitApiError && err.code === "tracing_not_configured") apply(null);
        else setError(formatApiError(err, "Unable to load trace export"));
      })
      .finally(() => active && setLoading(false));
    return () => {
      active = false;
    };
  }, [client]);

  const save = async () => {
    setBusy("save");
    setError(null);
    setTest(null);
    try {
      const input: Parameters<typeof client.putTracing>[0] = { endpoint: endpoint.trim(), enabled, service_name: service.trim() || undefined };
      if (replaceHeaders) {
        input.headers = Object.fromEntries(headers.filter((h) => h.name.trim()).map((h) => [h.name.trim(), h.value]));
      }
      apply(await client.putTracing(input));
    } catch (err) {
      setError(formatApiError(err, "Unable to save trace export"));
    } finally {
      setBusy(null);
    }
  };

  const sendTest = async () => {
    setBusy("test");
    setError(null);
    try {
      setTest(await client.testTracing());
      apply(await client.getTracing());
    } catch (err) {
      setError(formatApiError(err, "Unable to send a test span"));
    } finally {
      setBusy(null);
    }
  };

  const remove = async () => {
    setBusy("delete");
    setError(null);
    setTest(null);
    try {
      await client.deleteTracing();
      apply(null);
    } catch (err) {
      setError(formatApiError(err, "Unable to turn off trace export"));
    } finally {
      setBusy(null);
    }
  };

  const setHeader = (i: number, patch: Partial<HeaderRow>) => setHeaders((rows) => rows.map((r, j) => (j === i ? { ...r, ...patch } : r)));

  return (
    <div className="mt-4 space-y-3 border-t border-solid border-[var(--border-subtle)] pt-4">
      <div className="flex flex-wrap items-center gap-2">
        <div className="text-[14px] font-semibold text-[var(--text-primary)]">Tracing</div>
        {config &&
          (config.enabled ? (
            <Badge className="bg-[var(--status-success)]/10 text-[var(--status-success)]">Exporting</Badge>
          ) : (
            <Badge>Paused</Badge>
          ))}
      </div>
      <p className="m-0 text-[12px] text-[var(--text-secondary)]">
        Send a span for every agent turn, phone run and call in this project to your own OpenTelemetry collector (OTLP over HTTP, GenAI conventions).
        Export runs in the background and never slows or fails a request.
      </p>

      {loading ? (
        <div role="status" aria-label="Loading trace export">
          <SkeletonRow lines={2} />
        </div>
      ) : (
        <>
          {error && <ErrorBanner message={error} />}
          {config && (
            <div className="flex flex-wrap gap-x-4 gap-y-1 text-[12px] text-[var(--text-secondary)]">
              <span>Last export: {formatDateTime(config.last_export_at)}</span>
              {config.last_error && <span className="text-[var(--status-error)]">Last error: {config.last_error}</span>}
            </div>
          )}
          {!config && (
            <div className="flex flex-wrap items-center gap-2 text-[12px] text-[var(--text-secondary)]">
              Start from:
              {PRESETS.map((p) => (
                <button
                  key={p.label}
                  type="button"
                  className={QUIET_BUTTON_CLASS}
                  title={p.hint}
                  onClick={() => {
                    setEndpoint(p.endpoint);
                    setHeaders([{ name: p.header, value: "" }]);
                  }}
                >
                  {p.label}
                </button>
              ))}
            </div>
          )}
          <div className="grid gap-3 sm:grid-cols-2">
            <label className="block text-[12px] font-medium text-[var(--text-secondary)] sm:col-span-2">
              OTLP endpoint
              <input
                value={endpoint}
                onChange={(e) => setEndpoint(e.target.value)}
                placeholder="https://cloud.langfuse.com/api/public/otel"
                maxLength={2048}
                inputMode="url"
                className={INPUT_CLASS}
              />
              <span className="mt-1 block font-normal text-[var(--text-tertiary)]">Public https URL. /v1/traces is added unless it is already there.</span>
            </label>
            <label className="block text-[12px] font-medium text-[var(--text-secondary)]">
              Service name
              <input value={service} onChange={(e) => setService(e.target.value)} placeholder="allternit-platform" maxLength={120} className={INPUT_CLASS} />
            </label>
            <label className="inline-flex items-center gap-2 self-end pb-2 text-[13px] text-[var(--text-primary)]">
              <input type="checkbox" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} />
              Export spans
            </label>
          </div>

          <fieldset className="m-0 border-0 p-0">
            <legend className="text-[12px] font-medium text-[var(--text-secondary)]">Headers</legend>
            {config && !replaceHeaders ? (
              <div className="mt-1.5 flex flex-wrap items-center gap-2 text-[12px] text-[var(--text-secondary)]">
                {config.header_names.length ? `Stored: ${config.header_names.join(", ")} (values hidden)` : "No headers stored."}
                <button type="button" className={QUIET_BUTTON_CLASS} onClick={() => setReplaceHeaders(true)}>
                  Replace headers
                </button>
              </div>
            ) : (
              <div className="mt-1.5 space-y-2">
                {headers.map((h, i) => (
                  <div key={i} className="flex flex-col gap-2 sm:flex-row sm:items-start">
                    <input
                      value={h.name}
                      onChange={(e) => setHeader(i, { name: e.target.value })}
                      placeholder="Header"
                      aria-label="Header name"
                      className={cn(INPUT_CLASS, "mt-0 sm:w-48")}
                    />
                    <input
                      value={h.value}
                      onChange={(e) => setHeader(i, { value: e.target.value })}
                      placeholder="Value (stored encrypted)"
                      aria-label="Header value"
                      type="password"
                      autoComplete="off"
                      className={cn(INPUT_CLASS, "mt-0 min-w-0 flex-1")}
                    />
                    <button
                      type="button"
                      className={cn(QUIET_BUTTON_CLASS, "self-start")}
                      aria-label="Remove header"
                      onClick={() => setHeaders((rows) => rows.filter((_, j) => j !== i))}
                    >
                      <X size={14} aria-hidden />
                    </button>
                  </div>
                ))}
                <button type="button" className={QUIET_BUTTON_CLASS} disabled={headers.length >= 20} onClick={() => setHeaders((rows) => [...rows, { name: "", value: "" }])}>
                  <Plus size={14} aria-hidden /> Add header
                </button>
              </div>
            )}
          </fieldset>

          <div className="flex flex-wrap items-center gap-2">
            <button type="button" className={PRIMARY_BUTTON_CLASS} disabled={!endpoint.trim() || busy !== null} onClick={() => void save()}>
              {busy === "save" ? <CircleNotch size={14} className="animate-spin" aria-hidden /> : null} Save
            </button>
            {config && (
              <>
                <button type="button" className={QUIET_BUTTON_CLASS} disabled={busy !== null} onClick={() => void sendTest()}>
                  {busy === "test" ? <CircleNotch size={14} className="animate-spin" aria-hidden /> : <PaperPlaneTilt size={14} aria-hidden />} Send test span
                </button>
                <button type="button" className={DESTRUCTIVE_BUTTON_CLASS} disabled={busy !== null} onClick={() => void remove()}>
                  <Trash size={14} aria-hidden /> Turn off
                </button>
              </>
            )}
          </div>
          {test && (
            <div
              role="status"
              className={cn("flex items-start gap-2 text-[13px]", test.ok ? "text-[var(--status-success)]" : "text-[var(--status-error)]")}
            >
              {test.ok && <CheckCircle size={16} className="mt-0.5 shrink-0" aria-hidden />}
              <span className="min-w-0 break-words">{test.message}</span>
            </div>
          )}
        </>
      )}
    </div>
  );
}
