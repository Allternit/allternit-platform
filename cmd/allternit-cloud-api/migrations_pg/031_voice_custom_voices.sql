-- 031_voice_custom_voices.sql
--
-- Custom voices with consent on file (wave 2, item 8). A row is the consent
-- record for one reference clip: who the voice belongs to, the exact consent
-- text that was shown and accepted, who uploaded it and from where, and the
-- sha256 of the clip. The clip itself is kept as bytes (24 s of 16-bit mono
-- audio is about 1 MB at 24 kHz) so that revoking can delete the audio in
-- the same statement that revokes the consent; there is no copy in a bucket
-- to forget. Applied by hand to prod, like 020/021/024/029.
--
-- Nothing may be synthesised from a clip unless its row is status 'active'
-- (the voice service asks cloud-api before every session and every 20 s).

CREATE TABLE IF NOT EXISTS public.voice_custom_voices (
    id uuid PRIMARY KEY,
    user_id text NOT NULL,
    name text NOT NULL,
    speaker_name text NOT NULL,
    -- 'self' (the uploader is the speaker) | 'authorized' (the speaker agreed to this use)
    relationship text NOT NULL CHECK (relationship IN ('self', 'authorized')),
    consent_version text NOT NULL,
    consent_text text NOT NULL,
    consent_accepted_at timestamptz NOT NULL,
    consent_ip text,
    consent_user_agent text,
    clip_sha256 text NOT NULL,
    clip_bytes integer NOT NULL CHECK (clip_bytes > 0),
    clip_seconds real NOT NULL,
    -- NULL once revoked: revoking deletes the audio.
    clip bytea,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'revoked')),
    revoked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((status = 'active' AND clip IS NOT NULL) OR (status = 'revoked' AND clip IS NULL))
);
CREATE INDEX IF NOT EXISTS idx_voice_custom_voices_user
    ON public.voice_custom_voices (user_id, status);
