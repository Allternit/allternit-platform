-- Custom-voice consent clips move from bytea to the private R2 bucket
-- allternit-user-files at voices/<userId>/<voiceId>.<ext>. New rows carry
-- clip_key; old rows keep their bytea until the admin backfill moves them.
-- Revoking deletes the object and nulls clip_key.
ALTER TABLE voice_custom_voices ADD COLUMN IF NOT EXISTS clip_key text;

DO $$
DECLARE c text;
BEGIN
    FOR c IN
        SELECT conname FROM pg_constraint
        WHERE conrelid = 'voice_custom_voices'::regclass AND contype = 'c'
          AND pg_get_constraintdef(oid) LIKE '%clip IS%'
    LOOP
        EXECUTE format('ALTER TABLE voice_custom_voices DROP CONSTRAINT %I', c);
    END LOOP;
END $$;

ALTER TABLE voice_custom_voices ADD CONSTRAINT voice_custom_voices_clip_present CHECK (
    (status = 'active' AND (clip IS NOT NULL OR clip_key IS NOT NULL))
    OR (status = 'revoked' AND clip IS NULL)
);
