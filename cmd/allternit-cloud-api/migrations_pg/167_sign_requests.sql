-- 167_sign_requests.sql
-- Review & sign (plan handoffs/review-sign-2026-11-26, contract "SignRequest").
-- Additive only.
--
-- One row per ask to sign a document the owner holds in user_files (042).
-- status: pending -> signed | declined | cancelled, or expired (lazy: a pending
-- row past expires_at is written as expired when the owner next reads).
-- End states never change. Only an interactive signed-in session can sign
-- (routes/sign_requests.rs); an agent, runtime or API key never can.
-- signature_png, signer_ip and signer_user_agent are the signing record and
-- are never returned in the JSON view.
CREATE TABLE IF NOT EXISTS sign_requests (
    id uuid PRIMARY KEY,
    user_id text NOT NULL,
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'signed', 'declined', 'cancelled', 'expired')),
    requester_kind text NOT NULL CHECK (requester_kind IN ('self', 'agent', 'mcp_app')),
    requester_name text,
    source_surface text NOT NULL,
    thread_id text,
    title text NOT NULL,
    summary text,
    details jsonb NOT NULL DEFAULT '[]'::jsonb,
    document_file_id uuid NOT NULL,
    document_name text NOT NULL,
    document_sha256 text NOT NULL CHECK (document_sha256 ~ '^[0-9a-f]{64}$'),
    slots jsonb NOT NULL DEFAULT '[]'::jsonb,
    send_to jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    resolved_at timestamptz,
    decline_reason text,
    signature_method text CHECK (signature_method IS NULL OR signature_method IN ('draw', 'type', 'upload')),
    signature_slot jsonb,
    signature_png bytea,
    signed_file_id uuid,
    signed_sha256 text CHECK (signed_sha256 IS NULL OR signed_sha256 ~ '^[0-9a-f]{64}$'),
    consent_version text,
    consent_text text,
    signer_name text,
    signer_email text,
    signer_ip text,
    signer_user_agent text,
    signed_surface text,
    sent_at timestamptz,
    sent_to text
);

-- The owner's list, newest first, optionally by status (Inbox, Office floor).
CREATE INDEX IF NOT EXISTS idx_sign_requests_user_status_created
    ON sign_requests (user_id, status, created_at DESC);

-- A chat thread's requests (BotTranscript card, gizzi status).
CREATE INDEX IF NOT EXISTS idx_sign_requests_user_thread
    ON sign_requests (user_id, thread_id, created_at DESC)
    WHERE thread_id IS NOT NULL;
