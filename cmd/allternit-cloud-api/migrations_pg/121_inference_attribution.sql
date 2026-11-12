-- 121_inference_attribution.sql
--
-- Console usage, cost and logs on real cloud data (console Waves 1–6, lane C0).
--
-- * inference_usage gains who made each settled completion: the Platform
--   project and api_keys row it billed to (NULL for user keys, device tokens
--   and Clerk sessions), the client that sent it (`runtime` for a paired
--   computer's device token, `playground` for a console playground key,
--   `gizzi-code` when the request says `X-Allternit-Client: gizzi-code/<ver>`,
--   else `api`; NULL for internal callers such as artifact runtimes), the
--   `x-request-id` the response carried, and the wall time until settlement.
--   Rows written before this migration keep NULLs.
-- * platform_projects.log_bodies: a project opts in to keeping request and
--   response bodies (also needs the PLATFORM_LOG_BODIES flag on the server).
-- * inference_request_bodies holds those bodies, keyed by request id, until
--   expires_at.
--
-- Additive only, idempotent (safe to re-run).

ALTER TABLE public.inference_usage ADD COLUMN IF NOT EXISTS project_id text;
ALTER TABLE public.inference_usage ADD COLUMN IF NOT EXISTS key_id text;
ALTER TABLE public.inference_usage ADD COLUMN IF NOT EXISTS client text;
ALTER TABLE public.inference_usage ADD COLUMN IF NOT EXISTS request_id text;
ALTER TABLE public.inference_usage ADD COLUMN IF NOT EXISTS latency_ms bigint;

CREATE INDEX IF NOT EXISTS idx_inference_usage_project_created
    ON public.inference_usage (project_id, created_at) WHERE project_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_inference_usage_client_created
    ON public.inference_usage (client, created_at);
CREATE INDEX IF NOT EXISTS idx_inference_usage_request
    ON public.inference_usage (request_id) WHERE request_id IS NOT NULL;

ALTER TABLE public.platform_projects ADD COLUMN IF NOT EXISTS log_bodies boolean NOT NULL DEFAULT false;

CREATE TABLE IF NOT EXISTS public.inference_request_bodies (
    request_id text PRIMARY KEY,
    project_id text NOT NULL REFERENCES public.platform_projects (id),
    request jsonb,
    response jsonb,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at timestamp with time zone NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_inference_request_bodies_expires
    ON public.inference_request_bodies (expires_at);
CREATE INDEX IF NOT EXISTS idx_inference_request_bodies_project
    ON public.inference_request_bodies (project_id, created_at);
