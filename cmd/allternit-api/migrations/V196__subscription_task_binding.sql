-- Subscription Fabric (HARDENING D16): a human action minted on an approval
-- card is bound to the exact task the card showed. `task_digest` is the
-- SHA-256 of (capability, provider, prompt, options); the forwarder accepts a
-- `POST v1/tasks` with that action only when the submitted task hashes to the
-- same digest, so a confirmed card can never carry a different prompt to the
-- provider. NULL (a chat send) keeps the unbound behaviour.
ALTER TABLE subs_human_actions ADD COLUMN task_digest TEXT;
