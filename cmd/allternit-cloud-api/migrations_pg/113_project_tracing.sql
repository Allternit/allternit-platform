-- 113_project_tracing.sql
--
-- Per-project trace export (routes/platform_v1/tracing.rs). A developer points their project at
-- an OTLP/HTTP collector (Langfuse, Arize Phoenix, any OpenTelemetry collector) and cloud-api
-- sends a span for each agent turn, phone run and call of that project. The headers usually carry
-- the collector's secret, so they are encrypted with the credential cipher
-- (ALLTERNIT_CREDENTIALS_KEY) like platform_model_keys; only header names are ever returned.

CREATE TABLE IF NOT EXISTS public.platform_project_tracing (
    project_id text PRIMARY KEY REFERENCES public.platform_projects (id),
    endpoint text NOT NULL,
    headers_encrypted text NOT NULL,
    header_names text[] NOT NULL DEFAULT '{}',
    service_name text NOT NULL DEFAULT 'allternit-platform',
    enabled boolean NOT NULL DEFAULT true,
    last_export_at timestamp with time zone,
    last_error text,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP
);
