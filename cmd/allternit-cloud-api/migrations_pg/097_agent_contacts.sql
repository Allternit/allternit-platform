-- 097_agent_contacts.sql
--
-- Agent network phase 2 (WP14): outside agents the owner knows. A2A callers at the
-- cloud edge are classified team / contact / stranger (contract C8); a contact is
-- an `accepted` row here. `card_hash` pins the peer's Agent Card at add time; a later
-- change sets `card_changed` and the contact stops counting until the owner re-accepts.

CREATE TABLE IF NOT EXISTS public.agent_contacts (
    owner_user_id text NOT NULL,
    peer_card_url text NOT NULL,
    peer_name text NOT NULL DEFAULT '',
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'accepted', 'blocked')),
    card_hash text,
    card_changed boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (owner_user_id, peer_card_url)
);

CREATE INDEX IF NOT EXISTS idx_agent_contacts_peer ON public.agent_contacts (peer_card_url);
