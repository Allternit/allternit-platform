-- 135: one-scan voice-key handoff relay (Eoj, 2026-11-13).
--
-- After the user confirms the 6-digit code on both screens, the sending
-- computer PUTs the sealed key to /api/v1/voice/handoff/:sid/sealed and the
-- receiving iPhone fetches it once from the same path (routes/voice_keys.rs,
-- services/voice_keys.rs). QR2 stays as the fallback.
--
-- What the server holds is end-to-end encrypted: sealed_nonce is the 12-byte
-- AES-256-GCM nonce and sealed_ciphertext the ciphertext plus tag, both as
-- unpadded base64url text, under a key derived by P-256 ECDH between the two
-- devices. The server never has that key and never sees the sender's public
-- key (it travels only in QR1), so it cannot read the payload or forge a
-- matching 6-digit code. The take is one-time: it sets delivered_at and clears
-- both sealed columns in the same statement. Rows still expire as before
-- (sealing extends expires_at to at least 30 seconds ahead).
--
-- * sealed_nonce, sealed_ciphertext: set once by the sender; NULL after the take.
-- * delivered_at: when the receiver took the sealed key; the sender reads it
--   as `delivered` on GET /api/v1/voice/handoff/:sid.
--
-- Additive only, idempotent (safe to re-run). No grants, no DO blocks.

ALTER TABLE public.voice_key_handoffs ADD COLUMN IF NOT EXISTS sealed_nonce text;
ALTER TABLE public.voice_key_handoffs ADD COLUMN IF NOT EXISTS sealed_ciphertext text;
ALTER TABLE public.voice_key_handoffs ADD COLUMN IF NOT EXISTS delivered_at timestamp with time zone;
