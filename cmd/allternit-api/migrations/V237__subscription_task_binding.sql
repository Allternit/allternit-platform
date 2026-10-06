-- Subscription Fabric (D16 task binding): a human action minted when a person
-- approves a subscription card is bound to the exact task the card carried.
-- `task_digest` is the SHA-256 of (capability family, provider, prompt,
-- options); the forwarder accepts a `POST v1/tasks` with that action only when
-- the submitted task hashes to the same digest, so an approval can never carry
-- a different prompt to the provider. NULL (a send the person typed in the chat
-- composer) keeps the unbound behaviour.
ALTER TABLE subs_human_actions ADD COLUMN task_digest TEXT;
