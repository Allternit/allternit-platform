-- 139_platform_agent_number_resync.sql
--
-- A Platform agent's runtime definition now lists the live numbers bound to it
-- (`phoneNumbers`, see routes/platform_v1/conversations.rs `runtime_spec`), which
-- gives the agent outbound texting and calling on its hosted runtime. Agents the
-- runtime already holds were synced without that list and would never be re-sent
-- on their own, so every agent with a live bound number is marked for a resync.
-- From now on binding, rebinding, unbinding and releasing a number mark the
-- agents involved themselves.
--
-- Data only, idempotent (re-running only causes one more re-send). Applied
-- automatically before the binary swap by deploy-contabo.sh via the
-- ops_applied_migrations ledger; never edit after it ships.

UPDATE public.platform_agents SET synced_at = NULL
WHERE synced_at IS NOT NULL
  AND id IN (
      SELECT agent_id FROM public.phone_numbers
      WHERE agent_id IS NOT NULL AND project_id IS NOT NULL AND released_at IS NULL
  );
