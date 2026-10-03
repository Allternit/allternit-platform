-- 035_phone_invites.sql
--
-- Invite links for a bot's phone (channels swarm, j-invites-platform). An owner
-- mints a single-use link for one person; the invitee opens allternit.com/i/<code>,
-- proves they own a phone number with a texted code (which records explicit
-- consent in sms_consent_log), and may also sign up to reach the bot in the app.
-- Applied by hand to prod, like 020/021/024.
--
-- phone_invites     : one row per link. Only the sha256 of the code is stored.
-- phone_invite_otps : verification codes (hashed), attempt-limited and short lived.
-- invite_contacts   : app users linked to an owner's bot through an invite.

CREATE TABLE IF NOT EXISTS public.phone_invites (
    id text PRIMARY KEY,
    code_hash text NOT NULL UNIQUE,
    user_id text NOT NULL,
    bot_id text NOT NULL,
    bot_name text NOT NULL DEFAULT 'Assistant',
    number_id text NOT NULL REFERENCES public.phone_numbers(id) ON DELETE CASCADE,
    label text NOT NULL,
    about text NOT NULL DEFAULT '',
    -- pending -> verified -> joining -> joined; declined and revoked are terminal
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'verified', 'joining', 'joined', 'declined', 'revoked')),
    phone_e164 text,
    join_token_hash text,
    joined_user_id text,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    used_at timestamptz
);
CREATE INDEX IF NOT EXISTS idx_phone_invites_owner ON public.phone_invites (user_id, bot_id, created_at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_phone_invites_join_token ON public.phone_invites (join_token_hash) WHERE join_token_hash IS NOT NULL;

CREATE TABLE IF NOT EXISTS public.phone_invite_otps (
    id bigserial PRIMARY KEY,
    invite_id text NOT NULL REFERENCES public.phone_invites(id) ON DELETE CASCADE,
    e164 text NOT NULL,
    code_hash text NOT NULL,
    -- sms | call
    channel text NOT NULL,
    -- what the invitee agreed to, and where from; copied into sms_consent_log on verify
    consent_text text NOT NULL DEFAULT '',
    ip_hash text,
    user_agent text,
    attempts integer NOT NULL DEFAULT 0,
    consumed boolean NOT NULL DEFAULT false,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_phone_invite_otps_invite ON public.phone_invite_otps (invite_id, id);
CREATE INDEX IF NOT EXISTS idx_phone_invite_otps_e164 ON public.phone_invite_otps (e164, created_at);

CREATE TABLE IF NOT EXISTS public.invite_contacts (
    invite_id text PRIMARY KEY REFERENCES public.phone_invites(id) ON DELETE CASCADE,
    owner_user_id text NOT NULL,
    bot_id text NOT NULL,
    contact_user_id text NOT NULL,
    label text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_invite_contacts_owner ON public.invite_contacts (owner_user_id, bot_id);
