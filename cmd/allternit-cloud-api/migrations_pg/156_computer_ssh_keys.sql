-- 156_computer_ssh_keys.sql
--
-- Computers console (surfaces/docs/dev/computers-console-contract.mdx, D9),
-- step 2.7. The contract planned this as 155; 155 is taken by
-- 155_platform_agent_runtime.sql (Harness & Bots P6, #2784), so it is 156.
--
-- computer_ssh_keys : the SSH public keys an owner allows on one computer.
--   computer_kind 'cloud' = provisioned_instances.id (the console row id);
--   other kinds are reserved for paired/desktop computers once the node can
--   write its marked authorized_keys block. kind 'computer' = a person's key
--   (no expiry); 'agent' = a short-lived key for an agent, expires_at set.
--   public_key is the OpenSSH public key line as sent (one key, no options).
--   Private keys are never stored: the route refuses them before any write.
--   fingerprint = OpenSSH "SHA256:<base64 of sha256(key blob), no padding>",
--   unique per computer. There is no last-used time: Orgo does not report
--   SSH logins.
--
-- Additive only, IF NOT EXISTS everywhere: old code keeps running on the new
-- schema. No DO blocks: the test harness splits statements.

CREATE TABLE IF NOT EXISTS public.computer_ssh_keys (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    computer_kind TEXT NOT NULL DEFAULT 'cloud',
    computer_id TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT 'computer',
    label TEXT NOT NULL,
    public_key TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ,
    CONSTRAINT computer_ssh_keys_kind_check CHECK (kind IN ('computer', 'agent')),
    CONSTRAINT computer_ssh_keys_agent_expiry_check CHECK (kind <> 'agent' OR expires_at IS NOT NULL)
);
COMMENT ON COLUMN public.computer_ssh_keys.public_key IS 'OpenSSH public key line, private keys are refused and never stored';
COMMENT ON COLUMN public.computer_ssh_keys.expires_at IS 'Agent keys only: removed from the computer after this time';
CREATE UNIQUE INDEX IF NOT EXISTS uq_computer_ssh_keys_fingerprint
    ON public.computer_ssh_keys (computer_kind, computer_id, fingerprint);
CREATE INDEX IF NOT EXISTS idx_computer_ssh_keys_user
    ON public.computer_ssh_keys (user_id);
CREATE INDEX IF NOT EXISTS idx_computer_ssh_keys_expiry
    ON public.computer_ssh_keys (expires_at) WHERE expires_at IS NOT NULL;
