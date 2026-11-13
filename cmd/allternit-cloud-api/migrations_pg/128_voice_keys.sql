-- 128_voice_keys.sql
--
-- Voice keys saved to the account (opt-in "Save to my Allternit account") and
-- the QR handoff relay (routes/voice_keys.rs, services/voice_keys.rs).
--
-- user_voice_keys: one OpenAI or ElevenLabs key per user and provider, sealed
-- with the platform credential cipher (ALLTERNIT_CREDENTIALS_KEY) exactly like
-- user_inference_keys (010): key_nonce is the 12-byte GCM nonce and
-- key_ciphertext the sealed bytes. Unlike inference keys these are returned
-- decrypted to their owner's signed-in session (platform API keys are refused),
-- so a second device can use the key. Turning sync off deletes every row.
--
-- voice_key_handoffs: the relay for the device-to-device QR handoff. It holds
-- only the session id and the receiver's one-time P-256 public key; the API
-- key itself travels from screen to camera and never reaches the server. A row
-- lives 120 seconds; expired rows read as not found and are pruned on insert.
--
-- Additive only, idempotent (safe to re-run). No grants.

CREATE TABLE IF NOT EXISTS public.user_voice_keys (
    user_id text NOT NULL,
    provider text NOT NULL CHECK (provider IN ('openai', 'elevenlabs')),
    key_ciphertext bytea NOT NULL,
    key_nonce bytea NOT NULL,
    voice_id text,
    label text,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, provider)
);

CREATE TABLE IF NOT EXISTS public.voice_key_handoffs (
    sid text PRIMARY KEY,
    user_id text NOT NULL,
    receiver_pubkey text,
    created_at timestamp with time zone NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at timestamp with time zone NOT NULL DEFAULT (CURRENT_TIMESTAMP + interval '120 seconds')
);

CREATE INDEX IF NOT EXISTS idx_voice_key_handoffs_expires_at
    ON public.voice_key_handoffs (expires_at);
CREATE INDEX IF NOT EXISTS idx_voice_key_handoffs_user
    ON public.voice_key_handoffs (user_id);
